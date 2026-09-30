use super::*;
use crate::CoreCapabilityProfile;
use std::sync::{Mutex, OnceLock};

static DESCRIPTORS: OnceLock<Mutex<BTreeMap<String, Vec<GuidedSettingDescriptor>>>> =
    OnceLock::new();

#[cfg(test)]
mod tests;

struct SettingSpec {
    id: &'static str,
    category: &'static str,
    label: &'static str,
    path: &'static [&'static str],
    control: GuidedControl,
    options: &'static [&'static str],
    advanced: bool,
    maximum: Option<u64>,
}

macro_rules! setting {
    ($id:literal, $group:literal, $label:literal, [$($path:literal),+], $control:ident, [$($option:literal),*], $advanced:literal) => {
        SettingSpec { id: $id, category: $group, label: $label, path: &[$($path),+], control: GuidedControl::$control, options: &[$($option),*], advanced: $advanced, maximum: None }
    };
}

const SING_BOX_SETTINGS: &[SettingSpec] = &[
    setting!(
        "logging.level",
        "logging",
        "Log detail",
        ["log", "level"],
        Select,
        ["trace", "debug", "info", "warn", "error", "fatal", "panic"],
        false
    ),
    setting!(
        "logging.timestamp",
        "logging",
        "Show log time",
        ["log", "timestamp"],
        Toggle,
        [],
        true
    ),
    setting!(
        "dns.strategy",
        "dns",
        "DNS address preference",
        ["dns", "strategy"],
        Select,
        ["prefer_ipv4", "prefer_ipv6", "ipv4_only", "ipv6_only"],
        false
    ),
    setting!(
        "dns.disableCache",
        "dns",
        "Disable DNS cache",
        ["dns", "disable_cache"],
        Toggle,
        [],
        true
    ),
    setting!(
        "dns.timeout",
        "dns",
        "DNS timeout",
        ["dns", "timeout"],
        Text,
        [],
        true
    ),
    setting!(
        "routing.autoDetectInterface",
        "routing",
        "Choose network interface automatically",
        ["route", "auto_detect_interface"],
        Toggle,
        [],
        false
    ),
    setting!(
        "routing.defaultInterface",
        "routing",
        "Network interface",
        ["route", "default_interface"],
        Text,
        [],
        true
    ),
    setting!(
        "routing.final",
        "routing",
        "Default connection",
        ["route", "final"],
        Text,
        [],
        false
    ),
    setting!(
        "dns.final",
        "dns",
        "Default DNS server",
        ["dns", "final"],
        Text,
        [],
        false
    ),
];

const XRAY_SETTINGS: &[SettingSpec] = &[
    setting!(
        "logging.level",
        "logging",
        "Log detail",
        ["log", "loglevel"],
        Select,
        ["debug", "info", "warning", "error", "none"],
        false
    ),
    setting!(
        "logging.dns",
        "logging",
        "Log DNS requests",
        ["log", "dnsLog"],
        Toggle,
        [],
        true
    ),
    setting!(
        "logging.maskAddress",
        "logging",
        "Hide addresses in logs",
        ["log", "maskAddress"],
        Select,
        ["", "quarter", "half", "full"],
        true
    ),
    setting!(
        "routing.domainStrategy",
        "routing",
        "Domain matching",
        ["routing", "domainStrategy"],
        Select,
        ["AsIs", "IPIfNonMatch", "IPOnDemand"],
        false
    ),
    setting!(
        "dns.queryStrategy",
        "dns",
        "DNS address preference",
        ["dns", "queryStrategy"],
        Select,
        ["UseIP", "UseIPv4", "UseIPv6"],
        false
    ),
    setting!(
        "dns.disableCache",
        "dns",
        "Disable DNS cache",
        ["dns", "disableCache"],
        Toggle,
        [],
        true
    ),
];

const MIHOMO_SETTINGS: &[SettingSpec] = &[
    setting!(
        "logging.level",
        "logging",
        "Log detail",
        ["log-level"],
        Select,
        ["debug", "info", "warning", "error", "silent"],
        false
    ),
    setting!(
        "network.ipv6",
        "routing",
        "Allow IPv6 connections",
        ["ipv6"],
        Toggle,
        [],
        true
    ),
    setting!(
        "routing.mode",
        "routing",
        "Traffic mode",
        ["mode"],
        Select,
        ["rule", "global", "direct"],
        false
    ),
    setting!(
        "dns.enabled",
        "dns",
        "Use program DNS",
        ["dns", "enable"],
        Toggle,
        [],
        false
    ),
    setting!(
        "dns.mode",
        "dns",
        "DNS mode",
        ["dns", "enhanced-mode"],
        Select,
        ["normal", "fake-ip", "redir-host"],
        false
    ),
    setting!(
        "dns.ipv6",
        "dns",
        "Resolve IPv6 addresses",
        ["dns", "ipv6"],
        Toggle,
        [],
        true
    ),
    setting!(
        "tun.enabled",
        "tun",
        "Use a virtual network adapter",
        ["tun", "enable"],
        Toggle,
        [],
        false
    ),
    setting!(
        "tun.autoRoute",
        "tun",
        "Route traffic automatically",
        ["tun", "auto-route"],
        Toggle,
        [],
        false
    ),
    setting!(
        "tun.autoDetectInterface",
        "tun",
        "Choose network interface automatically",
        ["tun", "auto-detect-interface"],
        Toggle,
        [],
        false
    ),
    setting!(
        "tun.strictRoute",
        "tun",
        "Prevent traffic outside these routes",
        ["tun", "strict-route"],
        Toggle,
        [],
        true
    ),
    setting!(
        "tun.stack",
        "tun",
        "Virtual adapter implementation",
        ["tun", "stack"],
        Select,
        ["system", "gvisor", "mixed"],
        true
    ),
    SettingSpec {
        maximum: Some(65535),
        ..setting!(
            "tun.mtu",
            "tun",
            "Packet size (MTU)",
            ["tun", "mtu"],
            Number,
            [],
            true
        )
    },
];

fn specs(kind: ProgramKind) -> &'static [SettingSpec] {
    match kind {
        ProgramKind::SingBox => SING_BOX_SETTINGS,
        ProgramKind::Xray => XRAY_SETTINGS,
        ProgramKind::Mihomo => MIHOMO_SETTINGS,
        ProgramKind::Generic => &[],
    }
}

pub(super) fn intent_setting_path(kind: ProgramKind, id: &str) -> Option<&'static [&'static str]> {
    specs(kind)
        .iter()
        .find(|spec| spec.id == id)
        .map(|spec| spec.path)
}

pub(super) fn intent_setting_descriptors(kind: ProgramKind) -> Vec<GuidedSettingDescriptor> {
    specs(kind)
        .iter()
        .map(|spec| GuidedSettingDescriptor {
            id: spec.id.into(),
            category: spec.category.into(),
            label: spec.label.into(),
            description: String::new(),
            control: spec.control,
            allowed_values: spec.options.iter().map(|option| (*option).into()).collect(),
            enabled_when: (spec.category == "tun"
                && spec.id != "tun.enabled"
                && kind == ProgramKind::Mihomo)
                .then(|| "tun.enabled".into()),
            advanced: spec.advanced,
            minimum: (spec.control == GuidedControl::Number).then_some(0),
            maximum: spec.maximum,
            default_value: None,
            available: true,
            unavailable_reason: None,
        })
        .collect()
}

fn setting_probe(spec: &SettingSpec, value: &Value) -> Result<Value> {
    let mut document = json!({});
    set_object_path(&mut document, spec.path, value.clone())?;
    if spec.category == "tun" {
        set_object_path(&mut document, &["tun", "enable"], json!(true))?;
    }
    Ok(document)
}

fn supports_setting(
    profile: &CoreCapabilityProfile,
    spec: &SettingSpec,
    value: &Value,
) -> Result<bool> {
    let document = setting_probe(spec, value)?;
    Ok(
        crate::configuration_field_evidence::undeclared_entries(profile, &document)?.is_empty()
            && profile.assess(&document)?.issues.is_empty(),
    )
}

/// Resolves controls against the exact reviewed release and observed build.
pub fn project_intent_capabilities(
    profile: &CoreCapabilityProfile,
    view: &mut ConfigurationStateView,
) -> Result<()> {
    profile.assess(&json!({}))?;
    let document = parse_semantic_document(view.format, view.desired.content.as_bytes())?;
    let cache = DESCRIPTORS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let cached = cache
        .lock()
        .map_err(|_| CamelliaNexusError::invalid_spec("Intent catalog unavailable"))?
        .get(&profile.profile_hash)
        .cloned();
    let mut descriptors = if let Some(descriptors) = cached {
        descriptors
    } else {
        resolve_descriptors(profile)?
    };
    {
        let mut cache = cache
            .lock()
            .map_err(|_| CamelliaNexusError::invalid_spec("Intent catalog unavailable"))?;
        if cache.len() >= 64 {
            cache.clear();
        }
        cache.insert(profile.profile_hash.clone(), descriptors.clone());
    }
    descriptors.retain(|descriptor| {
        descriptor.available
            || intent_setting_path(profile.program, &descriptor.id)
                .and_then(|path| get_object_path(&document, path))
                .is_some()
    });
    for projection in &mut view.guided_projection {
        if let Some(path) = intent_setting_path(profile.program, &projection.setting_id) {
            projection.value = get_object_path(&document, path).cloned();
        }
    }
    view.guided_descriptors = descriptors;
    Ok(())
}

fn resolve_descriptors(profile: &CoreCapabilityProfile) -> Result<Vec<GuidedSettingDescriptor>> {
    let mut descriptors = Vec::new();
    for (spec, mut descriptor) in specs(profile.program)
        .iter()
        .zip(intent_setting_descriptors(profile.program))
    {
        let probe = match spec.control {
            GuidedControl::Toggle => json!(true),
            GuidedControl::Number => json!(1500),
            GuidedControl::Select => json!(spec.options.first().copied().unwrap_or_default()),
            GuidedControl::Text => json!(if spec.id == "dns.timeout" {
                "5s"
            } else {
                "intent-probe"
            }),
        };
        descriptor.available = supports_setting(profile, spec, &probe)?;
        if spec.control == GuidedControl::Select {
            descriptor
                .allowed_values
                .retain(|option| supports_setting(profile, spec, &json!(option)).unwrap_or(false));
            descriptor.available &= !descriptor.allowed_values.is_empty();
        }
        if !descriptor.available {
            descriptor.unavailable_reason =
                Some("This setting is not available in this program.".into());
        }
        descriptors.push(descriptor);
    }
    Ok(descriptors)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ConfigurationIntentAction {
    Set {
        setting_id: String,
        value: Value,
    },
    Follow {
        setting_id: String,
    },
    CreateObject {
        object_kind: IntentObjectKind,
        values: BTreeMap<String, Value>,
    },
    UpdateObject {
        object_id: String,
        expected_hash: String,
        values: BTreeMap<String, Value>,
    },
    RemoveObject {
        object_id: String,
        expected_hash: String,
    },
    FollowObject {
        object_id: String,
        expected_hash: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationIntentRequest {
    pub operation_id: String,
    pub expected_state_revision: u64,
    pub editor_session_id: Option<String>,
    pub expected_draft_revision: Option<u64>,
    pub change: ConfigurationIntentAction,
}

impl ConfigurationIntentRequest {
    pub fn context(&self) -> Result<ConfigurationMutationContext> {
        Ok(ConfigurationMutationContext {
            operation_id: self.operation_id.clone(),
            kind: ConfigurationOperationKind::Intent,
            expected_state_revision: self.expected_state_revision,
            editor_session_id: self.editor_session_id.clone(),
            expected_draft_revision: self.expected_draft_revision,
            payload_hash: Some(hash_bytes(&serde_json::to_vec(&self.change)?)),
        })
    }
}

impl ConfigurationState {
    pub fn update_intent(
        &mut self,
        profile: &CoreCapabilityProfile,
        change: &ConfigurationIntentAction,
        now: u64,
    ) -> Result<()> {
        if profile.program != self.kind {
            return Err(CamelliaNexusError::invalid_spec(
                "Intent profile does not match program",
            ));
        }
        match change {
            ConfigurationIntentAction::Set { setting_id, value } => {
                validate_guided_value(self.kind, setting_id, value)?;
                let spec = specs(self.kind)
                    .iter()
                    .find(|spec| spec.id == setting_id)
                    .ok_or_else(|| CamelliaNexusError::invalid_spec("Unknown Intent setting"))?;
                if !supports_setting(profile, spec, value)? {
                    return Err(CamelliaNexusError::new(
                        ErrorCode::ConfigInvalid,
                        "Setting is not supported by this program",
                    )
                    .with_message_key("INTENT_SETTING_UNAVAILABLE"));
                }
                if setting_id == "dns.timeout"
                    && parse_dashboard_interval_nanos(value.as_str().unwrap_or_default()).is_none()
                {
                    return Err(CamelliaNexusError::new(
                        ErrorCode::ConfigInvalid,
                        "Enter a positive duration",
                    )
                    .with_message_key("INTENT_DURATION_INVALID"));
                }
                if matches!(setting_id.as_str(), "routing.final" | "dns.final") {
                    let document =
                        parse_semantic_document(self.format, self.desired.content.as_bytes())?;
                    let target_kind = if setting_id == "routing.final" {
                        "connection"
                    } else {
                        "dns"
                    };
                    let target = value.as_str().unwrap_or_default();
                    if !intent_targets(self.kind, &document)
                        .iter()
                        .any(|item| item.kind == target_kind && item.id == target)
                    {
                        return Err(CamelliaNexusError::new(
                            ErrorCode::ConfigInvalid,
                            "Choose an existing target",
                        )
                        .with_message_key("INTENT_TARGET_MISSING"));
                    }
                }
                if self.guided_intent.values.get(setting_id) == Some(value) {
                    self.claim_guided_setting(setting_id)?;
                } else {
                    self.guided_intent.set(setting_id.clone(), value.clone());
                }
            }
            ConfigurationIntentAction::Follow { setting_id } => {
                if intent_setting_path(self.kind, setting_id).is_none() {
                    return Err(CamelliaNexusError::invalid_spec("Unknown Intent setting"));
                }
                self.guided_intent.reset(setting_id);
            }
            _ => self.update_intent_object(profile, change)?,
        }
        self.rebuild_desired(now)
    }
}
