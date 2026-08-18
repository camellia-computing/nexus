use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::share_compatibility::{
    SourceItemProvenance, SourceParseSummary, TranslationFidelity, preview_share_import_for_version,
};
use crate::{
    CamelliaNexusError, CoreCompatibilityProfile, CoreTargetIdentity, CoreValidationEvidence,
    ErrorCode, ManagedConfigSpec, ProgramKind, Result, XrayDashboardSpec,
    config_service::hash_bytes, embedded_core_compatibility_catalog,
    embedded_core_upstream_manifest, normalize_dashboard_interval, normalize_jsonc,
    parse_dashboard_interval_nanos,
};

pub const CONFIGURATION_STATE_SCHEMA_VERSION: u32 = 5;
pub const LEGACY_CONFIGURATION_STATE_SCHEMA_VERSION: u32 = 4;
const LEGACY_MANAGED_CONFIGURATION_STATE_SCHEMA_VERSION: u32 = 3;
const DASHBOARD_INTENT_PREFIX: &str = "dashboard.";
const MANAGED_SING_BOX_API_TAG: &str = "camellia-nexus-api";
const MANAGED_SING_BOX_CLASH_UI: &str = "clash-dashboard";
const MANAGED_MIHOMO_UI_DIRECTORY: &str = "camellia-nexus-mihomo-dashboard";
const MANAGED_XRAY_API_TAG: &str = "camellia-nexus-api";
const MANAGED_XRAY_METRICS_TAG: &str = "camellia-nexus-metrics";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigurationFormat {
    Jsonc,
    Yaml,
}

impl ConfigurationFormat {
    pub fn for_kind(kind: ProgramKind) -> Option<Self> {
        match kind {
            ProgramKind::SingBox | ProgramKind::Xray => Some(Self::Jsonc),
            ProgramKind::Mihomo => Some(Self::Yaml),
            ProgramKind::Generic => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationRevision {
    pub generation: u64,
    pub content_hash: String,
    pub created_unix_ms: u64,
}

impl ConfigurationRevision {
    pub fn new(generation: u64, content: &str, created_unix_ms: u64) -> Self {
        Self {
            generation,
            content_hash: hash_bytes(content.as_bytes()),
            created_unix_ms,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceFreshness {
    Fresh,
    Stale,
    Unavailable,
    Invalid,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceStatus {
    pub source_id: String,
    pub source_name: String,
    pub freshness: SourceFreshness,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_unix_ms: Option<u64>,
}

pub type SourceObservation = SourceStatus;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceSnapshot {
    pub source_id: String,
    pub source_name: String,
    pub format: ConfigurationFormat,
    pub content: String,
    pub content_hash: String,
    pub parsed_unix_ms: u64,
    #[serde(default)]
    pub append_xray_outbounds: bool,
    /// Present when the source was detected as a share collection.  Native
    /// configuration sources leave this unset and continue using the same
    /// semantic merge path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_summary: Option<SourceParseSummary>,
    #[serde(default)]
    pub share_provenance: Vec<SourceItemProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_observation_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translated_fragment_ref: Option<String>,
}

impl SourceSnapshot {
    pub fn parse(
        source_id: impl Into<String>,
        source_name: impl Into<String>,
        format: ConfigurationFormat,
        content: &[u8],
        parsed_unix_ms: u64,
        append_xray_outbounds: bool,
    ) -> Result<Self> {
        if content.iter().all(u8::is_ascii_whitespace) {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Configuration source is empty",
            ));
        }
        let value = parse_semantic_document(format, content)?;
        let content = serialize_semantic_document(format, &value)?;
        Ok(Self {
            source_id: source_id.into(),
            source_name: source_name.into(),
            format,
            content_hash: hash_bytes(content.as_bytes()),
            content,
            parsed_unix_ms,
            append_xray_outbounds,
            share_summary: None,
            share_provenance: Vec::new(),
            raw_observation_ref: None,
            translated_fragment_ref: None,
        })
    }

    /// Parses a share-link collection, translates each accepted item for the
    /// selected Core, and turns the resulting fragments into an ordinary
    /// semantic source snapshot.  Invalid/unsupported items remain visible in
    /// the summary; callers must keep the previous snapshot when no item is
    /// accepted.
    pub fn parse_share(
        source_id: impl Into<String>,
        source_name: impl Into<String>,
        target: &CoreTargetIdentity,
        content: &[u8],
        parsed_unix_ms: u64,
    ) -> Result<Self> {
        let source_id = source_id.into();
        let source_name = source_name.into();
        let preview = preview_share_import_for_version(content, target)?;
        let kind = target.program;
        if preview.summary.accepted_items == 0 {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Share source contains no translatable items",
            ));
        }
        let mut fragments = Vec::new();
        let mut provenance = Vec::new();
        for item in preview.items {
            if item.fragment.is_none()
                || item.summary.fidelity == TranslationFidelity::Unsupported
                || !item.summary.blocking_issues.is_empty()
            {
                continue;
            }
            let mut fragment = item.fragment.expect("checked above");
            match kind {
                ProgramKind::SingBox => {
                    if fragment.get("tag").is_none() {
                        fragment["tag"] = Value::String(item.item_id.clone());
                    }
                }
                ProgramKind::Xray => {
                    if fragment
                        .get("settings")
                        .and_then(Value::as_object)
                        .and_then(|settings| settings.get("vnext"))
                        .is_some()
                    {
                        fragment["tag"] = Value::String(item.item_id.clone());
                    }
                }
                ProgramKind::Mihomo => {
                    let name = item
                        .semantic
                        .name
                        .clone()
                        .unwrap_or_else(|| item.item_id.clone());
                    fragment["name"] = Value::String(name);
                }
                ProgramKind::Generic => continue,
            }
            fragments.push(fragment);
            provenance.push(item.provenance);
        }
        if fragments.is_empty() {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Share source contains no compatible items for this Core",
            ));
        }
        let value = match kind {
            ProgramKind::SingBox | ProgramKind::Xray => json!({ "outbounds": fragments }),
            ProgramKind::Mihomo => json!({ "proxies": fragments }),
            ProgramKind::Generic => unreachable!("generic rejected by translators"),
        };
        let format = ConfigurationFormat::for_kind(kind).expect("target Core format");
        let serialized = serialize_semantic_document(format, &value)?;
        let translated_fragment_ref = hash_bytes(serialized.as_bytes());
        Ok(Self {
            source_id,
            source_name,
            format,
            content_hash: hash_bytes(serialized.as_bytes()),
            content: serialized,
            parsed_unix_ms,
            append_xray_outbounds: false,
            share_summary: Some(preview.summary),
            share_provenance: provenance,
            raw_observation_ref: Some(hash_bytes(content)),
            translated_fragment_ref: Some(translated_fragment_ref),
        })
    }

    fn value(&self) -> Result<Value> {
        parse_semantic_document(self.format, self.content.as_bytes())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProvenanceEntry {
    pub semantic_path: String,
    pub source_ids: Vec<String>,
    #[serde(default)]
    pub guided: bool,
    #[serde(default)]
    pub raw: bool,
}

pub type Provenance = Vec<ProvenanceEntry>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationConflict {
    pub semantic_path: String,
    pub reason: String,
    pub severity: ConflictSeverity,
    /// Stable UI mapping key.  `reason` remains technical context for logs,
    /// while clients use this key for the localized explanation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_key: Option<String>,
    #[serde(default)]
    pub scope: ConfigurationIssueScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guided_value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_value: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SemanticMergeResult {
    pub content: String,
    pub content_hash: String,
    pub provenance: Vec<ProvenanceEntry>,
    pub conflicts: Vec<ConfigurationConflict>,
}

/// A semantic merge keeps a parallel tree so provenance follows the same
/// replacement, identity matching, and ordered-list rules as the document.
/// Container source ids are only emitted when the container is empty; once a
/// container has children, each final leaf has a more useful precise origin.
#[derive(Debug, Clone)]
enum ProvenanceNode {
    Leaf(Vec<String>),
    Object {
        fields: BTreeMap<String, ProvenanceNode>,
        source_ids: Vec<String>,
    },
    Array {
        items: Vec<ProvenanceNode>,
        source_ids: Vec<String>,
    },
}

impl ProvenanceNode {
    fn from_value(value: &Value, source_id: &str) -> Self {
        match value {
            Value::Object(fields) => Self::Object {
                fields: fields
                    .iter()
                    .map(|(key, value)| (key.clone(), Self::from_value(value, source_id)))
                    .collect(),
                source_ids: if fields.is_empty() {
                    vec![source_id.to_owned()]
                } else {
                    Vec::new()
                },
            },
            Value::Array(items) => Self::Array {
                items: items
                    .iter()
                    .map(|value| Self::from_value(value, source_id))
                    .collect(),
                source_ids: if items.is_empty() {
                    vec![source_id.to_owned()]
                } else {
                    Vec::new()
                },
            },
            _ => Self::Leaf(vec![source_id.to_owned()]),
        }
    }
}

fn merge_source_ids(current: &mut Vec<String>, next: Vec<String>) {
    for source_id in next {
        if !current.contains(&source_id) {
            current.push(source_id);
        }
    }
}

pub fn merge_configuration_sources(
    kind: ProgramKind,
    snapshots: &[SourceSnapshot],
) -> Result<SemanticMergeResult> {
    let format = ConfigurationFormat::for_kind(kind).ok_or_else(|| {
        CamelliaNexusError::invalid_spec(
            "Generic programs do not support semantic configuration sources",
        )
    })?;
    if snapshots.is_empty() {
        return Err(CamelliaNexusError::invalid_spec(
            "At least one configuration source snapshot is required",
        ));
    }
    if snapshots.iter().any(|snapshot| snapshot.format != format) {
        return Err(CamelliaNexusError::invalid_spec(
            "Configuration source format does not match the Core",
        ));
    }

    let mut values = Vec::with_capacity(snapshots.len());
    for snapshot in snapshots {
        values.push(snapshot.value()?);
    }
    let conflicts = collect_source_value_conflicts(kind, snapshots, &values);
    let mut merged = values.remove(0);
    let mut merged_provenance = ProvenanceNode::from_value(&merged, &snapshots[0].source_id);
    ensure_root_mapping(&merged)?;
    for (index, value) in values.into_iter().enumerate() {
        ensure_root_mapping(&value)?;
        let snapshot = &snapshots[index + 1];
        let next_provenance = ProvenanceNode::from_value(&value, &snapshot.source_id);
        match kind {
            ProgramKind::SingBox => {
                merge_sing_box_value(&mut merged, &mut merged_provenance, value, next_provenance)
            }
            ProgramKind::Xray => merge_xray_root(
                &mut merged,
                &mut merged_provenance,
                value,
                next_provenance,
                snapshot.append_xray_outbounds,
            )?,
            ProgramKind::Mihomo => merge_mihomo_value(
                &mut merged,
                &mut merged_provenance,
                value,
                next_provenance,
                None,
            ),
            ProgramKind::Generic => unreachable!("generic rejected above"),
        }
    }
    let content = serialize_semantic_document(format, &merged)?;
    Ok(SemanticMergeResult {
        content_hash: hash_bytes(content.as_bytes()),
        content,
        provenance: build_source_provenance(&merged_provenance),
        conflicts,
    })
}

fn ensure_root_mapping(value: &Value) -> Result<()> {
    if value.is_object() {
        Ok(())
    } else {
        Err(CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "Configuration source root must be an object or mapping",
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceSequencePolicy {
    Append,
    Identity(&'static str),
    Replace,
}

/// Detect same-level Source conflicts independently from the deterministic
/// preview merge.  The preview remains useful while a conflict is being
/// repaired, but the returned blocking issues prevent Save/Validate/Apply.
/// Comparing the original source documents (instead of the progressively
/// merged preview) also keeps both owning source ids available to the UI.
fn collect_source_value_conflicts(
    kind: ProgramKind,
    snapshots: &[SourceSnapshot],
    values: &[Value],
) -> Vec<ConfigurationConflict> {
    let mut conflicts = BTreeMap::new();
    for right_index in 1..values.len() {
        for left_index in 0..right_index {
            compare_source_values(
                kind,
                &values[left_index],
                &values[right_index],
                &mut Vec::new(),
                &snapshots[left_index].source_id,
                &snapshots[right_index].source_id,
                &mut conflicts,
            );
        }
    }
    conflicts.into_values().collect()
}

fn compare_source_values(
    kind: ProgramKind,
    left: &Value,
    right: &Value,
    path: &mut SemanticPath,
    left_source: &str,
    right_source: &str,
    conflicts: &mut BTreeMap<String, ConfigurationConflict>,
) {
    if values_semantically_equal(left, right, path) {
        return;
    }
    match (left, right) {
        (Value::Object(left), Value::Object(right)) => {
            for (key, left_value) in left {
                let Some(right_value) = right.get(key) else {
                    continue;
                };
                path.push(SemanticPathSegment::Key { key: key.clone() });
                compare_source_values(
                    kind,
                    left_value,
                    right_value,
                    path,
                    left_source,
                    right_source,
                    conflicts,
                );
                path.pop();
            }
        }
        (Value::Array(left), Value::Array(right)) => match source_sequence_policy(kind, path) {
            SourceSequencePolicy::Append => {
                if let Some(identity_field) = shared_sequence_identity_field(left, right) {
                    compare_identity_source_sequences(
                        kind,
                        left,
                        right,
                        identity_field,
                        path,
                        left_source,
                        right_source,
                        conflicts,
                    );
                }
            }
            SourceSequencePolicy::Identity(identity_field) => {
                compare_identity_source_sequences(
                    kind,
                    left,
                    right,
                    identity_field,
                    path,
                    left_source,
                    right_source,
                    conflicts,
                );
            }
            SourceSequencePolicy::Replace => insert_source_value_conflict(
                kind,
                path,
                &Value::Array(left.clone()),
                &Value::Array(right.clone()),
                left_source,
                right_source,
                conflicts,
            ),
        },
        // Xray uses null as an explicit "do not replace this section" marker.
        // Treating that marker as a value conflict would block a merge whose
        // established Core-specific semantics already preserve the old value.
        (_, Value::Null) if kind == ProgramKind::Xray => {}
        _ => insert_source_value_conflict(
            kind,
            path,
            left,
            right,
            left_source,
            right_source,
            conflicts,
        ),
    }
}

fn source_sequence_policy(kind: ProgramKind, path: &[SemanticPathSegment]) -> SourceSequencePolicy {
    let last_key = path.iter().rev().find_map(|segment| match segment {
        SemanticPathSegment::Key { key } => Some(key.as_str()),
        SemanticPathSegment::Identity { .. } => None,
    });
    match kind {
        ProgramKind::SingBox => SourceSequencePolicy::Append,
        ProgramKind::Xray if matches!(last_key, Some("inbounds" | "outbounds")) => {
            SourceSequencePolicy::Identity("tag")
        }
        ProgramKind::Xray => SourceSequencePolicy::Replace,
        ProgramKind::Mihomo if matches!(last_key, Some("proxies" | "proxy-groups" | "listeners")) => {
            SourceSequencePolicy::Identity("name")
        }
        ProgramKind::Mihomo
            if last_key == Some("rules")
                || path.iter().any(|segment| {
                    matches!(segment, SemanticPathSegment::Key { key } if key == "sub-rules")
                }) =>
        {
            SourceSequencePolicy::Append
        }
        ProgramKind::Mihomo => SourceSequencePolicy::Replace,
        ProgramKind::Generic => SourceSequencePolicy::Replace,
    }
}

fn shared_sequence_identity_field(left: &[Value], right: &[Value]) -> Option<&'static str> {
    ["tag", "name", "id"].into_iter().find(|field| {
        left.iter()
            .any(|item| item.get(*field).and_then(Value::as_str).is_some())
            && right
                .iter()
                .any(|item| item.get(*field).and_then(Value::as_str).is_some())
    })
}

#[allow(clippy::too_many_arguments)]
fn compare_identity_source_sequences(
    kind: ProgramKind,
    left: &[Value],
    right: &[Value],
    identity_field: &str,
    path: &mut SemanticPath,
    left_source: &str,
    right_source: &str,
    conflicts: &mut BTreeMap<String, ConfigurationConflict>,
) {
    for left_item in left {
        let Some(identity) = left_item.get(identity_field).and_then(Value::as_str) else {
            continue;
        };
        let Some(right_item) = right
            .iter()
            .find(|item| item.get(identity_field).and_then(Value::as_str) == Some(identity))
        else {
            continue;
        };
        path.push(SemanticPathSegment::Identity {
            field: identity_field.to_owned(),
            value: identity.to_owned(),
        });
        compare_source_values(
            kind,
            left_item,
            right_item,
            path,
            left_source,
            right_source,
            conflicts,
        );
        path.pop();
    }
}

fn insert_source_value_conflict(
    kind: ProgramKind,
    path: &[SemanticPathSegment],
    left: &Value,
    right: &Value,
    left_source: &str,
    right_source: &str,
    conflicts: &mut BTreeMap<String, ConfigurationConflict>,
) {
    let semantic_path = display_semantic_path(path);
    conflicts.entry(semantic_path.clone()).or_insert_with(|| {
        let effective = if kind == ProgramKind::SingBox {
            left
        } else {
            right
        };
        ConfigurationConflict {
            semantic_path,
            reason: format!(
                "Configuration sources {left_source} and {right_source} provide different values"
            ),
            severity: ConflictSeverity::Error,
            message_key: Some("SOURCE_VALUE_CONFLICT".into()),
            scope: ConfigurationIssueScope::sources(right_source.to_owned()),
            source_value: Some(left.clone()),
            guided_value: None,
            raw_value: Some(right.clone()),
            effective_value: Some(effective.clone()),
        }
    });
}

fn merge_sing_box_value(
    current: &mut Value,
    current_provenance: &mut ProvenanceNode,
    next: Value,
    next_provenance: ProvenanceNode,
) {
    match (current, current_provenance, next, next_provenance) {
        (
            Value::Object(current),
            ProvenanceNode::Object {
                fields: current_provenance,
                source_ids: current_sources,
            },
            Value::Object(next),
            ProvenanceNode::Object {
                fields: mut next_provenance,
                source_ids: next_sources,
            },
        ) => {
            for (key, value) in next {
                let provenance = next_provenance
                    .remove(&key)
                    .expect("provenance tree follows source document");
                if let Some(existing) = current.get_mut(&key) {
                    merge_sing_box_value(
                        existing,
                        current_provenance
                            .get_mut(&key)
                            .expect("provenance tree follows source document"),
                        value,
                        provenance,
                    );
                } else {
                    current.insert(key.clone(), value);
                    current_provenance.insert(key, provenance);
                }
            }
            merge_source_ids(current_sources, next_sources);
        }
        (
            Value::Array(current),
            ProvenanceNode::Array {
                items: current_provenance,
                source_ids: current_sources,
            },
            Value::Array(mut next),
            ProvenanceNode::Array {
                items: mut next_provenance,
                source_ids: next_sources,
            },
        ) => {
            current.append(&mut next);
            current_provenance.append(&mut next_provenance);
            merge_source_ids(current_sources, next_sources);
        }
        // badjson.MergeJSON keeps the earlier scalar/leaf value.
        (_, _, _, _) => {}
    }
}

fn merge_xray_root(
    current: &mut Value,
    current_provenance: &mut ProvenanceNode,
    next: Value,
    next_provenance: ProvenanceNode,
    append_outbounds: bool,
) -> Result<()> {
    let (current, current_provenance) = match (current, current_provenance) {
        (
            Value::Object(current),
            ProvenanceNode::Object {
                fields: current_provenance,
                ..
            },
        ) => (current, current_provenance),
        _ => {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Xray configuration must be an object",
            ));
        }
    };
    let (next, mut next_provenance) = match (next, next_provenance) {
        (
            Value::Object(next),
            ProvenanceNode::Object {
                fields: next_provenance,
                ..
            },
        ) => (next, next_provenance),
        _ => {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Xray configuration must be an object",
            ));
        }
    };
    for (key, value) in next {
        let provenance = next_provenance
            .remove(&key)
            .expect("provenance tree follows source document");
        match key.as_str() {
            "env" => merge_xray_env(current, current_provenance, value, provenance)?,
            "inbounds" => merge_identity_sequence(
                current,
                current_provenance,
                &key,
                value,
                provenance,
                "tag",
                false,
            )?,
            "outbounds" => merge_identity_sequence(
                current,
                current_provenance,
                &key,
                value,
                provenance,
                "tag",
                !append_outbounds,
            )?,
            _ => {
                if value.is_null() {
                    continue;
                }
                if let Some(existing) = current.get_mut(&key) {
                    merge_xray_value(
                        existing,
                        current_provenance
                            .get_mut(&key)
                            .expect("provenance tree follows source document"),
                        value,
                        provenance,
                    );
                } else {
                    current.insert(key.clone(), value);
                    current_provenance.insert(key, provenance);
                }
            }
        }
    }
    Ok(())
}

/// Xray's top-level named sequences retain their dedicated identity policy,
/// while ordinary object sections can safely merge disjoint fields.  A later
/// scalar/list remains the deterministic preview winner; the independent
/// Source conflict pass blocks activation when those values differ.
fn merge_xray_value(
    current: &mut Value,
    current_provenance: &mut ProvenanceNode,
    next: Value,
    next_provenance: ProvenanceNode,
) {
    match (current, current_provenance, next, next_provenance) {
        (
            Value::Object(current),
            ProvenanceNode::Object {
                fields: current_provenance,
                source_ids: current_sources,
            },
            Value::Object(next),
            ProvenanceNode::Object {
                fields: mut next_provenance,
                source_ids: next_sources,
            },
        ) => {
            for (key, value) in next {
                let provenance = next_provenance
                    .remove(&key)
                    .expect("provenance tree follows source document");
                if let Some(existing) = current.get_mut(&key) {
                    merge_xray_value(
                        existing,
                        current_provenance
                            .get_mut(&key)
                            .expect("provenance tree follows source document"),
                        value,
                        provenance,
                    );
                } else {
                    current.insert(key.clone(), value);
                    current_provenance.insert(key, provenance);
                }
            }
            merge_source_ids(current_sources, next_sources);
        }
        (current, current_provenance, next, next_provenance) => {
            *current = next;
            *current_provenance = next_provenance;
        }
    }
}

fn merge_xray_env(
    current: &mut Map<String, Value>,
    current_provenance: &mut BTreeMap<String, ProvenanceNode>,
    next: Value,
    next_provenance: ProvenanceNode,
) -> Result<()> {
    let (
        Value::Object(next),
        ProvenanceNode::Object {
            fields: mut next_provenance,
            source_ids: next_sources,
        },
    ) = (next, next_provenance)
    else {
        return Err(CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "Xray env must be an object",
        ));
    };
    let entry = current
        .entry("env".to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    let target = entry.as_object_mut().ok_or_else(|| {
        CamelliaNexusError::new(ErrorCode::ConfigInvalid, "Xray env must be an object")
    })?;
    let entry_provenance = current_provenance
        .entry("env".to_owned())
        .or_insert_with(|| ProvenanceNode::Object {
            fields: BTreeMap::new(),
            source_ids: Vec::new(),
        });
    let (target_provenance, target_sources) = match entry_provenance {
        ProvenanceNode::Object { fields, source_ids } => (fields, source_ids),
        _ => {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Xray env must be an object",
            ));
        }
    };
    for (key, value) in next {
        let provenance = next_provenance
            .remove(&key)
            .expect("provenance tree follows source document");
        target.insert(key.clone(), value);
        target_provenance.insert(key, provenance);
    }
    merge_source_ids(target_sources, next_sources);
    Ok(())
}

fn merge_identity_sequence(
    current: &mut Map<String, Value>,
    current_provenance: &mut BTreeMap<String, ProvenanceNode>,
    key: &str,
    next: Value,
    next_provenance: ProvenanceNode,
    identity_key: &str,
    prepend_new: bool,
) -> Result<()> {
    let (
        Value::Array(next),
        ProvenanceNode::Array {
            items: next_provenance,
            source_ids: next_sources,
        },
    ) = (next, next_provenance)
    else {
        return Err(CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            format!("{key} must be an array"),
        ));
    };
    let target = current
        .entry(key.to_owned())
        .or_insert_with(|| Value::Array(Vec::new()));
    let target = target.as_array_mut().ok_or_else(|| {
        CamelliaNexusError::new(ErrorCode::ConfigInvalid, format!("{key} must be an array"))
    })?;
    let entry_provenance =
        current_provenance
            .entry(key.to_owned())
            .or_insert_with(|| ProvenanceNode::Array {
                items: Vec::new(),
                source_ids: Vec::new(),
            });
    let (target_provenance, target_sources) = match entry_provenance {
        ProvenanceNode::Array { items, source_ids } => (items, source_ids),
        _ => {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                format!("{key} must be an array"),
            ));
        }
    };
    let mut additions = Vec::new();
    let mut addition_provenance = Vec::new();
    for (value, provenance) in next.into_iter().zip(next_provenance) {
        let identity = value
            .get(identity_key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        if let Some(identity) = identity
            && let Some(index) = target.iter().position(|existing| {
                existing.get(identity_key).and_then(Value::as_str) == Some(identity)
            })
        {
            target[index] = value;
            target_provenance[index] = provenance;
        } else {
            additions.push(value);
            addition_provenance.push(provenance);
        }
    }
    if prepend_new {
        additions.append(target);
        addition_provenance.append(target_provenance);
        *target = additions;
        *target_provenance = addition_provenance;
    } else {
        target.extend(additions);
        target_provenance.extend(addition_provenance);
    }
    merge_source_ids(target_sources, next_sources);
    Ok(())
}

fn merge_mihomo_value(
    current: &mut Value,
    current_provenance: &mut ProvenanceNode,
    next: Value,
    next_provenance: ProvenanceNode,
    key: Option<&str>,
) {
    match (current, current_provenance, next, next_provenance) {
        (
            Value::Object(current),
            ProvenanceNode::Object {
                fields: current_provenance,
                source_ids: current_sources,
            },
            Value::Object(next),
            ProvenanceNode::Object {
                fields: mut next_provenance,
                source_ids: next_sources,
            },
        ) => {
            for (child_key, value) in next {
                let provenance = next_provenance
                    .remove(&child_key)
                    .expect("provenance tree follows source document");
                if let Some(existing) = current.get_mut(&child_key) {
                    let existing_provenance = current_provenance
                        .get_mut(&child_key)
                        .expect("provenance tree follows source document");
                    if key == Some("sub-rules") {
                        merge_mihomo_sub_rule_value(
                            existing,
                            existing_provenance,
                            value,
                            provenance,
                        );
                    } else {
                        merge_mihomo_value(
                            existing,
                            existing_provenance,
                            value,
                            provenance,
                            Some(&child_key),
                        );
                    }
                } else {
                    current.insert(child_key.clone(), value);
                    current_provenance.insert(child_key, provenance);
                }
            }
            merge_source_ids(current_sources, next_sources);
        }
        (
            Value::Array(current),
            ProvenanceNode::Array {
                items: current_provenance,
                source_ids: current_sources,
            },
            Value::Array(next),
            ProvenanceNode::Array {
                items: next_provenance,
                source_ids: next_sources,
            },
        ) => match key {
            Some("proxies" | "proxy-groups" | "listeners") => {
                merge_mihomo_named_sequence(current, current_provenance, next, next_provenance);
                merge_source_ids(current_sources, next_sources);
            }
            Some("rules") => {
                current.extend(next);
                current_provenance.extend(next_provenance);
                merge_source_ids(current_sources, next_sources);
            }
            // Ordinary option lists are replaced conservatively. They do not all share
            // the ordered rule semantics.
            _ => {
                *current = next;
                *current_provenance = next_provenance;
                *current_sources = next_sources;
            }
        },
        (current, current_provenance, next, next_provenance) => {
            *current = next;
            *current_provenance = next_provenance;
        }
    }
}

fn merge_mihomo_sub_rule_value(
    current: &mut Value,
    current_provenance: &mut ProvenanceNode,
    next: Value,
    next_provenance: ProvenanceNode,
) {
    match (current, current_provenance, next, next_provenance) {
        (
            Value::Array(current),
            ProvenanceNode::Array {
                items: current_provenance,
                source_ids: current_sources,
            },
            Value::Array(next),
            ProvenanceNode::Array {
                items: next_provenance,
                source_ids: next_sources,
            },
        ) => {
            current.extend(next);
            current_provenance.extend(next_provenance);
            merge_source_ids(current_sources, next_sources);
        }
        (
            Value::Object(current),
            ProvenanceNode::Object {
                fields: current_provenance,
                source_ids: current_sources,
            },
            Value::Object(next),
            ProvenanceNode::Object {
                fields: mut next_provenance,
                source_ids: next_sources,
            },
        ) => {
            for (key, value) in next {
                let provenance = next_provenance
                    .remove(&key)
                    .expect("provenance tree follows source document");
                if let Some(existing) = current.get_mut(&key) {
                    merge_mihomo_sub_rule_value(
                        existing,
                        current_provenance
                            .get_mut(&key)
                            .expect("provenance tree follows source document"),
                        value,
                        provenance,
                    );
                } else {
                    current.insert(key.clone(), value);
                    current_provenance.insert(key, provenance);
                }
            }
            merge_source_ids(current_sources, next_sources);
        }
        (current, current_provenance, next, next_provenance) => {
            *current = next;
            *current_provenance = next_provenance;
        }
    }
}

fn merge_mihomo_named_sequence(
    current: &mut Vec<Value>,
    current_provenance: &mut Vec<ProvenanceNode>,
    next: Vec<Value>,
    next_provenance: Vec<ProvenanceNode>,
) {
    for (value, provenance) in next.into_iter().zip(next_provenance) {
        let name = value
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty());
        if let Some(name) = name
            && let Some(index) = current
                .iter()
                .position(|existing| existing.get("name").and_then(Value::as_str) == Some(name))
        {
            current[index] = value;
            current_provenance[index] = provenance;
        } else {
            current.push(value);
            current_provenance.push(provenance);
        }
    }
}

fn build_source_provenance(root: &ProvenanceNode) -> Vec<ProvenanceEntry> {
    let mut entries = Vec::new();
    collect_provenance_paths(root, "", &mut entries);
    entries
}

fn collect_provenance_paths(
    node: &ProvenanceNode,
    prefix: &str,
    output: &mut Vec<ProvenanceEntry>,
) {
    match node {
        ProvenanceNode::Leaf(source_ids) => output.push(ProvenanceEntry {
            semantic_path: if prefix.is_empty() {
                "/".to_owned()
            } else {
                prefix.to_owned()
            },
            source_ids: source_ids.clone(),
            guided: false,
            raw: false,
        }),
        ProvenanceNode::Object { fields, source_ids } => {
            if fields.is_empty() {
                output.push(ProvenanceEntry {
                    semantic_path: if prefix.is_empty() {
                        "/".to_owned()
                    } else {
                        prefix.to_owned()
                    },
                    source_ids: source_ids.clone(),
                    guided: false,
                    raw: false,
                });
            } else {
                for (key, child) in fields {
                    let path = format!("{prefix}/{}", escape_pointer(key));
                    collect_provenance_paths(child, &path, output);
                }
            }
        }
        ProvenanceNode::Array { items, source_ids } => {
            if items.is_empty() {
                output.push(ProvenanceEntry {
                    semantic_path: if prefix.is_empty() {
                        "/".to_owned()
                    } else {
                        prefix.to_owned()
                    },
                    source_ids: source_ids.clone(),
                    guided: false,
                    raw: false,
                });
            } else {
                for (index, child) in items.iter().enumerate() {
                    let path = format!("{prefix}/{index}");
                    collect_provenance_paths(child, &path, output);
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ProvenanceLayer {
    Guided,
    Raw,
}

fn apply_provenance_layer(
    before: &Value,
    after: &Value,
    previous: &[ProvenanceEntry],
    layer: ProvenanceLayer,
) -> Vec<ProvenanceEntry> {
    let mut result = Vec::new();
    reconcile_provenance(before, after, "", "", previous, layer, false, &mut result);
    result
}

#[allow(clippy::too_many_arguments)]
fn reconcile_provenance(
    before: &Value,
    after: &Value,
    before_path: &str,
    after_path: &str,
    previous: &[ProvenanceEntry],
    layer: ProvenanceLayer,
    ancestor_modified: bool,
    output: &mut Vec<ProvenanceEntry>,
) {
    if is_provenance_leaf(after) {
        let mut entry = previous
            .iter()
            .find(|entry| entry.semantic_path == display_pointer_path(before_path))
            .cloned()
            .unwrap_or_else(|| aggregate_provenance(previous, before_path));
        entry.semantic_path = display_pointer_path(after_path);
        if ancestor_modified || before != after {
            mark_provenance_layer(&mut entry, layer);
        }
        output.push(entry);
        return;
    }

    match (before, after) {
        (Value::Object(before), Value::Object(after)) => {
            for (key, after_child) in after {
                let after_child_path = format!("{after_path}/{}", escape_pointer(key));
                if let Some(before_child) = before.get(key) {
                    let before_child_path = format!("{before_path}/{}", escape_pointer(key));
                    reconcile_provenance(
                        before_child,
                        after_child,
                        &before_child_path,
                        &after_child_path,
                        previous,
                        layer,
                        ancestor_modified,
                        output,
                    );
                } else {
                    collect_modified_provenance(
                        after_child,
                        &after_child_path,
                        ProvenanceEntry {
                            semantic_path: String::new(),
                            source_ids: Vec::new(),
                            guided: false,
                            raw: false,
                        },
                        layer,
                        output,
                    );
                }
            }
        }
        (Value::Array(before), Value::Array(after)) => {
            if let (Some(before_identities), Some(after_identities)) =
                (sequence_identities(before), sequence_identities(after))
            {
                let before_indices = before_identities
                    .into_iter()
                    .enumerate()
                    .map(|(index, identity)| (identity, index))
                    .collect::<HashMap<_, _>>();
                for (after_index, (after_child, identity)) in
                    after.iter().zip(after_identities).enumerate()
                {
                    let after_child_path = format!("{after_path}/{after_index}");
                    if let Some(before_index) = before_indices.get(&identity).copied() {
                        let before_child_path = format!("{before_path}/{before_index}");
                        reconcile_provenance(
                            &before[before_index],
                            after_child,
                            &before_child_path,
                            &after_child_path,
                            previous,
                            layer,
                            ancestor_modified || before_index != after_index,
                            output,
                        );
                    } else {
                        collect_modified_provenance(
                            after_child,
                            &after_child_path,
                            ProvenanceEntry {
                                semantic_path: String::new(),
                                source_ids: Vec::new(),
                                guided: false,
                                raw: false,
                            },
                            layer,
                            output,
                        );
                    }
                }
            } else if ancestor_modified || before != after {
                let seed = aggregate_provenance(previous, before_path);
                for (index, child) in after.iter().enumerate() {
                    let child_path = format!("{after_path}/{index}");
                    collect_modified_provenance(child, &child_path, seed.clone(), layer, output);
                }
            } else {
                for (index, after_child) in after.iter().enumerate() {
                    let after_child_path = format!("{after_path}/{index}");
                    if let Some(before_child) = before.get(index) {
                        let before_child_path = format!("{before_path}/{index}");
                        reconcile_provenance(
                            before_child,
                            after_child,
                            &before_child_path,
                            &after_child_path,
                            previous,
                            layer,
                            ancestor_modified,
                            output,
                        );
                    } else {
                        collect_modified_provenance(
                            after_child,
                            &after_child_path,
                            ProvenanceEntry {
                                semantic_path: String::new(),
                                source_ids: Vec::new(),
                                guided: false,
                                raw: false,
                            },
                            layer,
                            output,
                        );
                    }
                }
            }
        }
        _ => {
            collect_modified_provenance(
                after,
                after_path,
                aggregate_provenance(previous, before_path),
                layer,
                output,
            );
        }
    }
}

fn collect_modified_provenance(
    value: &Value,
    path: &str,
    seed: ProvenanceEntry,
    layer: ProvenanceLayer,
    output: &mut Vec<ProvenanceEntry>,
) {
    match value {
        Value::Object(fields) if !fields.is_empty() => {
            for (key, child) in fields {
                let child_path = format!("{path}/{}", escape_pointer(key));
                collect_modified_provenance(child, &child_path, seed.clone(), layer, output);
            }
        }
        Value::Array(items) if !items.is_empty() => {
            for (index, child) in items.iter().enumerate() {
                let child_path = format!("{path}/{index}");
                collect_modified_provenance(child, &child_path, seed.clone(), layer, output);
            }
        }
        _ => {
            let mut entry = seed;
            entry.semantic_path = display_pointer_path(path);
            mark_provenance_layer(&mut entry, layer);
            output.push(entry);
        }
    }
}

fn aggregate_provenance(previous: &[ProvenanceEntry], path: &str) -> ProvenanceEntry {
    let mut result = ProvenanceEntry {
        semantic_path: display_pointer_path(path),
        source_ids: Vec::new(),
        guided: false,
        raw: false,
    };
    let displayed = display_pointer_path(path);
    for entry in previous.iter().filter(|entry| {
        path.is_empty()
            || entry.semantic_path == displayed
            || entry
                .semantic_path
                .strip_prefix(&displayed)
                .is_some_and(|suffix| suffix.starts_with('/'))
    }) {
        merge_source_ids(&mut result.source_ids, entry.source_ids.clone());
        result.guided |= entry.guided;
        result.raw |= entry.raw;
    }
    result
}

fn mark_provenance_layer(entry: &mut ProvenanceEntry, layer: ProvenanceLayer) {
    match layer {
        ProvenanceLayer::Guided => entry.guided = true,
        ProvenanceLayer::Raw => entry.raw = true,
    }
}

fn is_provenance_leaf(value: &Value) -> bool {
    match value {
        Value::Object(fields) => fields.is_empty(),
        Value::Array(items) => items.is_empty(),
        _ => true,
    }
}

fn display_pointer_path(path: &str) -> String {
    if path.is_empty() {
        "/".to_owned()
    } else {
        path.to_owned()
    }
}

fn escape_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SemanticPathSegment {
    Key { key: String },
    Identity { field: String, value: String },
}

pub type SemanticPath = Vec<SemanticPathSegment>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum IntentOperation {
    Set {
        path: SemanticPath,
        value: Value,
    },
    Delete {
        path: SemanticPath,
    },
    ReorderIdentities {
        path: SemanticPath,
        order: Vec<(String, String)>,
    },
    ReplaceSequence {
        path: SemanticPath,
        expected: Vec<Value>,
        value: Vec<Value>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RawDecisionOrigin {
    User,
    Migrated,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RawDecisionStatus {
    Active,
    Superseded,
    Dormant,
    Resolved,
}

impl RawDecisionStatus {
    fn participates_in_candidate(self) -> bool {
        matches!(self, Self::Active | Self::Resolved)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawDecisionBasis {
    pub upstream_generation: u64,
    pub upstream_content_hash: String,
    pub upstream_path_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawDecision {
    pub decision_id: String,
    pub operation: IntentOperation,
    pub basis: RawDecisionBasis,
    pub status: RawDecisionStatus,
    pub origin: RawDecisionOrigin,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum RawDecisionResolution {
    AcceptUpstream,
    KeepRaw,
    ManualEdit { value: Value },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawManualIntent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub based_on_revision: Option<ConfigurationRevision>,
    /// v4 compatibility input.  Schema v5 converts these operations into
    /// basis-bound decisions before rebuilding a candidate and never writes
    /// new entries here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<IntentOperation>,
    #[serde(default)]
    pub decisions: Vec<RawDecision>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawConflict {
    pub conflict_id: String,
    pub segments: SemanticPath,
    pub semantic_path: String,
    pub display_path: String,
    pub conflict_type: String,
    pub severity: ConflictSeverity,
    pub original_base: Value,
    pub updated_base: Value,
    pub user_value: Value,
    #[serde(default)]
    pub suggested_actions: Vec<String>,
    #[serde(default)]
    pub can_combine: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawRebaseResult {
    pub document: Value,
    #[serde(default)]
    pub conflicts: Vec<RawConflict>,
    pub deterministic_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum RawConflictResolution {
    KeepMine,
    UseUpdated,
    Combine,
    ManualEdit { value: Value },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawDraftSession {
    pub session_id: String,
    pub draft_revision: u64,
    pub based_on_generation: u64,
    pub base_content: String,
    pub user_content: String,
    pub working_content: String,
    #[serde(default)]
    pub conflicts: Vec<RawConflict>,
    #[serde(default)]
    pub resolutions: BTreeMap<String, RawConflictResolution>,
    #[serde(default)]
    pub unresolved_conflict_ids: Vec<String>,
    pub updated_unix_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuidedIntent {
    #[serde(default)]
    pub values: BTreeMap<String, Value>,
}

/// Settings owned by the Details surface (currently the managed dashboard
/// integrations).  Keeping this in a separate store is important: these
/// values are not Common Guided settings and must never create an Intent-page
/// conflict or projection entry.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedIntegrationIntent {
    #[serde(default)]
    pub values: BTreeMap<String, Value>,
}

impl ManagedIntegrationIntent {
    pub fn set(&mut self, setting_id: impl Into<String>, value: Value) {
        self.values.insert(setting_id.into(), value);
    }

    pub fn reset(&mut self, setting_id: &str) {
        self.values.remove(setting_id);
    }
}

/// The UI surface that owns an issue.  Consumers must filter by this value;
/// Configuration is the only surface that intentionally shows all issues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ConfigurationSurface {
    Intent,
    Details,
    Sources,
    Compatibility,
    #[default]
    Configuration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationIssueScope {
    pub surface: ConfigurationSurface,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
}

impl ConfigurationIssueScope {
    pub fn intent(owner_id: impl Into<String>) -> Self {
        Self {
            surface: ConfigurationSurface::Intent,
            owner_id: Some(owner_id.into()),
        }
    }

    pub fn details(owner_id: impl Into<String>) -> Self {
        Self {
            surface: ConfigurationSurface::Details,
            owner_id: Some(owner_id.into()),
        }
    }

    pub fn sources(owner_id: impl Into<String>) -> Self {
        Self {
            surface: ConfigurationSurface::Sources,
            owner_id: Some(owner_id.into()),
        }
    }

    pub fn compatibility() -> Self {
        Self {
            surface: ConfigurationSurface::Compatibility,
            owner_id: None,
        }
    }

    pub fn configuration() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ManagedIntegrationStatus {
    Inactive,
    Explicit,
    Overridden,
    RawOnly,
    NeedsAttention,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedIntegrationProjection {
    pub integration_id: String,
    pub status: ManagedIntegrationStatus,
    pub effective_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_value: Option<Value>,
    #[serde(default)]
    pub raw_paths: Vec<String>,
    #[serde(default)]
    pub issue_ids: Vec<String>,
}

impl GuidedIntent {
    pub fn set(&mut self, setting_id: impl Into<String>, value: Value) {
        self.values.insert(setting_id.into(), value);
    }

    pub fn reset(&mut self, setting_id: &str) {
        self.values.remove(setting_id);
    }
}

pub trait IntentValueStore {
    fn values(&self) -> &BTreeMap<String, Value>;
    fn values_mut(&mut self) -> &mut BTreeMap<String, Value>;
}

impl IntentValueStore for GuidedIntent {
    fn values(&self) -> &BTreeMap<String, Value> {
        &self.values
    }
    fn values_mut(&mut self) -> &mut BTreeMap<String, Value> {
        &mut self.values
    }
}

impl IntentValueStore for ManagedIntegrationIntent {
    fn values(&self) -> &BTreeMap<String, Value> {
        &self.values
    }
    fn values_mut(&mut self) -> &mut BTreeMap<String, Value> {
        &mut self.values
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GuidedControl {
    Toggle,
    Select,
    Number,
    Text,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuidedSettingDescriptor {
    pub id: String,
    pub category: String,
    pub label: String,
    pub description: String,
    pub control: GuidedControl,
    #[serde(default)]
    pub allowed_values: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_when: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GuidedProjectionStatus {
    Inherited,
    Explicit,
    Custom,
    Overridden,
    RawDecision,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuidedProjection {
    pub setting_id: String,
    pub status: GuidedProjectionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_value: Option<Value>,
}

pub fn guided_setting_descriptors(kind: ProgramKind) -> Vec<GuidedSettingDescriptor> {
    let mut descriptors = vec![GuidedSettingDescriptor {
        id: "logging.level".into(),
        category: "logging".into(),
        label: "Log level".into(),
        description: "Control the Core log verbosity, or follow the source configuration.".into(),
        control: GuidedControl::Select,
        allowed_values: match kind {
            ProgramKind::Xray => vec!["debug", "info", "warning", "error", "none"],
            ProgramKind::SingBox | ProgramKind::Mihomo => {
                vec!["trace", "debug", "info", "warn", "error", "fatal", "panic"]
            }
            ProgramKind::Generic => Vec::new(),
        }
        .into_iter()
        .map(str::to_owned)
        .collect(),
        enabled_when: None,
    }];
    match kind {
        ProgramKind::Mihomo => descriptors.extend([
            descriptor(
                "network.ipv6",
                "network",
                "IPv6",
                GuidedControl::Toggle,
                &[],
            ),
            descriptor("tun.enabled", "tun", "TUN", GuidedControl::Toggle, &[]),
            descriptor(
                "tun.strictRoute",
                "tun",
                "Strict routing",
                GuidedControl::Toggle,
                &[],
            ),
            descriptor("dns.enabled", "dns", "DNS", GuidedControl::Toggle, &[]),
            descriptor(
                "dns.mode",
                "dns",
                "DNS mode",
                GuidedControl::Select,
                &["normal", "fake-ip", "redir-host"],
            ),
            descriptor(
                "routing.mode",
                "routing",
                "Routing mode",
                GuidedControl::Select,
                &["rule", "global", "direct"],
            ),
        ]),
        ProgramKind::SingBox => descriptors.extend([
            descriptor(
                "dns.strategy",
                "dns",
                "IP strategy",
                GuidedControl::Select,
                &["prefer_ipv4", "prefer_ipv6", "ipv4_only", "ipv6_only"],
            ),
            descriptor(
                "routing.autoDetectInterface",
                "routing",
                "Auto-detect interface",
                GuidedControl::Toggle,
                &[],
            ),
        ]),
        ProgramKind::Xray => descriptors.push(descriptor(
            "routing.domainStrategy",
            "routing",
            "Domain strategy",
            GuidedControl::Select,
            &["AsIs", "IPIfNonMatch", "IPOnDemand"],
        )),
        ProgramKind::Generic => descriptors.clear(),
    }
    descriptors
}

pub fn validate_guided_value(kind: ProgramKind, setting_id: &str, value: &Value) -> Result<()> {
    let descriptor = guided_setting_descriptors(kind)
        .into_iter()
        .find(|descriptor| descriptor.id == setting_id)
        .ok_or_else(|| {
            CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                format!("Unsupported Guided setting {setting_id}"),
            )
        })?;
    match descriptor.control {
        GuidedControl::Toggle => {
            if !value.is_boolean() {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    format!("Guided setting {setting_id} expects a boolean"),
                ));
            }
        }
        GuidedControl::Select => {
            let value = value.as_str().ok_or_else(|| {
                CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    format!("Guided setting {setting_id} expects a string option"),
                )
            })?;
            if !descriptor
                .allowed_values
                .iter()
                .any(|allowed| allowed == value)
            {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    format!("Guided setting {setting_id} has an unsupported value"),
                ));
            }
        }
        GuidedControl::Number => {
            if !value.is_number() {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    format!("Guided setting {setting_id} expects a number"),
                ));
            }
        }
        GuidedControl::Text => {
            if !value.is_string() {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    format!("Guided setting {setting_id} expects text"),
                ));
            }
        }
    }
    Ok(())
}

fn descriptor(
    id: &str,
    category: &str,
    label: &str,
    control: GuidedControl,
    values: &[&str],
) -> GuidedSettingDescriptor {
    GuidedSettingDescriptor {
        id: id.into(),
        category: category.into(),
        label: label.into(),
        description: format!("Configure {label}, or follow the source configuration."),
        control,
        allowed_values: values.iter().map(|value| (*value).to_owned()).collect(),
        enabled_when: (id == "tun.strictRoute").then(|| "tun.enabled".into()),
    }
}

pub fn apply_guided_intent(
    kind: ProgramKind,
    base: &Value,
    intent: &GuidedIntent,
) -> Result<Value> {
    let mut effective = base.clone();
    for (setting, value) in &intent.values {
        if setting.starts_with(DASHBOARD_INTENT_PREFIX) {
            continue;
        }
        validate_guided_value(kind, setting, value)?;
        let path: &[&str] = match (kind, setting.as_str()) {
            (ProgramKind::SingBox, "logging.level") => &["log", "level"],
            (ProgramKind::SingBox, "dns.strategy") => &["dns", "strategy"],
            (ProgramKind::SingBox, "routing.autoDetectInterface") => {
                &["route", "auto_detect_interface"]
            }
            (ProgramKind::Xray, "logging.level") => &["log", "loglevel"],
            (ProgramKind::Xray, "routing.domainStrategy") => &["routing", "domainStrategy"],
            (ProgramKind::Mihomo, "logging.level") => &["log-level"],
            (ProgramKind::Mihomo, "network.ipv6") => &["ipv6"],
            (ProgramKind::Mihomo, "tun.enabled") => &["tun", "enable"],
            (ProgramKind::Mihomo, "tun.strictRoute") => &["tun", "strict-route"],
            (ProgramKind::Mihomo, "dns.enabled") => &["dns", "enable"],
            (ProgramKind::Mihomo, "dns.mode") => &["dns", "enhanced-mode"],
            (ProgramKind::Mihomo, "routing.mode") => &["mode"],
            _ => {
                return Err(CamelliaNexusError::invalid_spec(format!(
                    "Unsupported Guided setting {setting} for {kind:?}",
                )));
            }
        };
        set_object_path(&mut effective, path, value.clone())?;
    }
    // Keep accepting legacy callers that still pass dashboard values, while
    // the authoritative ConfigurationState stores them in managed_intent.
    if intent
        .values
        .keys()
        .any(|key| key.starts_with(DASHBOARD_INTENT_PREFIX))
    {
        apply_dashboard_intent(kind, &mut effective, intent)?;
    }
    Ok(effective)
}

/// Synchronize the Dashboard form into its dedicated Managed Integration
/// intent store.  The generic store bound remains only for deterministic v3
/// migration tests; authoritative state always passes ManagedIntegrationIntent.
pub fn sync_managed_dashboard_intent<T: IntentValueStore>(
    intent: &mut T,
    managed: &ManagedConfigSpec,
) {
    intent
        .values_mut()
        .retain(|setting, _| !setting.starts_with(DASHBOARD_INTENT_PREFIX));
    if let Some(dashboard) = managed.sing_box_dashboard.as_ref() {
        intent.values_mut().insert(
            "dashboard.singBoxApi.listenPort".into(),
            Value::from(dashboard.listen_port),
        );
        intent.values_mut().insert(
            "dashboard.singBoxApi.updateInterval".into(),
            Value::String(dashboard.update_interval.clone()),
        );
    }
    if let Some(dashboard) = managed.sing_box_clash_dashboard.as_ref() {
        intent.values_mut().insert(
            "dashboard.singBoxClash.listenPort".into(),
            Value::from(dashboard.listen_port),
        );
        if let Some(url) = &dashboard.download_url {
            intent.values_mut().insert(
                "dashboard.singBoxClash.downloadUrl".into(),
                Value::String(url.clone()),
            );
        }
    }
    if let Some(dashboard) = managed.xray_dashboard.as_ref() {
        sync_xray_dashboard_intent(intent, dashboard);
    }
    if let Some(dashboard) = managed.mihomo_dashboard.as_ref() {
        intent.values_mut().insert(
            "dashboard.mihomo.listenPort".into(),
            Value::from(dashboard.listen_port),
        );
        if let Some(url) = &dashboard.download_url {
            intent.values_mut().insert(
                "dashboard.mihomo.downloadUrl".into(),
                Value::String(url.clone()),
            );
        }
    }
}

fn sync_xray_dashboard_intent<T: IntentValueStore>(intent: &mut T, dashboard: &XrayDashboardSpec) {
    intent.values_mut().insert(
        "dashboard.xray.apiPort".into(),
        Value::from(dashboard.api_port),
    );
    intent.values_mut().insert(
        "dashboard.xray.metricsPort".into(),
        Value::from(dashboard.metrics_port),
    );
}

fn apply_dashboard_intent<T: IntentValueStore>(
    kind: ProgramKind,
    root: &mut Value,
    intent: &T,
) -> Result<()> {
    match kind {
        ProgramKind::SingBox => {
            apply_sing_box_dashboard_intent(root, intent)?;
            apply_sing_box_clash_dashboard_intent(root, intent)?;
        }
        ProgramKind::Xray => apply_xray_dashboard_intent(root, intent)?,
        ProgramKind::Mihomo => apply_mihomo_dashboard_intent(root, intent)?,
        ProgramKind::Generic => {}
    }
    Ok(())
}

fn intent_port<T: IntentValueStore>(intent: &T, id: &str) -> Result<Option<u16>> {
    let Some(value) = intent.values().get(id) else {
        return Ok(None);
    };
    let port = value.as_u64().and_then(|port| u16::try_from(port).ok());
    if port.is_none_or(|port| !(1024..=u16::MAX).contains(&port)) {
        return Err(CamelliaNexusError::invalid_spec(format!(
            "Dashboard setting {id} must be a port between 1024 and 65535"
        )));
    }
    Ok(port)
}

fn intent_text<T: IntentValueStore>(intent: &T, id: &str) -> Result<Option<String>> {
    let Some(value) = intent.values().get(id) else {
        return Ok(None);
    };
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| {
            CamelliaNexusError::invalid_spec(format!("Dashboard setting {id} must be text"))
        })
        .map(Some)
}

fn apply_sing_box_dashboard_intent<T: IntentValueStore>(
    root: &mut Value,
    intent: &T,
) -> Result<()> {
    let Some(port) = intent_port(intent, "dashboard.singBoxApi.listenPort")? else {
        return Ok(());
    };
    let update_interval =
        intent_text(intent, "dashboard.singBoxApi.updateInterval")?.unwrap_or_else(|| "1d".into());
    let update_interval = normalize_dashboard_interval(&update_interval).unwrap_or(update_interval);
    let root = root.as_object_mut().ok_or_else(|| {
        CamelliaNexusError::invalid_spec("sing-box configuration root must be an object")
    })?;
    let services = root
        .entry("services")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| CamelliaNexusError::invalid_spec("sing-box services must be an array"))?;
    services.retain(|service| {
        service.get("tag").and_then(Value::as_str) != Some(MANAGED_SING_BOX_API_TAG)
    });
    services.push(serde_json::json!({
        "type": "api",
        "tag": MANAGED_SING_BOX_API_TAG,
        "listen": "127.0.0.1",
        "listen_port": port,
        "dashboard": { "enabled": true, "update_interval": update_interval },
    }));
    Ok(())
}

fn apply_sing_box_clash_dashboard_intent<T: IntentValueStore>(
    root: &mut Value,
    intent: &T,
) -> Result<()> {
    let Some(port) = intent_port(intent, "dashboard.singBoxClash.listenPort")? else {
        return Ok(());
    };
    let download_url = intent_text(intent, "dashboard.singBoxClash.downloadUrl")?;
    let root = root.as_object_mut().ok_or_else(|| {
        CamelliaNexusError::invalid_spec("sing-box configuration root must be an object")
    })?;
    let experimental = root
        .entry("experimental")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| {
            CamelliaNexusError::invalid_spec("sing-box experimental must be an object")
        })?;
    let clash = experimental
        .entry("clash_api")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| CamelliaNexusError::invalid_spec("sing-box clash_api must be an object"))?;
    clash.insert(
        "external_controller".into(),
        Value::String(format!("127.0.0.1:{port}")),
    );
    clash.insert(
        "external_ui".into(),
        Value::String(MANAGED_SING_BOX_CLASH_UI.into()),
    );
    match download_url {
        Some(url) => {
            clash.insert("external_ui_download_url".into(), Value::String(url));
        }
        None => {
            clash.remove("external_ui_download_url");
        }
    }
    Ok(())
}

fn apply_xray_dashboard_intent<T: IntentValueStore>(root: &mut Value, intent: &T) -> Result<()> {
    let Some(api_port) = intent_port(intent, "dashboard.xray.apiPort")? else {
        return Ok(());
    };
    let metrics_port = intent_port(intent, "dashboard.xray.metricsPort")?.ok_or_else(|| {
        CamelliaNexusError::invalid_spec("Xray Dashboard metrics port is required")
    })?;
    let root = root.as_object_mut().ok_or_else(|| {
        CamelliaNexusError::invalid_spec("Xray configuration root must be an object")
    })?;
    let api = ensure_object(root, "api", "Xray api must be an object")?;
    api.insert("tag".into(), Value::String(MANAGED_XRAY_API_TAG.into()));
    api.insert(
        "listen".into(),
        Value::String(format!("127.0.0.1:{api_port}")),
    );
    let services = api
        .entry("services")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| CamelliaNexusError::invalid_spec("Xray api.services must be an array"))?;
    for service in [
        "HandlerService",
        "LoggerService",
        "StatsService",
        "RoutingService",
        "ReflectionService",
    ] {
        if !services
            .iter()
            .any(|existing| existing.as_str() == Some(service))
        {
            services.push(Value::String(service.into()));
        }
    }
    let metrics = ensure_object(root, "metrics", "Xray metrics must be an object")?;
    metrics.insert("tag".into(), Value::String(MANAGED_XRAY_METRICS_TAG.into()));
    metrics.insert(
        "listen".into(),
        Value::String(format!("127.0.0.1:{metrics_port}")),
    );
    match root.get("stats") {
        Some(Value::Object(_)) => {}
        Some(_) => {
            return Err(CamelliaNexusError::invalid_spec(
                "Xray stats must be an object",
            ));
        }
        None => {
            root.insert("stats".into(), Value::Object(Map::new()));
        }
    }
    let policy = ensure_object(root, "policy", "Xray policy must be an object")?;
    let system = policy
        .entry("system")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| CamelliaNexusError::invalid_spec("Xray policy.system must be an object"))?;
    for key in [
        "statsInboundUplink",
        "statsInboundDownlink",
        "statsOutboundUplink",
        "statsOutboundDownlink",
    ] {
        system.insert(key.into(), Value::Bool(true));
    }
    Ok(())
}

fn apply_mihomo_dashboard_intent<T: IntentValueStore>(root: &mut Value, intent: &T) -> Result<()> {
    let Some(port) = intent_port(intent, "dashboard.mihomo.listenPort")? else {
        return Ok(());
    };
    set_object_path(
        root,
        &["external-controller"],
        Value::String(format!("127.0.0.1:{port}")),
    )?;
    set_object_path(
        root,
        &["external-ui"],
        Value::String(MANAGED_MIHOMO_UI_DIRECTORY.into()),
    )?;
    match intent_text(intent, "dashboard.mihomo.downloadUrl")? {
        Some(url) => set_object_path(root, &["external-ui-url"], Value::String(url))?,
        None => {
            if let Some(object) = root.as_object_mut() {
                object.remove("external-ui-url");
            }
        }
    }
    Ok(())
}

fn ensure_object<'a>(
    root: &'a mut Map<String, Value>,
    key: &str,
    message: &str,
) -> Result<&'a mut Map<String, Value>> {
    root.entry(key)
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| CamelliaNexusError::invalid_spec(message))
}

pub fn project_guided_settings(
    kind: ProgramKind,
    effective: &Value,
    guided: &GuidedIntent,
    raw: &RawManualIntent,
) -> Vec<GuidedProjection> {
    // Superseded decisions remain visible to the owning surface so the user
    // can reopen the Final configuration workspace.  Only dormant history is
    // hidden from the projection; candidate application still filters through
    // `candidate_raw_operations`.
    let raw_paths = visible_raw_operations(raw)
        .map(intent_operation_path)
        .collect::<Vec<_>>();
    guided_setting_descriptors(kind)
        .into_iter()
        .map(|descriptor| {
            let path = guided_path(kind, &descriptor.id);
            let value = path
                .and_then(|path| get_object_path(effective, path))
                .cloned();
            let overridden = path.is_some_and(|path| {
                let semantic = key_path(path);
                raw_paths
                    .iter()
                    .any(|raw_path| path_overlaps(&semantic, raw_path))
            });
            GuidedProjection {
                setting_id: descriptor.id.clone(),
                status: if overridden {
                    GuidedProjectionStatus::RawDecision
                } else if guided.values.contains_key(&descriptor.id) {
                    GuidedProjectionStatus::Explicit
                } else if value
                    .as_ref()
                    .is_some_and(|value| !guided_value_is_representable(&descriptor, value))
                {
                    GuidedProjectionStatus::Custom
                } else {
                    GuidedProjectionStatus::Inherited
                },
                value,
                intent_value: guided.values.get(&descriptor.id).cloned(),
            }
        })
        .collect()
}

fn candidate_raw_operations(raw: &RawManualIntent) -> impl Iterator<Item = &IntentOperation> {
    raw.operations.iter().chain(
        raw.decisions
            .iter()
            .filter(|decision| decision.status.participates_in_candidate())
            .map(|decision| &decision.operation),
    )
}

fn visible_raw_operations(raw: &RawManualIntent) -> impl Iterator<Item = &IntentOperation> {
    raw.operations.iter().chain(
        raw.decisions
            .iter()
            .filter(|decision| decision.status != RawDecisionStatus::Dormant)
            .map(|decision| &decision.operation),
    )
}

fn guided_value_is_representable(descriptor: &GuidedSettingDescriptor, value: &Value) -> bool {
    match descriptor.control {
        GuidedControl::Toggle => value.is_boolean(),
        GuidedControl::Select => value.as_str().is_some_and(|value| {
            descriptor
                .allowed_values
                .iter()
                .any(|allowed| allowed == value)
        }),
        GuidedControl::Number => value.is_number(),
        GuidedControl::Text => value.is_string(),
    }
}

fn guided_path(kind: ProgramKind, setting: &str) -> Option<&'static [&'static str]> {
    match (kind, setting) {
        (ProgramKind::SingBox, "logging.level") => Some(&["log", "level"]),
        (ProgramKind::SingBox, "dns.strategy") => Some(&["dns", "strategy"]),
        (ProgramKind::SingBox, "routing.autoDetectInterface") => {
            Some(&["route", "auto_detect_interface"])
        }
        (ProgramKind::Xray, "logging.level") => Some(&["log", "loglevel"]),
        (ProgramKind::Xray, "routing.domainStrategy") => Some(&["routing", "domainStrategy"]),
        (ProgramKind::Mihomo, "logging.level") => Some(&["log-level"]),
        (ProgramKind::Mihomo, "network.ipv6") => Some(&["ipv6"]),
        (ProgramKind::Mihomo, "tun.enabled") => Some(&["tun", "enable"]),
        (ProgramKind::Mihomo, "tun.strictRoute") => Some(&["tun", "strict-route"]),
        (ProgramKind::Mihomo, "dns.enabled") => Some(&["dns", "enable"]),
        (ProgramKind::Mihomo, "dns.mode") => Some(&["dns", "enhanced-mode"]),
        (ProgramKind::Mihomo, "routing.mode") => Some(&["mode"]),
        _ => None,
    }
}

fn set_object_path(root: &mut Value, path: &[&str], value: Value) -> Result<()> {
    let mut current = root;
    for key in &path[..path.len().saturating_sub(1)] {
        let values = current.as_object_mut().ok_or_else(|| {
            CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Guided setting crosses a non-object value",
            )
        })?;
        current = values
            .entry((*key).to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    let Some(last) = path.last() else {
        return Err(CamelliaNexusError::internal(
            "Guided setting has no semantic path",
        ));
    };
    current
        .as_object_mut()
        .ok_or_else(|| {
            CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Guided setting parent is not an object",
            )
        })?
        .insert((*last).to_owned(), value);
    Ok(())
}

fn get_object_path<'a>(root: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(root, |current, key| current.get(key))
}

fn key_path(path: &[&str]) -> SemanticPath {
    path.iter()
        .map(|key| SemanticPathSegment::Key {
            key: (*key).to_owned(),
        })
        .collect()
}

pub fn diff_raw_intent(base: &Value, edited: &Value) -> RawManualIntent {
    let mut operations = Vec::new();
    diff_value(base, edited, &mut Vec::new(), &mut operations);
    RawManualIntent {
        based_on_revision: None,
        operations,
        decisions: Vec::new(),
    }
}

pub fn diff_raw_decisions(
    upstream: &Value,
    edited: &Value,
    upstream_generation: u64,
) -> RawManualIntent {
    let legacy = diff_raw_intent(upstream, edited);
    RawManualIntent {
        based_on_revision: None,
        operations: Vec::new(),
        decisions: bind_raw_operations(
            legacy.operations,
            upstream,
            upstream_generation,
            RawDecisionOrigin::User,
        ),
    }
}

fn semantic_path_values_equal(
    left_root: &Value,
    right_root: &Value,
    path: &[SemanticPathSegment],
) -> bool {
    let left = raw_conflict_path_value(left_root, path).ok().flatten();
    let right = raw_conflict_path_value(right_root, path).ok().flatten();
    match (left, right) {
        (Some(left), Some(right)) => values_semantically_equal(left, right, path),
        (None, None) => true,
        _ => false,
    }
}

fn bind_raw_operations(
    operations: Vec<IntentOperation>,
    upstream: &Value,
    upstream_generation: u64,
    origin: RawDecisionOrigin,
) -> Vec<RawDecision> {
    let upstream_content_hash = semantic_document_hash(upstream);
    operations
        .into_iter()
        .enumerate()
        .map(|(index, operation)| {
            let path = intent_operation_path(&operation);
            let upstream_path_hash = semantic_path_value_hash(upstream, &path);
            let decision_id = raw_decision_id(&operation, &upstream_content_hash, index);
            RawDecision {
                decision_id,
                operation,
                basis: RawDecisionBasis {
                    upstream_generation,
                    upstream_content_hash: upstream_content_hash.clone(),
                    upstream_path_hash,
                },
                status: RawDecisionStatus::Active,
                origin,
            }
        })
        .collect()
}

fn semantic_document_hash(value: &Value) -> String {
    hash_bytes(
        serde_json::to_vec(value)
            .expect("semantic JSON values are serializable")
            .as_slice(),
    )
}

fn semantic_path_value_hash(root: &Value, path: &[SemanticPathSegment]) -> String {
    let value = raw_conflict_path_value(root, path).ok().flatten().cloned();
    hash_bytes(
        serde_json::to_vec(&value)
            .expect("semantic path values are serializable")
            .as_slice(),
    )
}

fn raw_decision_id(
    operation: &IntentOperation,
    upstream_content_hash: &str,
    index: usize,
) -> String {
    let encoded = serde_json::to_string(&(operation, upstream_content_hash, index))
        .expect("Raw decisions are serializable");
    format!("raw-{}", hash_bytes(encoded.as_bytes()))
}

fn reconcile_raw_decisions(
    intent: &mut RawManualIntent,
    upstream: &Value,
    upstream_generation: u64,
) {
    if !intent.operations.is_empty() {
        let legacy = std::mem::take(&mut intent.operations);
        intent.decisions.extend(bind_raw_operations(
            legacy,
            upstream,
            upstream_generation,
            RawDecisionOrigin::Migrated,
        ));
    }
    for decision in &mut intent.decisions {
        if !decision.status.participates_in_candidate() {
            continue;
        }
        let path = intent_operation_path(&decision.operation);
        let current_path_hash = semantic_path_value_hash(upstream, &path);
        if current_path_hash != decision.basis.upstream_path_hash {
            decision.status = RawDecisionStatus::Superseded;
        }
    }
}

fn diff_value(
    base: &Value,
    edited: &Value,
    path: &mut SemanticPath,
    operations: &mut Vec<IntentOperation>,
) {
    if values_semantically_equal(base, edited, path) {
        return;
    }
    match (base, edited) {
        (Value::Object(base), Value::Object(edited)) => {
            let keys = base
                .keys()
                .chain(edited.keys())
                .cloned()
                .collect::<BTreeSet<_>>();
            for key in keys {
                path.push(SemanticPathSegment::Key { key: key.clone() });
                match (base.get(&key), edited.get(&key)) {
                    (Some(base), Some(edited)) => diff_value(base, edited, path, operations),
                    (None, Some(value)) => operations.push(IntentOperation::Set {
                        path: path.clone(),
                        value: value.clone(),
                    }),
                    (Some(_), None) => {
                        operations.push(IntentOperation::Delete { path: path.clone() })
                    }
                    (None, None) => {}
                }
                path.pop();
            }
        }
        (Value::Array(base), Value::Array(edited)) => {
            if let (Some(base_identities), Some(edited_identities)) =
                (sequence_identities(base), sequence_identities(edited))
            {
                let base_map = base_identities
                    .iter()
                    .cloned()
                    .zip(base.iter())
                    .collect::<HashMap<_, _>>();
                let edited_map = edited_identities
                    .iter()
                    .cloned()
                    .zip(edited.iter())
                    .collect::<HashMap<_, _>>();
                for identity in &base_identities {
                    let segment = SemanticPathSegment::Identity {
                        field: identity.0.clone(),
                        value: identity.1.clone(),
                    };
                    path.push(segment);
                    if let Some(edited) = edited_map.get(identity) {
                        diff_value(base_map[identity], edited, path, operations);
                    } else {
                        operations.push(IntentOperation::Delete { path: path.clone() });
                    }
                    path.pop();
                }
                for identity in &edited_identities {
                    if !base_map.contains_key(identity) {
                        path.push(SemanticPathSegment::Identity {
                            field: identity.0.clone(),
                            value: identity.1.clone(),
                        });
                        operations.push(IntentOperation::Set {
                            path: path.clone(),
                            value: (*edited_map[identity]).clone(),
                        });
                        path.pop();
                    }
                }
                if base_identities != edited_identities {
                    operations.push(IntentOperation::ReorderIdentities {
                        path: path.clone(),
                        order: edited_identities,
                    });
                }
            } else {
                operations.push(IntentOperation::ReplaceSequence {
                    path: path.clone(),
                    expected: base.clone(),
                    value: edited.clone(),
                });
            }
        }
        (_, edited) => operations.push(IntentOperation::Set {
            path: path.clone(),
            value: edited.clone(),
        }),
    }
}

fn values_semantically_equal(base: &Value, edited: &Value, path: &[SemanticPathSegment]) -> bool {
    if base == edited {
        return true;
    }
    if !is_sing_box_dashboard_interval_path(path) {
        return false;
    }
    let (Some(base), Some(edited)) = (base.as_str(), edited.as_str()) else {
        return false;
    };
    parse_dashboard_interval_nanos(base).is_some()
        && parse_dashboard_interval_nanos(base) == parse_dashboard_interval_nanos(edited)
}

fn is_sing_box_dashboard_interval_path(path: &[SemanticPathSegment]) -> bool {
    path.iter()
        .any(|segment| matches!(segment, SemanticPathSegment::Key { key } if key == "dashboard"))
        && path.iter().any(|segment| {
            matches!(
                segment,
                SemanticPathSegment::Identity { field, value }
                    if field == "tag" && value == MANAGED_SING_BOX_API_TAG
            )
        })
        && matches!(
            path.last(),
            Some(SemanticPathSegment::Key { key }) if key == "update_interval"
        )
}

fn sequence_identities(values: &[Value]) -> Option<Vec<(String, String)>> {
    if values.is_empty() {
        return None;
    }
    let mut identities = Vec::with_capacity(values.len());
    let mut unique = BTreeSet::new();
    for value in values {
        let object = value.as_object()?;
        let identity = ["tag", "name", "id"].into_iter().find_map(|field| {
            object
                .get(field)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(|value| (field.to_owned(), value.to_owned()))
        })?;
        if !unique.insert(identity.clone()) {
            return None;
        }
        identities.push(identity);
    }
    Some(identities)
}

pub fn apply_raw_intent(
    base: &Value,
    intent: &RawManualIntent,
) -> (Value, Vec<ConfigurationConflict>) {
    let mut effective = base.clone();
    let mut conflicts = Vec::new();
    for operation in candidate_raw_operations(intent) {
        if let Err(conflict) = apply_intent_operation(&mut effective, operation) {
            conflicts.push(*conflict);
        }
    }
    (effective, conflicts)
}

fn raw_decision_conflicts(
    upstream: &Value,
    intent: &RawManualIntent,
) -> Vec<ConfigurationConflict> {
    intent
        .decisions
        .iter()
        .filter(|decision| decision.status == RawDecisionStatus::Superseded)
        .map(|decision| {
            let path = intent_operation_path(&decision.operation);
            let upstream_value = raw_conflict_path_value(upstream, &path)
                .ok()
                .flatten()
                .cloned();
            ConfigurationConflict {
                semantic_path: display_semantic_path(&path),
                reason: "The upstream configuration changed after this Raw decision was made"
                    .into(),
                severity: ConflictSeverity::Error,
                message_key: Some("RAW_DECISION_SUPERSEDED".into()),
                scope: ConfigurationIssueScope {
                    surface: ConfigurationSurface::Configuration,
                    owner_id: Some(decision.decision_id.clone()),
                },
                source_value: upstream_value.clone(),
                guided_value: None,
                raw_value: intent_operation_value(&decision.operation),
                effective_value: upstream_value,
            }
        })
        .collect()
}

fn intent_operation_value(operation: &IntentOperation) -> Option<Value> {
    match operation {
        IntentOperation::Set { value, .. } => Some(value.clone()),
        IntentOperation::Delete { .. } => None,
        IntentOperation::ReorderIdentities { order, .. } => Some(Value::Array(
            order
                .iter()
                .map(|(field, value)| json!({ "field": field, "value": value }))
                .collect(),
        )),
        IntentOperation::ReplaceSequence { value, .. } => Some(Value::Array(value.clone())),
    }
}

pub fn resolve_raw_decision(
    intent: &mut RawManualIntent,
    upstream: &Value,
    upstream_generation: u64,
    decision_id: &str,
    resolution: RawDecisionResolution,
) -> Result<()> {
    let upstream_content_hash = semantic_document_hash(upstream);
    let decision = intent
        .decisions
        .iter_mut()
        .find(|decision| decision.decision_id == decision_id)
        .ok_or_else(|| {
            CamelliaNexusError::new(ErrorCode::NotFound, "Raw decision was not found")
        })?;
    let path = intent_operation_path(&decision.operation);
    match resolution {
        RawDecisionResolution::AcceptUpstream => {
            decision.status = RawDecisionStatus::Dormant;
        }
        RawDecisionResolution::KeepRaw => {
            decision.basis = RawDecisionBasis {
                upstream_generation,
                upstream_content_hash,
                upstream_path_hash: semantic_path_value_hash(upstream, &path),
            };
            decision.status = RawDecisionStatus::Resolved;
        }
        RawDecisionResolution::ManualEdit { value } => {
            decision.operation = IntentOperation::Set {
                path: path.clone(),
                value,
            };
            decision.basis = RawDecisionBasis {
                upstream_generation,
                upstream_content_hash,
                upstream_path_hash: semantic_path_value_hash(upstream, &path),
            };
            decision.status = RawDecisionStatus::Resolved;
        }
    }
    Ok(())
}

/// Performs a semantic three-way rebase of a Raw document.  The original
/// source snapshot, the user's working document and the newest source
/// snapshot are compared independently; formatting/key-order changes are
/// naturally ignored by the `Value` representation.  A conflict is emitted
/// only when both sides changed the same semantic unit differently.
pub fn rebase_raw_document(
    original_base: &Value,
    user_document: &Value,
    updated_base: &Value,
) -> RawRebaseResult {
    let mut conflicts = Vec::new();
    let document = rebase_value(
        original_base,
        user_document,
        updated_base,
        &mut Vec::new(),
        &mut conflicts,
    );
    let deterministic_hash = hash_bytes(
        serde_json::to_string(&document)
            .expect("semantic JSON values are serializable")
            .as_bytes(),
    );
    RawRebaseResult {
        document,
        conflicts,
        deterministic_hash,
    }
}

pub fn resolve_raw_draft_conflict(
    draft: &mut RawDraftSession,
    format: ConfigurationFormat,
    conflict_id: &str,
    resolution: RawConflictResolution,
) -> Result<()> {
    let conflict = draft
        .conflicts
        .iter()
        .find(|conflict| conflict.conflict_id == conflict_id)
        .cloned()
        .ok_or_else(|| {
            CamelliaNexusError::new(ErrorCode::NotFound, "Configuration conflict was not found")
        })?;
    let mut document = parse_semantic_document(format, draft.working_content.as_bytes())?;
    match raw_conflict_resolution_value(&conflict, &resolution)? {
        Some(value) => set_semantic_path(&mut document, &conflict.segments, value),
        None => delete_semantic_path(&mut document, &conflict.segments),
    }
    .map_err(|conflict| CamelliaNexusError::new(ErrorCode::ConfigConflict, conflict.reason))?;
    draft.working_content = serialize_semantic_document(format, &document)?;
    draft
        .resolutions
        .insert(conflict.conflict_id.clone(), resolution);
    refresh_raw_draft_conflicts(draft, format);
    Ok(())
}

pub fn refresh_raw_draft_conflicts(draft: &mut RawDraftSession, format: ConfigurationFormat) {
    let document = parse_semantic_document(format, draft.working_content.as_bytes()).ok();
    draft.unresolved_conflict_ids = draft
        .conflicts
        .iter()
        .filter(|conflict| {
            let Some(resolution) = draft.resolutions.get(&conflict.conflict_id) else {
                return true;
            };
            document.as_ref().is_none_or(|document| {
                !raw_conflict_resolution_matches(document, conflict, resolution).unwrap_or(false)
            })
        })
        .map(|conflict| conflict.conflict_id.clone())
        .collect();
}

fn raw_conflict_resolution_value(
    conflict: &RawConflict,
    resolution: &RawConflictResolution,
) -> Result<Option<Value>> {
    match resolution {
        RawConflictResolution::KeepMine if conflict.conflict_type == "delete-vs-modify" => Ok(None),
        RawConflictResolution::UseUpdated if conflict.conflict_type == "modify-vs-delete" => {
            Ok(None)
        }
        RawConflictResolution::KeepMine => Ok(Some(conflict.user_value.clone())),
        RawConflictResolution::UseUpdated => Ok(Some(conflict.updated_base.clone())),
        RawConflictResolution::Combine if conflict.can_combine => Ok(Some(
            combine_raw_conflict_values(&conflict.updated_base, &conflict.user_value),
        )),
        RawConflictResolution::Combine => Err(CamelliaNexusError::new(
            ErrorCode::ConfigConflict,
            "This configuration conflict cannot be combined safely",
        )),
        RawConflictResolution::ManualEdit { value } => Ok(Some(value.clone())),
    }
}

fn raw_conflict_resolution_matches(
    document: &Value,
    conflict: &RawConflict,
    resolution: &RawConflictResolution,
) -> Result<bool> {
    let expected = raw_conflict_resolution_value(conflict, resolution)?;
    if conflict.segments.is_empty() {
        return Ok(match expected {
            Some(expected) => document == &expected,
            None => document.as_object().is_some_and(|object| object.is_empty()),
        });
    }
    let observed = raw_conflict_path_value(document, &conflict.segments)
        .map_err(|conflict| CamelliaNexusError::new(ErrorCode::ConfigConflict, conflict.reason))?;
    Ok(match expected {
        Some(expected) => observed == Some(&expected),
        None => observed.is_none(),
    })
}

fn raw_conflict_path_value<'a>(
    root: &'a Value,
    path: &[SemanticPathSegment],
) -> std::result::Result<Option<&'a Value>, Box<ConfigurationConflict>> {
    let mut current = root;
    for segment in path {
        match segment {
            SemanticPathSegment::Key { key } => {
                let Some(next) = current.as_object().and_then(|object| object.get(key)) else {
                    return Ok(None);
                };
                current = next;
            }
            SemanticPathSegment::Identity { field, value } => {
                let Some(sequence) = current.as_array() else {
                    return Ok(None);
                };
                let Some(index) = identity_index(sequence, field, value)? else {
                    return Ok(None);
                };
                current = &sequence[index];
            }
        }
    }
    Ok(Some(current))
}

fn combine_raw_conflict_values(updated: &Value, user: &Value) -> Value {
    match (updated, user) {
        (Value::Object(updated), Value::Object(user)) => {
            let mut merged = updated.clone();
            for (key, value) in user {
                let existing = merged.get(key).cloned().unwrap_or(Value::Null);
                merged.insert(key.clone(), combine_raw_conflict_values(&existing, value));
            }
            Value::Object(merged)
        }
        (_, user) => user.clone(),
    }
}

fn rebase_value(
    original: &Value,
    user: &Value,
    updated: &Value,
    path: &mut SemanticPath,
    conflicts: &mut Vec<RawConflict>,
) -> Value {
    if user == original {
        return updated.clone();
    }
    if updated == original || user == updated {
        return user.clone();
    }
    match (original, user, updated) {
        (Value::Object(original), Value::Object(user), Value::Object(updated)) => {
            let keys = original
                .keys()
                .chain(user.keys())
                .chain(updated.keys())
                .cloned()
                .collect::<BTreeSet<_>>();
            let mut result = Map::new();
            for key in keys {
                path.push(SemanticPathSegment::Key { key: key.clone() });
                match (original.get(&key), user.get(&key), updated.get(&key)) {
                    (None, Some(user), None) => {
                        result.insert(key.clone(), user.clone());
                    }
                    (None, None, Some(updated)) => {
                        result.insert(key.clone(), updated.clone());
                    }
                    (None, Some(user), Some(updated)) => {
                        result.insert(
                            key.clone(),
                            rebase_value(&Value::Null, user, updated, path, conflicts),
                        );
                    }
                    (Some(original), Some(user), Some(updated)) => {
                        result.insert(
                            key.clone(),
                            rebase_value(original, user, updated, path, conflicts),
                        );
                    }
                    (Some(original), Some(user), None) => {
                        add_raw_conflict(
                            original,
                            &Value::Null,
                            user,
                            path,
                            "modify-vs-delete",
                            conflicts,
                        );
                        result.insert(key.clone(), user.clone());
                    }
                    (Some(original), None, Some(updated)) => {
                        add_raw_conflict(
                            original,
                            updated,
                            &Value::Null,
                            path,
                            "delete-vs-modify",
                            conflicts,
                        );
                        result.insert(key.clone(), updated.clone());
                    }
                    (None, None, None) => {}
                    (Some(_), None, None) => {}
                }
                path.pop();
            }
            Value::Object(result)
        }
        (Value::Array(original), Value::Array(user), Value::Array(updated)) => {
            if let (Some(original_ids), Some(user_ids), Some(updated_ids)) = (
                sequence_identities(original),
                sequence_identities(user),
                sequence_identities(updated),
            ) {
                let original_map = original_ids
                    .iter()
                    .cloned()
                    .zip(original.iter())
                    .collect::<HashMap<_, _>>();
                let user_map = user_ids
                    .iter()
                    .cloned()
                    .zip(user.iter())
                    .collect::<HashMap<_, _>>();
                let updated_map = updated_ids
                    .iter()
                    .cloned()
                    .zip(updated.iter())
                    .collect::<HashMap<_, _>>();
                if user_ids != original_ids
                    && updated_ids != original_ids
                    && user_ids != updated_ids
                {
                    add_raw_conflict(
                        &Value::Array(original.to_vec()),
                        &Value::Array(updated.to_vec()),
                        &Value::Array(user.to_vec()),
                        path,
                        "order",
                        conflicts,
                    );
                }
                let mut identities = user_ids.clone();
                for identity in &updated_ids {
                    if !identities.contains(identity) {
                        identities.push(identity.clone());
                    }
                }
                let mut result = Vec::new();
                for identity in identities {
                    path.push(SemanticPathSegment::Identity {
                        field: identity.0.clone(),
                        value: identity.1.clone(),
                    });
                    match (
                        original_map.get(&identity),
                        user_map.get(&identity),
                        updated_map.get(&identity),
                    ) {
                        (None, Some(user), None) => result.push((*user).clone()),
                        (None, None, Some(updated)) => result.push((*updated).clone()),
                        (None, Some(user), Some(updated)) => {
                            result.push(rebase_value(&Value::Null, user, updated, path, conflicts))
                        }
                        (Some(original), Some(user), Some(updated)) => {
                            result.push(rebase_value(original, user, updated, path, conflicts))
                        }
                        (Some(original), Some(user), None) => {
                            add_raw_conflict(
                                original,
                                &Value::Null,
                                user,
                                path,
                                "modify-vs-delete",
                                conflicts,
                            );
                            result.push((*user).clone());
                        }
                        (Some(original), None, Some(updated)) => {
                            add_raw_conflict(
                                original,
                                updated,
                                &Value::Null,
                                path,
                                "delete-vs-modify",
                                conflicts,
                            );
                            result.push((*updated).clone());
                        }
                        (None, None, None) => {}
                        (Some(_), None, None) => {}
                    }
                    path.pop();
                }
                Value::Array(result)
            } else {
                add_raw_conflict(
                    &Value::Array(original.to_vec()),
                    &Value::Array(updated.to_vec()),
                    &Value::Array(user.to_vec()),
                    path,
                    "ordered-sequence",
                    conflicts,
                );
                Value::Array(user.clone())
            }
        }
        _ => {
            add_raw_conflict(original, updated, user, path, "value", conflicts);
            user.clone()
        }
    }
}

fn add_raw_conflict(
    original: &Value,
    updated: &Value,
    user: &Value,
    path: &[SemanticPathSegment],
    conflict_type: &str,
    conflicts: &mut Vec<RawConflict>,
) {
    let semantic_path = display_semantic_path(path);
    let conflict_id =
        hash_bytes(format!("{semantic_path}|{conflict_type}").as_bytes())[..16].to_owned();
    conflicts.push(RawConflict {
        conflict_id,
        segments: path.to_vec(),
        semantic_path: semantic_path.clone(),
        display_path: semantic_path,
        conflict_type: conflict_type.to_owned(),
        severity: ConflictSeverity::Error,
        original_base: original.clone(),
        updated_base: updated.clone(),
        user_value: user.clone(),
        suggested_actions: vec!["keepMine".into(), "useUpdated".into(), "manualEdit".into()],
        can_combine: conflict_type == "value"
            && original.is_object()
            && updated.is_object()
            && user.is_object(),
    });
}

fn apply_intent_operation(
    root: &mut Value,
    operation: &IntentOperation,
) -> std::result::Result<(), Box<ConfigurationConflict>> {
    match operation {
        IntentOperation::Set { path, value } => set_semantic_path(root, path, value.clone()),
        IntentOperation::Delete { path } => delete_semantic_path(root, path),
        IntentOperation::ReorderIdentities { path, order } => {
            let target = resolve_semantic_path_mut(root, path)?;
            let values = target.as_array_mut().ok_or_else(|| {
                intent_conflict(path, "Raw reorder target is no longer an array", None)
            })?;
            let mut indexed = BTreeMap::new();
            let mut extras = Vec::new();
            for value in std::mem::take(values) {
                if let Some(identity) = semantic_identity(&value) {
                    if indexed.insert(identity, value).is_some() {
                        return Err(intent_conflict(
                            path,
                            "Raw reorder identities became ambiguous",
                            None,
                        ));
                    }
                } else {
                    extras.push(value);
                }
            }
            for identity in order {
                let Some(value) = indexed.remove(identity) else {
                    return Err(intent_conflict(
                        path,
                        "A Raw reorder anchor no longer exists in the source configuration",
                        None,
                    ));
                };
                values.push(value);
            }
            values.extend(indexed.into_values());
            values.extend(extras);
            Ok(())
        }
        IntentOperation::ReplaceSequence {
            path,
            expected,
            value,
        } => {
            let target = resolve_semantic_path_mut(root, path)?;
            if target.as_array() != Some(expected) {
                return Err(intent_conflict(
                    path,
                    "An ordered Raw sequence changed in the source and cannot be rebased safely",
                    Some(Value::Array(value.clone())),
                ));
            }
            *target = Value::Array(value.clone());
            Ok(())
        }
    }
}

fn semantic_identity(value: &Value) -> Option<(String, String)> {
    let object = value.as_object()?;
    ["tag", "name", "id"].into_iter().find_map(|field| {
        object
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(|value| (field.to_owned(), value.to_owned()))
    })
}

fn set_semantic_path(
    root: &mut Value,
    path: &[SemanticPathSegment],
    value: Value,
) -> std::result::Result<(), Box<ConfigurationConflict>> {
    if path.is_empty() {
        *root = value;
        return Ok(());
    }
    let (parent_path, last) = path.split_at(path.len() - 1);
    let parent = resolve_semantic_path_mut(root, parent_path)?;
    match &last[0] {
        SemanticPathSegment::Key { key } => {
            parent
                .as_object_mut()
                .ok_or_else(|| {
                    intent_conflict(
                        path,
                        "Raw target parent is not an object",
                        Some(value.clone()),
                    )
                })?
                .insert(key.clone(), value);
        }
        SemanticPathSegment::Identity {
            field,
            value: identity,
        } => {
            let values = parent.as_array_mut().ok_or_else(|| {
                intent_conflict(
                    path,
                    "Raw identity target parent is not an array",
                    Some(value.clone()),
                )
            })?;
            if let Some(index) = identity_index(values, field, identity)? {
                values[index] = value;
            } else {
                values.push(value);
            }
        }
    }
    Ok(())
}

fn delete_semantic_path(
    root: &mut Value,
    path: &[SemanticPathSegment],
) -> std::result::Result<(), Box<ConfigurationConflict>> {
    if path.is_empty() {
        *root = Value::Object(Map::new());
        return Ok(());
    }
    let (parent_path, last) = path.split_at(path.len() - 1);
    let parent = resolve_semantic_path_mut(root, parent_path)?;
    match &last[0] {
        SemanticPathSegment::Key { key } => {
            let values = parent.as_object_mut().ok_or_else(|| {
                intent_conflict(path, "Raw delete target parent is not an object", None)
            })?;
            values.remove(key);
        }
        SemanticPathSegment::Identity {
            field,
            value: identity,
        } => {
            let values = parent.as_array_mut().ok_or_else(|| {
                intent_conflict(path, "Raw delete target parent is not an array", None)
            })?;
            if let Some(index) = identity_index(values, field, identity)? {
                values.remove(index);
            }
        }
    }
    Ok(())
}

fn resolve_semantic_path_mut<'a>(
    mut current: &'a mut Value,
    path: &[SemanticPathSegment],
) -> std::result::Result<&'a mut Value, Box<ConfigurationConflict>> {
    for (offset, segment) in path.iter().enumerate() {
        match segment {
            SemanticPathSegment::Key { key } => {
                current = current
                    .as_object_mut()
                    .and_then(|values| values.get_mut(key))
                    .ok_or_else(|| {
                        intent_conflict(
                            &path[..=offset],
                            "A Raw object anchor no longer exists",
                            None,
                        )
                    })?;
            }
            SemanticPathSegment::Identity { field, value } => {
                let values = current.as_array_mut().ok_or_else(|| {
                    intent_conflict(
                        &path[..=offset],
                        "A Raw identity parent is no longer an array",
                        None,
                    )
                })?;
                let index = identity_index(values, field, value)?.ok_or_else(|| {
                    intent_conflict(
                        &path[..=offset],
                        "A Raw identity anchor no longer exists",
                        None,
                    )
                })?;
                current = &mut values[index];
            }
        }
    }
    Ok(current)
}

fn identity_index(
    values: &[Value],
    field: &str,
    identity: &str,
) -> std::result::Result<Option<usize>, Box<ConfigurationConflict>> {
    let matches = values
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            (value.get(field).and_then(Value::as_str) == Some(identity)).then_some(index)
        })
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        Err(Box::new(ConfigurationConflict {
            semantic_path: format!("[{field}={identity}]"),
            reason: "A semantic identity is duplicated".into(),
            severity: ConflictSeverity::Error,
            message_key: Some("CONFIGURATION_IDENTITY_DUPLICATED".into()),
            scope: ConfigurationIssueScope::configuration(),
            source_value: None,
            guided_value: None,
            raw_value: None,
            effective_value: None,
        }))
    } else {
        Ok(matches.first().copied())
    }
}

fn intent_operation_path(operation: &IntentOperation) -> SemanticPath {
    match operation {
        IntentOperation::Set { path, .. }
        | IntentOperation::Delete { path }
        | IntentOperation::ReorderIdentities { path, .. }
        | IntentOperation::ReplaceSequence { path, .. } => path.clone(),
    }
}

fn path_overlaps(left: &[SemanticPathSegment], right: &[SemanticPathSegment]) -> bool {
    let shared = left.len().min(right.len());
    left[..shared] == right[..shared]
}

fn intent_conflict(
    path: &[SemanticPathSegment],
    reason: impl Into<String>,
    raw_value: Option<Value>,
) -> Box<ConfigurationConflict> {
    Box::new(ConfigurationConflict {
        semantic_path: display_semantic_path(path),
        reason: reason.into(),
        severity: ConflictSeverity::Error,
        message_key: Some("CONFIGURATION_RAW_CONFLICT".into()),
        scope: ConfigurationIssueScope::configuration(),
        source_value: None,
        guided_value: None,
        raw_value,
        effective_value: None,
    })
}

fn display_semantic_path(path: &[SemanticPathSegment]) -> String {
    if path.is_empty() {
        return "/".into();
    }
    let mut value = String::new();
    for segment in path {
        match segment {
            SemanticPathSegment::Key { key } => {
                value.push('/');
                value.push_str(&escape_pointer(key));
            }
            SemanticPathSegment::Identity {
                field,
                value: identity,
            } => {
                value.push('[');
                value.push_str(field);
                value.push('=');
                value.push_str(identity);
                value.push(']');
            }
        }
    }
    value
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CandidateValidationStatus {
    Pending,
    Valid,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationDiagnostic {
    pub code: String,
    pub message: String,
    /// Stable UI mapping key; the message is retained as technical context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_key: Option<String>,
    #[serde(default)]
    pub scope: ConfigurationIssueScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationCandidate {
    pub revision: ConfigurationRevision,
    pub content: String,
    pub compatibility_profile_hash: String,
    pub validation: CandidateValidationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_evidence: Option<CoreValidationEvidence>,
    #[serde(default)]
    pub diagnostics: Vec<ConfigurationDiagnostic>,
    #[serde(default)]
    pub conflicts: Vec<ConfigurationConflict>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationState {
    pub schema_version: u32,
    pub kind: ProgramKind,
    pub format: ConfigurationFormat,
    pub generation: u64,
    pub compatibility_profile: CoreCompatibilityProfile,
    #[serde(default)]
    pub source_statuses: BTreeMap<String, SourceStatus>,
    #[serde(default)]
    pub source_snapshots: BTreeMap<String, SourceSnapshot>,
    pub base: ConfigurationCandidate,
    #[serde(default)]
    pub base_provenance: Vec<ProvenanceEntry>,
    #[serde(default)]
    pub provenance: Vec<ProvenanceEntry>,
    #[serde(default)]
    pub guided_intent: GuidedIntent,
    #[serde(default)]
    pub managed_intent: ManagedIntegrationIntent,
    #[serde(default)]
    pub raw_intent: RawManualIntent,
    pub desired: ConfigurationCandidate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied: Option<ConfigurationCandidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_known_good: Option<ConfigurationCandidate>,
}

impl ConfigurationState {
    /// Upgrade persisted configuration state without touching Applied/LKG.
    /// v3 separated Details-owned dashboard values from Common Guided; v5
    /// replaces unconditional Raw operations with basis-bound decisions.
    /// Raw operation binding is completed by `rebuild_desired`, where the
    /// authoritative upstream document is available.
    pub fn migrate_legacy_schema(&mut self) -> bool {
        let mut migrated = false;
        if self.schema_version == LEGACY_MANAGED_CONFIGURATION_STATE_SCHEMA_VERSION {
            let legacy_dashboard = self
                .guided_intent
                .values
                .keys()
                .filter(|key| key.starts_with(DASHBOARD_INTENT_PREFIX))
                .cloned()
                .collect::<Vec<_>>();
            for key in legacy_dashboard {
                if let Some(value) = self.guided_intent.values.remove(&key) {
                    self.managed_intent.values.insert(key, value);
                }
            }
            self.schema_version = LEGACY_CONFIGURATION_STATE_SCHEMA_VERSION;
            migrated = true;
        }
        if self.schema_version == LEGACY_CONFIGURATION_STATE_SCHEMA_VERSION {
            self.schema_version = CONFIGURATION_STATE_SCHEMA_VERSION;
            migrated = true;
        }
        migrated
    }

    /// Canonicalize only v3 Raw operations that overlap managed fields.  Old
    /// clients could persist a whole generated Dashboard container as Raw;
    /// comparing that operation with the newly separated Managed layer turns
    /// identical generated fields into leaf no-ops while retaining genuine
    /// overrides and unrelated extension fields.  Non-overlapping tombstones
    /// are left byte-for-byte untouched.
    pub fn canonicalize_legacy_managed_raw(&mut self) -> Result<()> {
        let base = parse_semantic_document(self.format, self.base.content.as_bytes())?;
        let mut generated = apply_guided_intent(self.kind, &base, &self.guided_intent)?;
        apply_dashboard_intent(self.kind, &mut generated, &self.managed_intent)?;
        canonicalize_managed_raw_operations(
            self.kind,
            &generated,
            &self.managed_intent,
            &mut self.raw_intent,
        );
        Ok(())
    }

    pub fn from_merge(
        kind: ProgramKind,
        generation: u64,
        created_unix_ms: u64,
        merge: SemanticMergeResult,
        compatibility_profile: CoreCompatibilityProfile,
    ) -> Result<Self> {
        let format = ConfigurationFormat::for_kind(kind).ok_or_else(|| {
            CamelliaNexusError::invalid_spec("Generic programs do not have configuration state")
        })?;
        let revision = ConfigurationRevision::new(generation, &merge.content, created_unix_ms);
        if compatibility_profile.target.program != kind {
            return Err(CamelliaNexusError::invalid_spec(
                "Configuration compatibility profile does not match the program kind",
            ));
        }
        let base_provenance = merge.provenance;
        let candidate = ConfigurationCandidate {
            revision,
            content: merge.content,
            compatibility_profile_hash: compatibility_profile.profile_hash.clone(),
            validation: CandidateValidationStatus::Pending,
            validation_evidence: None,
            diagnostics: Vec::new(),
            conflicts: merge.conflicts,
        };
        Ok(Self {
            schema_version: CONFIGURATION_STATE_SCHEMA_VERSION,
            kind,
            format,
            generation,
            compatibility_profile,
            source_statuses: BTreeMap::new(),
            source_snapshots: BTreeMap::new(),
            base: candidate.clone(),
            provenance: base_provenance.clone(),
            base_provenance,
            guided_intent: GuidedIntent::default(),
            managed_intent: ManagedIntegrationIntent::default(),
            raw_intent: RawManualIntent::default(),
            desired: candidate,
            applied: None,
            last_known_good: None,
        })
    }

    pub fn upstream_document(&self) -> Result<Value> {
        let base = parse_semantic_document(self.format, self.base.content.as_bytes())?;
        let mut upstream = apply_guided_intent(self.kind, &base, &self.guided_intent)?;
        apply_dashboard_intent(self.kind, &mut upstream, &self.managed_intent)?;
        Ok(upstream)
    }

    pub fn upstream_content(&self) -> Result<String> {
        serialize_semantic_document(self.format, &self.upstream_document()?)
    }

    pub fn resolve_raw_decision(
        &mut self,
        decision_id: &str,
        resolution: RawDecisionResolution,
        created_unix_ms: u64,
    ) -> Result<()> {
        let upstream = self.upstream_document()?;
        resolve_raw_decision(
            &mut self.raw_intent,
            &upstream,
            self.generation,
            decision_id,
            resolution,
        )?;
        self.rebuild_desired(created_unix_ms)
    }

    pub fn rebuild_desired(&mut self, created_unix_ms: u64) -> Result<()> {
        let previous_content = self.desired.content.clone();
        let previous_conflicts = self.desired.conflicts.clone();
        let previous_profile_hash = self.desired.compatibility_profile_hash.clone();
        let previous_decisions = self.raw_intent.decisions.clone();
        let base = parse_semantic_document(self.format, self.base.content.as_bytes())?;
        // Deterministically migrate dashboard values that may have been loaded
        // from a v3 state (or supplied by an older caller) before building the
        // new semantic pipeline.  They are never treated as Common Guided
        // settings after this point.
        let legacy_dashboard = self
            .guided_intent
            .values
            .keys()
            .filter(|key| key.starts_with(DASHBOARD_INTENT_PREFIX))
            .cloned()
            .collect::<Vec<_>>();
        for key in legacy_dashboard {
            if let Some(value) = self.guided_intent.values.remove(&key) {
                self.managed_intent.values.insert(key, value);
            }
        }
        let guided = apply_guided_intent(self.kind, &base, &self.guided_intent)?;
        let guided_provenance = apply_provenance_layer(
            &base,
            &guided,
            &self.base_provenance,
            ProvenanceLayer::Guided,
        );
        // apply_dashboard_intent mutates its argument; keep the common Guided
        // provenance separate from the managed layer.
        let mut effective_guided = guided;
        apply_dashboard_intent(self.kind, &mut effective_guided, &self.managed_intent)?;
        reconcile_raw_decisions(&mut self.raw_intent, &effective_guided, self.generation);
        let (effective, mut conflicts) = apply_raw_intent(&effective_guided, &self.raw_intent);
        self.provenance = apply_provenance_layer(
            &effective_guided,
            &effective,
            &guided_provenance,
            ProvenanceLayer::Raw,
        );
        // Preserve blocking issues produced while merging Sources. Rebuilding
        // downstream layers must not make a source conflict disappear.
        conflicts.extend(self.base.conflicts.clone());
        conflicts.extend(raw_decision_conflicts(&effective_guided, &self.raw_intent));
        let content = serialize_semantic_document(self.format, &effective)?;
        let decisions_changed = previous_decisions != self.raw_intent.decisions;
        let recovered_source_issue = !self.source_statuses.is_empty()
            && self.desired.diagnostics.iter().any(|diagnostic| {
                diagnostic.scope.surface == ConfigurationSurface::Sources
                    && matches!(
                        diagnostic.code.as_str(),
                        "SOURCE_INVALID" | "SOURCE_UNAVAILABLE"
                    )
            })
            && self.source_statuses.values().all(|status| {
                !matches!(
                    status.freshness,
                    SourceFreshness::Invalid | SourceFreshness::Unavailable
                )
            });
        let candidate_changed = previous_content != content
            || previous_conflicts != conflicts
            || previous_profile_hash != self.compatibility_profile.profile_hash
            || recovered_source_issue;
        if !decisions_changed && !candidate_changed {
            // Re-entering a tab, refreshing an unchanged source, or repeating
            // the same Guided/Details value is a read-equivalent operation.
            // Keep generation and native evidence stable instead of creating a
            // phantom revision that can invalidate an otherwise valid Apply.
            self.schema_version = CONFIGURATION_STATE_SCHEMA_VERSION;
            return Ok(());
        }
        self.generation = self.generation.saturating_add(1);
        self.schema_version = CONFIGURATION_STATE_SCHEMA_VERSION;
        self.desired = ConfigurationCandidate {
            revision: ConfigurationRevision::new(self.generation, &content, created_unix_ms),
            content,
            compatibility_profile_hash: self.compatibility_profile.profile_hash.clone(),
            validation: if conflicts
                .iter()
                .any(|conflict| conflict.severity == ConflictSeverity::Error)
            {
                CandidateValidationStatus::Invalid
            } else {
                CandidateValidationStatus::Pending
            },
            validation_evidence: None,
            diagnostics: Vec::new(),
            conflicts,
        };
        Ok(())
    }

    pub fn replace_raw_from_edited(&mut self, edited: &[u8], created_unix_ms: u64) -> Result<()> {
        let base = parse_semantic_document(self.format, self.base.content.as_bytes())?;
        let mut guided = apply_guided_intent(self.kind, &base, &self.guided_intent)?;
        apply_dashboard_intent(self.kind, &mut guided, &self.managed_intent)?;
        let edited = parse_semantic_document(self.format, edited)?;
        let mut next = diff_raw_decisions(&guided, &edited, self.generation);
        // Preserve an unchanged participating decision verbatim. Recreating
        // it would change its id/basis even though the final document did not
        // change, producing a phantom generation and invalidating otherwise
        // current native evidence. Superseded and dormant decisions are not
        // reused: writing their Raw value again is an explicit new decision
        // against the current upstream basis.
        for next_decision in &mut next.decisions {
            if let Some(existing) = self.raw_intent.decisions.iter().find(|existing| {
                existing.status.participates_in_candidate()
                    && existing.operation == next_decision.operation
            }) {
                *next_decision = existing.clone();
            }
        }
        let next_paths = next
            .decisions
            .iter()
            .map(|decision| intent_operation_path(&decision.operation))
            .collect::<Vec<_>>();
        let mut retained = self
            .raw_intent
            .decisions
            .drain(..)
            .filter_map(|mut decision| {
                let decision_path = intent_operation_path(&decision.operation);
                let overlaps = next_paths
                    .iter()
                    .any(|path| path_overlaps(path, &decision_path));
                if overlaps {
                    // The edited document now contains a new decision for this
                    // semantic unit.  The new diff below replaces the old
                    // basis-bound decision without touching unrelated paths.
                    return None;
                }
                if semantic_path_values_equal(&edited, &guided, &decision_path) {
                    // Editing an active Raw value back to the current upstream
                    // value is an explicit Accept-upstream action.  Keep the
                    // history as a dormant decision so the operation is
                    // reversible, but never let it participate in the
                    // candidate again.
                    decision.status = RawDecisionStatus::Dormant;
                }
                Some(decision)
            })
            .collect::<Vec<_>>();
        retained.extend(next.decisions);
        self.raw_intent.operations.clear();
        self.raw_intent.decisions = retained;
        self.rebuild_desired(created_unix_ms)
    }

    pub fn mark_validation(
        &mut self,
        valid: bool,
        diagnostics: Vec<ConfigurationDiagnostic>,
        evidence: Option<CoreValidationEvidence>,
    ) -> Result<()> {
        if valid {
            let binary_sha256 = self
                .compatibility_profile
                .target
                .fingerprint_sha256
                .as_deref()
                .ok_or_else(|| {
                    CamelliaNexusError::new(
                        ErrorCode::InvalidState,
                        "Validated configuration target is not bound to a binary fingerprint",
                    )
                })?;
            if !evidence.as_ref().is_some_and(|evidence| {
                evidence.validates(
                    binary_sha256,
                    &self.compatibility_profile.profile_hash,
                    &self.desired.revision.content_hash,
                )
            }) {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigConflict,
                    "Native validation evidence does not match the exact binary, profile, and candidate",
                ));
            }
        }
        self.desired.validation = if valid {
            CandidateValidationStatus::Valid
        } else {
            CandidateValidationStatus::Invalid
        };
        self.desired.diagnostics = diagnostics;
        self.desired.validation_evidence = valid.then_some(evidence).flatten();
        Ok(())
    }

    pub fn ensure_apply_ready(&self) -> Result<()> {
        if self
            .desired
            .conflicts
            .iter()
            .any(|conflict| conflict.severity == ConflictSeverity::Error)
        {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Blocking configuration conflicts must be resolved before Apply",
            ));
        }
        if self.desired.validation != CandidateValidationStatus::Valid {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Only a valid Desired configuration can be applied",
            ));
        }
        let binary_sha256 = self
            .compatibility_profile
            .target
            .fingerprint_sha256
            .as_deref()
            .ok_or_else(|| {
                CamelliaNexusError::new(
                    ErrorCode::ConfigConflict,
                    "Apply requires an exact binary fingerprint",
                )
            })?;
        if !self
            .desired
            .validation_evidence
            .as_ref()
            .is_some_and(|evidence| {
                evidence.validates(
                    binary_sha256,
                    &self.compatibility_profile.profile_hash,
                    &self.desired.revision.content_hash,
                )
            })
        {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Native validation evidence does not match the exact binary, profile, and candidate",
            ));
        }
        Ok(())
    }

    pub fn mark_applied(&mut self) -> Result<()> {
        self.ensure_apply_ready()?;
        self.applied = Some(self.desired.clone());
        self.last_known_good = Some(self.desired.clone());
        Ok(())
    }

    pub fn view(&self) -> ConfigurationStateView {
        let base = parse_semantic_document(self.format, self.base.content.as_bytes())
            .unwrap_or(Value::Null);
        let guided = apply_guided_intent(self.kind, &base, &self.guided_intent)
            .unwrap_or_else(|_| base.clone());
        let mut upstream = guided.clone();
        let _ = apply_dashboard_intent(self.kind, &mut upstream, &self.managed_intent);
        let effective = parse_semantic_document(self.format, self.desired.content.as_bytes())
            .unwrap_or(Value::Null);
        let mut compatibility_references: Vec<crate::CoreCompatibilityReference> =
            embedded_core_compatibility_catalog()
                .ok()
                .and_then(|catalog| catalog.program(self.kind))
                .map(|program| {
                    program
                        .versions
                        .iter()
                        .rev()
                        .map(|version| crate::CoreCompatibilityReference::Release {
                            tag: version.tag.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default();
        if let Ok(manifest) = embedded_core_upstream_manifest()
            && let Some(tracks) = manifest.program(self.kind)
        {
            for commit_sha in [
                tracks.development.commit_sha.clone(),
                tracks.stable.commit_sha.clone(),
            ] {
                if !compatibility_references.iter().any(|reference| {
                    matches!(reference, crate::CoreCompatibilityReference::Commit { commit_sha: existing } if existing == &commit_sha)
                }) {
                    compatibility_references.push(
                        crate::CoreCompatibilityReference::Commit { commit_sha },
                    );
                }
            }
        }
        let workspace = build_configuration_workspace(self, &base, &guided, &upstream, &effective);
        ConfigurationStateView {
            schema_version: self.schema_version,
            kind: self.kind,
            format: self.format,
            generation: self.generation,
            compatibility_profile: self.compatibility_profile.clone(),
            source_statuses: self.source_statuses.values().cloned().collect(),
            source_parse_summaries: self
                .source_snapshots
                .iter()
                .filter_map(|(source_id, snapshot)| {
                    snapshot
                        .share_summary
                        .clone()
                        .map(|summary| (source_id.clone(), summary))
                })
                .collect(),
            provenance: self.provenance.clone(),
            desired: self.desired.clone(),
            applied_revision: self
                .applied
                .as_ref()
                .map(|candidate| candidate.revision.clone()),
            last_known_good_revision: self
                .last_known_good
                .as_ref()
                .map(|candidate| candidate.revision.clone()),
            guided_descriptors: guided_setting_descriptors(self.kind),
            guided_projection: project_guided_settings(
                self.kind,
                &effective,
                &self.guided_intent,
                &self.raw_intent,
            ),
            managed_integrations: project_managed_integrations(
                self.kind,
                &effective,
                &self.managed_intent,
                &self.raw_intent,
                &self.desired.conflicts,
            ),
            compatibility_references,
            workspace,
        }
    }
}

fn build_configuration_workspace(
    state: &ConfigurationState,
    base: &Value,
    guided: &Value,
    upstream: &Value,
    effective: &Value,
) -> ConfigurationWorkspaceView {
    let upstream_document = serialize_semantic_document(state.format, upstream)
        .unwrap_or_else(|_| state.base.content.clone());
    let final_preview_document = serialize_semantic_document(state.format, effective)
        .unwrap_or_else(|_| state.desired.content.clone());
    let raw_decisions = state
        .raw_intent
        .decisions
        .iter()
        .map(|decision| {
            let path = intent_operation_path(&decision.operation);
            RawDecisionProjection {
                decision_id: decision.decision_id.clone(),
                semantic_path: display_semantic_path(&path),
                operation: decision.operation.clone(),
                status: decision.status,
                origin: decision.origin,
                basis: decision.basis.clone(),
                upstream_value: raw_conflict_path_value(upstream, &path)
                    .ok()
                    .flatten()
                    .cloned(),
                raw_value: intent_operation_value(&decision.operation),
            }
        })
        .collect::<Vec<_>>();
    let source_conflicts = state
        .desired
        .conflicts
        .iter()
        .filter(|conflict| {
            conflict.scope.surface == ConfigurationSurface::Sources
                || conflict.message_key.as_deref() == Some("SOURCE_VALUE_CONFLICT")
        })
        .cloned()
        .collect::<Vec<_>>();
    let layer_conflicts = state
        .desired
        .conflicts
        .iter()
        .filter(|conflict| conflict.message_key.as_deref() == Some("LAYER_OWNERSHIP_CONFLICT"))
        .cloned()
        .collect::<Vec<_>>();
    let raw_conflicts = state
        .desired
        .conflicts
        .iter()
        .filter(|conflict| {
            matches!(
                conflict.message_key.as_deref(),
                Some("RAW_DECISION_SUPERSEDED")
                    | Some("CONFIGURATION_RAW_CONFLICT")
                    | Some("CONFIGURATION_IDENTITY_DUPLICATED")
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    let has_blocking_conflicts = state
        .desired
        .conflicts
        .iter()
        .any(|conflict| conflict.severity == ConflictSeverity::Error);
    let can_apply = state.ensure_apply_ready().is_ok();
    ConfigurationWorkspaceView {
        upstream_document,
        final_preview_document: final_preview_document.clone(),
        editable_document: final_preview_document,
        layer_trace: build_layer_trace(state, base, guided, upstream, effective),
        raw_decisions,
        source_conflicts,
        layer_conflicts,
        raw_conflicts,
        diagnostics: state.desired.diagnostics.clone(),
        save_status: if has_blocking_conflicts {
            CandidateSaveStatus::Blocked
        } else if state.desired.validation == CandidateValidationStatus::Pending {
            CandidateSaveStatus::PendingValidation
        } else {
            CandidateSaveStatus::Saved
        },
        validation_status: state.desired.validation,
        can_save: !has_blocking_conflicts,
        can_validate: !has_blocking_conflicts,
        can_apply,
    }
}

fn build_layer_trace(
    state: &ConfigurationState,
    base: &Value,
    guided: &Value,
    upstream: &Value,
    effective: &Value,
) -> Vec<ConfigurationLayerTrace> {
    let mut paths = Vec::new();
    collect_trace_paths(base, &mut Vec::new(), &mut paths);
    collect_trace_paths(guided, &mut Vec::new(), &mut paths);
    collect_trace_paths(upstream, &mut Vec::new(), &mut paths);
    collect_trace_paths(effective, &mut Vec::new(), &mut paths);
    for decision in &state.raw_intent.decisions {
        paths.push(intent_operation_path(&decision.operation));
    }
    paths.sort_by_key(|path| display_semantic_path(path));
    paths.dedup();
    paths
        .into_iter()
        .map(|path| {
            let semantic_path = display_semantic_path(&path);
            let source_value = trace_path_value(base, &path);
            let guided_value = trace_path_value(guided, &path);
            let managed_value = trace_path_value(upstream, &path);
            let effective_value = trace_path_value(effective, &path);
            let raw_decision = state.raw_intent.decisions.iter().find(|decision| {
                path_overlaps(&path, &intent_operation_path(&decision.operation))
                    && decision.status != RawDecisionStatus::Dormant
            });
            let winner_layer = if raw_decision
                .is_some_and(|decision| decision.status.participates_in_candidate())
            {
                ConfigurationLayer::RawDecision
            } else if managed_value != guided_value {
                ConfigurationLayer::Details
            } else if guided_value != source_value {
                ConfigurationLayer::Intent
            } else {
                ConfigurationLayer::Source
            };
            let source_ids = semantic_path_to_pointer(base, &path)
                .map(|pointer| aggregate_provenance(&state.base_provenance, &pointer).source_ids)
                .unwrap_or_default();
            let issue_ids = state
                .desired
                .conflicts
                .iter()
                .filter(|conflict| {
                    conflict.semantic_path == semantic_path
                        || conflict
                            .semantic_path
                            .starts_with(&format!("{semantic_path}/"))
                        || semantic_path.starts_with(&format!("{}/", conflict.semantic_path))
                })
                .map(|conflict| {
                    format!(
                        "{}:{}",
                        conflict
                            .message_key
                            .as_deref()
                            .unwrap_or("CONFIGURATION_CONFLICT"),
                        conflict.semantic_path
                    )
                })
                .collect();
            ConfigurationLayerTrace {
                semantic_path,
                source_ids,
                source_value,
                guided_value,
                managed_value,
                raw_value: raw_decision
                    .and_then(|decision| intent_operation_value(&decision.operation)),
                effective_value,
                winner_layer,
                raw_decision_id: raw_decision.map(|decision| decision.decision_id.clone()),
                raw_decision_status: raw_decision.map(|decision| decision.status),
                issue_ids,
            }
        })
        .collect()
}

fn semantic_path_to_pointer(root: &Value, path: &[SemanticPathSegment]) -> Option<String> {
    let mut current = root;
    let mut pointer = String::new();
    for segment in path {
        match segment {
            SemanticPathSegment::Key { key } => {
                current = current.as_object()?.get(key)?;
                pointer.push('/');
                pointer.push_str(&escape_pointer(key));
            }
            SemanticPathSegment::Identity { field, value } => {
                let values = current.as_array()?;
                let index = values.iter().position(|item| {
                    item.get(field).and_then(Value::as_str) == Some(value.as_str())
                })?;
                current = values.get(index)?;
                pointer.push('/');
                pointer.push_str(&index.to_string());
            }
        }
    }
    Some(pointer)
}

fn trace_path_value(root: &Value, path: &[SemanticPathSegment]) -> Option<Value> {
    raw_conflict_path_value(root, path).ok().flatten().cloned()
}

fn collect_trace_paths(value: &Value, path: &mut SemanticPath, output: &mut Vec<SemanticPath>) {
    match value {
        Value::Object(object) if !object.is_empty() => {
            for (key, child) in object {
                path.push(SemanticPathSegment::Key { key: key.clone() });
                collect_trace_paths(child, path, output);
                path.pop();
            }
        }
        Value::Array(values) if sequence_identities(values).is_some() => {
            let identities = sequence_identities(values).expect("identity shape was checked");
            for ((field, identity), child) in identities.into_iter().zip(values) {
                path.push(SemanticPathSegment::Identity {
                    field,
                    value: identity,
                });
                collect_trace_paths(child, path, output);
                path.pop();
            }
        }
        Value::Array(values) => {
            if values.is_empty() {
                output.push(path.clone());
            } else {
                // Ordered identity-less lists are one semantic unit.  Their
                // provenance and Raw decisions are intentionally path-level.
                output.push(path.clone());
            }
        }
        _ => output.push(path.clone()),
    }
}

fn project_managed_integrations(
    kind: ProgramKind,
    effective: &Value,
    managed: &ManagedIntegrationIntent,
    raw: &RawManualIntent,
    conflicts: &[ConfigurationConflict],
) -> Vec<ManagedIntegrationProjection> {
    let integration_ids: &[&str] = match kind {
        ProgramKind::SingBox => &["dashboard.singBoxApi", "dashboard.singBoxClash"],
        ProgramKind::Xray => &["dashboard.xray"],
        ProgramKind::Mihomo => &["dashboard.mihomo"],
        ProgramKind::Generic => &[],
    };
    let targets = managed_ownership_targets(kind);
    integration_ids
        .iter()
        .map(|integration_id| {
            let integration_targets = targets
                .iter()
                .filter(|(_, integration, _)| integration == integration_id)
                .collect::<Vec<_>>();
            let raw_paths = visible_raw_operations(raw)
                .map(intent_operation_path)
                .filter(|raw_path| {
                    integration_targets
                        .iter()
                        .any(|(_, _, target)| managed_path_overlaps(target, raw_path))
                })
                .map(|path| display_semantic_path(&path))
                .collect::<Vec<_>>();
            let issue_ids = conflicts
                .iter()
                .filter(|conflict| conflict.scope.owner_id.as_deref() == Some(integration_id))
                .map(|conflict| {
                    format!(
                        "{}:{}",
                        conflict.message_key.as_deref().unwrap_or(&conflict.reason),
                        conflict.semantic_path
                    )
                })
                .collect::<Vec<_>>();
            let mut issue_ids = issue_ids;
            for decision in raw
                .decisions
                .iter()
                .filter(|decision| decision.status == RawDecisionStatus::Superseded)
            {
                let decision_path = intent_operation_path(&decision.operation);
                if integration_targets
                    .iter()
                    .any(|(_, _, target)| managed_path_overlaps(target, &decision_path))
                {
                    issue_ids.push(format!(
                        "RAW_DECISION_SUPERSEDED:{}",
                        display_semantic_path(&decision_path)
                    ));
                }
            }
            let intent_value = managed
                .values
                .iter()
                .find(|(key, _)| key.starts_with(&format!("{integration_id}.")))
                .map(|(_, value)| value.clone());
            let effective_enabled = integration_targets.iter().any(|(_, _, target)| {
                raw_conflict_path_value(effective, target)
                    .ok()
                    .flatten()
                    .is_some()
            });
            let status = if !issue_ids.is_empty()
                || (intent_value.is_some() && !raw_paths.is_empty())
            {
                ManagedIntegrationStatus::Overridden
            } else if intent_value.is_some() {
                ManagedIntegrationStatus::Explicit
            } else if !raw_paths.is_empty() && effective_enabled {
                ManagedIntegrationStatus::RawOnly
            } else if !effective_enabled && (!managed.values.is_empty() || !raw_paths.is_empty()) {
                ManagedIntegrationStatus::NeedsAttention
            } else {
                ManagedIntegrationStatus::Inactive
            };
            ManagedIntegrationProjection {
                integration_id: (*integration_id).into(),
                status,
                effective_enabled,
                intent_value,
                raw_paths,
                issue_ids,
            }
        })
        .collect()
}

fn managed_ownership_targets(kind: ProgramKind) -> Vec<(&'static str, &'static str, SemanticPath)> {
    let identity = |field: &'static str, value: &'static str| SemanticPathSegment::Identity {
        field: field.into(),
        value: value.into(),
    };
    match kind {
        ProgramKind::SingBox => vec![
            (
                "dashboard.singBoxApi.listenPort",
                "dashboard.singBoxApi",
                vec![
                    SemanticPathSegment::Key {
                        key: "services".into(),
                    },
                    identity("tag", MANAGED_SING_BOX_API_TAG),
                    SemanticPathSegment::Key { key: "type".into() },
                ],
            ),
            (
                "dashboard.singBoxApi.listenPort",
                "dashboard.singBoxApi",
                vec![
                    SemanticPathSegment::Key {
                        key: "services".into(),
                    },
                    identity("tag", MANAGED_SING_BOX_API_TAG),
                    SemanticPathSegment::Key { key: "tag".into() },
                ],
            ),
            (
                "dashboard.singBoxApi.listenPort",
                "dashboard.singBoxApi",
                vec![
                    SemanticPathSegment::Key {
                        key: "services".into(),
                    },
                    identity("tag", MANAGED_SING_BOX_API_TAG),
                    SemanticPathSegment::Key {
                        key: "listen_port".into(),
                    },
                ],
            ),
            (
                "dashboard.singBoxApi.listenPort",
                "dashboard.singBoxApi",
                vec![
                    SemanticPathSegment::Key {
                        key: "services".into(),
                    },
                    identity("tag", MANAGED_SING_BOX_API_TAG),
                    SemanticPathSegment::Key {
                        key: "listen".into(),
                    },
                ],
            ),
            (
                "dashboard.singBoxApi.updateInterval",
                "dashboard.singBoxApi",
                vec![
                    SemanticPathSegment::Key {
                        key: "services".into(),
                    },
                    identity("tag", MANAGED_SING_BOX_API_TAG),
                    SemanticPathSegment::Key {
                        key: "dashboard".into(),
                    },
                    SemanticPathSegment::Key {
                        key: "enabled".into(),
                    },
                ],
            ),
            (
                "dashboard.singBoxApi.updateInterval",
                "dashboard.singBoxApi",
                vec![
                    SemanticPathSegment::Key {
                        key: "services".into(),
                    },
                    identity("tag", MANAGED_SING_BOX_API_TAG),
                    SemanticPathSegment::Key {
                        key: "dashboard".into(),
                    },
                    SemanticPathSegment::Key {
                        key: "update_interval".into(),
                    },
                ],
            ),
            (
                "dashboard.singBoxClash.listenPort",
                "dashboard.singBoxClash",
                key_path(&["experimental", "clash_api", "external_controller"]),
            ),
            (
                "dashboard.singBoxClash.listenPort",
                "dashboard.singBoxClash",
                key_path(&["experimental", "clash_api", "external_ui"]),
            ),
            (
                "dashboard.singBoxClash.downloadUrl",
                "dashboard.singBoxClash",
                key_path(&["experimental", "clash_api", "external_ui_download_url"]),
            ),
        ],
        ProgramKind::Xray => vec![
            (
                "dashboard.xray.apiPort",
                "dashboard.xray",
                key_path(&["api", "tag"]),
            ),
            (
                "dashboard.xray.apiPort",
                "dashboard.xray",
                key_path(&["api", "listen"]),
            ),
            (
                "dashboard.xray.apiPort",
                "dashboard.xray",
                key_path(&["api", "services"]),
            ),
            (
                "dashboard.xray.metricsPort",
                "dashboard.xray",
                key_path(&["metrics", "tag"]),
            ),
            (
                "dashboard.xray.metricsPort",
                "dashboard.xray",
                key_path(&["metrics", "listen"]),
            ),
            (
                "dashboard.xray.apiPort",
                "dashboard.xray",
                key_path(&["stats"]),
            ),
            (
                "dashboard.xray.apiPort",
                "dashboard.xray",
                key_path(&["policy", "system", "statsInboundUplink"]),
            ),
            (
                "dashboard.xray.apiPort",
                "dashboard.xray",
                key_path(&["policy", "system", "statsInboundDownlink"]),
            ),
            (
                "dashboard.xray.apiPort",
                "dashboard.xray",
                key_path(&["policy", "system", "statsOutboundUplink"]),
            ),
            (
                "dashboard.xray.apiPort",
                "dashboard.xray",
                key_path(&["policy", "system", "statsOutboundDownlink"]),
            ),
        ],
        ProgramKind::Mihomo => vec![
            (
                "dashboard.mihomo.listenPort",
                "dashboard.mihomo",
                key_path(&["external-controller"]),
            ),
            (
                "dashboard.mihomo.listenPort",
                "dashboard.mihomo",
                key_path(&["external-ui"]),
            ),
            (
                "dashboard.mihomo.downloadUrl",
                "dashboard.mihomo",
                key_path(&["external-ui-url"]),
            ),
        ],
        ProgramKind::Generic => Vec::new(),
    }
}

fn managed_integration_active(managed: &ManagedIntegrationIntent, integration_id: &str) -> bool {
    let prefix = format!("{integration_id}.");
    managed.values.keys().any(|key| key.starts_with(&prefix))
}

/// A managed leaf is overridden by a Raw ancestor, or by the exact leaf.
/// Descendants of a managed leaf are intentionally not considered overlap so
/// extensions such as Clash `secret` remain user-owned Raw fields.
fn managed_path_overlaps(target: &[SemanticPathSegment], raw: &[SemanticPathSegment]) -> bool {
    raw.len() <= target.len() && raw.iter().zip(target).all(|(left, right)| left == right)
}

fn raw_operation_matches_managed_duration(
    setting: &str,
    target: &[SemanticPathSegment],
    operation: &IntentOperation,
    managed: &ManagedIntegrationIntent,
) -> bool {
    if setting != "dashboard.singBoxApi.updateInterval"
        || !is_sing_box_dashboard_interval_path(target)
        || intent_operation_path(operation) != target
    {
        return false;
    }
    let IntentOperation::Set { value, .. } = operation else {
        return false;
    };
    let Some(raw_value) = value.as_str() else {
        return false;
    };
    let managed_value = managed
        .values
        .get(setting)
        .and_then(Value::as_str)
        .unwrap_or("1d");
    parse_dashboard_interval_nanos(managed_value).is_some()
        && parse_dashboard_interval_nanos(managed_value)
            == parse_dashboard_interval_nanos(raw_value)
}

pub fn managed_raw_override_paths(
    kind: ProgramKind,
    managed: &ManagedIntegrationIntent,
    raw: &RawManualIntent,
) -> Vec<String> {
    let targets = managed_ownership_targets(kind)
        .into_iter()
        .filter(|(_, integration, _)| managed_integration_active(managed, integration))
        .collect::<Vec<_>>();
    let mut paths = visible_raw_operations(raw)
        .map(|operation| {
            let raw_path = intent_operation_path(operation);
            let is_override = targets.iter().any(|(setting, _, target)| {
                managed_path_overlaps(target, &raw_path)
                    && !raw_operation_matches_managed_duration(setting, target, operation, managed)
            });
            (is_override, raw_path)
        })
        .filter_map(|(is_override, path)| is_override.then_some(path))
        .map(|path| display_semantic_path(&path))
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

/// Remove only Raw semantics owned by an enabled Details integration.  A Set
/// on an ancestor object is trimmed so unrelated extension fields (for
/// example Clash `secret`) survive the takeover.  Destructive sequence
/// operations cannot be split safely and are removed as the single confirmed
/// overlapping operation.
pub fn remove_managed_raw_overrides(
    kind: ProgramKind,
    managed: &ManagedIntegrationIntent,
    raw: &mut RawManualIntent,
) -> usize {
    let targets = managed_ownership_targets(kind)
        .into_iter()
        .filter(|(_, integration, _)| managed_integration_active(managed, integration))
        .collect::<Vec<_>>();
    let mut removed = 0usize;
    let mut retained = Vec::new();
    for operation in std::mem::take(&mut raw.operations) {
        let operation_path = intent_operation_path(&operation);
        let overlapping = targets
            .iter()
            .filter(|(setting, _, target)| {
                managed_path_overlaps(target, &operation_path)
                    && !raw_operation_matches_managed_duration(setting, target, &operation, managed)
            })
            .collect::<Vec<_>>();
        if overlapping.is_empty() {
            retained.push(operation);
            continue;
        }
        removed = removed.saturating_add(1);
        let IntentOperation::Set { path, value } = operation else {
            continue;
        };
        let mut leaves = Vec::new();
        flatten_raw_set(&path, &value, &mut leaves);
        retained.extend(leaves.into_iter().filter(|leaf| {
            let leaf_path = intent_operation_path(leaf);
            !targets
                .iter()
                .any(|(_, _, target)| managed_path_overlaps(target, &leaf_path))
        }));
    }
    raw.operations = retained;
    raw.decisions.retain(|decision| {
        let operation_path = intent_operation_path(&decision.operation);
        let overlaps = targets.iter().any(|(setting, _, target)| {
            managed_path_overlaps(target, &operation_path)
                && !raw_operation_matches_managed_duration(
                    setting,
                    target,
                    &decision.operation,
                    managed,
                )
        });
        if overlaps {
            removed = removed.saturating_add(1);
        }
        !overlaps
    });
    removed
}

fn canonicalize_managed_raw_operations(
    kind: ProgramKind,
    generated: &Value,
    managed: &ManagedIntegrationIntent,
    raw: &mut RawManualIntent,
) {
    let targets = managed_ownership_targets(kind)
        .into_iter()
        .filter(|(_, integration, _)| managed_integration_active(managed, integration))
        .map(|(_, _, target)| target)
        .collect::<Vec<_>>();
    let mut canonical = Vec::new();
    for operation in std::mem::take(&mut raw.operations) {
        let path = intent_operation_path(&operation);
        if !targets
            .iter()
            .any(|target| managed_path_overlaps(target, &path))
        {
            canonical.push(operation);
            continue;
        }
        let mut operation_effective = generated.clone();
        if apply_intent_operation(&mut operation_effective, &operation).is_err() {
            canonical.push(operation);
            continue;
        }
        canonical.extend(diff_raw_intent(generated, &operation_effective).operations);
    }
    raw.operations = canonical;
}

fn flatten_raw_set(
    path: &[SemanticPathSegment],
    value: &Value,
    operations: &mut Vec<IntentOperation>,
) {
    match value {
        Value::Object(object) if !object.is_empty() => {
            for (key, child) in object {
                let mut child_path = path.to_vec();
                child_path.push(SemanticPathSegment::Key { key: key.clone() });
                flatten_raw_set(&child_path, child, operations);
            }
        }
        Value::Array(array) if sequence_identities(array).is_some() => {
            let identities = sequence_identities(array).expect("identity shape was checked");
            for ((field, identity), child) in identities.into_iter().zip(array) {
                let mut child_path = path.to_vec();
                child_path.push(SemanticPathSegment::Identity {
                    field,
                    value: identity,
                });
                flatten_raw_set(&child_path, child, operations);
            }
        }
        _ => operations.push(IntentOperation::Set {
            path: path.to_vec(),
            value: value.clone(),
        }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigurationLayer {
    Source,
    Intent,
    Details,
    RawDecision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CandidateSaveStatus {
    Blocked,
    Saved,
    PendingValidation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationLayerTrace {
    pub semantic_path: String,
    #[serde(default)]
    pub source_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guided_value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_value: Option<Value>,
    pub winner_layer: ConfigurationLayer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_decision_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_decision_status: Option<RawDecisionStatus>,
    #[serde(default)]
    pub issue_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawDecisionProjection {
    pub decision_id: String,
    pub semantic_path: String,
    pub operation: IntentOperation,
    pub status: RawDecisionStatus,
    pub origin: RawDecisionOrigin,
    pub basis: RawDecisionBasis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_value: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationWorkspaceView {
    pub upstream_document: String,
    pub final_preview_document: String,
    pub editable_document: String,
    #[serde(default)]
    pub layer_trace: Vec<ConfigurationLayerTrace>,
    #[serde(default)]
    pub raw_decisions: Vec<RawDecisionProjection>,
    #[serde(default)]
    pub source_conflicts: Vec<ConfigurationConflict>,
    #[serde(default)]
    pub layer_conflicts: Vec<ConfigurationConflict>,
    #[serde(default)]
    pub raw_conflicts: Vec<ConfigurationConflict>,
    #[serde(default)]
    pub diagnostics: Vec<ConfigurationDiagnostic>,
    pub save_status: CandidateSaveStatus,
    pub validation_status: CandidateValidationStatus,
    pub can_save: bool,
    pub can_validate: bool,
    pub can_apply: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationStateView {
    pub schema_version: u32,
    pub kind: ProgramKind,
    pub format: ConfigurationFormat,
    pub generation: u64,
    pub compatibility_profile: CoreCompatibilityProfile,
    pub source_statuses: Vec<SourceStatus>,
    #[serde(default)]
    pub source_parse_summaries: BTreeMap<String, SourceParseSummary>,
    pub provenance: Vec<ProvenanceEntry>,
    pub desired: ConfigurationCandidate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_revision: Option<ConfigurationRevision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_known_good_revision: Option<ConfigurationRevision>,
    pub guided_descriptors: Vec<GuidedSettingDescriptor>,
    pub guided_projection: Vec<GuidedProjection>,
    #[serde(default)]
    pub managed_integrations: Vec<ManagedIntegrationProjection>,
    #[serde(default)]
    pub compatibility_references: Vec<crate::CoreCompatibilityReference>,
    pub workspace: ConfigurationWorkspaceView,
}

pub fn parse_semantic_document(format: ConfigurationFormat, content: &[u8]) -> Result<Value> {
    let value = match format {
        ConfigurationFormat::Jsonc => {
            serde_json::from_slice(&normalize_jsonc(content)).map_err(|error| {
                CamelliaNexusError::new(ErrorCode::ConfigInvalid, "Configuration is not valid JSON")
                    .with_details(error.to_string())
            })?
        }
        ConfigurationFormat::Yaml => {
            let yaml: serde_yaml_ng::Value =
                serde_yaml_ng::from_slice(content).map_err(|error| {
                    CamelliaNexusError::new(
                        ErrorCode::ConfigInvalid,
                        "Configuration is not valid YAML",
                    )
                    .with_details(error.to_string())
                })?;
            serde_json::to_value(yaml).map_err(|error| {
                CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    "YAML configuration contains unsupported non-string keys",
                )
                .with_details(error.to_string())
            })?
        }
    };
    ensure_root_mapping(&value)?;
    Ok(value)
}

pub fn serialize_semantic_document(format: ConfigurationFormat, value: &Value) -> Result<String> {
    ensure_root_mapping(value)?;
    match format {
        ConfigurationFormat::Jsonc => serde_json::to_string_pretty(value).map_err(Into::into),
        ConfigurationFormat::Yaml => serde_yaml_ng::to_string(value).map_err(|error| {
            CamelliaNexusError::new(
                ErrorCode::Internal,
                "Failed to serialize YAML configuration",
            )
            .with_details(error.to_string())
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MihomoDashboardSpec, SingBoxClashDashboardSpec, SingBoxDashboardSpec};

    fn snapshot(kind: ProgramKind, id: &str, content: &str) -> SourceSnapshot {
        SourceSnapshot::parse(
            id,
            id,
            ConfigurationFormat::for_kind(kind).expect("format"),
            content.as_bytes(),
            1,
            false,
        )
        .expect("snapshot")
    }

    fn compatibility_profile(kind: ProgramKind) -> CoreCompatibilityProfile {
        let fingerprint = crate::CoreBinaryFingerprint {
            sha256: "a".repeat(64),
            size: 1,
            modified_unix_ms: 1,
        };
        CoreCompatibilityProfile::resolve(
            &CoreTargetIdentity::unknown(kind, None).bind_fingerprint(&fingerprint),
        )
        .expect("compatibility profile")
    }

    fn validation_evidence(state: &ConfigurationState) -> CoreValidationEvidence {
        CoreValidationEvidence {
            binary_sha256: state
                .compatibility_profile
                .target
                .fingerprint_sha256
                .clone()
                .expect("fingerprint"),
            profile_hash: state.compatibility_profile.profile_hash.clone(),
            config_hash: state.desired.revision.content_hash.clone(),
            validator_contract_revision: "test-validator-v1".into(),
            native_accepted: true,
            validated_unix_ms: 1,
        }
    }

    fn provenance_at<'a>(
        result: &'a SemanticMergeResult,
        semantic_path: &str,
    ) -> &'a ProvenanceEntry {
        result
            .provenance
            .iter()
            .find(|entry| entry.semantic_path == semantic_path)
            .unwrap_or_else(|| panic!("missing provenance for {semantic_path}"))
    }

    #[test]
    fn sing_box_merge_matches_early_leaf_and_array_append_semantics() {
        let result = merge_configuration_sources(
            ProgramKind::SingBox,
            &[
                snapshot(
                    ProgramKind::SingBox,
                    "a",
                    r#"{"log":{"level":"info"},"outbounds":[{"tag":"same","type":"direct"}]}"#,
                ),
                snapshot(
                    ProgramKind::SingBox,
                    "b",
                    r#"{"log":{"level":"debug","timestamp":true},"outbounds":[{"tag":"same","type":"block"}]}"#,
                ),
            ],
        )
        .expect("merge");
        let value = parse_semantic_document(ConfigurationFormat::Jsonc, result.content.as_bytes())
            .expect("value");
        assert_eq!(value["log"]["level"], "info");
        assert_eq!(value["log"]["timestamp"], true);
        assert_eq!(value["outbounds"].as_array().expect("array").len(), 2);
        assert_eq!(
            provenance_at(&result, "/log/level").source_ids,
            vec!["a".to_owned()]
        );
        assert_eq!(
            provenance_at(&result, "/log/timestamp").source_ids,
            vec!["b".to_owned()]
        );
        assert_eq!(
            provenance_at(&result, "/outbounds/0/type").source_ids,
            vec!["a".to_owned()]
        );
        assert_eq!(
            provenance_at(&result, "/outbounds/1/type").source_ids,
            vec!["b".to_owned()]
        );
        assert!(result.conflicts.iter().any(|conflict| {
            conflict.semantic_path == "/log/level"
                && conflict.message_key.as_deref() == Some("SOURCE_VALUE_CONFLICT")
                && conflict.scope.surface == ConfigurationSurface::Sources
        }));
        assert!(result.conflicts.iter().any(|conflict| {
            conflict.semantic_path == "/outbounds[tag=same]/type"
                && conflict.message_key.as_deref() == Some("SOURCE_VALUE_CONFLICT")
        }));
    }

    #[test]
    fn identical_and_disjoint_source_values_do_not_create_conflicts() {
        let result = merge_configuration_sources(
            ProgramKind::SingBox,
            &[
                snapshot(
                    ProgramKind::SingBox,
                    "a",
                    r#"{"log":{"level":"info"},"dns":{"strategy":"prefer_ipv4"}}"#,
                ),
                snapshot(
                    ProgramKind::SingBox,
                    "b",
                    r#"{"log":{"level":"info"},"route":{"final":"direct"}}"#,
                ),
            ],
        )
        .expect("merge");
        assert!(result.conflicts.is_empty());
    }

    #[test]
    fn semantically_equal_dashboard_durations_do_not_create_source_conflicts() {
        let service = |duration: &str| {
            format!(
                r#"{{"services":[{{"type":"derp","tag":"{MANAGED_SING_BOX_API_TAG}","dashboard":{{"update_interval":"{duration}"}}}}]}}"#
            )
        };
        let result = merge_configuration_sources(
            ProgramKind::SingBox,
            &[
                snapshot(ProgramKind::SingBox, "a", &service("1d")),
                snapshot(ProgramKind::SingBox, "b", &service("24h0m0s")),
            ],
        )
        .expect("merge");
        assert!(result.conflicts.is_empty());
    }

    #[test]
    fn xray_merge_replaces_sections_merges_env_and_respects_tail() {
        let mut tail = snapshot(
            ProgramKind::Xray,
            "tail",
            r#"{"env":{"B":"2"},"routing":{"domainStrategy":"IPIfNonMatch"},"outbounds":[{"tag":"new"},{"tag":"same","protocol":"block"}]}"#,
        );
        tail.append_xray_outbounds = true;
        let result = merge_configuration_sources(
            ProgramKind::Xray,
            &[
                snapshot(
                    ProgramKind::Xray,
                    "base",
                    r#"{"env":{"A":"1"},"routing":{"domainStrategy":"AsIs"},"outbounds":[{"tag":"same","protocol":"freedom"}]}"#,
                ),
                tail,
            ],
        )
        .expect("merge");
        let value = parse_semantic_document(ConfigurationFormat::Jsonc, result.content.as_bytes())
            .expect("value");
        assert_eq!(value["env"], serde_json::json!({"A":"1","B":"2"}));
        assert_eq!(value["routing"]["domainStrategy"], "IPIfNonMatch");
        assert_eq!(value["outbounds"][0]["tag"], "same");
        assert_eq!(value["outbounds"][0]["protocol"], "block");
        assert_eq!(value["outbounds"][1]["tag"], "new");
        assert_eq!(
            provenance_at(&result, "/env/A").source_ids,
            vec!["base".to_owned()]
        );
        assert_eq!(
            provenance_at(&result, "/env/B").source_ids,
            vec!["tail".to_owned()]
        );
        assert_eq!(
            provenance_at(&result, "/routing/domainStrategy").source_ids,
            vec!["tail".to_owned()]
        );
        assert_eq!(
            provenance_at(&result, "/outbounds/0/protocol").source_ids,
            vec!["tail".to_owned()]
        );
        assert!(result.conflicts.iter().any(|conflict| {
            conflict.semantic_path == "/routing/domainStrategy"
                && conflict.message_key.as_deref() == Some("SOURCE_VALUE_CONFLICT")
        }));
        assert!(result.conflicts.iter().any(|conflict| {
            conflict.semantic_path == "/outbounds[tag=same]/protocol"
                && conflict.message_key.as_deref() == Some("SOURCE_VALUE_CONFLICT")
        }));
    }

    #[test]
    fn xray_disjoint_section_fields_merge_without_conflict() {
        let result = merge_configuration_sources(
            ProgramKind::Xray,
            &[
                snapshot(
                    ProgramKind::Xray,
                    "a",
                    r#"{"routing":{"domainStrategy":"AsIs"}}"#,
                ),
                snapshot(
                    ProgramKind::Xray,
                    "b",
                    r#"{"routing":{"domainMatcher":"hybrid"}}"#,
                ),
            ],
        )
        .expect("merge");
        let value = parse_semantic_document(ConfigurationFormat::Jsonc, result.content.as_bytes())
            .expect("value");
        assert_eq!(value["routing"]["domainStrategy"], "AsIs");
        assert_eq!(value["routing"]["domainMatcher"], "hybrid");
        assert!(result.conflicts.is_empty());
    }

    #[test]
    fn mihomo_merge_distinguishes_named_rules_and_option_lists() {
        let result = merge_configuration_sources(
            ProgramKind::Mihomo,
            &[
                snapshot(
                    ProgramKind::Mihomo,
                    "a",
                    "proxies:\n  - name: p\n    type: direct\nrules: [A]\ndns:\n  nameserver: [1.1.1.1]\n",
                ),
                snapshot(
                    ProgramKind::Mihomo,
                    "b",
                    "proxies:\n  - name: p\n    type: reject\n  - name: q\n    type: direct\nrules: [B]\ndns:\n  nameserver: [8.8.8.8]\n",
                ),
            ],
        )
        .expect("merge");
        let value = parse_semantic_document(ConfigurationFormat::Yaml, result.content.as_bytes())
            .expect("value");
        assert_eq!(value["proxies"][0]["type"], "reject");
        assert_eq!(value["proxies"][1]["name"], "q");
        assert_eq!(value["rules"], serde_json::json!(["A", "B"]));
        assert_eq!(value["dns"]["nameserver"], serde_json::json!(["8.8.8.8"]));
        assert_eq!(
            provenance_at(&result, "/proxies/0/type").source_ids,
            vec!["b".to_owned()]
        );
        assert_eq!(
            provenance_at(&result, "/rules/0").source_ids,
            vec!["a".to_owned()]
        );
        assert_eq!(
            provenance_at(&result, "/rules/1").source_ids,
            vec!["b".to_owned()]
        );
        assert_eq!(
            provenance_at(&result, "/dns/nameserver/0").source_ids,
            vec!["b".to_owned()]
        );
    }

    #[test]
    fn provenance_tracks_guided_and_raw_layers_without_losing_source_origin() {
        let merge = merge_configuration_sources(
            ProgramKind::SingBox,
            &[snapshot(
                ProgramKind::SingBox,
                "source",
                r#"{"log":{"level":"info"},"outbounds":[{"tag":"direct","type":"direct"}]}"#,
            )],
        )
        .expect("merge");
        let mut state = ConfigurationState::from_merge(
            ProgramKind::SingBox,
            1,
            1,
            merge,
            compatibility_profile(ProgramKind::SingBox),
        )
        .expect("state");
        state
            .guided_intent
            .set("logging.level", Value::String("debug".into()));
        state.rebuild_desired(2).expect("guided rebuild");
        state
            .replace_raw_from_edited(
                br#"{"log":{"level":"debug"},"outbounds":[{"tag":"direct","type":"block"}]}"#,
                3,
            )
            .expect("raw rebuild");

        let log = state
            .provenance
            .iter()
            .find(|entry| entry.semantic_path == "/log/level")
            .expect("log provenance");
        assert_eq!(log.source_ids, vec!["source".to_owned()]);
        assert!(log.guided);
        assert!(!log.raw);

        let outbound = state
            .provenance
            .iter()
            .find(|entry| entry.semantic_path == "/outbounds/0/type")
            .expect("outbound provenance");
        assert_eq!(outbound.source_ids, vec!["source".to_owned()]);
        assert!(!outbound.guided);
        assert!(outbound.raw);
    }

    #[test]
    fn raw_identity_delete_survives_a_new_base() {
        let old = serde_json::json!({"outbounds":[{"tag":"a"},{"tag":"b"}]});
        let edited = serde_json::json!({"outbounds":[{"tag":"b"}]});
        let intent = diff_raw_intent(&old, &edited);
        let new =
            serde_json::json!({"outbounds":[{"tag":"a","type":"direct"},{"tag":"b"},{"tag":"c"}]});
        let (effective, conflicts) = apply_raw_intent(&new, &intent);
        assert!(conflicts.is_empty());
        assert_eq!(
            effective["outbounds"],
            serde_json::json!([{"tag":"b"},{"tag":"c"}])
        );
    }

    #[test]
    fn deleting_all_guided_fields_is_only_regular_raw_delete_operations() {
        let base = serde_json::json!({
            "log": {"loglevel": "info"},
            "routing": {"domainStrategy": "AsIs"},
            "unknown": {"keep": true}
        });
        let edited = serde_json::json!({"unknown": {"keep": true}});
        let intent = diff_raw_intent(&base, &edited);
        assert!(
            intent
                .operations
                .iter()
                .all(|operation| matches!(operation, IntentOperation::Delete { .. }))
        );
        assert_eq!(intent.operations.len(), 2);

        let updated_base = serde_json::json!({
            "log": {"loglevel": "debug"},
            "routing": {"domainStrategy": "IPIfNonMatch"},
            "unknown": {"keep": true, "new": 1}
        });
        let (effective, conflicts) = apply_raw_intent(&updated_base, &intent);
        assert!(conflicts.is_empty());
        assert_eq!(
            effective["unknown"],
            serde_json::json!({"keep": true, "new": 1})
        );
        assert!(effective["log"].get("loglevel").is_none());
        assert!(effective["routing"].get("domainStrategy").is_none());
    }

    #[test]
    fn identityless_sequence_conflicts_instead_of_guessing_after_source_change() {
        let old = serde_json::json!({"rules":["A","B"]});
        let edited = serde_json::json!({"rules":["A","C"]});
        let intent = diff_raw_intent(&old, &edited);
        let new = serde_json::json!({"rules":["A","B","D"]});
        let (effective, conflicts) = apply_raw_intent(&new, &intent);
        assert_eq!(effective, new);
        assert_eq!(conflicts.len(), 1);
    }

    #[test]
    fn semantic_three_way_rebase_merges_disjoint_changes_and_detects_same_path_conflict() {
        let base = serde_json::json!({"a":1,"b":1});
        let user = serde_json::json!({"a":9,"b":1});
        let updated = serde_json::json!({"a":1,"b":2});
        let result = rebase_raw_document(&base, &user, &updated);
        assert!(result.conflicts.is_empty());
        assert_eq!(result.document, serde_json::json!({"a":9,"b":2}));

        let user = serde_json::json!({"a":3,"b":1});
        let updated = serde_json::json!({"a":2,"b":1});
        let result = rebase_raw_document(&base, &user, &updated);
        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].semantic_path, "/a");
        assert_eq!(result.document["a"], 3);
    }

    #[test]
    fn semantic_three_way_rebase_uses_identity_for_add_delete_and_nested_updates() {
        let base = serde_json::json!({
            "outbounds": [{"tag":"a","server":"old"},{"tag":"b","server":"b"}]
        });
        let user = serde_json::json!({
            "outbounds": [{"tag":"a","server":"mine"},{"tag":"b","server":"b"},{"tag":"c","server":"c"}]
        });
        let updated = serde_json::json!({
            "outbounds": [{"tag":"a","server":"new"},{"tag":"b","server":"b"}]
        });
        let result = rebase_raw_document(&base, &user, &updated);
        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(
            result.conflicts[0].segments,
            vec![
                SemanticPathSegment::Key {
                    key: "outbounds".into(),
                },
                SemanticPathSegment::Identity {
                    field: "tag".into(),
                    value: "a".into(),
                },
                SemanticPathSegment::Key {
                    key: "server".into(),
                },
            ]
        );
        assert_eq!(
            result.conflicts[0].semantic_path,
            "/outbounds[tag=a]/server"
        );
        assert_eq!(result.document["outbounds"][0]["server"], "mine");
        assert_eq!(result.document["outbounds"][2]["tag"], "c");
    }

    #[test]
    fn raw_conflict_resolution_uses_structured_identity_and_delete_semantics() {
        let original = serde_json::json!({
            "outbounds": [
                {"tag": "direct", "type": "direct"},
                {"tag": "edge", "server": "old.example"}
            ]
        });
        let user = serde_json::json!({
            "outbounds": [{"tag": "direct", "type": "direct"}]
        });
        let updated = serde_json::json!({
            "outbounds": [
                {"tag": "direct", "type": "direct"},
                {"tag": "edge", "server": "new.example"}
            ]
        });
        let rebased = rebase_raw_document(&original, &user, &updated);
        let conflict = rebased.conflicts[0].clone();
        assert_eq!(conflict.conflict_type, "delete-vs-modify");
        assert!(matches!(
            conflict.segments.as_slice(),
            [
                SemanticPathSegment::Key { key },
                SemanticPathSegment::Identity { field, value }
            ] if key == "outbounds" && field == "tag" && value == "edge"
        ));
        let mut draft = RawDraftSession {
            session_id: "identity-delete".into(),
            draft_revision: 1,
            based_on_generation: 2,
            base_content: serialize_semantic_document(ConfigurationFormat::Jsonc, &updated)
                .expect("base"),
            user_content: serialize_semantic_document(ConfigurationFormat::Jsonc, &user)
                .expect("user"),
            working_content: serialize_semantic_document(
                ConfigurationFormat::Jsonc,
                &rebased.document,
            )
            .expect("working"),
            conflicts: rebased.conflicts,
            resolutions: BTreeMap::new(),
            unresolved_conflict_ids: vec![conflict.conflict_id.clone()],
            updated_unix_ms: 1,
        };

        resolve_raw_draft_conflict(
            &mut draft,
            ConfigurationFormat::Jsonc,
            &conflict.conflict_id,
            RawConflictResolution::KeepMine,
        )
        .expect("keep deletion");

        let resolved =
            parse_semantic_document(ConfigurationFormat::Jsonc, draft.working_content.as_bytes())
                .expect("resolved document");
        assert_eq!(resolved["outbounds"], user["outbounds"]);
        assert!(draft.unresolved_conflict_ids.is_empty());
        assert_eq!(draft.conflicts, vec![conflict]);
    }

    #[test]
    fn raw_conflict_resolution_state_tracks_editor_undo_and_redo() {
        let original = serde_json::json!({"route": {"final": "old"}});
        let user = serde_json::json!({"route": {"final": "mine"}});
        let updated = serde_json::json!({"route": {"final": "updated"}});
        let rebased = rebase_raw_document(&original, &user, &updated);
        let conflict = rebased.conflicts[0].clone();
        let mut draft = RawDraftSession {
            session_id: "undo-redo".into(),
            draft_revision: 1,
            based_on_generation: 2,
            base_content: serialize_semantic_document(ConfigurationFormat::Jsonc, &updated)
                .expect("base"),
            user_content: serialize_semantic_document(ConfigurationFormat::Jsonc, &user)
                .expect("user"),
            working_content: serialize_semantic_document(
                ConfigurationFormat::Jsonc,
                &rebased.document,
            )
            .expect("working"),
            conflicts: rebased.conflicts,
            resolutions: BTreeMap::new(),
            unresolved_conflict_ids: Vec::new(),
            updated_unix_ms: 1,
        };
        refresh_raw_draft_conflicts(&mut draft, ConfigurationFormat::Jsonc);
        assert_eq!(
            draft.unresolved_conflict_ids,
            std::slice::from_ref(&conflict.conflict_id)
        );

        resolve_raw_draft_conflict(
            &mut draft,
            ConfigurationFormat::Jsonc,
            &conflict.conflict_id,
            RawConflictResolution::UseUpdated,
        )
        .expect("resolve");
        let resolved_content = draft.working_content.clone();
        assert!(draft.unresolved_conflict_ids.is_empty());

        draft.working_content = draft.user_content.clone();
        refresh_raw_draft_conflicts(&mut draft, ConfigurationFormat::Jsonc);
        assert_eq!(
            draft.unresolved_conflict_ids,
            std::slice::from_ref(&conflict.conflict_id)
        );

        draft.working_content = resolved_content;
        refresh_raw_draft_conflicts(&mut draft, ConfigurationFormat::Jsonc);
        assert!(draft.unresolved_conflict_ids.is_empty());
        assert_eq!(draft.conflicts, vec![conflict]);
    }

    #[test]
    fn share_snapshot_keeps_partial_summary_and_builds_an_ordinary_source_fragment() {
        let snapshot = SourceSnapshot::parse_share(
            "share",
            "Share links",
            &CoreTargetIdentity::unknown(ProgramKind::Mihomo, None),
            b"hy2://password@example.com\nvmess://unsupported",
            1,
        )
        .expect("share snapshot");
        assert_eq!(
            snapshot
                .share_summary
                .as_ref()
                .expect("summary")
                .accepted_items,
            1
        );
        assert_eq!(snapshot.share_provenance.len(), 1);
        assert_eq!(
            snapshot.share_provenance[0].target.program,
            ProgramKind::Mihomo
        );
        assert_eq!(
            snapshot.share_provenance[0].translator_revision,
            crate::SHARE_TRANSLATOR_REVISION
        );
        let value = parse_semantic_document(ConfigurationFormat::Yaml, snapshot.content.as_bytes())
            .expect("fragment");
        assert_eq!(value["proxies"][0]["type"], "hysteria2");
        assert!(snapshot.raw_observation_ref.is_some());
    }

    #[test]
    fn invalid_desired_does_not_replace_applied_or_lkg() {
        let merge = merge_configuration_sources(
            ProgramKind::SingBox,
            &[snapshot(
                ProgramKind::SingBox,
                "a",
                r#"{"log":{"level":"info"}}"#,
            )],
        )
        .expect("merge");
        let mut state = ConfigurationState::from_merge(
            ProgramKind::SingBox,
            1,
            1,
            merge,
            compatibility_profile(ProgramKind::SingBox),
        )
        .expect("state");
        state
            .mark_validation(true, Vec::new(), Some(validation_evidence(&state)))
            .expect("validation");
        state.mark_applied().expect("apply");
        let applied = state.applied.clone();
        state
            .guided_intent
            .set("logging.level", Value::String("debug".into()));
        state.rebuild_desired(2).expect("rebuild");
        state
            .mark_validation(
                false,
                vec![ConfigurationDiagnostic {
                    code: "CORE_INVALID".into(),
                    message: "invalid".into(),
                    message_key: None,
                    scope: ConfigurationIssueScope::configuration(),
                    details: None,
                }],
                None,
            )
            .expect("invalid validation");
        assert_eq!(state.applied, applied);
        assert_eq!(state.last_known_good, applied);
        assert_ne!(
            state.desired.content,
            state.applied.expect("applied").content
        );
    }

    #[test]
    fn stale_native_evidence_is_rejected_before_apply() {
        let merge = merge_configuration_sources(
            ProgramKind::SingBox,
            &[snapshot(
                ProgramKind::SingBox,
                "source",
                r#"{"log":{"level":"info"}}"#,
            )],
        )
        .expect("merge");
        let mut state = ConfigurationState::from_merge(
            ProgramKind::SingBox,
            1,
            1,
            merge,
            compatibility_profile(ProgramKind::SingBox),
        )
        .expect("state");
        state
            .mark_validation(true, Vec::new(), Some(validation_evidence(&state)))
            .expect("validation");
        state
            .desired
            .validation_evidence
            .as_mut()
            .expect("evidence")
            .config_hash = "stale-config-hash".into();

        assert!(!state.view().workspace.can_apply);
        let error = state.mark_applied().expect_err("stale evidence must fail");
        assert_eq!(error.code, ErrorCode::ConfigConflict);
        assert!(state.applied.is_none());
        assert!(state.last_known_good.is_none());
    }

    #[test]
    fn guided_values_reject_wrong_types_and_unknown_options() {
        let wrong_toggle = validate_guided_value(
            ProgramKind::Mihomo,
            "tun.enabled",
            &Value::String("true".into()),
        )
        .expect_err("toggle must be boolean");
        assert_eq!(wrong_toggle.code, ErrorCode::ConfigInvalid);

        let wrong_option = validate_guided_value(
            ProgramKind::Xray,
            "routing.domainStrategy",
            &Value::String("Guess".into()),
        )
        .expect_err("select must use a supported option");
        assert_eq!(wrong_option.code, ErrorCode::ConfigInvalid);

        validate_guided_value(
            ProgramKind::Xray,
            "routing.domainStrategy",
            &Value::String("AsIs".into()),
        )
        .expect("known option");
    }

    #[test]
    fn guided_projection_marks_unrepresentable_source_values_as_custom() {
        let effective = serde_json::json!({"dns":{"enhanced-mode":{"advanced":true}}});
        let projection = project_guided_settings(
            ProgramKind::Mihomo,
            &effective,
            &GuidedIntent::default(),
            &RawManualIntent::default(),
        );
        let dns_mode = projection
            .iter()
            .find(|projection| projection.setting_id == "dns.mode")
            .expect("dns mode projection");
        assert_eq!(dns_mode.status, GuidedProjectionStatus::Custom);
        assert_eq!(dns_mode.value, Some(serde_json::json!({"advanced": true})));
        assert_eq!(dns_mode.intent_value, None);
    }

    #[test]
    fn managed_dashboard_intent_maps_each_core_without_dropping_unknown_fields() {
        let managed = ManagedConfigSpec {
            sing_box_dashboard: Some(SingBoxDashboardSpec {
                listen_port: 9090,
                update_interval: "1h".into(),
            }),
            sing_box_clash_dashboard: Some(SingBoxClashDashboardSpec {
                listen_port: 9091,
                download_url: Some("https://example.test/clash.zip".into()),
            }),
            ..ManagedConfigSpec::default()
        };
        let mut intent = GuidedIntent::default();
        sync_managed_dashboard_intent(&mut intent, &managed);
        let sing_box = apply_guided_intent(
            ProgramKind::SingBox,
            &serde_json::json!({
                "services": [{"tag":"user-service"}],
                "experimental": {"clash_api": {"secret":"keep"}}
            }),
            &intent,
        )
        .expect("sing-box dashboard intent");
        assert_eq!(sing_box["services"][0]["tag"], "user-service");
        assert_eq!(sing_box["services"][1]["listen_port"], 9090);
        assert_eq!(sing_box["experimental"]["clash_api"]["secret"], "keep");
        assert_eq!(
            sing_box["experimental"]["clash_api"]["external_controller"],
            "127.0.0.1:9091"
        );

        let xray_managed = ManagedConfigSpec {
            xray_dashboard: Some(XrayDashboardSpec {
                api_port: 10085,
                metrics_port: 11111,
            }),
            ..ManagedConfigSpec::default()
        };
        sync_managed_dashboard_intent(&mut intent, &xray_managed);
        let xray = apply_guided_intent(
            ProgramKind::Xray,
            &serde_json::json!({"api":{"services":["UserService"]},"custom":{"keep":true}}),
            &intent,
        )
        .expect("xray dashboard intent");
        assert_eq!(xray["api"]["listen"], "127.0.0.1:10085");
        assert_eq!(xray["custom"]["keep"], true);
        assert_eq!(xray["metrics"]["listen"], "127.0.0.1:11111");

        let mihomo_managed = ManagedConfigSpec {
            mihomo_dashboard: Some(MihomoDashboardSpec {
                listen_port: 9092,
                download_url: None,
            }),
            ..ManagedConfigSpec::default()
        };
        sync_managed_dashboard_intent(&mut intent, &mihomo_managed);
        let mihomo = apply_guided_intent(
            ProgramKind::Mihomo,
            &serde_json::json!({"secret":"keep"}),
            &intent,
        )
        .expect("mihomo dashboard intent");
        assert_eq!(mihomo["secret"], "keep");
        assert_eq!(mihomo["external-controller"], "127.0.0.1:9092");
        assert_eq!(mihomo["external-ui"], "camellia-nexus-mihomo-dashboard");
    }

    #[test]
    fn upstream_managed_change_supersedes_a_prior_raw_decision() {
        let merge = merge_configuration_sources(
            ProgramKind::Mihomo,
            &[snapshot(
                ProgramKind::Mihomo,
                "base",
                "external-controller: 127.0.0.1:9090\n",
            )],
        )
        .expect("merge");
        let mut state = ConfigurationState::from_merge(
            ProgramKind::Mihomo,
            1,
            1,
            merge,
            compatibility_profile(ProgramKind::Mihomo),
        )
        .expect("state");
        state
            .replace_raw_from_edited(b"external-controller: 127.0.0.1:9999\n", 2)
            .expect("Raw decision");
        assert_eq!(state.raw_intent.decisions.len(), 1);
        assert_eq!(state.desired.validation, CandidateValidationStatus::Pending);

        sync_managed_dashboard_intent(
            &mut state.managed_intent,
            &ManagedConfigSpec {
                mihomo_dashboard: Some(MihomoDashboardSpec {
                    listen_port: 9092,
                    download_url: None,
                }),
                ..ManagedConfigSpec::default()
            },
        );
        state.rebuild_desired(3).expect("rebuild");

        assert_eq!(state.desired.validation, CandidateValidationStatus::Invalid);
        assert_eq!(state.desired.conflicts.len(), 1);
        assert_eq!(
            state.desired.conflicts[0].message_key.as_deref(),
            Some("RAW_DECISION_SUPERSEDED")
        );
        assert_eq!(
            parse_semantic_document(state.format, state.desired.content.as_bytes()).unwrap()["external-controller"],
            "127.0.0.1:9092"
        );
        assert_eq!(
            state.raw_intent.decisions[0].status,
            RawDecisionStatus::Superseded
        );
        assert_eq!(
            state.desired.conflicts[0].scope,
            ConfigurationIssueScope {
                surface: ConfigurationSurface::Configuration,
                owner_id: Some(state.raw_intent.decisions[0].decision_id.clone()),
            }
        );
        assert!(state.guided_intent.values.is_empty());
        assert!(
            state
                .managed_intent
                .values
                .contains_key("dashboard.mihomo.listenPort")
        );
    }

    #[test]
    fn sing_box_dashboard_duration_formats_are_semantically_equivalent() {
        let base = serde_json::json!({
            "services": [{
                "type": "api",
                "tag": MANAGED_SING_BOX_API_TAG,
                "dashboard": {"enabled": true, "update_interval": "1d"}
            }]
        });
        let edited = serde_json::json!({
            "services": [{
                "type": "api",
                "tag": MANAGED_SING_BOX_API_TAG,
                "dashboard": {"enabled": true, "update_interval": "24h0m0s"}
            }]
        });
        assert!(diff_raw_intent(&base, &edited).operations.is_empty());

        let mut managed = ManagedIntegrationIntent::default();
        managed.set("dashboard.singBoxApi.listenPort", Value::from(9090));
        managed.set(
            "dashboard.singBoxApi.updateInterval",
            Value::String("1d".into()),
        );
        let mut generated = serde_json::json!({});
        apply_dashboard_intent(ProgramKind::SingBox, &mut generated, &managed).unwrap();
        assert_eq!(
            generated["services"][0]["dashboard"]["update_interval"],
            "24h0m0s"
        );
        let equivalent = RawManualIntent {
            based_on_revision: None,
            operations: vec![IntentOperation::Set {
                path: vec![
                    SemanticPathSegment::Key {
                        key: "services".into(),
                    },
                    SemanticPathSegment::Identity {
                        field: "tag".into(),
                        value: MANAGED_SING_BOX_API_TAG.into(),
                    },
                    SemanticPathSegment::Key {
                        key: "dashboard".into(),
                    },
                    SemanticPathSegment::Key {
                        key: "update_interval".into(),
                    },
                ],
                value: Value::String("24h0m0s".into()),
            }],
            decisions: Vec::new(),
        };
        assert!(managed_raw_override_paths(ProgramKind::SingBox, &managed, &equivalent).is_empty());

        let non_equivalent = RawManualIntent {
            operations: vec![IntentOperation::Set {
                path: intent_operation_path(&equivalent.operations[0]),
                value: Value::String("25h".into()),
            }],
            ..equivalent.clone()
        };
        assert_eq!(
            managed_raw_override_paths(ProgramKind::SingBox, &managed, &non_equivalent),
            vec!["/services[tag=camellia-nexus-api]/dashboard/update_interval"]
        );
    }

    #[test]
    fn clash_secret_and_custom_service_do_not_override_managed_dashboard() {
        let merge = merge_configuration_sources(
            ProgramKind::SingBox,
            &[snapshot(ProgramKind::SingBox, "base", r#"{"experimental":{"clash_api":{"secret":"keep"}},"services":[{"type":"api","tag":"custom","listen_port":9000}]}"#)],
        )
        .expect("merge");
        let mut state = ConfigurationState::from_merge(
            ProgramKind::SingBox,
            1,
            1,
            merge,
            compatibility_profile(ProgramKind::SingBox),
        )
        .expect("state");
        sync_managed_dashboard_intent(
            &mut state.managed_intent,
            &ManagedConfigSpec {
                sing_box_clash_dashboard: Some(SingBoxClashDashboardSpec {
                    listen_port: 9091,
                    download_url: None,
                }),
                ..ManagedConfigSpec::default()
            },
        );
        state.raw_intent.operations.extend([
            IntentOperation::Set {
                path: key_path(&["experimental", "clash_api", "secret"]),
                value: Value::String("new-secret".into()),
            },
            IntentOperation::Set {
                path: vec![
                    SemanticPathSegment::Key {
                        key: "services".into(),
                    },
                    SemanticPathSegment::Identity {
                        field: "tag".into(),
                        value: "custom".into(),
                    },
                    SemanticPathSegment::Key {
                        key: "listen_port".into(),
                    },
                ],
                value: Value::from(9001),
            },
        ]);
        state.rebuild_desired(2).expect("rebuild");
        assert!(state.desired.conflicts.is_empty());
        assert_eq!(
            parse_semantic_document(state.format, state.desired.content.as_bytes()).unwrap()["experimental"]
                ["clash_api"]["secret"],
            "new-secret"
        );
    }

    #[test]
    fn managed_takeover_preserves_unowned_fields_from_an_ancestor_set() {
        let mut managed = ManagedIntegrationIntent::default();
        managed.set("dashboard.singBoxClash.listenPort", Value::from(9091));
        let mut raw = RawManualIntent {
            based_on_revision: None,
            operations: vec![IntentOperation::Set {
                path: key_path(&["experimental", "clash_api"]),
                value: serde_json::json!({
                    "external_controller": "127.0.0.1:9999",
                    "external_ui": "custom",
                    "secret": "keep"
                }),
            }],
            decisions: Vec::new(),
        };
        assert_eq!(
            remove_managed_raw_overrides(ProgramKind::SingBox, &managed, &mut raw),
            1
        );
        assert_eq!(raw.operations.len(), 1);
        assert_eq!(
            display_semantic_path(&intent_operation_path(&raw.operations[0])),
            "/experimental/clash_api/secret"
        );
        let mut generated = serde_json::json!({});
        apply_dashboard_intent(ProgramKind::SingBox, &mut generated, &managed).unwrap();
        let (effective, conflicts) = apply_raw_intent(&generated, &raw);
        assert!(conflicts.is_empty());
        assert_eq!(effective["experimental"]["clash_api"]["secret"], "keep");
        assert_eq!(
            effective["experimental"]["clash_api"]["external_controller"],
            "127.0.0.1:9091"
        );
    }

    #[test]
    fn xray_ownership_ignores_extensions_but_blocks_managed_containers() {
        let mut managed = ManagedIntegrationIntent::default();
        managed.set("dashboard.xray.apiPort", Value::from(10085));
        managed.set("dashboard.xray.metricsPort", Value::from(11111));
        let extensions = RawManualIntent {
            based_on_revision: None,
            operations: vec![
                IntentOperation::Set {
                    path: key_path(&["api", "custom"]),
                    value: Value::Bool(true),
                },
                IntentOperation::Set {
                    path: key_path(&["policy", "system", "custom"]),
                    value: Value::Bool(true),
                },
            ],
            decisions: Vec::new(),
        };
        assert!(managed_raw_override_paths(ProgramKind::Xray, &managed, &extensions).is_empty());

        let container = RawManualIntent {
            based_on_revision: None,
            operations: vec![IntentOperation::Delete {
                path: key_path(&["api"]),
            }],
            decisions: Vec::new(),
        };
        assert_eq!(
            managed_raw_override_paths(ProgramKind::Xray, &managed, &container),
            vec!["/api".to_owned()]
        );
    }

    #[test]
    fn raw_only_projection_is_owned_by_details_without_leaking_to_intent() {
        let merge = merge_configuration_sources(
            ProgramKind::Mihomo,
            &[snapshot(ProgramKind::Mihomo, "base", "mode: rule\n")],
        )
        .expect("merge");
        let mut state = ConfigurationState::from_merge(
            ProgramKind::Mihomo,
            1,
            1,
            merge,
            compatibility_profile(ProgramKind::Mihomo),
        )
        .expect("state");
        state.raw_intent.operations.push(IntentOperation::Set {
            path: key_path(&["external-controller"]),
            value: Value::String("127.0.0.1:9092".into()),
        });
        state.rebuild_desired(2).expect("rebuild");
        let view = state.view();
        assert!(view.desired.conflicts.is_empty());
        assert!(
            view.guided_projection
                .iter()
                .all(|projection| projection.setting_id != "dashboard.mihomo")
        );
        let projection = view
            .managed_integrations
            .iter()
            .find(|projection| projection.integration_id == "dashboard.mihomo")
            .expect("managed projection");
        assert_eq!(projection.status, ManagedIntegrationStatus::RawOnly);
        assert!(projection.effective_enabled);
    }

    #[test]
    fn v3_dashboard_values_migrate_without_touching_applied_or_lkg() {
        let merge = merge_configuration_sources(
            ProgramKind::Mihomo,
            &[snapshot(ProgramKind::Mihomo, "base", "mode: rule\n")],
        )
        .expect("merge");
        let mut state = ConfigurationState::from_merge(
            ProgramKind::Mihomo,
            1,
            1,
            merge,
            compatibility_profile(ProgramKind::Mihomo),
        )
        .expect("state");
        state.schema_version = LEGACY_MANAGED_CONFIGURATION_STATE_SCHEMA_VERSION;
        state
            .guided_intent
            .set("dashboard.mihomo.listenPort", Value::from(9092));
        let applied = state.applied.clone();
        let lkg = state.last_known_good.clone();
        assert!(state.migrate_legacy_schema());
        assert_eq!(state.schema_version, CONFIGURATION_STATE_SCHEMA_VERSION);
        assert!(state.guided_intent.values.is_empty());
        assert_eq!(
            state.managed_intent.values["dashboard.mihomo.listenPort"],
            9092
        );
        assert_eq!(state.applied, applied);
        assert_eq!(state.last_known_good, lkg);
    }

    #[test]
    fn v3_generated_dashboard_raw_container_is_canonicalized_without_false_override() {
        let merge = merge_configuration_sources(
            ProgramKind::SingBox,
            &[snapshot(
                ProgramKind::SingBox,
                "base",
                r#"{"experimental":{}}"#,
            )],
        )
        .expect("merge");
        let mut state = ConfigurationState::from_merge(
            ProgramKind::SingBox,
            1,
            1,
            merge,
            compatibility_profile(ProgramKind::SingBox),
        )
        .expect("state");
        state.schema_version = LEGACY_MANAGED_CONFIGURATION_STATE_SCHEMA_VERSION;
        state
            .guided_intent
            .set("dashboard.singBoxClash.listenPort", Value::from(9091));
        state.raw_intent.operations.push(IntentOperation::Set {
            path: key_path(&["experimental", "clash_api"]),
            value: serde_json::json!({
                "external_controller": "127.0.0.1:9091",
                "external_ui": MANAGED_SING_BOX_CLASH_UI,
                "secret": "keep"
            }),
        });

        assert!(state.migrate_legacy_schema());
        state
            .canonicalize_legacy_managed_raw()
            .expect("canonicalize");
        state.rebuild_desired(2).expect("rebuild");

        assert!(state.desired.conflicts.is_empty());
        assert!(state.raw_intent.operations.is_empty());
        assert_eq!(state.raw_intent.decisions.len(), 1);
        assert_eq!(
            display_semantic_path(&intent_operation_path(
                &state.raw_intent.decisions[0].operation
            )),
            "/experimental/clash_api/secret"
        );
    }

    #[test]
    fn raw_decision_resolution_is_path_scoped_and_reentrant() {
        let merge = merge_configuration_sources(
            ProgramKind::SingBox,
            &[snapshot(
                ProgramKind::SingBox,
                "base",
                r#"{"log":{"level":"info"},"dns":{"strategy":"prefer_ipv4"}}"#,
            )],
        )
        .expect("merge");
        let mut state = ConfigurationState::from_merge(
            ProgramKind::SingBox,
            1,
            1,
            merge,
            compatibility_profile(ProgramKind::SingBox),
        )
        .expect("state");
        state
            .replace_raw_from_edited(
                br#"{"log":{"level":"debug"},"dns":{"strategy":"prefer_ipv4"}}"#,
                2,
            )
            .expect("Raw decision");
        let decision_id = state.raw_intent.decisions[0].decision_id.clone();

        state
            .guided_intent
            .set("logging.level", Value::String("error".into()));
        state.rebuild_desired(3).expect("upstream change");
        assert_eq!(
            state.raw_intent.decisions[0].status,
            RawDecisionStatus::Superseded
        );
        assert_eq!(state.desired.validation, CandidateValidationStatus::Invalid);
        assert_eq!(
            state
                .view()
                .guided_projection
                .iter()
                .find(|projection| projection.setting_id == "logging.level")
                .map(|projection| projection.status),
            Some(GuidedProjectionStatus::RawDecision)
        );
        assert!(!state.view().workspace.can_save);
        assert_eq!(
            parse_semantic_document(state.format, state.desired.content.as_bytes()).unwrap()["log"]
                ["level"],
            "error"
        );

        state
            .resolve_raw_decision(&decision_id, RawDecisionResolution::KeepRaw, 4)
            .expect("keep Raw");
        assert!(state.desired.conflicts.is_empty());
        assert_eq!(
            state.raw_intent.decisions[0].status,
            RawDecisionStatus::Resolved
        );
        assert_eq!(
            parse_semantic_document(state.format, state.desired.content.as_bytes()).unwrap()["log"]
                ["level"],
            "debug"
        );

        state
            .guided_intent
            .set("dns.strategy", Value::String("prefer_ipv6".into()));
        state.rebuild_desired(5).expect("unrelated change");
        assert_eq!(
            state.raw_intent.decisions[0].status,
            RawDecisionStatus::Resolved
        );
        assert!(state.desired.conflicts.is_empty());

        state
            .guided_intent
            .set("logging.level", Value::String("warn".into()));
        state.rebuild_desired(6).expect("second upstream change");
        assert_eq!(
            state.raw_intent.decisions[0].status,
            RawDecisionStatus::Superseded
        );
        state
            .resolve_raw_decision(&decision_id, RawDecisionResolution::AcceptUpstream, 7)
            .expect("accept upstream");
        assert_eq!(
            state.raw_intent.decisions[0].status,
            RawDecisionStatus::Dormant
        );
        assert!(state.desired.conflicts.is_empty());
        assert_eq!(
            parse_semantic_document(state.format, state.desired.content.as_bytes()).unwrap()["log"]
                ["level"],
            "warn"
        );
        state.rebuild_desired(8).expect("repeat rebuild");
        assert!(state.desired.conflicts.is_empty());
    }

    #[test]
    fn raw_editor_accepting_upstream_dormants_only_that_decision() {
        let merge = merge_configuration_sources(
            ProgramKind::Xray,
            &[snapshot(
                ProgramKind::Xray,
                "base",
                r#"{"log":{"loglevel":"warning"},"routing":{"domainStrategy":"AsIs"}}"#,
            )],
        )
        .expect("merge");
        let mut state = ConfigurationState::from_merge(
            ProgramKind::Xray,
            1,
            1,
            merge,
            compatibility_profile(ProgramKind::Xray),
        )
        .expect("state");
        state
            .replace_raw_from_edited(
                br#"{"log":{"loglevel":"debug"},"routing":{"domainStrategy":"IPOnDemand"}}"#,
                2,
            )
            .expect("initial Raw decisions");
        assert_eq!(state.raw_intent.decisions.len(), 2);
        let routing_id = state
            .raw_intent
            .decisions
            .iter()
            .find(|decision| {
                display_semantic_path(&intent_operation_path(&decision.operation))
                    == "/routing/domainStrategy"
            })
            .map(|decision| decision.decision_id.clone())
            .expect("routing decision");

        state
            .replace_raw_from_edited(
                br#"{"log":{"loglevel":"debug"},"routing":{"domainStrategy":"AsIs"}}"#,
                3,
            )
            .expect("accept upstream through editor");

        let log = state
            .raw_intent
            .decisions
            .iter()
            .find(|decision| {
                display_semantic_path(&intent_operation_path(&decision.operation))
                    == "/log/loglevel"
            })
            .expect("log decision");
        let routing = state
            .raw_intent
            .decisions
            .iter()
            .find(|decision| decision.decision_id == routing_id)
            .expect("retained routing history");
        assert_eq!(log.status, RawDecisionStatus::Active);
        assert_eq!(routing.status, RawDecisionStatus::Dormant);
        assert_eq!(
            parse_semantic_document(state.format, state.desired.content.as_bytes()).unwrap()["routing"]
                ["domainStrategy"],
            "AsIs"
        );
        assert_eq!(
            parse_semantic_document(state.format, state.desired.content.as_bytes()).unwrap()["log"]
                ["loglevel"],
            "debug"
        );
    }

    #[test]
    fn repeated_identical_rebuild_preserves_generation_and_evidence() {
        let merge = merge_configuration_sources(
            ProgramKind::SingBox,
            &[snapshot(
                ProgramKind::SingBox,
                "source",
                r#"{"log":{"level":"info"}}"#,
            )],
        )
        .expect("merge");
        let mut state = ConfigurationState::from_merge(
            ProgramKind::SingBox,
            1,
            1,
            merge,
            compatibility_profile(ProgramKind::SingBox),
        )
        .expect("state");
        state
            .guided_intent
            .set("logging.level", Value::String("debug".into()));
        state.rebuild_desired(2).expect("initial rebuild");
        state
            .mark_validation(true, Vec::new(), Some(validation_evidence(&state)))
            .expect("validation");
        let generation = state.generation;
        let revision = state.desired.revision.clone();
        let evidence = state.desired.validation_evidence.clone();

        state.rebuild_desired(3).expect("identical rebuild");

        assert_eq!(state.generation, generation);
        assert_eq!(state.desired.revision, revision);
        assert_eq!(state.desired.validation_evidence, evidence);
        assert_eq!(state.desired.validation, CandidateValidationStatus::Valid);
    }

    #[test]
    fn repeated_identical_guided_value_does_not_drift_generation() {
        let merge = merge_configuration_sources(
            ProgramKind::Xray,
            &[snapshot(
                ProgramKind::Xray,
                "source",
                r#"{"log":{"loglevel":"warning"}}"#,
            )],
        )
        .expect("merge");
        let mut state = ConfigurationState::from_merge(
            ProgramKind::Xray,
            1,
            1,
            merge,
            compatibility_profile(ProgramKind::Xray),
        )
        .expect("state");
        state
            .guided_intent
            .set("logging.level", Value::String("debug".into()));
        state.rebuild_desired(2).expect("first Guided edit");
        let generation = state.generation;

        state
            .guided_intent
            .set("logging.level", Value::String("debug".into()));
        state.rebuild_desired(3).expect("same Guided edit");

        assert_eq!(state.generation, generation);
    }

    #[test]
    fn repeated_identical_raw_save_preserves_decision_and_generation() {
        let merge = merge_configuration_sources(
            ProgramKind::Xray,
            &[snapshot(
                ProgramKind::Xray,
                "source",
                r#"{"log":{"loglevel":"warning"}}"#,
            )],
        )
        .expect("merge");
        let mut state = ConfigurationState::from_merge(
            ProgramKind::Xray,
            1,
            1,
            merge,
            compatibility_profile(ProgramKind::Xray),
        )
        .expect("state");
        let edited = br#"{"log":{"loglevel":"debug"}}"#;
        state
            .replace_raw_from_edited(edited, 2)
            .expect("first Raw save");
        state
            .mark_validation(true, Vec::new(), Some(validation_evidence(&state)))
            .expect("validation");
        let generation = state.generation;
        let decision = state.raw_intent.decisions[0].clone();
        let evidence = state.desired.validation_evidence.clone();

        state
            .replace_raw_from_edited(edited, 3)
            .expect("identical Raw save");

        assert_eq!(state.generation, generation);
        assert_eq!(state.raw_intent.decisions, vec![decision]);
        assert_eq!(state.desired.validation_evidence, evidence);
    }

    #[test]
    fn recovered_source_status_reopens_an_identical_candidate() {
        let merge = merge_configuration_sources(
            ProgramKind::Xray,
            &[snapshot(
                ProgramKind::Xray,
                "source",
                r#"{"log":{"loglevel":"warning"}}"#,
            )],
        )
        .expect("merge");
        let mut state = ConfigurationState::from_merge(
            ProgramKind::Xray,
            1,
            1,
            merge,
            compatibility_profile(ProgramKind::Xray),
        )
        .expect("state");
        state
            .mark_validation(true, Vec::new(), Some(validation_evidence(&state)))
            .expect("initial validation");
        state.mark_applied().expect("initial apply");
        let applied = state.applied.clone();
        state.desired.validation = CandidateValidationStatus::Invalid;
        state.desired.validation_evidence = None;
        state.desired.diagnostics = vec![ConfigurationDiagnostic {
            code: "SOURCE_UNAVAILABLE".into(),
            message: "source unavailable".into(),
            message_key: Some("SOURCE_UNAVAILABLE".into()),
            scope: ConfigurationIssueScope::sources("source"),
            details: None,
        }];
        state.source_statuses.insert(
            "source".into(),
            SourceStatus {
                source_id: "source".into(),
                source_name: "Source".into(),
                freshness: SourceFreshness::Fresh,
                observed_hash: None,
                snapshot_hash: None,
                message: None,
                observed_unix_ms: Some(2),
            },
        );
        let generation = state.generation;

        state.rebuild_desired(3).expect("source recovery rebuild");

        assert_eq!(state.generation, generation + 1);
        assert_eq!(state.desired.validation, CandidateValidationStatus::Pending);
        assert!(state.desired.diagnostics.is_empty());
        assert_eq!(state.applied, applied);
        assert_eq!(state.last_known_good, applied);
    }

    #[test]
    fn identity_layer_trace_resolves_indexed_source_provenance() {
        let merge = merge_configuration_sources(
            ProgramKind::SingBox,
            &[snapshot(
                ProgramKind::SingBox,
                "source-a",
                r#"{"services":[{"tag":"first","type":"direct"},{"tag":"second","type":"direct","listen_port":8080}]}"#,
            )],
        )
        .expect("merge");
        let state = ConfigurationState::from_merge(
            ProgramKind::SingBox,
            1,
            1,
            merge,
            compatibility_profile(ProgramKind::SingBox),
        )
        .expect("state");
        let trace = state
            .view()
            .workspace
            .layer_trace
            .into_iter()
            .find(|trace| trace.semantic_path == "/services[tag=second]/listen_port")
            .expect("identity trace");

        assert_eq!(trace.source_ids, vec!["source-a".to_owned()]);
        assert_eq!(trace.source_value, Some(Value::from(8080)));
    }

    #[test]
    fn v4_raw_operations_migrate_to_basis_bound_decisions() {
        let merge = merge_configuration_sources(
            ProgramKind::Xray,
            &[snapshot(
                ProgramKind::Xray,
                "base",
                r#"{"log":{"loglevel":"warning"}}"#,
            )],
        )
        .expect("merge");
        let mut state = ConfigurationState::from_merge(
            ProgramKind::Xray,
            4,
            1,
            merge,
            compatibility_profile(ProgramKind::Xray),
        )
        .expect("state");
        state.schema_version = LEGACY_CONFIGURATION_STATE_SCHEMA_VERSION;
        state.raw_intent.operations.push(IntentOperation::Set {
            path: key_path(&["log", "loglevel"]),
            value: Value::String("debug".into()),
        });
        assert!(state.migrate_legacy_schema());
        state.rebuild_desired(2).expect("rebuild");
        assert_eq!(state.schema_version, CONFIGURATION_STATE_SCHEMA_VERSION);
        assert!(state.raw_intent.operations.is_empty());
        assert_eq!(state.raw_intent.decisions.len(), 1);
        assert_eq!(
            state.raw_intent.decisions[0].origin,
            RawDecisionOrigin::Migrated
        );
        assert_eq!(
            state.raw_intent.decisions[0].status,
            RawDecisionStatus::Active
        );
        assert_eq!(
            state.view().workspace.raw_decisions[0].semantic_path,
            "/log/loglevel"
        );
    }
}
