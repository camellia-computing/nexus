use std::{path::Path, sync::Arc, time::Duration};

use camellia_nexus_core::{
    CamelliaNexusError, ConfigSourceAuthentication, ConfigSourceSpec, ErrorCode, MAX_CONFIG_BYTES,
    ManagedConfigSpec, ProgramKind, ProgramSpec, Result,
};
use reqwest::{Client, redirect::Policy};
use serde_json::{Map, Value};
use serde_yaml_ng::{Mapping as YamlMapping, Value as YamlValue};
use tokio::io::AsyncReadExt;

const MAX_SOURCE_BYTES: usize = MAX_CONFIG_BYTES;
const MAX_TOTAL_SOURCE_BYTES: usize = 16 * 1024 * 1024;
const MAX_CONCURRENT_SOURCES: usize = 4;
const SOURCE_TIMEOUT: Duration = Duration::from_secs(25);

struct ResolvedSource {
    name: String,
    content: Vec<u8>,
    append_xray_outbounds: bool,
}

pub async fn materialize(
    spec: &ProgramSpec,
    fallback: Option<String>,
    require_sources: bool,
    local_base: Option<&Path>,
    credentials: &crate::config_credentials::CredentialSnapshot,
) -> Result<String> {
    let managed = spec.managed_config.as_ref().ok_or_else(|| {
        CamelliaNexusError::new(
            ErrorCode::InvalidState,
            "Managed configuration is not enabled",
        )
    })?;
    let enabled: Vec<_> = managed
        .sources
        .iter()
        .filter(|source| source.enabled())
        .collect();
    let mut content = if enabled.is_empty() {
        if require_sources {
            return Err(CamelliaNexusError::invalid_spec(
                "Enable at least one configuration source before updating",
            ));
        }
        fallback.unwrap_or_else(|| "{}".into())
    } else {
        let sources = resolve_sources(&enabled, local_base, credentials).await?;
        merge_sources(spec.program_type.kind(), &sources)?
    };
    content = apply_managed_features(spec, content)?;
    if content.len() > MAX_CONFIG_BYTES {
        return Err(CamelliaNexusError::invalid_spec(
            "Merged configuration exceeds the 4 MiB limit",
        ));
    }
    Ok(content)
}

pub fn apply_managed_features(spec: &ProgramSpec, content: String) -> Result<String> {
    let Some(managed) = spec.managed_config.as_ref() else {
        return Ok(content);
    };
    match spec.program_type.kind() {
        ProgramKind::Mihomo => apply_mihomo_managed_features(managed, content),
        ProgramKind::Generic | ProgramKind::SingBox | ProgramKind::Xray => {
            apply_json_managed_features(spec.program_type.kind(), managed, content)
        }
    }
}

fn apply_mihomo_managed_features(managed: &ManagedConfigSpec, content: String) -> Result<String> {
    let has_sing_box_dashboard =
        managed.sing_box_dashboard.is_some() || managed.sing_box_clash_dashboard.is_some();
    let has_xray_dashboard = managed.xray_dashboard.is_some();
    if has_sing_box_dashboard || has_xray_dashboard {
        return Err(CamelliaNexusError::invalid_spec(
            "Only the Mihomo Dashboard service is available for Mihomo",
        ));
    }
    let mut root = parse_yaml_mapping("managed configuration", content.as_bytes())?;
    if !crate::programs::mihomo::apply_features(&mut root, managed)? {
        return Ok(content);
    }
    serialize_yaml(root)
}

fn apply_json_managed_features(
    kind: ProgramKind,
    managed: &ManagedConfigSpec,
    content: String,
) -> Result<String> {
    let has_sing_box_dashboard =
        managed.sing_box_dashboard.is_some() || managed.sing_box_clash_dashboard.is_some();
    let has_xray_dashboard = managed.xray_dashboard.is_some();
    let has_mihomo_dashboard = managed.mihomo_dashboard.is_some();
    let mut root = parse_object("managed configuration", content.as_bytes())?;
    let changed = match kind {
        ProgramKind::SingBox => {
            if has_xray_dashboard || has_mihomo_dashboard {
                return Err(CamelliaNexusError::invalid_spec(
                    "Only sing-box Dashboard services are available for sing-box",
                ));
            }
            crate::programs::sing_box::apply_features(&mut root, managed)?
        }
        ProgramKind::Xray => {
            if has_sing_box_dashboard || has_mihomo_dashboard {
                return Err(CamelliaNexusError::invalid_spec(
                    "Only the Xray Dashboard service is available for Xray",
                ));
            }
            crate::programs::xray::apply_features(&mut root, managed)?
        }
        ProgramKind::Generic => {
            if has_sing_box_dashboard || has_xray_dashboard || has_mihomo_dashboard {
                return Err(CamelliaNexusError::invalid_spec(
                    "Dashboard services are only available for supported program types",
                ));
            }
            false
        }
        ProgramKind::Mihomo => {
            return Err(CamelliaNexusError::invalid_spec(
                "Mihomo managed configuration must use YAML",
            ));
        }
    };
    if !changed {
        return Ok(content);
    }
    serde_json::to_string_pretty(&Value::Object(root)).map_err(Into::into)
}

async fn resolve_sources(
    sources: &[&ConfigSourceSpec],
    local_base: Option<&Path>,
    credentials: &crate::config_credentials::CredentialSnapshot,
) -> Result<Vec<ResolvedSource>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = Client::builder()
        .redirect(Policy::limited(5))
        .timeout(SOURCE_TIMEOUT)
        .user_agent(concat!("camellia-nexus/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(CamelliaNexusError::internal)?;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_SOURCES));
    let mut tasks = tokio::task::JoinSet::new();
    for (index, source) in sources.iter().enumerate() {
        let client = client.clone();
        let semaphore = semaphore.clone();
        let source = (*source).clone();
        let local_base = local_base.map(Path::to_path_buf);
        let credentials = credentials.clone();
        tasks.spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(CamelliaNexusError::internal)?;
            resolve_source(&client, &source, local_base.as_deref(), &credentials)
                .await
                .map(|source| (index, source))
        });
    }
    let mut ordered: Vec<Option<ResolvedSource>> = std::iter::repeat_with(|| None)
        .take(sources.len())
        .collect();
    let mut total = 0usize;
    while let Some(task) = tasks.join_next().await {
        let (index, source) = task.map_err(CamelliaNexusError::internal)??;
        total = total.saturating_add(source.content.len());
        if total > MAX_TOTAL_SOURCE_BYTES {
            return Err(CamelliaNexusError::invalid_spec(
                "Configuration sources exceed the 16 MiB aggregate limit",
            ));
        }
        ordered[index] = Some(source);
    }
    ordered
        .into_iter()
        .map(|source| {
            source.ok_or_else(|| {
                CamelliaNexusError::new(ErrorCode::Internal, "Configuration source task was lost")
            })
        })
        .collect()
}

async fn resolve_source(
    client: &Client,
    source: &ConfigSourceSpec,
    local_base: Option<&Path>,
    credentials: &crate::config_credentials::CredentialSnapshot,
) -> Result<ResolvedSource> {
    let (name, content, append_xray_outbounds) = match source {
        ConfigSourceSpec::Local { name, path, .. } => {
            let resolved_path = if path.is_absolute() {
                path.clone()
            } else {
                local_base
                    .ok_or_else(|| {
                        CamelliaNexusError::new(
                            ErrorCode::InvalidPath,
                            "A relative configuration source requires a working folder",
                        )
                    })?
                    .join(path)
            };
            let file_name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            (
                name.clone(),
                read_local_source(&resolved_path)
                    .await
                    .map_err(|error| annotate_source_error(name, error))?,
                contains_tail_marker(file_name),
            )
        }
        ConfigSourceSpec::Remote {
            name,
            url,
            authentication,
            ..
        } => {
            let path = reqwest::Url::parse(url)
                .ok()
                .map(|url| url.path().to_owned())
                .unwrap_or_default();
            (
                name.clone(),
                fetch_remote_source(client, url, authentication.as_ref(), credentials)
                    .await
                    .map_err(|error| annotate_source_error(name, error))?,
                contains_tail_marker(&path),
            )
        }
    };
    Ok(ResolvedSource {
        name,
        content,
        append_xray_outbounds,
    })
}

async fn read_local_source(path: &Path) -> Result<Vec<u8>> {
    let file = tokio::fs::File::open(path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CamelliaNexusError::new(
                ErrorCode::NotFound,
                "Local configuration source was not found",
            )
            .with_details(path.display().to_string())
        } else {
            CamelliaNexusError::new(
                ErrorCode::Storage,
                "Failed to open local configuration source",
            )
            .with_details(error.to_string())
        }
    })?;
    let metadata = file.metadata().await.map_err(|error| {
        CamelliaNexusError::new(
            ErrorCode::Storage,
            "Failed to read local configuration source metadata",
        )
        .with_details(error.to_string())
    })?;
    if !metadata.is_file() || metadata.len() > MAX_SOURCE_BYTES as u64 {
        return Err(CamelliaNexusError::invalid_spec(
            "Local configuration source must be a file no larger than 4 MiB",
        ));
    }
    let mut content = Vec::with_capacity(metadata.len().min(MAX_SOURCE_BYTES as u64) as usize);
    file.take(MAX_SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut content)
        .await
        .map_err(|error| {
            CamelliaNexusError::new(
                ErrorCode::Storage,
                "Failed to read local configuration source",
            )
            .with_details(error.to_string())
        })?;
    if content.len() > MAX_SOURCE_BYTES {
        return Err(CamelliaNexusError::invalid_spec(
            "Local configuration source must be no larger than 4 MiB",
        ));
    }
    Ok(content)
}

async fn fetch_remote_source(
    client: &Client,
    url: &str,
    authentication: Option<&ConfigSourceAuthentication>,
    credentials: &crate::config_credentials::CredentialSnapshot,
) -> Result<Vec<u8>> {
    let parsed = reqwest::Url::parse(url).map_err(|error| {
        CamelliaNexusError::invalid_spec("Invalid remote configuration URL")
            .with_details(error.to_string())
    })?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.host_str().is_none()
    {
        return Err(CamelliaNexusError::invalid_spec(
            "Remote configuration URLs must use HTTPS without embedded credentials",
        ));
    }
    let request = client
        .get(parsed)
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/plain;q=0.9",
        )
        .header(reqwest::header::USER_AGENT, "camellia-nexus/2.0");
    let request = match authentication {
        Some(ConfigSourceAuthentication::Basic {
            username,
            credential_id,
            ..
        }) => {
            let password = credentials.basic_password(credential_id.as_deref(), username)?;
            request.basic_auth(username, Some(password.as_str()))
        }
        None => request,
    };
    let mut response = request
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| {
            CamelliaNexusError::new(
                ErrorCode::Network,
                "Failed to download configuration source",
            )
            .with_details(error.without_url().to_string())
        })?;
    let final_url = response.url();
    if final_url.scheme() != "https"
        || !final_url.username().is_empty()
        || final_url.password().is_some()
    {
        return Err(CamelliaNexusError::invalid_spec(
            "Remote configuration redirects must remain on HTTPS without embedded credentials",
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_SOURCE_BYTES as u64)
    {
        return Err(CamelliaNexusError::invalid_spec(
            "Remote configuration source exceeds the 4 MiB limit",
        ));
    }
    let mut content = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        CamelliaNexusError::new(ErrorCode::Network, "Failed to read configuration source")
            .with_details(error.without_url().to_string())
    })? {
        if content.len().saturating_add(chunk.len()) > MAX_SOURCE_BYTES {
            return Err(CamelliaNexusError::invalid_spec(
                "Remote configuration source exceeds the 4 MiB limit",
            ));
        }
        content.extend_from_slice(&chunk);
    }
    Ok(content)
}

fn merge_sources(kind: ProgramKind, sources: &[ResolvedSource]) -> Result<String> {
    match kind {
        ProgramKind::Mihomo => merge_mihomo_sources(sources),
        ProgramKind::SingBox | ProgramKind::Xray => merge_json_sources(kind, sources),
        ProgramKind::Generic => Err(CamelliaNexusError::invalid_spec(
            "Generic programs do not support managed configuration sources",
        )),
    }
}

fn merge_mihomo_sources(sources: &[ResolvedSource]) -> Result<String> {
    let mut merged = YamlMapping::new();
    for source in sources {
        let next = parse_yaml_mapping(&source.name, &source.content)?;
        crate::programs::mihomo::merge_mapping(&mut merged, next);
    }
    serialize_yaml(merged)
}

fn merge_json_sources(kind: ProgramKind, sources: &[ResolvedSource]) -> Result<String> {
    let mut merged = Map::new();
    for source in sources {
        let next = parse_object(&source.name, &source.content)?;
        match kind {
            ProgramKind::SingBox => crate::programs::sing_box::merge_object(&mut merged, next),
            ProgramKind::Xray => {
                crate::programs::xray::merge_object(&mut merged, next, source.append_xray_outbounds)
            }
            ProgramKind::Generic | ProgramKind::Mihomo => {
                return Err(CamelliaNexusError::invalid_spec(
                    "Program configuration format does not match JSON sources",
                ));
            }
        }
    }
    serde_json::to_string_pretty(&Value::Object(merged)).map_err(Into::into)
}

pub(crate) fn parse_yaml_mapping(name: &str, content: &[u8]) -> Result<YamlMapping> {
    let value: YamlValue = serde_yaml_ng::from_slice(content).map_err(|error| {
        CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "Configuration source is not valid YAML",
        )
        .with_details(format!("{name}: {error}"))
    })?;
    value.as_mapping().cloned().ok_or_else(|| {
        CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "Configuration source root must be a mapping",
        )
        .with_details(name.to_owned())
    })
}

fn serialize_yaml(mapping: YamlMapping) -> Result<String> {
    serde_yaml_ng::to_string(&YamlValue::Mapping(mapping)).map_err(|error| {
        CamelliaNexusError::new(
            ErrorCode::Internal,
            "Failed to serialize YAML configuration",
        )
        .with_details(error.to_string())
    })
}

pub(crate) fn parse_object(name: &str, content: &[u8]) -> Result<Map<String, Value>> {
    let normalized = camellia_nexus_core::normalize_jsonc(content);
    let value: Value = serde_json::from_slice(&normalized).map_err(|error| {
        CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "Configuration source is not valid JSON",
        )
        .with_details(format!("{name}: {error}"))
    })?;
    value.as_object().cloned().ok_or_else(|| {
        CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "Configuration source root must be an object",
        )
        .with_details(name.to_owned())
    })
}

fn contains_tail_marker(value: &str) -> bool {
    value.to_ascii_lowercase().contains("tail")
}

fn annotate_source_error(name: &str, mut error: CamelliaNexusError) -> CamelliaNexusError {
    error.details = Some(match error.details.take() {
        Some(details) => format!("{name}: {details}"),
        None => name.to_owned(),
    });
    error
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn sing_box_spec(dashboard: Option<camellia_nexus_core::SingBoxDashboardSpec>) -> ProgramSpec {
        ProgramSpec {
            schema_version: camellia_nexus_core::SCHEMA_VERSION,
            id: camellia_nexus_core::ProgramId::parse("api-test").expect("id"),
            name: "API test".into(),
            executable: camellia_nexus_core::ExecutableSpec::Managed {
                path: "bin/sing-box".into(),
                metadata: None,
            },
            program_type: camellia_nexus_core::ProgramType::SingBox {
                main_config: Some("config/config.json".into()),
                extra_args: Vec::new(),
            },
            managed_config: Some(camellia_nexus_core::ManagedConfigSpec {
                sources: Vec::new(),
                remote_update: None,
                sing_box_dashboard: dashboard,
                sing_box_clash_dashboard: None,
                xray_dashboard: None,
                mihomo_dashboard: None,
            }),
            working_directory: "bin".into(),
            environment: BTreeMap::new(),
            auto_start: false,
            restart_policy: camellia_nexus_core::RestartPolicy::Never,
            privilege_policy: Default::default(),
        }
    }

    fn xray_spec(dashboard: Option<camellia_nexus_core::XrayDashboardSpec>) -> ProgramSpec {
        ProgramSpec {
            schema_version: camellia_nexus_core::SCHEMA_VERSION,
            id: camellia_nexus_core::ProgramId::parse("xray-api-test").expect("id"),
            name: "Xray API test".into(),
            executable: camellia_nexus_core::ExecutableSpec::Managed {
                path: "bin/xray".into(),
                metadata: None,
            },
            program_type: camellia_nexus_core::ProgramType::Xray {
                main_config: Some("config/managed.json".into()),
                extra_args: Vec::new(),
            },
            managed_config: Some(camellia_nexus_core::ManagedConfigSpec {
                sources: Vec::new(),
                remote_update: None,
                sing_box_dashboard: None,
                sing_box_clash_dashboard: None,
                xray_dashboard: dashboard,
                mihomo_dashboard: None,
            }),
            working_directory: "bin".into(),
            environment: BTreeMap::new(),
            auto_start: false,
            restart_policy: camellia_nexus_core::RestartPolicy::Never,
            privilege_policy: Default::default(),
        }
    }

    fn mihomo_spec(dashboard: Option<camellia_nexus_core::MihomoDashboardSpec>) -> ProgramSpec {
        ProgramSpec {
            schema_version: camellia_nexus_core::SCHEMA_VERSION,
            id: camellia_nexus_core::ProgramId::parse("mihomo-api-test").expect("id"),
            name: "Mihomo API test".into(),
            executable: camellia_nexus_core::ExecutableSpec::Managed {
                path: "bin/mihomo".into(),
                metadata: None,
            },
            program_type: camellia_nexus_core::ProgramType::Mihomo {
                main_config: Some("config/managed.yaml".into()),
                extra_args: Vec::new(),
            },
            managed_config: Some(camellia_nexus_core::ManagedConfigSpec {
                mihomo_dashboard: dashboard,
                ..camellia_nexus_core::ManagedConfigSpec::default()
            }),
            working_directory: "bin".into(),
            environment: BTreeMap::new(),
            auto_start: false,
            restart_policy: camellia_nexus_core::RestartPolicy::Never,
            privilege_policy: Default::default(),
        }
    }

    #[test]
    fn xray_merge_obeys_tag_and_tail_rules() {
        let sources = vec![
            ResolvedSource {
                name: "01.json".into(),
                content: br#"{"outbounds":[{"tag":"direct"}],"log":{"loglevel":"warning"}}"#
                    .to_vec(),
                append_xray_outbounds: false,
            },
            ResolvedSource {
                name: "02.json".into(),
                content:
                    br#"{"outbounds":[{"tag":"block"},{"tag":"proxy"}],"log":{"loglevel":"debug"}}"#
                        .to_vec(),
                append_xray_outbounds: false,
            },
            ResolvedSource {
                name: "03_tail.json".into(),
                content: br#"{"outbounds":[{"tag":"last"}]}"#.to_vec(),
                append_xray_outbounds: true,
            },
        ];
        let merged: Value =
            serde_json::from_str(&merge_sources(ProgramKind::Xray, &sources).expect("merge"))
                .expect("json");
        assert_eq!(merged["log"]["loglevel"], "debug");
        assert_eq!(merged["outbounds"][0]["tag"], "block");
        assert_eq!(merged["outbounds"][1]["tag"], "proxy");
        assert_eq!(merged["outbounds"][3]["tag"], "last");
    }

    #[test]
    fn mihomo_merge_uses_named_sections_and_preserves_rule_priority() {
        let sources = vec![
            ResolvedSource {
                name: "01.yaml".into(),
                content: b"mode: rule\nproxies:\n  - name: edge\n    type: direct\nrules:\n  - DOMAIN,first.test,DIRECT\nsub-rules:\n  regional:\n    - DOMAIN-SUFFIX,first.test,DIRECT\n".to_vec(),
                append_xray_outbounds: false,
            },
            ResolvedSource {
                name: "02.yaml".into(),
                content: b"mode: global\nproxies:\n  - name: edge\n    type: socks5\n  - name: backup\n    type: direct\nrules:\n  - MATCH,edge\nsub-rules:\n  regional:\n    - MATCH,edge\n".to_vec(),
                append_xray_outbounds: false,
            },
        ];
        let merged: YamlValue =
            serde_yaml_ng::from_str(&merge_sources(ProgramKind::Mihomo, &sources).expect("merge"))
                .expect("yaml");
        assert_eq!(merged["mode"].as_str(), Some("global"));
        assert_eq!(merged["proxies"][0]["type"].as_str(), Some("socks5"));
        assert_eq!(merged["proxies"][1]["name"].as_str(), Some("backup"));
        assert_eq!(
            merged["rules"][0].as_str(),
            Some("DOMAIN,first.test,DIRECT")
        );
        assert_eq!(merged["rules"][1].as_str(), Some("MATCH,edge"));
        assert_eq!(
            merged["sub-rules"]["regional"][0].as_str(),
            Some("DOMAIN-SUFFIX,first.test,DIRECT")
        );
        assert_eq!(
            merged["sub-rules"]["regional"][1].as_str(),
            Some("MATCH,edge")
        );
    }

    #[test]
    fn mihomo_dashboard_materializes_as_yaml_and_keeps_user_secret() {
        let enabled = mihomo_spec(Some(camellia_nexus_core::MihomoDashboardSpec {
            listen_port: 9092,
            download_url: None,
        }));
        let configured: YamlValue = serde_yaml_ng::from_str(
            &apply_managed_features(&enabled, "secret: keep-me\n".into())
                .expect("inject dashboard"),
        )
        .expect("yaml");
        assert_eq!(
            configured["external-controller"].as_str(),
            Some("127.0.0.1:9092")
        );
        assert_eq!(
            configured["external-ui"].as_str(),
            Some(crate::programs::mihomo::managed_ui_directory())
        );
        assert_eq!(configured["secret"].as_str(), Some("keep-me"));
        assert!(configured["external-ui-url"].is_null());
    }

    #[test]
    fn mihomo_sources_require_a_yaml_mapping_root() {
        let source = ResolvedSource {
            name: "invalid.yaml".into(),
            content: b"- one\n- two\n".to_vec(),
            append_xray_outbounds: false,
        };
        let error = merge_sources(ProgramKind::Mihomo, &[source]).expect_err("invalid root");
        assert_eq!(error.code, ErrorCode::ConfigInvalid);
        assert!(error.message.contains("mapping"));
    }

    #[test]
    fn source_parser_accepts_jsonc_comments_and_trailing_commas() {
        let source = br#"{
            // comment
            "log": { "level": "info", },
            "value": "// retained",
        }"#;
        let parsed = parse_object("jsonc", source).expect("parse JSONC");
        assert_eq!(parsed["log"]["level"], "info");
        assert_eq!(parsed["value"], "// retained");
    }

    #[test]
    fn dashboard_uses_services_api_and_can_be_removed() {
        let enabled = sing_box_spec(Some(camellia_nexus_core::SingBoxDashboardSpec {
            listen_port: 9090,
            update_interval: "1d".into(),
        }));
        let configured: Value = serde_json::from_str(
            &apply_managed_features(&enabled, "{}".into()).expect("inject API service"),
        )
        .expect("JSON");
        assert_eq!(configured["services"][0]["type"], "api");
        assert_eq!(configured["services"][0]["listen"], "127.0.0.1");
        assert!(configured.get("experimental").is_none());

        let disabled = sing_box_spec(None);
        let removed: Value = serde_json::from_str(
            &apply_managed_features(&disabled, configured.to_string()).expect("remove API service"),
        )
        .expect("JSON");
        assert_eq!(removed["services"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn clash_dashboard_uses_experimental_clash_api() {
        let mut enabled = sing_box_spec(None);
        let managed = enabled
            .managed_config
            .as_mut()
            .expect("managed configuration");
        managed.sing_box_clash_dashboard = Some(camellia_nexus_core::SingBoxClashDashboardSpec {
            listen_port: 9091,
            download_url: Some("https://example.com/dashboard.zip".into()),
        });
        let configured: Value = serde_json::from_str(
            &apply_managed_features(&enabled, "{}".into()).expect("inject Clash API"),
        )
        .expect("JSON");
        assert_eq!(
            configured["experimental"]["clash_api"]["external_controller"],
            "127.0.0.1:9091"
        );
        assert_eq!(
            configured["experimental"]["clash_api"]["external_ui"],
            crate::programs::sing_box::managed_clash_ui()
        );
        let external_ui = configured["experimental"]["clash_api"]["external_ui"]
            .as_str()
            .expect("external UI path");
        assert_eq!(external_ui, "clash-dashboard");
        assert!(!external_ui.starts_with("dashboard/"));
    }

    #[test]
    fn sing_box_dashboards_are_injected_independently() {
        let mut enabled = sing_box_spec(Some(camellia_nexus_core::SingBoxDashboardSpec {
            listen_port: 9090,
            update_interval: "12h".into(),
        }));
        let managed = enabled
            .managed_config
            .as_mut()
            .expect("managed configuration");
        managed.sing_box_clash_dashboard = Some(camellia_nexus_core::SingBoxClashDashboardSpec {
            listen_port: 9091,
            download_url: None,
        });

        let configured: Value = serde_json::from_str(
            &apply_managed_features(&enabled, "{}".into()).expect("inject dashboards"),
        )
        .expect("JSON");
        let service = configured["services"][0]
            .as_object()
            .expect("sing-box API service");
        assert_eq!(service["listen_port"], 9090);
        let service_dashboard = service["dashboard"].as_object().expect("API dashboard");
        assert_eq!(service_dashboard["update_interval"], "12h");
        assert!(!service_dashboard.contains_key("external_ui"));

        let clash = configured["experimental"]["clash_api"]
            .as_object()
            .expect("Clash API");
        assert_eq!(clash["external_controller"], "127.0.0.1:9091");
        assert_eq!(
            clash["external_ui"],
            crate::programs::sing_box::managed_clash_ui()
        );
        assert!(!clash.contains_key("dashboard"));
    }

    #[test]
    fn xray_dashboard_enables_api_metrics_and_traffic_stats() {
        let enabled = xray_spec(Some(camellia_nexus_core::XrayDashboardSpec {
            api_port: 10085,
            metrics_port: 11111,
        }));
        let configured: Value = serde_json::from_str(
            &apply_managed_features(&enabled, "{}".into()).expect("inject Xray dashboard"),
        )
        .expect("JSON");
        assert_eq!(
            configured["api"]["tag"],
            crate::programs::xray::managed_api_tag()
        );
        assert_eq!(configured["api"]["listen"], "127.0.0.1:10085");
        assert_eq!(
            configured["api"]["services"],
            serde_json::json!([
                "HandlerService",
                "LoggerService",
                "StatsService",
                "RoutingService",
                "ReflectionService"
            ])
        );
        assert_eq!(
            configured["metrics"]["tag"],
            crate::programs::xray::managed_metrics_tag()
        );
        assert_eq!(configured["metrics"]["listen"], "127.0.0.1:11111");
        assert_eq!(configured["stats"], serde_json::json!({}));
        assert_eq!(configured["policy"]["system"]["statsInboundUplink"], true);
        assert_eq!(
            configured["policy"]["system"]["statsOutboundDownlink"],
            true
        );
    }

    #[test]
    fn xray_dashboard_restores_the_complete_managed_api_service_set() {
        let enabled = xray_spec(Some(camellia_nexus_core::XrayDashboardSpec {
            api_port: 10085,
            metrics_port: 11111,
        }));
        let previous = serde_json::json!({
            "api": {
                "tag": crate::programs::xray::managed_api_tag(),
                "listen": "127.0.0.1:10085",
                "services": ["StatsService", "RoutingService"]
            }
        });
        let configured: Value = serde_json::from_str(
            &apply_managed_features(&enabled, previous.to_string()).expect("migrate Xray API"),
        )
        .expect("JSON");
        assert_eq!(
            configured["api"]["services"],
            serde_json::json!([
                "StatsService",
                "RoutingService",
                "HandlerService",
                "LoggerService",
                "ReflectionService"
            ])
        );
    }
}
