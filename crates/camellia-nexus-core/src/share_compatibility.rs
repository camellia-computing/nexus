//! Share-link and subscription compatibility.
//!
//! This module deliberately stops at a protocol semantic object before it
//! produces a Core fragment.  Sources, envelopes and target Cores therefore
//! remain independent and a new input transport does not require a second
//! parser for every Core.

use std::collections::BTreeMap;

use base64::{Engine as _, engine::general_purpose};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use url::Url;
use uuid::Uuid;

use crate::{
    CamelliaNexusError, CoreCompatibilityProfile, CoreFeatureAvailability, CoreFeatureDecision,
    CoreTargetIdentity, ErrorCode, ProgramKind, Result, config_service::hash_bytes,
};

pub const MAX_SHARE_INPUT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_SHARE_TOTAL_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_SHARE_ITEMS: usize = 10_000;
pub const MAX_SHARE_LINE_BYTES: usize = 64 * 1024;
pub const MAX_SHARE_QUERY_PARAMETERS: usize = 128;
pub const MAX_SHARE_PARAMETER_BYTES: usize = 16 * 1024;
pub const SHARE_PARSER_REVISION: &str = crate::CORE_IMPLEMENTATION_REVISION;
pub const SHARE_TRANSLATOR_REVISION: &str = crate::CORE_IMPLEMENTATION_REVISION;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EnvelopeKind {
    Plain,
    Base64,
    Base64Url,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PayloadKind {
    NativeConfiguration,
    NativeFragment,
    SingleShareLink,
    ShareCollection,
    Mixed,
    Empty,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ShareProtocol {
    Vless,
    Shadowsocks,
    Hysteria2,
    Tuic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShareDialect {
    Standard,
    ShadowsocksSip002,
    ShadowsocksWholeBase64,
    Hysteria2Realm,
    TuicV4,
    Unknown(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ParseFidelity {
    Exact,
    Compatible,
    Partial,
    Ambiguous,
    Unsupported,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TranslationFidelity {
    Exact,
    Equivalent,
    LossyWarning,
    Unsupported,
    Unsafe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigurationStage {
    Acquire,
    Normalize,
    Detect,
    Decode,
    Parse,
    ProtocolSemantic,
    TargetTranslate,
    Snapshot,
    Rebase,
    Validate,
    Apply,
    Runtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Recoverability {
    Retry,
    KeepLastKnownGood,
    UserAction,
    Permanent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShareIssueSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationIssue {
    pub code: String,
    pub message: String,
    pub stage: ConfigurationStage,
    pub severity: ShareIssueSeverity,
    pub recoverability: Recoverability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_path: Option<String>,
}

impl ConfigurationIssue {
    pub fn error(
        code: impl Into<String>,
        message: impl Into<String>,
        stage: ConfigurationStage,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            stage,
            severity: ShareIssueSeverity::Error,
            recoverability: Recoverability::UserAction,
            semantic_path: None,
        }
    }

    pub fn warning(
        code: impl Into<String>,
        message: impl Into<String>,
        stage: ConfigurationStage,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            stage,
            severity: ShareIssueSeverity::Warning,
            recoverability: Recoverability::UserAction,
            semantic_path: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceParseSummary {
    pub envelope: EnvelopeKind,
    pub payload: PayloadKind,
    pub parser_revision: String,
    pub total_items: usize,
    pub accepted_items: usize,
    pub rejected_items: usize,
    pub warning_count: usize,
    pub protocols: BTreeMap<ShareProtocol, usize>,
    pub fidelity: ParseFidelity,
    pub collection_status: CollectionStatus,
    #[serde(default)]
    pub issues: Vec<ConfigurationIssue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CollectionStatus {
    Empty,
    Success,
    PartialSuccess,
    NoValidItems,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceItemProvenance {
    pub item_id: String,
    pub source_revision: String,
    pub item_index: usize,
    pub protocol: ShareProtocol,
    pub dialect: ShareDialect,
    pub parser_revision: String,
    pub translator_revision: String,
    pub original_hash: String,
    pub target: CoreTargetIdentity,
    pub profile_hash: String,
    pub knowledge_hash: String,
    #[serde(default)]
    pub feature_decisions: Vec<CoreFeatureDecision>,
    pub translation_fidelity: TranslationFidelity,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceParseItem {
    pub item_id: String,
    pub index: usize,
    pub protocol: Option<ShareProtocol>,
    pub dialect: Option<ShareDialect>,
    pub fidelity: ParseFidelity,
    pub name: Option<String>,
    pub semantic: Option<ProtocolSemantic>,
    #[serde(default)]
    pub issues: Vec<ConfigurationIssue>,
    /// Redacted summary safe for diagnostics.  The original URI is retained
    /// only in the source sidecar by the desktop storage layer.
    pub original_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareParseResult {
    pub normalized: NormalizedShareInput,
    pub summary: SourceParseSummary,
    pub items: Vec<SourceParseItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormalizedShareInput {
    pub text: String,
    pub envelope: EnvelopeKind,
    pub payload: PayloadKind,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProtocolSemantic {
    pub protocol: ShareProtocol,
    pub dialect: ShareDialect,
    pub host: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub authentication: ShareAuthentication,
    pub transport: ShareTransport,
    pub security: ShareSecurity,
    #[serde(default)]
    pub options: BTreeMap<String, String>,
    #[serde(default)]
    pub unknown_parameters: BTreeMap<String, Vec<String>>,
    pub original_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum ShareAuthentication {
    Vless { uuid: String },
    Shadowsocks { method: String, password: String },
    Hysteria2 { password: String },
    Tuic { uuid: String, password: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareTransport {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareSecurity {
    pub tls: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
    #[serde(default)]
    pub alpn: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(default)]
    pub insecure: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reality_public_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reality_short_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ech: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranslationSummary {
    pub target: CoreTargetIdentity,
    pub translator_revision: String,
    pub profile_hash: String,
    pub knowledge_hash: String,
    #[serde(default)]
    pub feature_decisions: Vec<CoreFeatureDecision>,
    pub fidelity: TranslationFidelity,
    #[serde(default)]
    pub warnings: Vec<ConfigurationIssue>,
    #[serde(default)]
    pub blocking_issues: Vec<ConfigurationIssue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranslatedShareItem {
    pub item_id: String,
    pub semantic: ProtocolSemantic,
    pub fragment: Option<Value>,
    pub summary: TranslationSummary,
    pub provenance: SourceItemProvenance,
}

type TranslationResult<T> = std::result::Result<T, ConfigurationIssue>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareImportPreview {
    pub normalized: NormalizedShareInput,
    pub summary: SourceParseSummary,
    pub items: Vec<TranslatedShareItem>,
}

impl ShareProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vless => "vless",
            Self::Shadowsocks => "shadowsocks",
            Self::Hysteria2 => "hysteria2",
            Self::Tuic => "tuic",
        }
    }
}

impl std::fmt::Display for ShareProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Normalizes BOMs and line endings and performs at most one whole-payload
/// base64 decode.  It intentionally never recursively decodes an already
/// decoded payload.
pub fn normalize_share_input(input: &[u8]) -> Result<NormalizedShareInput> {
    if input.len() > MAX_SHARE_INPUT_BYTES {
        return Err(CamelliaNexusError::new(
            ErrorCode::RequestTooLarge,
            "Share input exceeds the 4 MiB limit",
        ));
    }
    let source = std::str::from_utf8(input).map_err(|_| {
        CamelliaNexusError::new(ErrorCode::ConfigInvalid, "Share input must be UTF-8 text")
    })?;
    let source = source.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let source = source.trim().to_owned();
    if source.is_empty() {
        return Ok(NormalizedShareInput {
            text: String::new(),
            envelope: EnvelopeKind::Plain,
            payload: PayloadKind::Empty,
            content_hash: hash_bytes(&[]),
        });
    }
    let (text, envelope) = if looks_like_base64_envelope(&source) {
        let (decoded, kind) = decode_envelope(&source)?;
        let decoded = std::str::from_utf8(&decoded).map_err(|_| {
            CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Base64 share envelope does not contain UTF-8 text",
            )
        })?;
        (decoded.trim().replace("\r\n", "\n"), kind)
    } else {
        (source, EnvelopeKind::Plain)
    };
    if text.len() > MAX_SHARE_INPUT_BYTES {
        return Err(CamelliaNexusError::new(
            ErrorCode::RequestTooLarge,
            "Decoded share input exceeds the 4 MiB limit",
        ));
    }
    let payload = detect_payload_kind(&text);
    Ok(NormalizedShareInput {
        content_hash: hash_bytes(text.as_bytes()),
        text,
        envelope,
        payload,
    })
}

fn looks_like_base64_envelope(text: &str) -> bool {
    if text.contains("://") || text.starts_with('{') || text.starts_with('[') {
        return false;
    }
    let compact: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
    compact.len() >= 8
        && compact.len() <= MAX_SHARE_INPUT_BYTES
        && compact
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'+' | b'/' | b'=' | b'-' | b'_'))
}

fn decode_envelope(text: &str) -> Result<(Vec<u8>, EnvelopeKind)> {
    let compact: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
    let (decoded, kind) = if compact.contains('-') || compact.contains('_') {
        (
            general_purpose::URL_SAFE_NO_PAD
                .decode(compact.as_bytes())
                .or_else(|_| general_purpose::URL_SAFE.decode(compact.as_bytes())),
            EnvelopeKind::Base64Url,
        )
    } else {
        (
            general_purpose::STANDARD
                .decode(compact.as_bytes())
                .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(compact.as_bytes())),
            EnvelopeKind::Base64,
        )
    };
    let decoded = decoded.map_err(|_| {
        CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "Share envelope is not valid base64",
        )
    })?;
    if decoded.len() > MAX_SHARE_INPUT_BYTES {
        return Err(CamelliaNexusError::new(
            ErrorCode::RequestTooLarge,
            "Decoded share envelope exceeds the 4 MiB limit",
        ));
    }
    Ok((decoded, kind))
}

fn detect_payload_kind(text: &str) -> PayloadKind {
    let lines = share_lines(text);
    let share_count = lines.iter().filter(|line| is_uri_like(line)).count();
    let non_share = lines
        .iter()
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !is_uri_like(line))
        .count();
    if share_count == 0 {
        if text.trim_start().starts_with('{') || text.trim_start().starts_with('[') {
            PayloadKind::NativeConfiguration
        } else if text.contains(':') {
            PayloadKind::NativeFragment
        } else {
            PayloadKind::Unknown
        }
    } else if share_count == 1 && non_share == 0 {
        PayloadKind::SingleShareLink
    } else if non_share == 0 {
        PayloadKind::ShareCollection
    } else {
        PayloadKind::Mixed
    }
}

fn share_lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

fn is_uri_like(line: &str) -> bool {
    line.split_once("://")
        .is_some_and(|(scheme, rest)| !scheme.is_empty() && !rest.is_empty())
}

pub fn parse_share_input(input: &[u8]) -> Result<ShareParseResult> {
    let normalized = normalize_share_input(input)?;
    let lines = share_lines(&normalized.text);
    if lines.len() > MAX_SHARE_ITEMS {
        return Err(CamelliaNexusError::new(
            ErrorCode::RequestTooLarge,
            "Share collection contains too many items",
        ));
    }
    let mut items = Vec::with_capacity(lines.len());
    let mut protocols = BTreeMap::new();
    let mut issues = Vec::new();
    let parser = ShareParserRegistry::new();
    for (index, line) in lines.iter().enumerate() {
        if line.len() > MAX_SHARE_LINE_BYTES {
            let item_id = stable_item_id(line);
            let issue = ConfigurationIssue::error(
                "ITEM_TOO_LARGE",
                "Share item exceeds the 64 KiB limit",
                ConfigurationStage::Normalize,
            );
            issues.push(issue.clone());
            items.push(SourceParseItem {
                item_id,
                index,
                protocol: None,
                dialect: None,
                fidelity: ParseFidelity::Invalid,
                name: None,
                semantic: None,
                issues: vec![issue],
                original_hash: hash_bytes(line.as_bytes()),
            });
            continue;
        }
        let item_id = stable_item_id(line);
        match parser.parse(line) {
            Ok(semantic) => {
                *protocols.entry(semantic.protocol).or_default() += 1;
                items.push(SourceParseItem {
                    item_id,
                    index,
                    protocol: Some(semantic.protocol),
                    dialect: Some(semantic.dialect.clone()),
                    fidelity: if matches!(
                        semantic.dialect,
                        ShareDialect::Standard
                            | ShareDialect::ShadowsocksSip002
                            | ShareDialect::ShadowsocksWholeBase64
                    ) {
                        ParseFidelity::Exact
                    } else {
                        ParseFidelity::Compatible
                    },
                    name: semantic.name.clone(),
                    original_hash: semantic.original_hash.clone(),
                    semantic: Some(semantic),
                    issues: Vec::new(),
                });
            }
            Err(error) => {
                let issue = ConfigurationIssue::error(
                    format!("ITEM_{:?}", error.code),
                    error.message,
                    ConfigurationStage::Parse,
                );
                issues.push(issue.clone());
                items.push(SourceParseItem {
                    item_id,
                    index,
                    protocol: detect_protocol(line),
                    dialect: detect_dialect(line),
                    fidelity: if detect_protocol(line).is_some() {
                        ParseFidelity::Invalid
                    } else {
                        ParseFidelity::Unsupported
                    },
                    name: share_name(line),
                    semantic: None,
                    issues: vec![issue],
                    original_hash: hash_bytes(line.as_bytes()),
                });
            }
        }
    }
    let total_items = items.len();
    let accepted_items = items.iter().filter(|item| item.semantic.is_some()).count();
    let rejected_items = total_items.saturating_sub(accepted_items);
    let warning_count = issues
        .iter()
        .filter(|issue| issue.severity == ShareIssueSeverity::Warning)
        .count();
    let fidelity = if total_items == 0 {
        ParseFidelity::Invalid
    } else if accepted_items == total_items {
        ParseFidelity::Exact
    } else if accepted_items > 0 {
        ParseFidelity::Partial
    } else if items
        .iter()
        .any(|item| item.fidelity == ParseFidelity::Unsupported)
    {
        ParseFidelity::Unsupported
    } else {
        ParseFidelity::Invalid
    };
    let collection_status = if total_items == 0 {
        CollectionStatus::Empty
    } else if accepted_items == total_items {
        CollectionStatus::Success
    } else if accepted_items > 0 {
        CollectionStatus::PartialSuccess
    } else {
        CollectionStatus::NoValidItems
    };
    Ok(ShareParseResult {
        normalized: normalized.clone(),
        summary: SourceParseSummary {
            envelope: normalized.envelope,
            payload: normalized.payload,
            parser_revision: SHARE_PARSER_REVISION.into(),
            total_items,
            accepted_items,
            rejected_items,
            warning_count,
            protocols,
            fidelity,
            collection_status,
            issues,
        },
        items,
    })
}

fn detect_protocol(line: &str) -> Option<ShareProtocol> {
    match line.split_once("://")?.0.to_ascii_lowercase().as_str() {
        "vless" => Some(ShareProtocol::Vless),
        "ss" | "shadowsocks" => Some(ShareProtocol::Shadowsocks),
        "hysteria2" | "hy2" | "hysteria2+realm" | "hy2+realm" => Some(ShareProtocol::Hysteria2),
        "tuic" => Some(ShareProtocol::Tuic),
        _ => None,
    }
}

fn detect_dialect(line: &str) -> Option<ShareDialect> {
    match detect_protocol(line) {
        Some(ShareProtocol::Hysteria2) if line.to_ascii_lowercase().contains("+realm://") => {
            Some(ShareDialect::Hysteria2Realm)
        }
        Some(ShareProtocol::Tuic)
            if Url::parse(line)
                .ok()
                .is_some_and(|url| url.password().is_none()) =>
        {
            Some(ShareDialect::TuicV4)
        }
        _ => Some(ShareDialect::Standard),
    }
}

fn share_name(line: &str) -> Option<String> {
    let url = Url::parse(line).ok()?;
    let fragment = url.fragment()?;
    let encoded = format!("name={fragment}");
    let decoded = url::form_urlencoded::parse(encoded.as_bytes())
        .next()
        .map(|(_, value)| value.into_owned())?;
    (!decoded.is_empty()).then_some(decoded)
}

pub fn stable_item_id(line: &str) -> String {
    parse_share_item(line)
        .map(|semantic| {
            let mut identity = semantic_identity_material(&semantic);
            identity.push('|');
            identity.push_str(semantic.protocol.as_str());
            format!("share-{}", &hash_bytes(identity.as_bytes())[..16])
        })
        .unwrap_or_else(|_| format!("share-{}", &hash_bytes(line.as_bytes())[..16]))
}

fn semantic_identity_material(semantic: &ProtocolSemantic) -> String {
    // BTreeMap-backed fields and serde's struct field order make this stable;
    // display metadata, raw representation and unknown non-critical metadata
    // are deliberately excluded from connection identity.
    serde_json::to_string(&json!({
        "protocol": semantic.protocol,
        "host": semantic.host,
        "port": semantic.port,
        "authentication": semantic.authentication,
        "transport": semantic.transport,
        "security": semantic.security,
        "options": semantic.options,
    }))
    .expect("share semantic identity is serializable")
}

pub fn parse_share_item(line: &str) -> Result<ProtocolSemantic> {
    let scheme = line
        .split_once("://")
        .map(|(scheme, _)| scheme.to_ascii_lowercase())
        .ok_or_else(|| {
            CamelliaNexusError::new(ErrorCode::ConfigInvalid, "Share item has no scheme")
        })?;
    match scheme.as_str() {
        "vless" => parse_vless(line),
        "ss" | "shadowsocks" => parse_shadowsocks(line),
        "hysteria2" | "hy2" | "hysteria2+realm" | "hy2+realm" => parse_hysteria2(line),
        "tuic" => parse_tuic(line),
        _ => Err(CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "Unsupported share protocol",
        )),
    }
}

/// Small built-in registry surface kept intentionally data-driven at the
/// boundary.  Future protocol dialects can be registered here without
/// coupling a source/envelope combination to a target Core translator.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShareParserRegistry;

impl ShareParserRegistry {
    pub const fn new() -> Self {
        Self
    }

    pub fn revision(self) -> &'static str {
        SHARE_PARSER_REVISION
    }

    pub fn parse(self, item: &str) -> Result<ProtocolSemantic> {
        parse_share_item(item)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ShareTranslatorRegistry;

impl ShareTranslatorRegistry {
    pub const fn new() -> Self {
        Self
    }

    pub fn revision(self) -> &'static str {
        SHARE_TRANSLATOR_REVISION
    }

    pub fn translate(
        self,
        item: &ProtocolSemantic,
        target: &CoreTargetIdentity,
        source_revision: &str,
        item_index: usize,
    ) -> Result<TranslatedShareItem> {
        translate_share_item(item, target, source_revision, item_index)
    }
}

fn parse_url(line: &str) -> Result<Url> {
    Url::parse(line).map_err(|error| {
        CamelliaNexusError::new(ErrorCode::ConfigInvalid, "Share URL is invalid")
            .with_details(error.to_string())
    })
}

fn validate_query(url: &Url) -> Result<BTreeMap<String, Vec<String>>> {
    let mut values = BTreeMap::<String, Vec<String>>::new();
    for (key, value) in url.query_pairs() {
        if key.len() > MAX_SHARE_PARAMETER_BYTES || value.len() > MAX_SHARE_PARAMETER_BYTES {
            return Err(CamelliaNexusError::new(
                ErrorCode::RequestTooLarge,
                "Share query parameter exceeds the 16 KiB limit",
            ));
        }
        let entry = values.entry(key.into_owned()).or_default();
        entry.push(value.into_owned());
        if values.values().map(Vec::len).sum::<usize>() > MAX_SHARE_QUERY_PARAMETERS {
            return Err(CamelliaNexusError::new(
                ErrorCode::RequestTooLarge,
                "Share URL contains too many query parameters",
            ));
        }
    }
    Ok(values)
}

fn take_single(query: &mut BTreeMap<String, Vec<String>>, key: &str) -> Result<Option<String>> {
    let Some(values) = query.remove(key) else {
        return Ok(None);
    };
    if values.len() != 1 {
        return Err(CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            format!("Share parameter {key} must not be repeated"),
        ));
    }
    Ok(values.into_iter().next())
}

fn required_host_port(url: &Url, default_port: Option<u16>) -> Result<(String, u16)> {
    let host = url
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or_else(|| CamelliaNexusError::new(ErrorCode::ConfigInvalid, "Share URL has no host"))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let port = url.port().or(default_port).ok_or_else(|| {
        CamelliaNexusError::new(ErrorCode::ConfigInvalid, "Share URL has no port")
    })?;
    if port == 0 {
        return Err(CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "Share URL port is invalid",
        ));
    }
    Ok((host, port))
}

fn parse_vless(line: &str) -> Result<ProtocolSemantic> {
    let url = parse_url(line)?;
    let uuid = url.username().to_owned();
    Uuid::parse_str(&uuid).map_err(|_| {
        CamelliaNexusError::new(ErrorCode::ConfigInvalid, "VLESS userinfo must be a UUID")
    })?;
    let (host, port) = required_host_port(&url, None)?;
    let mut query = validate_query(&url)?;
    let encryption = take_single(&mut query, "encryption")?.unwrap_or_else(|| "none".into());
    if encryption != "none" {
        return Err(CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "VLESS encryption must be none",
        ));
    }
    let security_name = take_single(&mut query, "security")?.unwrap_or_else(|| "none".into());
    let mut security = ShareSecurity::default();
    match security_name.as_str() {
        "none" => {}
        "tls" => security.tls = true,
        "reality" => {
            security.tls = true;
            security.reality_public_key = take_single(&mut query, "pbk")?;
            security.reality_short_id = take_single(&mut query, "sid")?;
            if security.reality_public_key.is_none() || security.reality_short_id.is_none() {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    "VLESS REALITY requires pbk and sid",
                ));
            }
        }
        _ => {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Unsupported VLESS security mode",
            ));
        }
    }
    security.sni = take_single(&mut query, "sni")?;
    let allow_insecure = take_single(&mut query, "allowInsecure")?;
    let insecure = take_single(&mut query, "insecure")?;
    if allow_insecure.is_some() && insecure.is_some() {
        return Err(CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "VLESS insecure TLS aliases must not be combined",
        ));
    }
    security.insecure = allow_insecure
        .or(insecure)
        .as_deref()
        .map(parse_bool)
        .transpose()?
        .unwrap_or(false);
    security.fingerprint = take_single(&mut query, "fp")?;
    security.ech = take_single(&mut query, "ech")?;
    security.alpn = take_single(&mut query, "alpn")?
        .map(|value| value.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    let kind = take_single(&mut query, "type")?.unwrap_or_else(|| "tcp".into());
    let transport = ShareTransport {
        kind: kind.clone(),
        path: take_single(&mut query, "path")?,
        host: take_single(&mut query, "host")?,
        service_name: take_single(&mut query, "serviceName")?,
        headers: BTreeMap::new(),
    };
    if !matches!(
        kind.as_str(),
        "tcp" | "ws" | "grpc" | "quic" | "http" | "httpupgrade" | "xhttp"
    ) {
        return Err(CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "Unsupported VLESS transport",
        ));
    }
    let name = share_name(line);
    let unknown_parameters = query;
    Ok(ProtocolSemantic {
        protocol: ShareProtocol::Vless,
        dialect: ShareDialect::Standard,
        host,
        port,
        name,
        authentication: ShareAuthentication::Vless { uuid },
        transport,
        security,
        options: BTreeMap::new(),
        unknown_parameters,
        original_hash: hash_bytes(line.as_bytes()),
    })
}

fn parse_shadowsocks(line: &str) -> Result<ProtocolSemantic> {
    let url = parse_url(line)?;
    let mut dialect = ShareDialect::ShadowsocksSip002;
    let (url, method, password) = if !url.username().is_empty() && url.password().is_none() {
        // SIP002 encodes only `method:password` in URL-safe Base64 and keeps
        // the endpoint as the normal authority host/port.
        let decoded = decode_protocol_base64(url.username(), "Shadowsocks userinfo")?;
        let decoded = std::str::from_utf8(&decoded).map_err(|_| {
            CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Shadowsocks userinfo is not UTF-8",
            )
        })?;
        let (method, password) = decoded.split_once(':').ok_or_else(|| {
            CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Shadowsocks userinfo must contain method and password",
            )
        })?;
        if method.is_empty() || password.is_empty() {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Shadowsocks method and password are required",
            ));
        }
        (url, method.to_owned(), password.to_owned())
    } else if url.username().is_empty() {
        // `Url` treats a whole-authority Base64 payload as a host and may
        // lowercase it.  Read this protocol-internal envelope from the
        // original representation so Base64's case-sensitive alphabet is
        // preserved.
        let authority = line
            .split_once("://")
            .and_then(|(_, rest)| rest.split('#').next())
            .unwrap_or_default()
            .trim_start_matches('/');
        let decoded = decode_protocol_base64(authority, "Shadowsocks authority")?;
        let decoded = std::str::from_utf8(&decoded).map_err(|_| {
            CamelliaNexusError::new(ErrorCode::ConfigInvalid, "Invalid Shadowsocks authority")
        })?;
        let (credentials, host_part) = decoded.split_once('@').ok_or_else(|| {
            CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Shadowsocks authority must contain credentials and endpoint",
            )
        })?;
        let (method, password) = credentials.split_once(':').ok_or_else(|| {
            CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Shadowsocks authority must contain method and password",
            )
        })?;
        let decoded_url = parse_url(&format!("ss://{host_part}"))?;
        if method.is_empty() || password.is_empty() {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Shadowsocks authority must contain method and password",
            ));
        }
        dialect = ShareDialect::ShadowsocksWholeBase64;
        (decoded_url, method.to_owned(), password.to_owned())
    } else {
        let method = url.username().to_owned();
        let password = url.password().unwrap_or_default().to_owned();
        if method.is_empty() || password.is_empty() {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Shadowsocks method and password are required",
            ));
        }
        (url, method, password)
    };
    let (host, port) = required_host_port(&url, None)?;
    let mut query = validate_query(&url)?;
    let mut options = BTreeMap::new();
    if let Some(plugin) = take_single(&mut query, "plugin")? {
        let plugin_name = plugin.split(';').next().unwrap_or_default();
        if !matches!(plugin_name, "obfs-local" | "v2ray-plugin") {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Unsupported Shadowsocks plugin",
            ));
        }
        options.insert("plugin".into(), plugin);
    }
    Ok(ProtocolSemantic {
        protocol: ShareProtocol::Shadowsocks,
        dialect,
        host,
        port,
        name: share_name(line),
        authentication: ShareAuthentication::Shadowsocks { method, password },
        transport: ShareTransport {
            kind: "tcp".into(),
            path: None,
            host: None,
            service_name: None,
            headers: BTreeMap::new(),
        },
        security: ShareSecurity::default(),
        options,
        unknown_parameters: query,
        original_hash: hash_bytes(line.as_bytes()),
    })
}

fn decode_protocol_base64(input: &str, label: &str) -> Result<Vec<u8>> {
    for engine in [
        &general_purpose::URL_SAFE_NO_PAD,
        &general_purpose::URL_SAFE,
        &general_purpose::STANDARD_NO_PAD,
        &general_purpose::STANDARD,
    ] {
        if let Ok(decoded) = engine.decode(input.as_bytes()) {
            if decoded.len() > MAX_SHARE_LINE_BYTES {
                return Err(CamelliaNexusError::new(
                    ErrorCode::RequestTooLarge,
                    format!("Decoded {label} exceeds the 64 KiB item limit"),
                ));
            }
            return Ok(decoded);
        }
    }
    Err(CamelliaNexusError::new(
        ErrorCode::ConfigInvalid,
        format!("Invalid {label} Base64"),
    ))
}

fn parse_hysteria2(line: &str) -> Result<ProtocolSemantic> {
    let url = parse_url(line)?;
    let is_realm = line.to_ascii_lowercase().starts_with("hysteria2+realm://")
        || line.to_ascii_lowercase().starts_with("hy2+realm://");
    let (host, port) = required_host_port(&url, Some(443))?;
    let password = url
        .password()
        .or_else(|| (!url.username().is_empty()).then_some(url.username()))
        .unwrap_or_default();
    if password.is_empty() {
        return Err(CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "Hysteria2 password is required",
        ));
    }
    let mut query = validate_query(&url)?;
    let mut security = ShareSecurity {
        tls: true,
        ..ShareSecurity::default()
    };
    security.sni = take_single(&mut query, "sni")?;
    security.insecure = take_single(&mut query, "insecure")?
        .as_deref()
        .map(parse_bool)
        .transpose()?
        .unwrap_or(false);
    security.pin_sha256_placeholder(&mut query)?;
    security.ech = take_single(&mut query, "ech")?;
    let mut options = BTreeMap::new();
    for key in ["mport", "ports", "obfs", "obfs-password", "fastopen"] {
        if let Some(value) = take_single(&mut query, key)? {
            options.insert(key.into(), value);
        }
    }
    let dialect = if is_realm {
        ShareDialect::Hysteria2Realm
    } else {
        ShareDialect::Standard
    };
    if is_realm {
        return Err(CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "Hysteria2 realm dialect is not supported",
        ));
    }
    Ok(ProtocolSemantic {
        protocol: ShareProtocol::Hysteria2,
        dialect,
        host,
        port,
        name: share_name(line),
        authentication: ShareAuthentication::Hysteria2 {
            password: password.to_owned(),
        },
        transport: ShareTransport {
            kind: "quic".into(),
            path: None,
            host: None,
            service_name: None,
            headers: BTreeMap::new(),
        },
        security,
        options,
        unknown_parameters: query,
        original_hash: hash_bytes(line.as_bytes()),
    })
}

fn parse_bool(value: &str) -> Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Ok(true),
        "0" | "false" | "no" => Ok(false),
        _ => Err(CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "Share boolean parameter is invalid",
        )),
    }
}

fn parse_tuic(line: &str) -> Result<ProtocolSemantic> {
    let url = parse_url(line)?;
    let (host, port) = required_host_port(&url, Some(443))?;
    let user = url.username().to_owned();
    let password = url.password().unwrap_or_default().to_owned();
    if user.is_empty() {
        return Err(CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "TUIC credentials are required",
        ));
    }
    if password.is_empty() {
        return Err(CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "TUIC v5 requires uuid and password",
        ));
    }
    Uuid::parse_str(&user).map_err(|_| {
        CamelliaNexusError::new(ErrorCode::ConfigInvalid, "TUIC userinfo must be a UUID")
    })?;
    let mut query = validate_query(&url)?;
    let mut security = ShareSecurity {
        tls: true,
        ..ShareSecurity::default()
    };
    security.sni = take_single(&mut query, "sni")?;
    security.alpn = take_single(&mut query, "alpn")?
        .map(|value| value.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    security.insecure = take_single(&mut query, "allow_insecure")?
        .as_deref()
        .map(parse_bool)
        .transpose()?
        .unwrap_or(false);
    let mut options = BTreeMap::new();
    for key in [
        "congestion_control",
        "udp_relay_mode",
        "zero_rtt_handshake",
        "heartbeat",
        "disable_sni",
    ] {
        if let Some(value) = take_single(&mut query, key)? {
            options.insert(key.into(), value);
        }
    }
    Ok(ProtocolSemantic {
        protocol: ShareProtocol::Tuic,
        dialect: ShareDialect::Standard,
        host,
        port,
        name: share_name(line),
        authentication: ShareAuthentication::Tuic {
            uuid: user,
            password,
        },
        transport: ShareTransport {
            kind: "quic".into(),
            path: None,
            host: None,
            service_name: None,
            headers: BTreeMap::new(),
        },
        security,
        options,
        unknown_parameters: query,
        original_hash: hash_bytes(line.as_bytes()),
    })
}

trait PinSha256Query {
    fn pin_sha256_placeholder(&mut self, query: &mut BTreeMap<String, Vec<String>>) -> Result<()>;
}

impl PinSha256Query for ShareSecurity {
    fn pin_sha256_placeholder(&mut self, query: &mut BTreeMap<String, Vec<String>>) -> Result<()> {
        // Keep the pin as an opaque security extension until a target Core
        // explicitly supports it; never silently discard it.
        if let Some(pin) = take_single(query, "pinSHA256")? {
            self.fingerprint = Some(pin);
        }
        Ok(())
    }
}

pub fn translate_share_item(
    item: &ProtocolSemantic,
    target: &CoreTargetIdentity,
    source_revision: &str,
    item_index: usize,
) -> Result<TranslatedShareItem> {
    let profile = CoreCompatibilityProfile::resolve(target)?;
    let mut warnings = Vec::new();
    let mut blocking = Vec::new();
    let fragment = match translate_fragment(item, &profile, &mut warnings, &mut blocking) {
        Ok(fragment) => Some(fragment),
        Err(issue) => {
            blocking.push(issue);
            None
        }
    };
    let fidelity = if !blocking.is_empty() {
        TranslationFidelity::Unsupported
    } else if warnings.is_empty() {
        TranslationFidelity::Equivalent
    } else {
        TranslationFidelity::LossyWarning
    };
    let feature_decisions = share_feature_decisions(item, &profile)?;
    let summary = TranslationSummary {
        target: target.clone(),
        translator_revision: SHARE_TRANSLATOR_REVISION.into(),
        profile_hash: profile.profile_hash.clone(),
        knowledge_hash: target.knowledge_hash.clone(),
        feature_decisions: feature_decisions.clone(),
        fidelity,
        warnings: warnings.clone(),
        blocking_issues: blocking.clone(),
    };
    let provenance = SourceItemProvenance {
        item_id: stable_item_id_from_semantic(item),
        source_revision: source_revision.to_owned(),
        item_index,
        protocol: item.protocol,
        dialect: item.dialect.clone(),
        parser_revision: SHARE_PARSER_REVISION.into(),
        translator_revision: SHARE_TRANSLATOR_REVISION.into(),
        original_hash: item.original_hash.clone(),
        target: target.clone(),
        profile_hash: profile.profile_hash,
        knowledge_hash: target.knowledge_hash.clone(),
        feature_decisions,
        translation_fidelity: fidelity,
        warnings: warnings.into_iter().map(|issue| issue.message).collect(),
    };
    Ok(TranslatedShareItem {
        item_id: provenance.item_id.clone(),
        semantic: item.clone(),
        fragment,
        summary,
        provenance,
    })
}

fn stable_item_id_from_semantic(item: &ProtocolSemantic) -> String {
    format!(
        "share-{}",
        &hash_bytes(semantic_identity_material(item).as_bytes())[..16]
    )
}

fn translate_fragment(
    item: &ProtocolSemantic,
    target: &CoreCompatibilityProfile,
    warnings: &mut Vec<ConfigurationIssue>,
    blocking: &mut Vec<ConfigurationIssue>,
) -> TranslationResult<Value> {
    if let Some(issue) = critical_unknown_parameter(item) {
        return Err(issue);
    }
    if !item.unknown_parameters.is_empty() {
        warnings.push(ConfigurationIssue::warning(
            "UNKNOWN_PARAMETERS_PRESERVED",
            format!(
                "{} unsupported share parameter(s) were preserved in the source observation",
                item.unknown_parameters.len()
            ),
            ConfigurationStage::TargetTranslate,
        ));
    }
    let decision = target
        .decision(share_feature_id(item))
        .cloned()
        .ok_or_else(|| {
            ConfigurationIssue::error(
                "COMPATIBILITY_PROFILE_INCOMPLETE",
                "Core compatibility profile is missing a share protocol decision",
                ConfigurationStage::TargetTranslate,
            )
        })?;
    match decision.availability {
        CoreFeatureAvailability::SourceDeclared => {}
        CoreFeatureAvailability::SourceUnavailable
            if target.target.program == ProgramKind::Xray
                && item.protocol == ShareProtocol::Tuic => {}
        CoreFeatureAvailability::SourceUnavailable => {
            return Err(ConfigurationIssue::error(
                "TARGET_SOURCE_UNAVAILABLE",
                format!(
                    "The maintained source baseline has no registered {} outbound",
                    item.protocol
                ),
                ConfigurationStage::TargetTranslate,
            ));
        }
        CoreFeatureAvailability::Unconfirmed => warnings.push(ConfigurationIssue::warning(
            "TARGET_CAPABILITY_UNKNOWN",
            "Program identity must be confirmed before applying this translation",
            ConfigurationStage::TargetTranslate,
        )),
    }
    match (target.target.program, item.protocol) {
        (ProgramKind::Xray, ShareProtocol::Tuic) => Err(ConfigurationIssue::error(
            "UNSUPPORTED_TUIC",
            "Xray does not support TUIC v5 outbound semantics",
            ConfigurationStage::TargetTranslate,
        )),
        (ProgramKind::Xray, ShareProtocol::Hysteria2) => {
            // Xray exposes Hysteria v2 as the hysteria transport.  Keep the
            // explicit transport marker so the native validator remains the
            // final authority; unsupported extensions are blocking below.
            if item.options.contains_key("obfs") || item.security.ech.is_some() {
                blocking.push(ConfigurationIssue::error(
                    "UNSUPPORTED_HYSTERIA_FEATURE",
                    "The selected Xray target cannot represent this Hysteria2 extension",
                    ConfigurationStage::TargetTranslate,
                ));
            }
            let ShareAuthentication::Hysteria2 { password } = &item.authentication else {
                unreachable!()
            };
            Ok(
                json!({"protocol":"hysteria","settings":{"address":item.host,"port":item.port,"password":password},"streamSettings":{"network":"hysteria"}}),
            )
        }
        (ProgramKind::SingBox, ShareProtocol::Vless) => translate_sing_box_vless(item, warnings),
        (ProgramKind::Xray, ShareProtocol::Vless) => translate_xray_vless(item, warnings),
        (ProgramKind::Mihomo, ShareProtocol::Vless) => translate_mihomo_vless(item, warnings),
        (ProgramKind::SingBox, ShareProtocol::Shadowsocks) => translate_sing_box_ss(item, warnings),
        (ProgramKind::Xray, ShareProtocol::Shadowsocks) => translate_xray_ss(item, warnings),
        (ProgramKind::Mihomo, ShareProtocol::Shadowsocks) => translate_mihomo_ss(item, warnings),
        (ProgramKind::SingBox, ShareProtocol::Hysteria2) => {
            translate_sing_box_hysteria(item, warnings)
        }
        (ProgramKind::Mihomo, ShareProtocol::Hysteria2) => {
            translate_mihomo_hysteria(item, warnings)
        }
        (ProgramKind::SingBox, ShareProtocol::Tuic) => translate_sing_box_tuic(item, warnings),
        (ProgramKind::Mihomo, ShareProtocol::Tuic) => translate_mihomo_tuic(item, warnings),
        (ProgramKind::Generic, _) => Err(ConfigurationIssue::error(
            "UNSUPPORTED_TARGET",
            "Generic programs do not have protocol translators",
            ConfigurationStage::TargetTranslate,
        )),
    }
}

fn share_feature_decisions(
    item: &ProtocolSemantic,
    profile: &CoreCompatibilityProfile,
) -> Result<Vec<CoreFeatureDecision>> {
    let decision = profile
        .decision(share_feature_id(item))
        .cloned()
        .ok_or_else(|| {
            CamelliaNexusError::new(
                ErrorCode::Internal,
                "Core compatibility profile is missing a share protocol decision",
            )
        })?;
    Ok(vec![decision])
}

fn share_feature_id(item: &ProtocolSemantic) -> &'static str {
    match item.protocol {
        ShareProtocol::Vless => "proxy.outbound.vless",
        ShareProtocol::Shadowsocks => "proxy.outbound.shadowsocks",
        ShareProtocol::Hysteria2 => "proxy.outbound.hysteria2",
        ShareProtocol::Tuic => "proxy.outbound.tuicV5",
    }
}

fn critical_unknown_parameter(item: &ProtocolSemantic) -> Option<ConfigurationIssue> {
    let critical = match item.protocol {
        ShareProtocol::Vless => [
            "flow",
            "packetEncoding",
            "mode",
            "extra",
            "fm",
            "vcn",
            "pcs",
            "pqv",
            "spx",
        ]
        .as_slice(),
        ShareProtocol::Shadowsocks => ["plugin-opts"].as_slice(),
        ShareProtocol::Hysteria2 => ["salamander-password", "obfs-password"].as_slice(),
        ShareProtocol::Tuic => {
            return item.unknown_parameters.keys().next().map(|key| {
                ConfigurationIssue::error(
                    "CRITICAL_PARAMETER_UNSUPPORTED",
                    format!("TUIC share parameter {key} cannot be safely represented"),
                    ConfigurationStage::TargetTranslate,
                )
            });
        }
    };
    item.unknown_parameters.keys().find_map(|key| {
        critical.contains(&key.as_str()).then(|| {
            ConfigurationIssue::error(
                "CRITICAL_PARAMETER_UNSUPPORTED",
                format!("Share parameter {key} cannot be safely represented by every target Core"),
                ConfigurationStage::TargetTranslate,
            )
        })
    })
}

fn add_tls_object(
    map: &mut Map<String, Value>,
    security: &ShareSecurity,
    warnings: &mut Vec<ConfigurationIssue>,
) {
    if !security.tls {
        return;
    }
    let mut tls = Map::new();
    tls.insert("enabled".into(), Value::Bool(true));
    if let Some(sni) = &security.sni {
        tls.insert("server_name".into(), Value::String(sni.clone()));
    }
    if !security.alpn.is_empty() {
        tls.insert("alpn".into(), json!(security.alpn));
    }
    if let Some(fp) = &security.fingerprint {
        tls.insert("utls".into(), json!({"enabled":true,"fingerprint":fp}));
    }
    if security.insecure {
        tls.insert("insecure".into(), Value::Bool(true));
    }
    if security.reality_public_key.is_some() || security.reality_short_id.is_some() {
        warnings.push(ConfigurationIssue::warning(
            "REALITY_EXTENSION",
            "REALITY parameters require target-specific validation",
            ConfigurationStage::TargetTranslate,
        ));
    }
    if let Some(ech) = &security.ech {
        warnings.push(ConfigurationIssue::warning(
            "ECH_EXTENSION",
            format!("ECH data preserved as extension ({})", redacted_len(ech)),
            ConfigurationStage::TargetTranslate,
        ));
    }
    map.insert("tls".into(), Value::Object(tls));
}

fn redacted_len(value: &str) -> String {
    format!("{} bytes", value.len())
}

fn translate_sing_box_vless(
    item: &ProtocolSemantic,
    warnings: &mut Vec<ConfigurationIssue>,
) -> TranslationResult<Value> {
    let ShareAuthentication::Vless { uuid } = &item.authentication else {
        return Err(ConfigurationIssue::error(
            "AUTH_MISMATCH",
            "VLESS authentication is malformed",
            ConfigurationStage::ProtocolSemantic,
        ));
    };
    let mut map = Map::from_iter([
        ("type".into(), json!("vless")),
        ("server".into(), json!(item.host)),
        ("server_port".into(), json!(item.port)),
        ("uuid".into(), json!(uuid)),
    ]);
    add_tls_object(&mut map, &item.security, warnings);
    add_sing_box_transport(&mut map, &item.transport, warnings)?;
    Ok(Value::Object(map))
}

fn add_sing_box_transport(
    map: &mut Map<String, Value>,
    transport: &ShareTransport,
    warnings: &mut Vec<ConfigurationIssue>,
) -> TranslationResult<()> {
    match transport.kind.as_str() {
        "tcp" => {}
        "ws" => {
            map.insert("transport".into(), json!({"type":"ws","path":transport.path,"headers":transport.host.as_ref().map(|host| json!({"Host":host}))}));
        }
        "grpc" => {
            map.insert(
                "transport".into(),
                json!({"type":"grpc","service_name":transport.service_name}),
            );
        }
        "http" => {
            map.insert(
                "transport".into(),
                json!({"type":"http","path":transport.path}),
            );
        }
        "quic" => {
            map.insert("transport".into(), json!({"type":"quic"}));
        }
        "httpupgrade" => {
            map.insert(
                "transport".into(),
                json!({"type":"httpupgrade","path":transport.path}),
            );
        }
        "xhttp" => {
            warnings.push(ConfigurationIssue::warning(
                "XHTTP_EXTENSION",
                "XHTTP parameters are preserved but require target-specific review",
                ConfigurationStage::TargetTranslate,
            ));
        }
        other => {
            return Err(ConfigurationIssue::error(
                "UNSUPPORTED_TRANSPORT",
                format!("Unsupported sing-box transport {other}"),
                ConfigurationStage::TargetTranslate,
            ));
        }
    }
    Ok(())
}

fn translate_xray_vless(
    item: &ProtocolSemantic,
    warnings: &mut Vec<ConfigurationIssue>,
) -> TranslationResult<Value> {
    let ShareAuthentication::Vless { uuid } = &item.authentication else {
        return Err(ConfigurationIssue::error(
            "AUTH_MISMATCH",
            "VLESS authentication is malformed",
            ConfigurationStage::ProtocolSemantic,
        ));
    };
    let mut stream = Map::new();
    stream.insert("network".into(), json!(item.transport.kind));
    if item.security.tls {
        stream.insert(
            "security".into(),
            json!(if item.security.reality_public_key.is_some() {
                "reality"
            } else {
                "tls"
            }),
        );
    }
    let mut settings = Map::from_iter([(
        "vnext".into(),
        json!([{"address":item.host,"port":item.port,"users":[{"id":uuid,"encryption":"none"}]}]),
    )]);
    if let Some(sni) = &item.security.sni {
        stream.insert(
            "tlsSettings".into(),
            json!({"serverName":sni,"allowInsecure":item.security.insecure}),
        );
    }
    if let Some(path) = &item.transport.path {
        stream.insert("wsSettings".into(), json!({"path":path}));
    }
    if item.transport.kind == "xhttp" {
        warnings.push(ConfigurationIssue::warning(
            "XHTTP_EXTENSION",
            "XHTTP is represented by Xray stream settings and needs native validation",
            ConfigurationStage::TargetTranslate,
        ));
    }
    settings.insert("streamSettings".into(), Value::Object(stream));
    Ok(json!({"protocol":"vless","settings":settings}))
}

fn translate_mihomo_vless(
    item: &ProtocolSemantic,
    warnings: &mut Vec<ConfigurationIssue>,
) -> TranslationResult<Value> {
    let ShareAuthentication::Vless { uuid } = &item.authentication else {
        return Err(ConfigurationIssue::error(
            "AUTH_MISMATCH",
            "VLESS authentication is malformed",
            ConfigurationStage::ProtocolSemantic,
        ));
    };
    let mut map = Map::from_iter([
        ("type".into(), json!("vless")),
        ("server".into(), json!(item.host)),
        ("port".into(), json!(item.port)),
        ("uuid".into(), json!(uuid)),
        ("network".into(), json!(item.transport.kind)),
    ]);
    if item.security.tls {
        map.insert("tls".into(), Value::Bool(true));
    }
    if let Some(sni) = &item.security.sni {
        map.insert("servername".into(), json!(sni));
    }
    if item.security.insecure {
        map.insert("skip-cert-verify".into(), Value::Bool(true));
    }
    if let Some(path) = &item.transport.path {
        map.insert("ws-opts".into(), json!({"path":path}));
    }
    if item.security.ech.is_some() {
        warnings.push(ConfigurationIssue::warning(
            "ECH_EXTENSION",
            "Mihomo ECH support is version-dependent",
            ConfigurationStage::TargetTranslate,
        ));
    }
    Ok(Value::Object(map))
}

fn translate_sing_box_ss(
    item: &ProtocolSemantic,
    warnings: &mut Vec<ConfigurationIssue>,
) -> TranslationResult<Value> {
    let ShareAuthentication::Shadowsocks { method, password } = &item.authentication else {
        return Err(ConfigurationIssue::error(
            "AUTH_MISMATCH",
            "Shadowsocks authentication is malformed",
            ConfigurationStage::ProtocolSemantic,
        ));
    };
    let mut map = Map::from_iter([
        ("type".into(), json!("shadowsocks")),
        ("server".into(), json!(item.host)),
        ("server_port".into(), json!(item.port)),
        ("method".into(), json!(method)),
        ("password".into(), json!(password)),
    ]);
    if let Some(plugin) = item.options.get("plugin") {
        let name = plugin.split(';').next().unwrap_or_default();
        if name == "obfs-local" || name == "v2ray-plugin" {
            map.insert("plugin".into(), json!(plugin));
        } else {
            warnings.push(ConfigurationIssue::warning(
                "SS_PLUGIN",
                "Shadowsocks plugin requires native validation",
                ConfigurationStage::TargetTranslate,
            ));
        }
    }
    Ok(Value::Object(map))
}

fn translate_xray_ss(
    item: &ProtocolSemantic,
    _warnings: &mut Vec<ConfigurationIssue>,
) -> TranslationResult<Value> {
    let ShareAuthentication::Shadowsocks { method, password } = &item.authentication else {
        return Err(ConfigurationIssue::error(
            "AUTH_MISMATCH",
            "Shadowsocks authentication is malformed",
            ConfigurationStage::ProtocolSemantic,
        ));
    };
    Ok(
        json!({"protocol":"shadowsocks","settings":{"servers":[{"address":item.host,"port":item.port,"method":method,"password":password}]}}),
    )
}

fn translate_mihomo_ss(
    item: &ProtocolSemantic,
    warnings: &mut Vec<ConfigurationIssue>,
) -> TranslationResult<Value> {
    let ShareAuthentication::Shadowsocks { method, password } = &item.authentication else {
        return Err(ConfigurationIssue::error(
            "AUTH_MISMATCH",
            "Shadowsocks authentication is malformed",
            ConfigurationStage::ProtocolSemantic,
        ));
    };
    let mut map = Map::from_iter([
        ("type".into(), json!("ss")),
        ("server".into(), json!(item.host)),
        ("port".into(), json!(item.port)),
        ("cipher".into(), json!(method)),
        ("password".into(), json!(password)),
    ]);
    if let Some(plugin) = item.options.get("plugin") {
        let name = plugin.split(';').next().unwrap_or_default();
        if name != "obfs-local" && name != "v2ray-plugin" {
            warnings.push(ConfigurationIssue::warning(
                "SS_PLUGIN",
                "Unknown plugin was not translated",
                ConfigurationStage::TargetTranslate,
            ));
        } else {
            map.insert("plugin".into(), json!(name));
        }
    }
    Ok(Value::Object(map))
}

fn translate_sing_box_hysteria(
    item: &ProtocolSemantic,
    _warnings: &mut Vec<ConfigurationIssue>,
) -> TranslationResult<Value> {
    let ShareAuthentication::Hysteria2 { password } = &item.authentication else {
        return Err(ConfigurationIssue::error(
            "AUTH_MISMATCH",
            "Hysteria2 authentication is malformed",
            ConfigurationStage::ProtocolSemantic,
        ));
    };
    let mut map = Map::from_iter([
        ("type".into(), json!("hysteria2")),
        ("server".into(), json!(item.host)),
        ("server_port".into(), json!(item.port)),
        ("password".into(), json!(password)),
    ]);
    add_tls_object(&mut map, &item.security, _warnings);
    if let Some(obfs) = item.options.get("obfs") {
        map.insert(
            "obfs".into(),
            json!({"type":obfs,"password":item.options.get("obfs-password")}),
        );
    }
    Ok(Value::Object(map))
}

fn translate_mihomo_hysteria(
    item: &ProtocolSemantic,
    warnings: &mut Vec<ConfigurationIssue>,
) -> TranslationResult<Value> {
    let ShareAuthentication::Hysteria2 { password } = &item.authentication else {
        return Err(ConfigurationIssue::error(
            "AUTH_MISMATCH",
            "Hysteria2 authentication is malformed",
            ConfigurationStage::ProtocolSemantic,
        ));
    };
    let mut map = Map::from_iter([
        ("type".into(), json!("hysteria2")),
        ("server".into(), json!(item.host)),
        ("port".into(), json!(item.port)),
        ("password".into(), json!(password)),
    ]);
    if let Some(sni) = &item.security.sni {
        map.insert("sni".into(), json!(sni));
    }
    if item.security.insecure {
        map.insert("skip-cert-verify".into(), Value::Bool(true));
    }
    if item.options.contains_key("obfs") {
        warnings.push(ConfigurationIssue::warning(
            "HYSTERIA_OBFS",
            "Hysteria2 obfuscation is target-version dependent",
            ConfigurationStage::TargetTranslate,
        ));
    }
    Ok(Value::Object(map))
}

fn translate_sing_box_tuic(
    item: &ProtocolSemantic,
    _warnings: &mut Vec<ConfigurationIssue>,
) -> TranslationResult<Value> {
    let ShareAuthentication::Tuic { uuid, password } = &item.authentication else {
        return Err(ConfigurationIssue::error(
            "AUTH_MISMATCH",
            "TUIC authentication is malformed",
            ConfigurationStage::ProtocolSemantic,
        ));
    };
    let mut map = Map::from_iter([
        ("type".into(), json!("tuic")),
        ("server".into(), json!(item.host)),
        ("server_port".into(), json!(item.port)),
        ("uuid".into(), json!(uuid)),
        ("password".into(), json!(password)),
    ]);
    add_tls_object(&mut map, &item.security, _warnings);
    for (key, value) in &item.options {
        if key == "disable_sni" {
            return Err(ConfigurationIssue::error(
                "UNSUPPORTED_TUIC_OPTION",
                "sing-box cannot safely express the imported TUIC disable_sni option",
                ConfigurationStage::TargetTranslate,
            ));
        }
        let translated = if key == "zero_rtt_handshake" {
            Value::Bool(translation_bool(value, "zero_rtt_handshake")?)
        } else {
            Value::String(value.clone())
        };
        map.insert(key.clone(), translated);
    }
    Ok(Value::Object(map))
}

fn translate_mihomo_tuic(
    item: &ProtocolSemantic,
    _warnings: &mut Vec<ConfigurationIssue>,
) -> TranslationResult<Value> {
    let ShareAuthentication::Tuic { uuid, password } = &item.authentication else {
        return Err(ConfigurationIssue::error(
            "AUTH_MISMATCH",
            "TUIC authentication is malformed",
            ConfigurationStage::ProtocolSemantic,
        ));
    };
    let mut map = Map::from_iter([
        ("type".into(), json!("tuic")),
        ("server".into(), json!(item.host)),
        ("port".into(), json!(item.port)),
        ("uuid".into(), json!(uuid)),
        ("password".into(), json!(password)),
    ]);
    if let Some(sni) = &item.security.sni {
        map.insert("sni".into(), json!(sni));
    }
    if item.security.insecure {
        map.insert("skip-cert-verify".into(), Value::Bool(true));
    }
    for (key, value) in &item.options {
        let target_key = match key.as_str() {
            "congestion_control" => "congestion-controller",
            "udp_relay_mode" => "udp-relay-mode",
            "zero_rtt_handshake" => "reduce-rtt",
            "disable_sni" => "disable-sni",
            "heartbeat" => {
                return Err(ConfigurationIssue::error(
                    "UNSUPPORTED_TUIC_OPTION",
                    "Mihomo heartbeat interval semantics are not equivalent to this TUIC share option",
                    ConfigurationStage::TargetTranslate,
                ));
            }
            _ => {
                return Err(ConfigurationIssue::error(
                    "UNSUPPORTED_TUIC_OPTION",
                    "Mihomo cannot safely express this TUIC option",
                    ConfigurationStage::TargetTranslate,
                ));
            }
        };
        let translated = if matches!(key.as_str(), "zero_rtt_handshake" | "disable_sni") {
            Value::Bool(translation_bool(value, key)?)
        } else {
            Value::String(value.clone())
        };
        map.insert(target_key.into(), translated);
    }
    Ok(Value::Object(map))
}

fn translation_bool(value: &str, option: &str) -> TranslationResult<bool> {
    parse_bool(value).map_err(|_| {
        ConfigurationIssue::error(
            "INVALID_BOOLEAN_OPTION",
            format!("TUIC option {option} must be a boolean"),
            ConfigurationStage::TargetTranslate,
        )
    })
}

/// Converts a parsed collection for a target Core without losing item-level
/// acceptance information.  A caller may use only accepted fragments when
/// building the normal SourceSnapshot/merge input.
pub fn preview_share_import(input: &[u8], target: ProgramKind) -> Result<ShareImportPreview> {
    let target = CoreTargetIdentity::unknown(target, None);
    preview_share_import_for_version(input, &target)
}

pub fn preview_share_import_for_version(
    input: &[u8],
    target: &CoreTargetIdentity,
) -> Result<ShareImportPreview> {
    let parsed = parse_share_input(input)?;
    let source_revision = parsed.normalized.content_hash.clone();
    let translator = ShareTranslatorRegistry::new();
    let items = parsed
        .items
        .iter()
        .filter_map(|item| item.semantic.as_ref().map(|semantic| (item, semantic)))
        .map(|(item, semantic)| {
            translator.translate(semantic, target, &source_revision, item.index)
        })
        .collect::<Result<Vec<_>>>()?;
    let mut summary = parsed.summary;
    let accepted = items
        .iter()
        .filter(|item| item.fragment.is_some() && item.summary.blocking_issues.is_empty())
        .count();
    for item in &items {
        summary.issues.extend(item.summary.warnings.iter().cloned());
        summary
            .issues
            .extend(item.summary.blocking_issues.iter().cloned());
    }
    summary.accepted_items = accepted;
    summary.rejected_items = summary.total_items.saturating_sub(accepted);
    summary.warning_count = summary
        .issues
        .iter()
        .filter(|issue| issue.severity == ShareIssueSeverity::Warning)
        .count();
    summary.collection_status = match (summary.total_items, accepted) {
        (0, _) => CollectionStatus::Empty,
        (total, accepted) if total == accepted => CollectionStatus::Success,
        (_, 0) => CollectionStatus::NoValidItems,
        _ => CollectionStatus::PartialSuccess,
    };
    summary.fidelity = match (summary.total_items, accepted, summary.warning_count) {
        (0, _, _) => ParseFidelity::Invalid,
        (total, accepted, 0) if total == accepted => ParseFidelity::Exact,
        (total, accepted, _) if total == accepted => ParseFidelity::Compatible,
        (_, 0, _) => ParseFidelity::Unsupported,
        _ => ParseFidelity::Partial,
    };
    Ok(ShareImportPreview {
        normalized: parsed.normalized,
        summary,
        items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vless_parser_decodes_transport_security_and_name() {
        let input = b"vless://123e4567-e89b-12d3-a456-426614174000@example.com:443?encryption=none&security=tls&type=ws&path=%2Fedge&sni=cdn.example#Edge%20Node";
        let result = parse_share_input(input).expect("parse");
        assert_eq!(result.summary.collection_status, CollectionStatus::Success);
        let semantic = result.items[0].semantic.as_ref().expect("semantic");
        assert_eq!(semantic.protocol, ShareProtocol::Vless);
        assert_eq!(semantic.transport.path.as_deref(), Some("/edge"));
        assert_eq!(semantic.security.sni.as_deref(), Some("cdn.example"));
        assert_eq!(semantic.name.as_deref(), Some("Edge Node"));
    }

    #[test]
    fn shadowsocks_supports_sip002_and_whole_authority_base64() {
        let plain =
            parse_share_item("ss://aes-128-gcm:secret@example.com:8388#plain").expect("plain");
        assert_eq!(plain.dialect, ShareDialect::ShadowsocksSip002);
        let encoded = general_purpose::STANDARD.encode("aes-128-gcm:secret@example.com:8388");
        let whole = parse_share_item(&format!("ss://{encoded}#encoded")).expect("encoded");
        assert_eq!(whole.dialect, ShareDialect::ShadowsocksWholeBase64);
        assert_eq!(whole.host, "example.com");

        let userinfo = general_purpose::URL_SAFE_NO_PAD.encode("aes-256-gcm:p@ss:word");
        let sip002 = parse_share_item(&format!("ss://{userinfo}@example.com:8388#sip002"))
            .expect("SIP002 userinfo");
        assert_eq!(sip002.dialect, ShareDialect::ShadowsocksSip002);
        assert!(matches!(
            sip002.authentication,
            ShareAuthentication::Shadowsocks { ref method, ref password }
                if method == "aes-256-gcm" && password == "p@ss:word"
        ));
    }

    #[test]
    fn partial_success_is_item_isolated_and_keeps_rejection_details() {
        let result = parse_share_input(
            b"vless://123e4567-e89b-12d3-a456-426614174000@example.com:443\nss://bad\n",
        )
        .expect("parse");
        assert_eq!(
            result.summary.collection_status,
            CollectionStatus::PartialSuccess
        );
        assert_eq!(result.summary.accepted_items, 1);
        assert_eq!(result.summary.rejected_items, 1);
        assert!(!result.items[1].issues.is_empty());
    }

    #[test]
    fn unsupported_protocol_is_counted_as_an_item_without_being_translated() {
        let result = parse_share_input(b"vmess://opaque-token").expect("parse");
        assert_eq!(result.summary.total_items, 1);
        assert_eq!(result.summary.accepted_items, 0);
        assert_eq!(result.summary.fidelity, ParseFidelity::Unsupported);
        assert_eq!(
            result.summary.collection_status,
            CollectionStatus::NoValidItems
        );
    }

    #[test]
    fn base64_decodes_once_and_detects_collection() {
        let encoded = general_purpose::STANDARD.encode("hy2://password@example.com\ntuic://123e4567-e89b-12d3-a456-426614174000:secret@example.com:443");
        let normalized = normalize_share_input(encoded.as_bytes()).expect("normalize");
        assert_eq!(normalized.envelope, EnvelopeKind::Base64);
        assert_eq!(normalized.payload, PayloadKind::ShareCollection);

        let url_safe = general_purpose::URL_SAFE_NO_PAD
            .encode("hy2://password@example.com\ntuic://123e4567-e89b-12d3-a456-426614174000:secret@example.com:443#¾");
        let normalized = normalize_share_input(url_safe.as_bytes()).expect("url-safe normalize");
        assert_eq!(normalized.envelope, EnvelopeKind::Base64Url);
        assert_eq!(normalized.payload, PayloadKind::ShareCollection);
    }

    #[test]
    fn stable_identity_excludes_display_name_but_keeps_auth_and_transport() {
        let first = stable_item_id(
            "vless://123e4567-e89b-12d3-a456-426614174000@example.com:443?type=ws&path=%2F#a",
        );
        let second = stable_item_id(
            "vless://123e4567-e89b-12d3-a456-426614174000@example.com:443?type=ws&path=%2F#b",
        );
        let third = stable_item_id(
            "vless://123e4567-e89b-12d3-a456-426614174000@example.com:443?type=grpc&serviceName=x#a",
        );
        assert_eq!(first, second);
        assert_ne!(first, third);
        let insecure = stable_item_id(
            "vless://123e4567-e89b-12d3-a456-426614174000@example.com:443?security=tls&allowInsecure=true&type=ws&path=%2F#a",
        );
        assert_ne!(first, insecure);
    }

    #[test]
    fn translators_reject_tuic_on_xray_and_emit_native_fragments() {
        let tuic =
            parse_share_item("tuic://123e4567-e89b-12d3-a456-426614174000:secret@example.com:443")
                .expect("tuic");
        let xray = CoreTargetIdentity::unknown(ProgramKind::Xray, None);
        let translated = translate_share_item(&tuic, &xray, "rev", 0).expect("translation");
        assert!(translated.fragment.is_none());
        assert_eq!(
            translated.summary.fidelity,
            TranslationFidelity::Unsupported
        );
        let vless = parse_share_item(
            "vless://123e4567-e89b-12d3-a456-426614174000@example.com:443?security=tls",
        )
        .expect("vless");
        let sing_box = CoreTargetIdentity::unknown(ProgramKind::SingBox, None);
        let translated = translate_share_item(&vless, &sing_box, "rev", 0).expect("translation");
        assert_eq!(
            translated.summary.translator_revision,
            SHARE_TRANSLATOR_REVISION
        );
        assert_eq!(translated.provenance.target, translated.summary.target);
        assert_eq!(
            translated.provenance.profile_hash,
            translated.summary.profile_hash
        );
        assert_eq!(translated.fragment.expect("fragment")["type"], "vless");
    }

    #[test]
    fn maintained_xray_baselines_reject_tuic_translation() {
        let tuic =
            parse_share_item("tuic://123e4567-e89b-12d3-a456-426614174000:secret@example.com:443")
                .expect("tuic");
        let knowledge = crate::embedded_core_knowledge().unwrap();
        for baseline in &knowledge.program(ProgramKind::Xray).unwrap().releases {
            let probe = crate::CoreProbeReport::from_program_output(
                ProgramKind::Xray,
                &format!("Xray {}", baseline.version),
            );
            let xray =
                crate::CoreTargetIdentity::from_probe(ProgramKind::Xray, &probe, None).unwrap();
            let translated =
                translate_share_item(&tuic, &xray, "rev", 0).expect("translation decision");
            assert!(translated.fragment.is_none());
            assert_eq!(translated.summary.target, xray);
            assert_eq!(translated.summary.blocking_issues.len(), 1);
            assert_eq!(
                translated.summary.blocking_issues[0].code,
                "UNSUPPORTED_TUIC"
            );
        }
    }

    #[test]
    fn unknown_query_parameters_are_preserved_without_being_logged() {
        let semantic = parse_share_item(
            "vless://123e4567-e89b-12d3-a456-426614174000@example.com:443?foo=bar",
        )
        .expect("vless");
        assert_eq!(semantic.unknown_parameters["foo"], vec!["bar"]);
        let serialized = serde_json::to_string(&SourceParseItem {
            item_id: "id".into(),
            index: 0,
            protocol: Some(ShareProtocol::Vless),
            dialect: Some(ShareDialect::Standard),
            fidelity: ParseFidelity::Exact,
            name: None,
            semantic: None,
            issues: Vec::new(),
            original_hash: "hash".into(),
        })
        .expect("serialize");
        assert!(!serialized.contains("bar"));
    }

    #[test]
    fn url_edge_cases_are_decoded_and_ambiguous_query_is_rejected() {
        let semantic = parse_share_item(
            "vless://123e4567-e89b-12d3-a456-426614174000@[2001:db8::1]:443?type=ws&path=%2Fa%26b%3Dc&host=b%C3%BCcher.example#%E6%9D%B1%E4%BA%AC",
        )
        .expect("IPv6 and percent encoding");
        assert_eq!(semantic.host, "[2001:db8::1]".trim_matches(['[', ']']));
        assert_eq!(semantic.transport.path.as_deref(), Some("/a&b=c"));
        assert_eq!(semantic.transport.host.as_deref(), Some("bücher.example"));
        assert_eq!(semantic.name.as_deref(), Some("東京"));

        let repeated = parse_share_item(
            "vless://123e4567-e89b-12d3-a456-426614174000@example.com:443?security=tls&security=none",
        )
        .expect_err("repeated critical query");
        assert_eq!(repeated.code, ErrorCode::ConfigInvalid);
        let invalid_boolean = parse_share_item("hy2://password@example.com:443?insecure=perhaps")
            .expect_err("invalid boolean");
        assert_eq!(invalid_boolean.code, ErrorCode::ConfigInvalid);
    }

    #[test]
    fn unsupported_dialects_and_target_features_never_count_as_accepted() {
        let realm = parse_share_input(b"hy2+realm://password@example.com:443").expect("realm");
        assert_eq!(realm.summary.accepted_items, 0);
        assert_eq!(realm.items[0].dialect, Some(ShareDialect::Hysteria2Realm));

        let tuic_v4 = parse_share_input(b"tuic://opaque-token@example.com:443").expect("v4");
        assert_eq!(tuic_v4.summary.accepted_items, 0);
        assert_eq!(tuic_v4.items[0].dialect, Some(ShareDialect::TuicV4));

        let preview = preview_share_import(
            b"tuic://123e4567-e89b-12d3-a456-426614174000:secret@example.com:443",
            ProgramKind::Xray,
        )
        .expect("preview");
        assert_eq!(preview.summary.accepted_items, 0);
        assert_eq!(preview.summary.rejected_items, 1);
        assert_eq!(
            preview.summary.collection_status,
            CollectionStatus::NoValidItems
        );
    }

    #[test]
    fn critical_unknown_parameters_block_translation_but_metadata_is_warned() {
        let critical = preview_share_import(
            b"vless://123e4567-e89b-12d3-a456-426614174000@example.com:443?flow=xtls-rprx-vision",
            ProgramKind::SingBox,
        )
        .expect("critical preview");
        assert_eq!(critical.summary.accepted_items, 0);
        assert!(!critical.items[0].summary.blocking_issues.is_empty());

        let compatible = preview_share_import(
            b"vless://123e4567-e89b-12d3-a456-426614174000@example.com:443?futureMetadata=value",
            ProgramKind::SingBox,
        )
        .expect("metadata preview");
        assert_eq!(compatible.summary.accepted_items, 1);
        assert!(compatible.summary.warning_count >= 2);
        assert_eq!(compatible.summary.fidelity, ParseFidelity::Compatible);
    }
}
