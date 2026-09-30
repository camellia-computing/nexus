use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};

#[cfg(test)]
use camellia_nexus_core::ProgramKind;
use camellia_nexus_core::{
    CamelliaNexusError, ConfigSourceAuthentication, ConfigSourceSpec, ConfigurationFormat,
    CoreTargetIdentity, ErrorCode, MAX_CONFIG_BYTES, PayloadKind, ProgramSpec, Result,
    SourceFreshness, SourceSnapshot, SourceStatus, merge_configuration_sources,
    normalize_share_input,
};
use reqwest::{Client, redirect::Policy};
use serde_json::{Map, Value};
use tokio::io::AsyncReadExt;

const MAX_SOURCE_BYTES: usize = MAX_CONFIG_BYTES;
const MAX_TOTAL_SOURCE_BYTES: usize = 16 * 1024 * 1024;
const MAX_CONCURRENT_SOURCES: usize = 4;
const MAX_SOURCE_REDIRECTS: usize = 5;
const SOURCE_TIMEOUT: Duration = Duration::from_secs(25);

struct ResolvedSource {
    id: String,
    name: String,
    content: Vec<u8>,
    append_xray_outbounds: bool,
}

pub struct SourceRefreshResult {
    pub snapshots: Vec<SourceSnapshot>,
    pub statuses: BTreeMap<String, SourceStatus>,
    pub unavailable: bool,
    pub raw_observations: BTreeMap<String, Vec<u8>>,
}

pub struct MaterializedConfiguration {
    pub content: String,
    pub observations: SourceRefreshResult,
}

pub async fn materialize(
    spec: &ProgramSpec,
    local_base: Option<&Path>,
    credentials: &crate::config_credentials::CredentialSnapshot,
) -> Result<MaterializedConfiguration> {
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
    if enabled.is_empty() {
        return Err(CamelliaNexusError::invalid_spec(
            "Enable at least one configuration source before updating",
        ));
    }
    let sources = resolve_sources(&enabled, local_base, credentials).await?;
    let observed_unix_ms = now_unix_ms();
    let target = spec.core_target_identity().ok_or_else(|| {
        CamelliaNexusError::new(
            ErrorCode::InvalidState,
            "Managed configuration requires a supported Core version target",
        )
    })?;
    let snapshots = parse_sources(&target, &sources, observed_unix_ms)?;
    let raw_observations = sources
        .iter()
        .map(|source| (source.id.clone(), source.content.clone()))
        .collect();
    let content = merge_configuration_sources(spec.program_type.kind(), &snapshots)?.content;
    if content.len() > MAX_CONFIG_BYTES {
        return Err(CamelliaNexusError::invalid_spec(
            "Merged configuration exceeds the 4 MiB limit",
        )
        .with_message_key("SOURCE_TOO_LARGE"));
    }
    let snapshots_by_id = snapshots
        .iter()
        .map(|snapshot| (snapshot.source_id.as_str(), snapshot))
        .collect::<BTreeMap<_, _>>();
    let statuses = managed
        .sources
        .iter()
        .map(|source| {
            let snapshot = snapshots_by_id.get(source.id()).copied();
            let freshness = if source.enabled() {
                SourceFreshness::Fresh
            } else {
                SourceFreshness::Disabled
            };
            (
                source.id().to_owned(),
                SourceStatus {
                    source_id: source.id().to_owned(),
                    source_name: source.name().to_owned(),
                    freshness,
                    observed_hash: snapshot.map(|snapshot| snapshot.content_hash.clone()),
                    snapshot_hash: snapshot.map(|snapshot| snapshot.content_hash.clone()),
                    message_key: None,
                    observed_unix_ms: snapshot.map(|_| observed_unix_ms),
                },
            )
        })
        .collect();
    Ok(MaterializedConfiguration {
        content,
        observations: SourceRefreshResult {
            snapshots,
            statuses,
            unavailable: false,
            raw_observations,
        },
    })
}

pub async fn refresh_snapshots(
    spec: &ProgramSpec,
    local_base: Option<&Path>,
    credentials: &crate::config_credentials::CredentialSnapshot,
    previous: &BTreeMap<String, SourceSnapshot>,
    observed_unix_ms: u64,
) -> Result<SourceRefreshResult> {
    let managed = spec.managed_config.as_ref().ok_or_else(|| {
        CamelliaNexusError::new(
            ErrorCode::InvalidState,
            "Managed configuration is not enabled",
        )
    })?;
    let enabled = managed
        .sources
        .iter()
        .filter(|source| source.enabled())
        .collect::<Vec<_>>();
    if enabled.is_empty() {
        return Ok(SourceRefreshResult {
            snapshots: Vec::new(),
            statuses: managed
                .sources
                .iter()
                .map(|source| disabled_source_status(source, previous))
                .collect(),
            unavailable: false,
            raw_observations: BTreeMap::new(),
        });
    }

    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = Client::builder()
        .https_only(true)
        .redirect(remote_source_redirect_policy())
        .timeout(SOURCE_TIMEOUT)
        .user_agent(concat!("camellia-nexus/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(CamelliaNexusError::internal)?;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_SOURCES));
    let mut tasks = tokio::task::JoinSet::new();
    for (index, source) in enabled.iter().enumerate() {
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
            Ok::<_, CamelliaNexusError>((
                index,
                source.clone(),
                resolve_source(&client, &source, local_base.as_deref(), &credentials).await,
            ))
        });
    }
    let mut ordered = std::iter::repeat_with(|| None)
        .take(enabled.len())
        .collect::<Vec<Option<(ConfigSourceSpec, Result<ResolvedSource>)>>>();
    while let Some(task) = tasks.join_next().await {
        let (index, source, result) = task.map_err(CamelliaNexusError::internal)??;
        ordered[index] = Some((source, result));
    }

    let mut snapshots = Vec::with_capacity(ordered.len());
    let mut statuses = BTreeMap::new();
    let mut unavailable = false;
    let mut raw_observations = BTreeMap::new();
    let mut total = 0usize;
    let target = spec.core_target_identity().ok_or_else(|| {
        CamelliaNexusError::new(
            ErrorCode::InvalidState,
            "Managed configuration requires a supported Core version target",
        )
    })?;
    for entry in ordered {
        let (source, result) = entry.ok_or_else(|| {
            CamelliaNexusError::new(ErrorCode::Internal, "Configuration source task was lost")
        })?;
        let id = source.id().to_owned();
        let name = source.name().to_owned();
        let mut raw_content = None;
        let (parsed, failure_freshness, observed_hash) = match result {
            Ok(resolved) => {
                raw_content = Some(resolved.content.clone());
                total = total.saturating_add(resolved.content.len());
                let observed_hash = Some(camellia_nexus_core::config_service::hash_bytes(
                    &resolved.content,
                ));
                let parsed = if total > MAX_TOTAL_SOURCE_BYTES {
                    Err(CamelliaNexusError::invalid_spec(
                        "Configuration sources exceed the 16 MiB aggregate limit",
                    )
                    .with_message_key("SOURCE_TOO_LARGE"))
                } else {
                    parse_source_snapshot(
                        &target,
                        resolved.id,
                        resolved.name,
                        &resolved.content,
                        observed_unix_ms,
                        resolved.append_xray_outbounds,
                    )
                };
                (parsed, SourceFreshness::Invalid, observed_hash)
            }
            Err(error) => (Err(error), SourceFreshness::Unavailable, None),
        };
        match parsed {
            Ok(snapshot) => {
                if let Some(raw_content) = raw_content {
                    raw_observations.insert(id.clone(), raw_content);
                }
                statuses.insert(
                    id.clone(),
                    SourceStatus {
                        source_id: id,
                        source_name: name,
                        freshness: SourceFreshness::Fresh,
                        observed_hash: Some(snapshot.content_hash.clone()),
                        snapshot_hash: Some(snapshot.content_hash.clone()),
                        message_key: None,
                        observed_unix_ms: Some(observed_unix_ms),
                    },
                );
                snapshots.push(snapshot);
            }
            Err(error) => {
                if let Some(snapshot) = previous.get(&id).cloned() {
                    statuses.insert(
                        id.clone(),
                        SourceStatus {
                            source_id: id,
                            source_name: name,
                            freshness: SourceFreshness::Stale,
                            observed_hash,
                            snapshot_hash: Some(snapshot.content_hash.clone()),
                            message_key: Some(source_error_key(&error).into()),
                            observed_unix_ms: Some(observed_unix_ms),
                        },
                    );
                    snapshots.push(snapshot);
                } else {
                    unavailable = true;
                    statuses.insert(
                        id.clone(),
                        SourceStatus {
                            source_id: id,
                            source_name: name,
                            freshness: failure_freshness,
                            observed_hash,
                            snapshot_hash: None,
                            message_key: Some(source_error_key(&error).into()),
                            observed_unix_ms: Some(observed_unix_ms),
                        },
                    );
                }
            }
        }
    }
    for source in managed.sources.iter().filter(|source| !source.enabled()) {
        let (source_id, status) = disabled_source_status(source, previous);
        statuses.insert(source_id, status);
    }
    Ok(SourceRefreshResult {
        snapshots,
        statuses,
        unavailable,
        raw_observations,
    })
}

fn disabled_source_status(
    source: &ConfigSourceSpec,
    previous: &BTreeMap<String, SourceSnapshot>,
) -> (String, SourceStatus) {
    let source_id = source.id().to_owned();
    (
        source_id.clone(),
        SourceStatus {
            source_id,
            source_name: source.name().to_owned(),
            freshness: SourceFreshness::Disabled,
            observed_hash: None,
            snapshot_hash: previous
                .get(source.id())
                .map(|snapshot| snapshot.content_hash.clone()),
            message_key: None,
            observed_unix_ms: None,
        },
    )
}

async fn resolve_sources(
    sources: &[&ConfigSourceSpec],
    local_base: Option<&Path>,
    credentials: &crate::config_credentials::CredentialSnapshot,
) -> Result<Vec<ResolvedSource>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = Client::builder()
        .https_only(true)
        .redirect(remote_source_redirect_policy())
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
            )
            .with_message_key("SOURCE_TOO_LARGE"));
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

fn remote_source_redirect_policy() -> Policy {
    Policy::custom(|attempt| {
        if attempt.previous().len() >= MAX_SOURCE_REDIRECTS {
            return attempt.error(std::io::Error::other(
                "remote configuration redirect limit exceeded",
            ));
        }
        if !valid_remote_source_url(attempt.url()) {
            return attempt.error(std::io::Error::other(
                "remote configuration redirect target is not secure",
            ));
        }
        attempt.follow()
    })
}

fn valid_remote_source_url(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
}

async fn resolve_source(
    client: &Client,
    source: &ConfigSourceSpec,
    local_base: Option<&Path>,
    credentials: &crate::config_credentials::CredentialSnapshot,
) -> Result<ResolvedSource> {
    let (id, name, content, append_xray_outbounds) = match source {
        ConfigSourceSpec::Inline {
            id, name, content, ..
        } => (id.clone(), name.clone(), content.as_bytes().to_vec(), false),
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
                source.id().to_owned(),
                name.clone(),
                read_local_source_stable(&resolved_path)
                    .await
                    .map_err(|error| annotate_source_error(source.id(), error))?,
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
                source.id().to_owned(),
                name.clone(),
                fetch_remote_source(client, url, authentication.as_ref(), credentials)
                    .await
                    .map_err(|error| annotate_source_error(source.id(), error))?,
                contains_tail_marker(&path),
            )
        }
    };
    Ok(ResolvedSource {
        id,
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
            .with_message_key("SOURCE_FILE_NOT_FOUND")
        } else {
            CamelliaNexusError::new(
                ErrorCode::Storage,
                "Failed to open local configuration source",
            )
            .with_message_key("SOURCE_READ_FAILED")
            .with_details(format!("{:?}", error.kind()))
        }
    })?;
    let metadata = file.metadata().await.map_err(|error| {
        CamelliaNexusError::new(
            ErrorCode::Storage,
            "Failed to read local configuration source metadata",
        )
        .with_message_key("SOURCE_READ_FAILED")
        .with_details(format!("{:?}", error.kind()))
    })?;
    if !metadata.is_file() {
        return Err(CamelliaNexusError::new(
            ErrorCode::InvalidPath,
            "Configuration source is not a file",
        )
        .with_message_key("SOURCE_READ_FAILED"));
    }
    if metadata.len() > MAX_SOURCE_BYTES as u64 {
        return Err(CamelliaNexusError::invalid_spec(
            "Local configuration source must be a file no larger than 4 MiB",
        )
        .with_message_key("SOURCE_TOO_LARGE"));
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
            .with_message_key("SOURCE_READ_FAILED")
            .with_details(format!("{:?}", error.kind()))
        })?;
    if content.len() > MAX_SOURCE_BYTES {
        return Err(CamelliaNexusError::invalid_spec(
            "Local configuration source must be no larger than 4 MiB",
        )
        .with_message_key("SOURCE_TOO_LARGE"));
    }
    Ok(content)
}

async fn read_local_source_stable(path: &Path) -> Result<Vec<u8>> {
    let mut last = None;
    for attempt in 0..3 {
        let before = tokio::fs::metadata(path)
            .await
            .ok()
            .map(|metadata| (metadata.len(), metadata.modified().ok()));
        let content = read_local_source(path).await?;
        let after = tokio::fs::metadata(path)
            .await
            .ok()
            .map(|metadata| (metadata.len(), metadata.modified().ok()));
        if before == after {
            if content.iter().all(u8::is_ascii_whitespace) {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    "Local configuration source is empty",
                )
                .with_message_key("SOURCE_INVALID"));
            }
            return Ok(content);
        }
        last = Some(content);
        if attempt < 2 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    Err(CamelliaNexusError::new(
        ErrorCode::ConfigConflict,
        "Local configuration source changed while it was being read",
    )
    .with_message_key("SOURCE_CHANGED")
    .with_details(format!(
        "Observed bytes: {}",
        last.as_ref().map_or(0, Vec::len)
    )))
}

async fn fetch_remote_source(
    client: &Client,
    url: &str,
    authentication: Option<&ConfigSourceAuthentication>,
    credentials: &crate::config_credentials::CredentialSnapshot,
) -> Result<Vec<u8>> {
    let parsed = reqwest::Url::parse(url).map_err(|_| {
        CamelliaNexusError::invalid_spec("Invalid remote configuration URL")
            .with_message_key("SOURCE_URL_INVALID")
    })?;
    if !valid_remote_source_url(&parsed) {
        return Err(CamelliaNexusError::invalid_spec(
            "Remote configuration URLs must use HTTPS without embedded credentials",
        )
        .with_message_key("SOURCE_URL_INVALID"));
    }
    let request = client
        .get(parsed)
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/plain;q=0.9",
        )
        .header(
            reqwest::header::USER_AGENT,
            concat!("camellia-nexus/", env!("CARGO_PKG_VERSION")),
        );
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
        .map_err(remote_source_error)?;
    let final_url = response.url();
    if final_url.scheme() != "https"
        || !final_url.username().is_empty()
        || final_url.password().is_some()
    {
        return Err(CamelliaNexusError::invalid_spec(
            "Remote configuration redirects must remain on HTTPS without embedded credentials",
        )
        .with_message_key("SOURCE_URL_INVALID"));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_SOURCE_BYTES as u64)
    {
        return Err(CamelliaNexusError::invalid_spec(
            "Remote configuration source exceeds the 4 MiB limit",
        )
        .with_message_key("SOURCE_TOO_LARGE"));
    }
    let mut content = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(remote_source_error)? {
        if content.len().saturating_add(chunk.len()) > MAX_SOURCE_BYTES {
            return Err(CamelliaNexusError::invalid_spec(
                "Remote configuration source exceeds the 4 MiB limit",
            )
            .with_message_key("SOURCE_TOO_LARGE"));
        }
        content.extend_from_slice(&chunk);
    }
    Ok(content)
}

fn parse_sources(
    target: &CoreTargetIdentity,
    sources: &[ResolvedSource],
    observed_unix_ms: u64,
) -> Result<Vec<SourceSnapshot>> {
    sources
        .iter()
        .map(|source| {
            parse_source_snapshot(
                target,
                &source.id,
                &source.name,
                &source.content,
                observed_unix_ms,
                source.append_xray_outbounds,
            )
            .map_err(|error| annotate_source_error(&source.id, error))
        })
        .collect()
}

fn parse_source_snapshot(
    target: &CoreTargetIdentity,
    source_id: impl Into<String>,
    source_name: impl Into<String>,
    content: &[u8],
    parsed_unix_ms: u64,
    append_xray_outbounds: bool,
) -> Result<SourceSnapshot> {
    let kind = target.program;
    let source_id = source_id.into();
    let source_name = source_name.into();
    let normalized = normalize_share_input(content).ok();
    if let Some(normalized) = normalized.as_ref()
        && matches!(
            normalized.payload,
            PayloadKind::SingleShareLink | PayloadKind::ShareCollection
        )
    {
        let source_log = source_id.clone();
        let parsed =
            SourceSnapshot::parse_share(source_id, source_name, target, content, parsed_unix_ms);
        match &parsed {
            Ok(snapshot) => {
                if let Some(summary) = &snapshot.share_summary {
                    tracing::info!(
                        source = %snapshot.source_id,
                        parser_revision = summary.parser_revision.as_str(),
                        accepted = summary.accepted_items,
                        rejected = summary.rejected_items,
                        fidelity = ?summary.fidelity,
                        "configuration share source parsed"
                    );
                }
            }
            Err(error) => {
                tracing::warn!(
                    source = %source_log,
                    code = ?error.code,
                    "configuration share source rejected"
                );
            }
        }
        return parsed;
    }
    let native_content = if normalized
        .as_ref()
        .is_some_and(|value| !matches!(value.envelope, camellia_nexus_core::EnvelopeKind::Plain))
    {
        normalized.as_ref().expect("checked above").text.as_bytes()
    } else {
        content
    };
    SourceSnapshot::parse(
        source_id,
        source_name,
        ConfigurationFormat::for_kind(kind).ok_or_else(|| {
            CamelliaNexusError::invalid_spec(
                "Generic programs do not support managed configuration sources",
            )
        })?,
        native_content,
        parsed_unix_ms,
        append_xray_outbounds,
    )
}

#[cfg(test)]
fn merge_sources(kind: ProgramKind, sources: &[ResolvedSource]) -> Result<String> {
    let target = CoreTargetIdentity::unknown(kind, None);
    let snapshots = parse_sources(&target, sources, 0)?;
    Ok(merge_configuration_sources(kind, &snapshots)?.content)
}

pub(crate) fn parse_object(content: &[u8]) -> Result<Map<String, Value>> {
    let value = camellia_nexus_core::parse_semantic_document(ConfigurationFormat::Jsonc, content)?;
    value.as_object().cloned().ok_or_else(|| {
        CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "Configuration source root must be an object",
        )
    })
}

fn contains_tail_marker(value: &str) -> bool {
    value.to_ascii_lowercase().contains("tail")
}

fn remote_source_error(error: reqwest::Error) -> CamelliaNexusError {
    let (code, key) = if error.is_timeout() {
        (ErrorCode::Timeout, "SOURCE_TIMEOUT")
    } else if error
        .status()
        .is_some_and(|status| matches!(status.as_u16(), 401 | 403))
    {
        (ErrorCode::Network, "SOURCE_ACCESS_DENIED")
    } else {
        (ErrorCode::Network, "SOURCE_DOWNLOAD_FAILED")
    };
    CamelliaNexusError::new(code, "Configuration source could not be downloaded")
        .with_message_key(key)
}

fn source_error_key(error: &CamelliaNexusError) -> &'static str {
    match error.message_key.as_deref() {
        Some("SOURCE_URL_INVALID") => "SOURCE_URL_INVALID",
        Some("SOURCE_FILE_NOT_FOUND") => "SOURCE_FILE_NOT_FOUND",
        Some("SOURCE_READ_FAILED") => "SOURCE_READ_FAILED",
        Some("SOURCE_CHANGED") => "SOURCE_CHANGED",
        Some("SOURCE_TOO_LARGE") => "SOURCE_TOO_LARGE",
        Some("SOURCE_ACCESS_DENIED") => "SOURCE_ACCESS_DENIED",
        Some("SOURCE_TIMEOUT") => "SOURCE_TIMEOUT",
        Some("SOURCE_DOWNLOAD_FAILED") => "SOURCE_DOWNLOAD_FAILED",
        Some("SOURCE_CREDENTIALS_UNAVAILABLE") => "SOURCE_CREDENTIALS_UNAVAILABLE",
        _ => match error.code {
            ErrorCode::NotFound => "SOURCE_FILE_NOT_FOUND",
            ErrorCode::Network => "SOURCE_DOWNLOAD_FAILED",
            ErrorCode::Timeout => "SOURCE_TIMEOUT",
            ErrorCode::RequestTooLarge | ErrorCode::OutputLimitExceeded => "SOURCE_TOO_LARGE",
            ErrorCode::ConfigConflict => "SOURCE_CHANGED",
            ErrorCode::ConfigInvalid | ErrorCode::InvalidSpec => "SOURCE_INVALID",
            ErrorCode::Storage | ErrorCode::InvalidPath => "SOURCE_READ_FAILED",
            _ => "SOURCE_REFRESH_FAILED",
        },
    }
}

fn annotate_source_error(source_id: &str, error: CamelliaNexusError) -> CamelliaNexusError {
    CamelliaNexusError::new(error.code, "Configuration source needs attention")
        .with_message_key(source_error_key(&error))
        .with_details(
            serde_json::json!({"sourceId": source_id, "category": error.code}).to_string(),
        )
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use super::*;
    use camellia_nexus_core::{
        ExecutableSpec, ManagedConfigSpec, ProgramId, ProgramType, RestartPolicy,
    };
    use serde_yaml_ng::Value as YamlValue;
    use tokio::io::AsyncWriteExt;

    fn source_test_spec(sources: Vec<ConfigSourceSpec>) -> ProgramSpec {
        ProgramSpec {
            id: ProgramId::parse("source-test").expect("id"),
            name: "Source test".into(),
            executable: ExecutableSpec::External {
                path: PathBuf::from("xray"),
                metadata: None,
            },
            program_type: ProgramType::Xray {
                extra_args: Vec::new(),
            },
            managed_config: Some(ManagedConfigSpec {
                sources,
                ..ManagedConfigSpec::default()
            }),
            working_directory: PathBuf::from("."),
            environment: BTreeMap::new(),
            auto_start: false,
            restart_policy: RestartPolicy::Never,
            privilege_policy: Default::default(),
        }
    }

    #[tokio::test]
    async fn refresh_distinguishes_invalid_content_from_unavailable_input() {
        let directory = tempfile::tempdir().expect("tempdir");
        let invalid_id = "invalid-inline";
        let unavailable_id = "missing-local";
        let spec = source_test_spec(vec![
            ConfigSourceSpec::Inline {
                id: invalid_id.into(),
                name: "Invalid inline".into(),
                enabled: true,
                content: "not-json".into(),
            },
            ConfigSourceSpec::Local {
                id: unavailable_id.into(),
                name: "Missing local".into(),
                enabled: true,
                path: PathBuf::from("missing.json"),
            },
        ]);

        let result = refresh_snapshots(
            &spec,
            Some(directory.path()),
            &crate::config_credentials::CredentialSnapshot::empty(),
            &BTreeMap::new(),
            42,
        )
        .await
        .expect("refresh result");

        assert_eq!(
            result.statuses[invalid_id].freshness,
            SourceFreshness::Invalid
        );
        assert!(result.statuses[invalid_id].observed_hash.is_some());
        assert_eq!(
            result.statuses[unavailable_id].freshness,
            SourceFreshness::Unavailable
        );
        assert!(result.statuses[unavailable_id].observed_hash.is_none());
        assert!(result.unavailable);
        assert!(result.snapshots.is_empty());
        assert_eq!(
            result.statuses[invalid_id].message_key.as_deref(),
            Some("SOURCE_INVALID")
        );
        assert_eq!(
            result.statuses[unavailable_id].message_key.as_deref(),
            Some("SOURCE_FILE_NOT_FOUND")
        );
    }

    #[test]
    fn source_error_context_retains_only_identity_and_a_fixed_category() {
        for (code, key) in [
            (ErrorCode::Network, "SOURCE_DOWNLOAD_FAILED"),
            (ErrorCode::ConfigInvalid, "SOURCE_INVALID"),
            (ErrorCode::Storage, "SOURCE_READ_FAILED"),
            (ErrorCode::Timeout, "SOURCE_TIMEOUT"),
            (ErrorCode::Internal, "SOURCE_REFRESH_FAILED"),
        ] {
            let error = annotate_source_error("source-a", CamelliaNexusError::new(code,
                "https://private-user:private-password@private.example/config?token=private-token")
                .with_message_key("private-message-key")
                .with_details("private-key-material"));
            assert_eq!(error.message_key.as_deref(), Some(key));
            let encoded = serde_json::to_string(&error).unwrap();
            assert!(!encoded.contains("private"));
            assert!(!encoded.contains("https://"));
            if code != ErrorCode::Internal {
                assert!(encoded.contains("source-a"));
            }
        }
    }

    #[tokio::test]
    async fn missing_source_reports_no_path_or_name_and_reenters_after_repair() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("private-fixture-token.json");
        let mut spec = source_test_spec(vec![ConfigSourceSpec::Local {
            id: "source".into(),
            name: "private-source-name".into(),
            enabled: true,
            path: path.clone(),
        }]);
        let credentials = crate::config_credentials::CredentialSnapshot::empty();
        let error = materialize(&spec, None, &credentials).await.err().unwrap();
        assert_eq!(error.message_key.as_deref(), Some("SOURCE_FILE_NOT_FOUND"));
        let encoded = serde_json::to_string(&error).unwrap();
        assert!(!encoded.contains("private"));
        assert!(!encoded.contains(&directory.path().display().to_string()));
        if let ConfigSourceSpec::Local { name, .. } =
            &mut spec.managed_config.as_mut().unwrap().sources[0]
        {
            *name = "Source".into();
        }
        let previous = SourceSnapshot::parse(
            "source",
            "Source",
            ConfigurationFormat::Jsonc,
            br#"{"log":{"loglevel":"info"}}"#,
            1,
            false,
        )
        .unwrap();
        let previous = BTreeMap::from([("source".into(), previous)]);
        let failed = refresh_snapshots(&spec, None, &credentials, &previous, 2)
            .await
            .unwrap();
        assert_eq!(
            failed.snapshots,
            previous.values().cloned().collect::<Vec<_>>()
        );
        assert_eq!(failed.statuses["source"].freshness, SourceFreshness::Stale);
        assert_eq!(
            failed.statuses["source"].message_key.as_deref(),
            Some("SOURCE_FILE_NOT_FOUND")
        );
        assert!(
            !serde_json::to_string(&failed.statuses)
                .unwrap()
                .contains("private")
        );
        std::fs::write(&path, br#"{"log":{"loglevel":"debug"}}"#).unwrap();
        let fixed = refresh_snapshots(&spec, None, &credentials, &previous, 3)
            .await
            .unwrap();
        assert_eq!(fixed.statuses["source"].freshness, SourceFreshness::Fresh);
        assert!(fixed.statuses["source"].message_key.is_none());
        assert!(fixed.snapshots[0].content.contains("debug"));
    }

    #[tokio::test]
    async fn remote_failure_reports_categories_without_request_or_response_values() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        for (status, key) in [
            (401, "SOURCE_ACCESS_DENIED"),
            (403, "SOURCE_ACCESS_DENIED"),
            (500, "SOURCE_DOWNLOAD_FAILED"),
        ] {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                stream.read_u8().await.unwrap();
                stream.write_all(format!("HTTP/1.1 {status} private-response\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            });
            let error = Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap()
                .get(format!("http://{address}/private-path?token=private-token"))
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap_err();
            let error = remote_source_error(error);
            assert_eq!(error.message_key.as_deref(), Some(key));
            assert!(!serde_json::to_string(&error).unwrap().contains("private"));
            server.await.unwrap();
        }
    }

    #[test]
    fn base64_native_json_and_yaml_are_parsed_as_normal_sources() {
        let json = parse_source_snapshot(
            &CoreTargetIdentity::unknown(ProgramKind::Xray, None),
            "json",
            "JSON",
            b"eyJsb2ciOnsibGV2ZWwiOiJpbmZvIn19",
            1,
            false,
        )
        .expect("base64 JSON");
        assert!(json.content.contains("\"log\""));
        assert!(json.share_summary.is_none());

        let yaml = parse_source_snapshot(
            &CoreTargetIdentity::unknown(ProgramKind::Mihomo, None),
            "yaml",
            "YAML",
            b"cHJveGllczoKICAtIG5hbWU6IHRlc3QKICAgIHR5cGU6IHNzCiAgICBzZXJ2ZXI6IGV4YW1wbGUuY29tCiAgICBwb3J0OiA4Mzg4CiAgICBjaXBoZXI6IGFlcy0xMjgtZ2NtCiAgICBwYXNzd29yZDogc2VjcmV0Cg==",
            1,
            false,
        )
        .expect("base64 YAML");
        assert!(yaml.content.contains("proxies:"));
        assert!(yaml.share_summary.is_none());
    }

    #[tokio::test]
    async fn all_invalid_share_refresh_keeps_the_previous_snapshot() {
        let source_id = "share-inline";
        let spec = source_test_spec(vec![ConfigSourceSpec::Inline {
            id: source_id.into(),
            name: "Share".into(),
            enabled: true,
            content: "tuic://token-only@example.com:443".into(),
        }]);
        let previous_snapshot = SourceSnapshot::parse(
            source_id,
            "Share",
            ConfigurationFormat::Jsonc,
            br#"{"outbounds":[]}"#,
            1,
            false,
        )
        .expect("previous snapshot");
        let previous_hash = previous_snapshot.content_hash.clone();
        let result = refresh_snapshots(
            &spec,
            None,
            &crate::config_credentials::CredentialSnapshot::empty(),
            &BTreeMap::from([(source_id.into(), previous_snapshot)]),
            42,
        )
        .await
        .expect("refresh");

        assert_eq!(result.statuses[source_id].freshness, SourceFreshness::Stale);
        assert_eq!(
            result.statuses[source_id].snapshot_hash.as_deref(),
            Some(previous_hash.as_str())
        );
        assert_eq!(result.snapshots.len(), 1);
        assert!(!result.unavailable);
    }

    #[tokio::test]
    async fn disabled_source_status_keeps_the_previous_snapshot_hash() {
        let disabled_id = "disabled-inline";
        let spec = source_test_spec(vec![
            ConfigSourceSpec::Inline {
                id: "enabled-inline".into(),
                name: "Enabled".into(),
                enabled: true,
                content: r#"{"enabled":true}"#.into(),
            },
            ConfigSourceSpec::Inline {
                id: disabled_id.into(),
                name: "Disabled".into(),
                enabled: false,
                content: r#"{"disabled":true}"#.into(),
            },
        ]);
        let previous_snapshot = SourceSnapshot::parse(
            disabled_id,
            "Disabled",
            ConfigurationFormat::Jsonc,
            br#"{"disabled":"previous"}"#,
            1,
            false,
        )
        .expect("previous snapshot");
        let previous_hash = previous_snapshot.content_hash.clone();
        let previous = BTreeMap::from([(disabled_id.into(), previous_snapshot)]);

        let result = refresh_snapshots(
            &spec,
            None,
            &crate::config_credentials::CredentialSnapshot::empty(),
            &previous,
            42,
        )
        .await
        .expect("refresh result");

        assert_eq!(
            result.statuses[disabled_id].freshness,
            SourceFreshness::Disabled
        );
        assert_eq!(
            result.statuses[disabled_id].snapshot_hash.as_deref(),
            Some(previous_hash.as_str())
        );
    }

    #[tokio::test]
    async fn refresh_accepts_an_all_disabled_source_list_without_emptying_snapshots() {
        let source_id = "disabled-inline";
        let spec = source_test_spec(vec![ConfigSourceSpec::Inline {
            id: source_id.into(),
            name: "Disabled".into(),
            enabled: false,
            content: r#"{"disabled":true}"#.into(),
        }]);
        let previous_snapshot = SourceSnapshot::parse(
            source_id,
            "Disabled",
            ConfigurationFormat::Jsonc,
            br#"{"disabled":"previous"}"#,
            1,
            false,
        )
        .expect("previous snapshot");
        let previous_hash = previous_snapshot.content_hash.clone();
        let previous = BTreeMap::from([(source_id.into(), previous_snapshot)]);

        let result = refresh_snapshots(
            &spec,
            None,
            &crate::config_credentials::CredentialSnapshot::empty(),
            &previous,
            42,
        )
        .await
        .expect("refresh result");

        assert!(result.snapshots.is_empty());
        assert!(!result.unavailable);
        assert_eq!(
            result.statuses[source_id].freshness,
            SourceFreshness::Disabled
        );
        assert_eq!(
            result.statuses[source_id].snapshot_hash.as_deref(),
            Some(previous_hash.as_str())
        );
    }

    #[tokio::test]
    async fn materialize_preserves_real_source_observations() {
        let spec = source_test_spec(vec![
            ConfigSourceSpec::Inline {
                id: "base-inline".into(),
                name: "Base".into(),
                enabled: true,
                content: r#"{"log":{"loglevel":"info"}}"#.into(),
            },
            ConfigSourceSpec::Inline {
                id: "override-inline".into(),
                name: "Override".into(),
                enabled: true,
                content: r#"{"log":{"loglevel":"debug"}}"#.into(),
            },
            ConfigSourceSpec::Inline {
                id: "disabled-inline".into(),
                name: "Disabled".into(),
                enabled: false,
                content: r#"{"log":{"loglevel":"warning"}}"#.into(),
            },
        ]);

        let materialized = materialize(
            &spec,
            None,
            &crate::config_credentials::CredentialSnapshot::empty(),
        )
        .await
        .expect("materialize");

        assert_eq!(
            materialized
                .observations
                .snapshots
                .iter()
                .map(|snapshot| snapshot.source_id.as_str())
                .collect::<Vec<_>>(),
            ["base-inline", "override-inline"]
        );
        assert_eq!(
            materialized.observations.statuses["base-inline"].freshness,
            SourceFreshness::Fresh
        );
        assert_eq!(
            materialized.observations.statuses["disabled-inline"].freshness,
            SourceFreshness::Disabled
        );
        assert!(materialized.content.contains("debug"));
        assert!(!materialized.observations.unavailable);
    }

    #[tokio::test]
    async fn remote_source_redirect_rejects_insecure_target_before_connecting() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let destination = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind redirect destination");
        let destination_address = destination
            .local_addr()
            .expect("redirect destination address");
        let origin = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind redirect origin");
        let origin_address = origin.local_addr().expect("redirect origin address");
        let origin_task = tokio::spawn(async move {
            let (mut stream, _) = origin.accept().await.expect("accept redirect request");
            let mut request = [0_u8; 1024];
            let _ = stream
                .read(&mut request)
                .await
                .expect("read redirect request");
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://{destination_address}/leak\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .expect("write redirect response");
        });
        let client = Client::builder()
            .redirect(remote_source_redirect_policy())
            .build()
            .expect("build redirect test client");

        let result = client
            .get(format!("http://{origin_address}/source"))
            .send()
            .await;

        assert!(
            result.is_err(),
            "an insecure redirect must fail the request"
        );
        origin_task.await.expect("join redirect origin");
        assert!(
            tokio::time::timeout(Duration::from_millis(250), destination.accept())
                .await
                .is_err(),
            "the insecure redirect target must never receive a connection"
        );
    }

    #[test]
    fn xray_merge_obeys_tag_and_tail_rules() {
        let sources = vec![
            ResolvedSource {
                id: "xray-01".into(),
                name: "01.json".into(),
                content: br#"{"outbounds":[{"tag":"direct"}],"log":{"loglevel":"warning"}}"#
                    .to_vec(),
                append_xray_outbounds: false,
            },
            ResolvedSource {
                id: "xray-02".into(),
                name: "02.json".into(),
                content:
                    br#"{"outbounds":[{"tag":"block"},{"tag":"proxy"}],"log":{"loglevel":"debug"}}"#
                        .to_vec(),
                append_xray_outbounds: false,
            },
            ResolvedSource {
                id: "xray-tail".into(),
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
                id: "mihomo-01".into(),
                name: "01.yaml".into(),
                content: b"mode: rule\nproxies:\n  - name: edge\n    type: direct\nrules:\n  - DOMAIN,first.test,DIRECT\nsub-rules:\n  regional:\n    - DOMAIN-SUFFIX,first.test,DIRECT\n".to_vec(),
                append_xray_outbounds: false,
            },
            ResolvedSource {
                id: "mihomo-02".into(),
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
    fn mihomo_sources_require_a_yaml_mapping_root() {
        let source = ResolvedSource {
            id: "mihomo-invalid".into(),
            name: "invalid.yaml".into(),
            content: b"- one\n- two\n".to_vec(),
            append_xray_outbounds: false,
        };
        let error = merge_sources(ProgramKind::Mihomo, &[source]).expect_err("invalid root");
        assert_eq!(error.code, ErrorCode::ConfigInvalid);
        assert_eq!(error.message_key.as_deref(), Some("SOURCE_INVALID"));
    }

    #[test]
    fn source_parser_accepts_jsonc_comments_and_trailing_commas() {
        let source = br#"{
            // comment
            "log": { "level": "info", },
            "value": "// retained",
        }"#;
        let parsed = parse_object(source).expect("parse JSONC");
        assert_eq!(parsed["log"]["level"], "info");
        assert_eq!(parsed["value"], "// retained");
    }

    #[test]
    fn object_queries_use_position_only_syntax_errors_and_reject_non_objects() {
        let malformed = br#"{"private-fixture-token":"unterminated"#;
        let error = parse_object(malformed).unwrap_err();
        assert_eq!(error.code, ErrorCode::ConfigInvalid);
        assert_eq!(
            error.message_key.as_deref(),
            Some("CONFIGURATION_SYNTAX_INVALID")
        );
        let details = error.details.unwrap();
        assert!(details.starts_with("line=1; column="));
        assert!(!details.contains("private-fixture-token"));
        assert!(!error.message.contains("private-fixture-token"));
        for input in [b"null".as_slice(), b"[]", br#""private-fixture-token""#] {
            let error = parse_object(input).unwrap_err();
            assert_eq!(error.code, ErrorCode::ConfigInvalid);
            assert!(error.details.is_none());
        }
        assert_eq!(parse_object(br#"{"fixed":true}"#).unwrap()["fixed"], true);
    }
}
