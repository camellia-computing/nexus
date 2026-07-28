use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{CamelliaNexusError, ErrorCode, Result};

pub const SCHEMA_VERSION: u32 = 3;
pub const MAX_CONFIG_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_CONFIGURATION_SCHEMA_BYTES: usize = 4 * 1024 * 1024;
const MAX_ARGUMENTS: usize = 256;
const MAX_ARGUMENT_UTF16_UNITS: usize = 24_000;
const MAX_ENVIRONMENT_ENTRIES: usize = 256;
const MAX_ENVIRONMENT_UTF16_UNITS: usize = 65_536;
/// Product-wide safety ceiling. Signed entitlements may grant a lower limit,
/// but no current plan can grant more than this many sources per program.
pub const MAX_CONFIG_SOURCES: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProgramId(String);

impl ProgramId {
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        let valid_len = !value.is_empty() && value.len() <= 63;
        let valid_start = value
            .bytes()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
        let valid_chars = value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-');
        if valid_len && valid_start && valid_chars {
            Ok(Self(value))
        } else {
            Err(CamelliaNexusError::invalid_spec(
                "Program id must match [a-z0-9][a-z0-9-]{0,62}",
            ))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProgramId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ExecutableSpec {
    Managed {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<ExecutableMetadata>,
    },
    External {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<ExecutableMetadata>,
    },
}

impl ExecutableSpec {
    pub fn path(&self) -> &Path {
        match self {
            Self::Managed { path, .. } | Self::External { path, .. } => path,
        }
    }

    pub fn metadata(&self) -> Option<&ExecutableMetadata> {
        match self {
            Self::Managed { metadata, .. } | Self::External { metadata, .. } => metadata.as_ref(),
        }
    }

    pub fn set_metadata(&mut self, value: ExecutableMetadata) {
        match self {
            Self::Managed { metadata, .. } | Self::External { metadata, .. } => {
                *metadata = Some(value)
            }
        }
    }

    pub fn is_managed(&self) -> bool {
        matches!(self, Self::Managed { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutableMetadata {
    pub size: u64,
    pub modified_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProgramKind {
    Generic,
    SingBox,
    Xray,
    Mihomo,
}

impl ProgramKind {
    pub const ALL: [Self; 4] = [Self::Generic, Self::SingBox, Self::Xray, Self::Mihomo];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProgramType {
    Generic {
        #[serde(default)]
        args: Vec<String>,
    },
    SingBox {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        main_config: Option<PathBuf>,
        #[serde(default)]
        extra_args: Vec<String>,
    },
    Xray {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        main_config: Option<PathBuf>,
        #[serde(default)]
        extra_args: Vec<String>,
    },
    Mihomo {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        main_config: Option<PathBuf>,
        #[serde(default)]
        extra_args: Vec<String>,
    },
}

impl ProgramType {
    pub fn kind(&self) -> ProgramKind {
        match self {
            Self::Generic { .. } => ProgramKind::Generic,
            Self::SingBox { .. } => ProgramKind::SingBox,
            Self::Xray { .. } => ProgramKind::Xray,
            Self::Mihomo { .. } => ProgramKind::Mihomo,
        }
    }

    pub fn main_config(&self) -> Option<&Path> {
        match self {
            Self::Generic { .. } => None,
            Self::SingBox { main_config, .. }
            | Self::Xray { main_config, .. }
            | Self::Mihomo { main_config, .. } => main_config.as_deref(),
        }
    }

    pub fn arguments(&self) -> &[String] {
        match self {
            Self::Generic { args } => args,
            Self::SingBox { extra_args, .. }
            | Self::Xray { extra_args, .. }
            | Self::Mihomo { extra_args, .. } => extra_args,
        }
    }

    pub fn has_explicit_config(&self) -> bool {
        let flags: &[&str] = match self {
            Self::Generic { .. } => return false,
            Self::SingBox { .. } => &["-c", "--config", "-C", "--config-directory"],
            Self::Xray { .. } => &["-c", "-config", "-confdir"],
            Self::Mihomo { .. } => &["-f", "--f", "-config", "--config"],
        };
        self.arguments().iter().any(|argument| {
            flags
                .iter()
                .any(|flag| argument == flag || argument.starts_with(&format!("{flag}=")))
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RestartPolicy {
    Never,
    OnFailure,
    Always,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PrivilegePolicy {
    Standard,
    #[default]
    Automatic,
    Elevated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PrivilegeRequirement {
    Standard,
    Elevated,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "code",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PrivilegeReason {
    TunInterface,
    TransparentProxy,
    PrivilegedPort { port: u16 },
    ExecutableManifest,
    ExplicitPolicy,
    ConfigurationUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivilegeAssessment {
    pub detected: PrivilegeRequirement,
    pub effective: PrivilegeRequirement,
    pub reasons: Vec<PrivilegeReason>,
    pub authoritative: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ConfigSourceSpec {
    Local {
        id: String,
        name: String,
        #[serde(default = "default_true")]
        enabled: bool,
        path: PathBuf,
    },
    Remote {
        id: String,
        name: String,
        #[serde(default = "default_true")]
        enabled: bool,
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        authentication: Option<ConfigSourceAuthentication>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "scheme",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ConfigSourceAuthentication {
    Basic {
        username: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credential_id: Option<String>,
        #[serde(default, skip_serializing)]
        password: Option<String>,
    },
}

pub const MIN_REMOTE_UPDATE_MINUTES: u32 = 5;
pub const MAX_REMOTE_UPDATE_MINUTES: u32 = 7 * 24 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteUpdateSpec {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub interval_minutes: u32,
}

impl ConfigSourceSpec {
    pub fn id(&self) -> &str {
        match self {
            Self::Local { id, .. } | Self::Remote { id, .. } => id,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Local { name, .. } | Self::Remote { name, .. } => name,
        }
    }

    pub fn enabled(&self) -> bool {
        match self {
            Self::Local { enabled, .. } | Self::Remote { enabled, .. } => *enabled,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SingBoxDashboardSpec {
    pub listen_port: u16,
    #[serde(default = "default_dashboard_update_interval")]
    pub update_interval: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SingBoxClashDashboardSpec {
    pub listen_port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct XrayDashboardSpec {
    pub api_port: u16,
    pub metrics_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MihomoDashboardSpec {
    pub listen_port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedConfigSpec {
    #[serde(default)]
    pub sources: Vec<ConfigSourceSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_update: Option<RemoteUpdateSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sing_box_dashboard: Option<SingBoxDashboardSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sing_box_clash_dashboard: Option<SingBoxClashDashboardSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xray_dashboard: Option<XrayDashboardSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mihomo_dashboard: Option<MihomoDashboardSpec>,
}

impl ManagedConfigSpec {
    pub fn automatic_remote_update_minutes(&self) -> Option<u32> {
        let has_enabled_remote = self
            .sources
            .iter()
            .any(|source| matches!(source, ConfigSourceSpec::Remote { enabled: true, .. }));
        if !has_enabled_remote {
            return None;
        }
        self.remote_update
            .as_ref()
            .filter(|update| update.enabled)
            .map(|update| update.interval_minutes)
    }
}

fn default_true() -> bool {
    true
}

fn default_dashboard_update_interval() -> String {
    "1d".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProgramSpec {
    pub schema_version: u32,
    pub id: ProgramId,
    pub name: String,
    pub executable: ExecutableSpec,
    #[serde(rename = "type")]
    pub program_type: ProgramType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_config: Option<ManagedConfigSpec>,
    pub working_directory: PathBuf,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub auto_start: bool,
    pub restart_policy: RestartPolicy,
    pub privilege_policy: PrivilegePolicy,
}

impl ProgramSpec {
    pub fn normalize_runtime_directory(&mut self) -> Result<bool> {
        let directory = match &self.executable {
            ExecutableSpec::Managed { path, .. } => path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .ok_or_else(|| {
                    CamelliaNexusError::new(
                        ErrorCode::InvalidPath,
                        "Managed executable must have a program directory",
                    )
                })?
                .to_path_buf(),
            ExecutableSpec::External { path, .. } => path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .ok_or_else(|| {
                    CamelliaNexusError::new(
                        ErrorCode::InvalidPath,
                        "External executable must have an absolute parent directory",
                    )
                })?
                .to_path_buf(),
        };
        if self.working_directory == directory {
            Ok(false)
        } else {
            self.working_directory = directory;
            Ok(true)
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(CamelliaNexusError::invalid_spec(format!(
                "Unsupported schema version {}",
                self.schema_version
            )));
        }
        if self.name.trim().is_empty() || self.name.len() > 128 || self.name.contains('\0') {
            return Err(CamelliaNexusError::invalid_spec(
                "Program name must contain 1 to 128 bytes",
            ));
        }
        validate_path_text(&self.working_directory)?;
        match &self.executable {
            ExecutableSpec::Managed { path, .. } => {
                if path.parent() != Some(self.working_directory.as_path()) {
                    return Err(CamelliaNexusError::invalid_spec(
                        "Managed working directory must match the executable directory",
                    ));
                }
                validate_relative_path(&self.working_directory, true)?;
                if !self.working_directory.starts_with("bin") {
                    return Err(CamelliaNexusError::invalid_spec(
                        "Managed working directory must be under bin/",
                    ));
                }
                validate_relative_path(path, false)?;
                validate_path_text(path)?;
                if !path.starts_with("bin") {
                    return Err(CamelliaNexusError::invalid_spec(
                        "Managed executable must be under bin/",
                    ));
                }
            }
            ExecutableSpec::External { path, .. } => {
                if !self.working_directory.is_absolute() {
                    validate_relative_path(&self.working_directory, true)?;
                }
                validate_path_text(path)?;
                if !path.is_absolute() {
                    return Err(CamelliaNexusError::invalid_spec(
                        "External executable path must be absolute",
                    ));
                }
                if path.parent() != Some(self.working_directory.as_path()) {
                    return Err(CamelliaNexusError::invalid_spec(
                        "External working directory must match the executable directory",
                    ));
                }
            }
        }
        if let Some(path) = self.program_type.main_config() {
            validate_relative_path(path, false)?;
            validate_path_text(path)?;
            if !path.starts_with("config") {
                return Err(CamelliaNexusError::invalid_spec(
                    "Main config must be under config/",
                ));
            }
        }
        self.validate_managed_config()?;
        validate_arguments(&self.program_type)?;
        validate_config_arguments(&self.program_type)?;
        validate_environment(&self.environment)?;
        validate_mihomo_contract(self)?;
        Ok(())
    }

    pub fn executable_path(&self, workspace: &Path) -> PathBuf {
        match &self.executable {
            ExecutableSpec::Managed { path, .. } => workspace.join(path),
            ExecutableSpec::External { path, .. } => path.clone(),
        }
    }

    pub fn working_directory_path(&self, workspace: &Path) -> PathBuf {
        if self.working_directory.is_absolute() {
            self.working_directory.clone()
        } else {
            workspace.join(&self.working_directory)
        }
    }

    pub fn runtime_data_directory_path(&self, workspace: &Path) -> PathBuf {
        if self.executable.is_managed() {
            workspace.join("data")
        } else {
            self.working_directory_path(workspace)
        }
    }

    pub fn log_path(&self, workspace: &Path, file_name: &str) -> PathBuf {
        if self.executable.is_managed() {
            workspace.join("logs").join(file_name)
        } else {
            workspace.join(file_name)
        }
    }

    pub fn has_authoritative_config(&self) -> bool {
        self.managed_config.is_some()
    }

    fn validate_managed_config(&self) -> Result<()> {
        let Some(managed) = &self.managed_config else {
            return Ok(());
        };
        if matches!(&self.program_type, ProgramType::Generic { .. })
            || self.program_type.main_config().is_none()
        {
            return Err(CamelliaNexusError::invalid_spec(
                "Managed configuration requires a program type with an editable main config",
            ));
        }
        if managed.sources.len() > MAX_CONFIG_SOURCES {
            return Err(CamelliaNexusError::invalid_spec(
                "Managed configuration supports at most 50 sources",
            ));
        }
        if managed.remote_update.as_ref().is_some_and(|update| {
            !(MIN_REMOTE_UPDATE_MINUTES..=MAX_REMOTE_UPDATE_MINUTES)
                .contains(&update.interval_minutes)
        }) {
            return Err(CamelliaNexusError::invalid_spec(
                "Remote update interval must be between 5 minutes and 7 days",
            ));
        }
        let mut ids = BTreeSet::new();
        for source in &managed.sources {
            let id = source.id();
            let valid_id = !id.is_empty()
                && id.len() <= 64
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
            if !valid_id || !ids.insert(id) {
                return Err(CamelliaNexusError::invalid_spec(
                    "Configuration source ids must be unique ASCII identifiers",
                ));
            }
            if source.name().trim().is_empty()
                || source.name().len() > 128
                || source.name().chars().any(char::is_control)
            {
                return Err(CamelliaNexusError::invalid_spec(
                    "Configuration source names must contain 1 to 128 bytes",
                ));
            }
            if !source.enabled() {
                continue;
            }
            match source {
                ConfigSourceSpec::Local { path, .. } => {
                    validate_path_text(path)?;
                    if !path.is_absolute() {
                        validate_relative_path(path, false)?;
                    }
                }
                ConfigSourceSpec::Remote {
                    url,
                    authentication,
                    ..
                } => {
                    if !valid_https_url_without_credentials(url) {
                        return Err(CamelliaNexusError::invalid_spec(
                            "Remote configuration sources must use HTTPS without embedded credentials",
                        ));
                    }
                    if let Some(ConfigSourceAuthentication::Basic {
                        username,
                        credential_id,
                        password,
                    }) = authentication
                        && (username.is_empty()
                            || username.len() > 256
                            || username.contains(':')
                            || username.chars().any(char::is_control)
                            || credential_id.as_ref().is_some_and(|id| {
                                id.is_empty()
                                    || id.len() > 160
                                    || !id.bytes().all(|byte| {
                                        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
                                    })
                            })
                            || password.as_ref().is_some_and(|password| {
                                password.is_empty()
                                    || password.len() > 4096
                                    || password.contains(['\0', '\r', '\n'])
                            })
                            || (credential_id.is_none() && password.is_none()))
                    {
                        return Err(CamelliaNexusError::invalid_spec(
                            "Basic authentication credentials are invalid",
                        ));
                    }
                }
            }
        }
        if (managed.sing_box_dashboard.is_some() || managed.sing_box_clash_dashboard.is_some())
            && !matches!(&self.program_type, ProgramType::SingBox { .. })
        {
            return Err(CamelliaNexusError::invalid_spec(
                "The sing-box Dashboard service is only available for sing-box",
            ));
        }
        if managed.xray_dashboard.is_some()
            && !matches!(&self.program_type, ProgramType::Xray { .. })
        {
            return Err(CamelliaNexusError::invalid_spec(
                "The Xray Dashboard service is only available for Xray",
            ));
        }
        if managed.mihomo_dashboard.is_some()
            && !matches!(&self.program_type, ProgramType::Mihomo { .. })
        {
            return Err(CamelliaNexusError::invalid_spec(
                "The Mihomo Dashboard service is only available for Mihomo",
            ));
        }
        if let Some(dashboard) = &managed.sing_box_dashboard {
            if dashboard.listen_port < 1024 {
                return Err(CamelliaNexusError::invalid_spec(
                    "Dashboard port must be between 1024 and 65535",
                ));
            }
            if !valid_dashboard_interval(&dashboard.update_interval) {
                return Err(CamelliaNexusError::invalid_spec(
                    "Dashboard update interval must use duration units such as 12h or 1d",
                ));
            }
        }
        if let Some(dashboard) = &managed.sing_box_clash_dashboard {
            if dashboard.listen_port < 1024 {
                return Err(CamelliaNexusError::invalid_spec(
                    "Dashboard port must be between 1024 and 65535",
                ));
            }
            if managed
                .sing_box_dashboard
                .as_ref()
                .is_some_and(|native| native.listen_port == dashboard.listen_port)
            {
                return Err(CamelliaNexusError::invalid_spec(
                    "sing-box API and Clash API must use different ports",
                ));
            }
            if dashboard
                .download_url
                .as_ref()
                .is_some_and(|url| !valid_https_url_without_credentials(url))
            {
                return Err(CamelliaNexusError::invalid_spec(
                    "Clash Dashboard download URL must use HTTPS without embedded credentials",
                ));
            }
        }
        if let Some(dashboard) = &managed.xray_dashboard {
            if dashboard.api_port < 1024 || dashboard.metrics_port < 1024 {
                return Err(CamelliaNexusError::invalid_spec(
                    "Dashboard ports must be between 1024 and 65535",
                ));
            }
            if dashboard.api_port == dashboard.metrics_port {
                return Err(CamelliaNexusError::invalid_spec(
                    "Xray API and Metrics ports must be different",
                ));
            }
        }
        if let Some(dashboard) = &managed.mihomo_dashboard {
            if dashboard.listen_port < 1024 {
                return Err(CamelliaNexusError::invalid_spec(
                    "Dashboard port must be between 1024 and 65535",
                ));
            }
            if dashboard
                .download_url
                .as_ref()
                .is_some_and(|url| !valid_https_url_without_credentials(url))
            {
                return Err(CamelliaNexusError::invalid_spec(
                    "Mihomo Dashboard download URL must use HTTPS without embedded credentials",
                ));
            }
        }
        if self.has_authoritative_config() && self.program_type.has_explicit_config() {
            return Err(CamelliaNexusError::invalid_spec(
                "Configuration path arguments are not allowed while managed configuration is enabled",
            ));
        }
        Ok(())
    }
}

fn valid_dashboard_interval(value: &str) -> bool {
    if value.is_empty() || value.len() > 16 {
        return false;
    }
    let mut digits = 0usize;
    for byte in value.bytes() {
        if byte.is_ascii_digit() {
            digits += 1;
        } else if matches!(byte, b's' | b'm' | b'h' | b'd') && digits > 0 {
            digits = 0;
        } else {
            return false;
        }
    }
    digits == 0
}

fn valid_https_url_without_credentials(value: &str) -> bool {
    if value.len() > 2048 || value.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((scheme, remainder)) = value.split_once("://") else {
        return false;
    };
    let authority_end = remainder.find(['/', '?', '#']).unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    scheme.eq_ignore_ascii_case("https") && !authority.is_empty() && !authority.contains('@')
}

pub fn validate_relative_path(path: &Path, allow_current: bool) -> Result<()> {
    if path.is_absolute() {
        return Err(CamelliaNexusError::new(
            ErrorCode::InvalidPath,
            "Path must be relative",
        ));
    }
    if path.as_os_str().is_empty() {
        return if allow_current {
            Ok(())
        } else {
            Err(CamelliaNexusError::new(
                ErrorCode::InvalidPath,
                "Path cannot be empty",
            ))
        };
    }
    if allow_current && path == Path::new(".") {
        return Ok(());
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(CamelliaNexusError::new(
            ErrorCode::InvalidPath,
            "Path cannot contain parent or root components",
        ));
    }
    Ok(())
}

fn validate_path_text(path: &Path) -> Result<()> {
    let text = path.as_os_str().to_string_lossy();
    if text.contains('\0') || text.encode_utf16().count() > 32_000 {
        Err(CamelliaNexusError::new(
            ErrorCode::InvalidPath,
            "Path contains a null byte or is too long",
        ))
    } else {
        Ok(())
    }
}

fn validate_arguments(program_type: &ProgramType) -> Result<()> {
    let args = match program_type {
        ProgramType::Generic { args } => args,
        ProgramType::SingBox { extra_args, .. }
        | ProgramType::Xray { extra_args, .. }
        | ProgramType::Mihomo { extra_args, .. } => extra_args,
    };
    let units = args
        .iter()
        .map(|argument| argument.encode_utf16().count().saturating_add(1))
        .sum::<usize>();
    if args.len() > MAX_ARGUMENTS
        || units > MAX_ARGUMENT_UTF16_UNITS
        || args
            .iter()
            .any(|argument| argument.contains(['\0', '\r', '\n']))
    {
        return Err(CamelliaNexusError::invalid_spec(
            "Arguments exceed 256 entries or the safe command-line length",
        ));
    }
    Ok(())
}

fn validate_config_arguments(program_type: &ProgramType) -> Result<()> {
    let flags: &[&str] = match program_type {
        ProgramType::Generic { .. } => return Ok(()),
        ProgramType::SingBox { .. } => &["-c", "--config", "-C", "--config-directory"],
        ProgramType::Xray { .. } => &["-c", "-config", "-confdir"],
        ProgramType::Mihomo { .. } => &["-f", "--f", "-config", "--config"],
    };
    let args = program_type.arguments();
    for (index, argument) in args.iter().enumerate() {
        if flags.iter().any(|flag| argument == flag)
            && args.get(index + 1).is_none_or(String::is_empty)
        {
            return Err(CamelliaNexusError::invalid_spec(format!(
                "Configuration argument {argument} requires a path"
            )));
        }
        if flags.iter().any(|flag| argument == &format!("{flag}=")) {
            return Err(CamelliaNexusError::invalid_spec(format!(
                "Configuration argument {argument} requires a path"
            )));
        }
    }
    Ok(())
}

fn validate_mihomo_contract(spec: &ProgramSpec) -> Result<()> {
    let ProgramType::Mihomo {
        main_config,
        extra_args,
    } = &spec.program_type
    else {
        return Ok(());
    };
    if contains_argument_option(extra_args, &["-config", "--config"]) {
        return Err(CamelliaNexusError::invalid_spec(
            "Mihomo inline configuration is unavailable; use a configuration file",
        ));
    }
    if main_config.is_some() && contains_argument_option(extra_args, &["-f", "--f"]) {
        return Err(CamelliaNexusError::invalid_spec(
            "Mihomo configuration path arguments conflict with the editable main configuration",
        ));
    }
    if spec
        .environment
        .keys()
        .any(|key| key.eq_ignore_ascii_case("CLASH_CONFIG_STRING"))
    {
        return Err(CamelliaNexusError::invalid_spec(
            "CLASH_CONFIG_STRING is unavailable; use a configuration file",
        ));
    }
    if contains_argument_option(extra_args, &["-age-secret-key", "--age-secret-key"])
        || spec
            .environment
            .keys()
            .any(|key| key.eq_ignore_ascii_case("CLASH_AGE_SECRET_KEY"))
    {
        return Err(CamelliaNexusError::invalid_spec(
            "Mihomo age secret keys must not be stored in program arguments or environment variables",
        ));
    }
    if spec
        .managed_config
        .as_ref()
        .is_some_and(|managed| managed.mihomo_dashboard.is_some())
    {
        let dashboard_arguments = ["-ext-ui", "--ext-ui", "-ext-ctl", "--ext-ctl"];
        if contains_argument_option(extra_args, &dashboard_arguments)
            || spec.environment.keys().any(|key| {
                key.eq_ignore_ascii_case("CLASH_OVERRIDE_EXTERNAL_UI_DIR")
                    || key.eq_ignore_ascii_case("CLASH_OVERRIDE_EXTERNAL_CONTROLLER")
            })
        {
            return Err(CamelliaNexusError::invalid_spec(
                "Mihomo dashboard command-line and environment overrides are unavailable while the managed dashboard is enabled",
            ));
        }
    }
    Ok(())
}

fn contains_argument_option(arguments: &[String], flags: &[&str]) -> bool {
    arguments.iter().any(|argument| {
        flags
            .iter()
            .any(|flag| argument == flag || argument.starts_with(&format!("{flag}=")))
    })
}

fn validate_environment(environment: &BTreeMap<String, String>) -> Result<()> {
    let units = environment
        .iter()
        .map(|(key, value)| {
            key.encode_utf16()
                .count()
                .saturating_add(value.encode_utf16().count())
                .saturating_add(2)
        })
        .sum::<usize>();
    let invalid_entry = environment.iter().any(|(key, value)| {
        key.is_empty() || key.len() > 256 || key.contains(['=', '\0']) || value.contains('\0')
    });
    let mut normalized_keys = BTreeSet::new();
    let duplicate_portable_key = environment
        .keys()
        .any(|key| !normalized_keys.insert(key.to_uppercase()));
    if environment.len() > MAX_ENVIRONMENT_ENTRIES
        || units > MAX_ENVIRONMENT_UTF16_UNITS
        || invalid_entry
        || duplicate_portable_key
    {
        return Err(CamelliaNexusError::invalid_spec(
            "Environment exceeds 256 entries, contains an invalid or duplicate key, or is too large",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ProgramState {
    #[default]
    Stopped,
    Starting,
    Running {
        pid: u32,
        started_unix_ms: u64,
    },
    Stopping,
    Exited {
        code: Option<i32>,
        success: bool,
    },
    Backoff {
        attempt: u32,
        delay_seconds: u64,
    },
    StopFailed {
        pid: u32,
        message: String,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
}

impl ProgramState {
    pub fn unix_ms_now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgramSummary {
    pub id: ProgramId,
    pub name: String,
    pub kind: ProgramKind,
    pub auto_start: bool,
    pub state: ProgramState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigDocument {
    pub content: String,
    pub base_hash: String,
    pub language: EditorLanguage,
    pub documentation_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration_schema: Option<ConfigurationSchemaDescriptor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigurationSchemaSource {
    ProgramBinary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JsonSchemaDialect {
    #[serde(rename = "draft2020-12")]
    Draft202012,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationSchemaDescriptor {
    pub source: ConfigurationSchemaSource,
    pub dialect: JsonSchemaDialect,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationSchemaDocument {
    pub source: ConfigurationSchemaSource,
    pub dialect: JsonSchemaDialect,
    pub content: String,
    pub content_hash: String,
}

#[derive(Debug, Clone)]
pub struct RawConfig {
    pub content: String,
    pub base_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EditorLanguage {
    Jsonc,
    Yaml,
    Toml,
    Text,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorDescriptor {
    pub language: EditorLanguage,
    pub documentation_url: String,
    pub configuration_schema: Option<ConfigurationSchemaDescriptor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActionState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Exited,
    Backoff,
    Error,
}

impl From<&ProgramState> for ActionState {
    fn from(state: &ProgramState) -> Self {
        match state {
            ProgramState::Stopped => Self::Stopped,
            ProgramState::Starting => Self::Starting,
            ProgramState::Running { .. } => Self::Running,
            ProgramState::Stopping => Self::Stopping,
            ProgramState::Exited { .. } => Self::Exited,
            ProgramState::Backoff { .. } => Self::Backoff,
            ProgramState::StopFailed { .. } => Self::Error,
            ProgramState::Error { .. } => Self::Error,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionDescriptor {
    pub id: String,
    pub label: String,
    pub allowed_states: Vec<ActionState>,
    pub confirmation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionResult {
    pub stdout: String,
    pub stderr: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationResult {
    pub valid: bool,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LogStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogChunk {
    pub content: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ManagerEvent {
    ProgramStateChanged { id: ProgramId, state: ProgramState },
    ProgramListChanged,
    ProgramAutoStartPrivilegeRequired { ids: Vec<ProgramId> },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_id_validation_is_strict() {
        assert!(ProgramId::parse("sing-box-main").is_ok());
        assert!(ProgramId::parse("SingBox").is_err());
        assert!(ProgramId::parse("-bad").is_err());
        assert!(ProgramId::parse("../bad").is_err());
    }

    #[test]
    fn program_specific_args_accept_and_detect_explicit_config() {
        let ty = ProgramType::Xray {
            main_config: Some("config/config.json".into()),
            extra_args: vec!["-config=other.json".into()],
        };
        assert!(ty.has_explicit_config());
    }

    #[test]
    fn explicit_config_flag_requires_a_value() {
        let ty = ProgramType::SingBox {
            main_config: None,
            extra_args: vec!["--config".into()],
        };
        assert!(validate_config_arguments(&ty).is_err());
        let valid = ProgramType::SingBox {
            main_config: None,
            extra_args: vec!["--config".into(), "/etc/sing-box/config.json".into()],
        };
        assert!(validate_config_arguments(&valid).is_ok());
    }

    #[test]
    fn relative_path_rejects_escape() {
        assert!(validate_relative_path(Path::new("config/main.json"), false).is_ok());
        assert!(validate_relative_path(Path::new("../main.json"), false).is_err());
    }

    #[test]
    fn managed_working_directory_follows_executable_parent() {
        let mut spec = ProgramSpec {
            schema_version: SCHEMA_VERSION,
            id: ProgramId::parse("nested-tool").expect("id"),
            name: "Nested tool".into(),
            executable: ExecutableSpec::Managed {
                path: "bin/tools/program".into(),
                metadata: None,
            },
            program_type: ProgramType::Generic { args: Vec::new() },
            managed_config: None,
            working_directory: "bin".into(),
            environment: BTreeMap::new(),
            auto_start: false,
            restart_policy: RestartPolicy::Never,
            privilege_policy: Default::default(),
        };
        assert!(spec.normalize_runtime_directory().expect("normalize"));
        assert_eq!(spec.working_directory, Path::new("bin/tools"));
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn current_program_contract_rejects_unknown_fields() {
        let spec = ProgramSpec {
            schema_version: SCHEMA_VERSION,
            id: ProgramId::parse("strict-contract").expect("id"),
            name: "Strict contract".into(),
            executable: ExecutableSpec::Managed {
                path: "bin/program".into(),
                metadata: None,
            },
            program_type: ProgramType::Generic { args: Vec::new() },
            managed_config: None,
            working_directory: "bin".into(),
            environment: BTreeMap::new(),
            auto_start: false,
            restart_policy: RestartPolicy::Never,
            privilege_policy: Default::default(),
        };

        let mut top_level = serde_json::to_value(&spec).expect("serialize spec");
        top_level["obsolete"] = serde_json::Value::Bool(true);
        assert!(serde_json::from_value::<ProgramSpec>(top_level).is_err());

        let mut missing_policy = serde_json::to_value(&spec).expect("serialize spec");
        missing_policy
            .as_object_mut()
            .expect("object")
            .remove("privilegePolicy");
        assert!(serde_json::from_value::<ProgramSpec>(missing_policy).is_err());

        let mut nested = serde_json::to_value(spec).expect("serialize spec");
        nested["executable"]["obsolete"] = serde_json::Value::Bool(true);
        assert!(serde_json::from_value::<ProgramSpec>(nested).is_err());
    }

    #[test]
    fn tagged_enum_fields_use_camel_case() {
        let value = serde_json::to_value(ProgramType::SingBox {
            main_config: Some("config/config.json".into()),
            extra_args: vec!["--verbose".into()],
        })
        .expect("serialize");
        assert_eq!(value["kind"], "singBox");
        assert_eq!(value["mainConfig"], "config/config.json");
        assert!(value.get("main_config").is_none());

        let state = serde_json::to_value(ProgramState::Running {
            pid: 42,
            started_unix_ms: 10,
        })
        .expect("serialize");
        assert_eq!(state["startedUnixMs"], 10);
    }

    #[test]
    fn configuration_schema_contract_is_explicit_and_camel_case() {
        let document = ConfigurationSchemaDocument {
            source: ConfigurationSchemaSource::ProgramBinary,
            dialect: JsonSchemaDialect::Draft202012,
            content: r#"{"type":"object"}"#.into(),
            content_hash: "a".repeat(64),
        };
        let value = serde_json::to_value(document).expect("serialize configuration schema");
        assert_eq!(value["source"], "programBinary");
        assert_eq!(value["dialect"], "draft2020-12");
        assert_eq!(value["contentHash"], "a".repeat(64));
        assert!(value.get("content_hash").is_none());

        let config = ConfigDocument {
            content: "{}".into(),
            base_hash: "hash".into(),
            language: EditorLanguage::Jsonc,
            documentation_url: "https://example.test".into(),
            configuration_schema: None,
        };
        assert!(
            serde_json::to_value(config)
                .expect("serialize config document")
                .get("configurationSchema")
                .is_none()
        );
    }

    fn mihomo_spec(extra_args: Vec<String>) -> ProgramSpec {
        ProgramSpec {
            schema_version: SCHEMA_VERSION,
            id: ProgramId::parse("mihomo-main").expect("id"),
            name: "Mihomo".into(),
            executable: ExecutableSpec::Managed {
                path: "bin/mihomo".into(),
                metadata: None,
            },
            program_type: ProgramType::Mihomo {
                main_config: Some("config/managed.yaml".into()),
                extra_args,
            },
            managed_config: None,
            working_directory: "bin".into(),
            environment: BTreeMap::new(),
            auto_start: false,
            restart_policy: RestartPolicy::Never,
            privilege_policy: Default::default(),
        }
    }

    #[test]
    fn mihomo_type_and_dashboard_use_the_peer_storage_contract() {
        let mut spec = mihomo_spec(Vec::new());
        spec.managed_config = Some(ManagedConfigSpec {
            mihomo_dashboard: Some(MihomoDashboardSpec {
                listen_port: 9092,
                download_url: Some("https://example.test/dashboard.zip".into()),
            }),
            ..ManagedConfigSpec::default()
        });
        assert!(spec.validate().is_ok());
        let value = serde_json::to_value(&spec).expect("serialize Mihomo program");
        assert_eq!(value["type"]["kind"], "mihomo");
        assert_eq!(value["type"]["mainConfig"], "config/managed.yaml");
        assert_eq!(
            value["managedConfig"]["mihomoDashboard"]["listenPort"],
            9092
        );
    }

    #[test]
    fn mihomo_rejects_ambiguous_configuration_inputs() {
        let mut spec = mihomo_spec(vec!["-f=/tmp/other.yaml".into()]);
        assert!(spec.validate().is_err());

        spec.program_type = ProgramType::Mihomo {
            main_config: None,
            extra_args: vec!["-f=/tmp/other.yaml".into()],
        };
        assert!(spec.validate().is_ok());

        spec.program_type = ProgramType::Mihomo {
            main_config: None,
            extra_args: vec!["-config=Zm9v".into()],
        };
        assert!(spec.validate().is_err());

        spec.program_type = ProgramType::Mihomo {
            main_config: None,
            extra_args: Vec::new(),
        };
        spec.environment
            .insert("clash_config_string".into(), "Zm9v".into());
        assert!(spec.validate().is_err());
    }

    #[test]
    fn mihomo_managed_dashboard_owns_its_cli_and_environment_inputs() {
        let mut spec = mihomo_spec(vec!["-ext-ctl=0.0.0.0:9090".into()]);
        spec.managed_config = Some(ManagedConfigSpec {
            mihomo_dashboard: Some(MihomoDashboardSpec {
                listen_port: 9092,
                download_url: None,
            }),
            ..ManagedConfigSpec::default()
        });
        assert!(spec.validate().is_err());

        if let ProgramType::Mihomo { extra_args, .. } = &mut spec.program_type {
            extra_args.clear();
        }
        spec.environment
            .insert("CLASH_OVERRIDE_EXTERNAL_UI_DIR".into(), "custom-ui".into());
        assert!(spec.validate().is_err());
    }

    #[test]
    fn mihomo_rejects_private_keys_in_process_metadata() {
        let mut spec = mihomo_spec(vec!["-age-secret-key".into(), "AGE-SECRET-KEY-TEST".into()]);
        assert!(spec.validate().is_err());

        if let ProgramType::Mihomo { extra_args, .. } = &mut spec.program_type {
            extra_args.clear();
        }
        spec.environment
            .insert("CLASH_AGE_SECRET_KEY".into(), "AGE-SECRET-KEY-TEST".into());
        assert!(spec.validate().is_err());
    }

    #[test]
    fn remote_basic_authentication_is_validated_without_serializing_the_password() {
        let source = ConfigSourceSpec::Remote {
            id: "remote-main".into(),
            name: "Remote main".into(),
            enabled: true,
            url: "https://example.com/config.json".into(),
            authentication: Some(ConfigSourceAuthentication::Basic {
                username: "subscriber".into(),
                credential_id: Some(format!("cfg-{}", "a".repeat(64))),
                password: Some("secret".into()),
            }),
        };
        let value = serde_json::to_value(&source).expect("serialize source");
        assert_eq!(value["mode"], "remote");
        assert_eq!(value["authentication"]["scheme"], "basic");
        assert_eq!(value["authentication"]["username"], "subscriber");
        assert!(value["authentication"].get("password").is_none());

        let mut spec = ProgramSpec {
            schema_version: SCHEMA_VERSION,
            id: ProgramId::parse("remote-auth").expect("id"),
            name: "Remote auth".into(),
            executable: ExecutableSpec::Managed {
                path: "bin/sing-box".into(),
                metadata: None,
            },
            program_type: ProgramType::SingBox {
                main_config: Some("config/managed.json".into()),
                extra_args: Vec::new(),
            },
            managed_config: Some(ManagedConfigSpec {
                sources: vec![source],
                remote_update: Some(RemoteUpdateSpec {
                    enabled: true,
                    interval_minutes: 60,
                }),
                sing_box_dashboard: None,
                sing_box_clash_dashboard: None,
                xray_dashboard: None,
                mihomo_dashboard: None,
            }),
            working_directory: "bin".into(),
            environment: BTreeMap::new(),
            auto_start: false,
            restart_policy: RestartPolicy::Never,
            privilege_policy: Default::default(),
        };
        assert!(spec.validate().is_ok());
        assert_eq!(
            spec.managed_config
                .as_ref()
                .and_then(ManagedConfigSpec::automatic_remote_update_minutes),
            Some(60),
        );
        if let Some(managed) = spec.managed_config.as_mut()
            && let Some(update) = managed.remote_update.as_mut()
        {
            update.enabled = false;
        }
        assert_eq!(
            spec.managed_config
                .as_ref()
                .and_then(ManagedConfigSpec::automatic_remote_update_minutes),
            None,
        );
        if let Some(managed) = spec.managed_config.as_mut() {
            managed.remote_update = Some(RemoteUpdateSpec {
                enabled: true,
                interval_minutes: 4,
            });
        }
        assert!(spec.validate().is_err());
        if let Some(managed) = spec.managed_config.as_mut() {
            managed.remote_update = Some(RemoteUpdateSpec {
                enabled: true,
                interval_minutes: 60,
            });
        }
        if let Some(ManagedConfigSpec { sources, .. }) = spec.managed_config.as_mut()
            && let ConfigSourceSpec::Remote {
                authentication: Some(ConfigSourceAuthentication::Basic { username, .. }),
                ..
            } = &mut sources[0]
        {
            *username = "invalid:name".into();
        }
        assert!(spec.validate().is_err());
    }

    #[test]
    fn managed_configuration_accepts_team_source_limit_and_rejects_overflow() {
        let sources = (0..MAX_CONFIG_SOURCES)
            .map(|index| ConfigSourceSpec::Local {
                id: format!("source-{index}"),
                name: format!("Source {index}"),
                enabled: true,
                path: format!("config/source-{index}.json").into(),
            })
            .collect();
        let mut spec = ProgramSpec {
            schema_version: SCHEMA_VERSION,
            id: ProgramId::parse("team-sources").expect("id"),
            name: "Team sources".into(),
            executable: ExecutableSpec::Managed {
                path: "bin/sing-box".into(),
                metadata: None,
            },
            program_type: ProgramType::SingBox {
                main_config: Some("config/managed.json".into()),
                extra_args: Vec::new(),
            },
            managed_config: Some(ManagedConfigSpec {
                sources,
                ..ManagedConfigSpec::default()
            }),
            working_directory: "bin".into(),
            environment: BTreeMap::new(),
            auto_start: false,
            restart_policy: RestartPolicy::Never,
            privilege_policy: Default::default(),
        };

        assert!(spec.validate().is_ok());
        spec.managed_config
            .as_mut()
            .expect("managed config")
            .sources
            .push(ConfigSourceSpec::Local {
                id: "source-overflow".into(),
                name: "Overflow".into(),
                enabled: true,
                path: "config/overflow.json".into(),
            });
        assert!(spec.validate().is_err());
    }

    #[test]
    fn environment_keys_are_portable_across_platforms() {
        let environment = BTreeMap::from([
            ("Path".to_owned(), "first".to_owned()),
            ("PATH".to_owned(), "second".to_owned()),
        ]);
        assert!(validate_environment(&environment).is_err());

        let unicode_environment = BTreeMap::from([
            ("Straße".to_owned(), "first".to_owned()),
            ("STRASSE".to_owned(), "second".to_owned()),
        ]);
        assert!(validate_environment(&unicode_environment).is_err());
    }

    #[test]
    fn dashboard_intervals_require_complete_duration_parts() {
        for valid in ["30s", "12h", "1d", "1h30m"] {
            assert!(valid_dashboard_interval(valid), "{valid}");
        }
        for invalid in ["", "1", "h", "1h30", "1 hour", "1w"] {
            assert!(!valid_dashboard_interval(invalid), "{invalid}");
        }
    }
}
