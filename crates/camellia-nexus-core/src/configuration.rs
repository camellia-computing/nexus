use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

mod conflict_operations;
mod operations;
mod upstream;
pub use conflict_operations::*;
pub use operations::*;
pub use upstream::{SourceUpdateKind, UpstreamState};

use crate::share_compatibility::{
    SourceItemProvenance, SourceParseSummary, TranslationFidelity, preview_share_import_for_version,
};
use crate::{
    CamelliaNexusError, CoreCompatibilityProfile, CoreTargetIdentity, CoreValidationEvidence,
    ErrorCode, ManagedConfigSpec, ProgramKind, Result, XrayDashboardSpec,
    config_service::hash_bytes, normalize_dashboard_interval, normalize_jsonc,
    parse_dashboard_interval_nanos,
};

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
    pub message_key: Option<String>,
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
            )
            .with_message_key("SOURCE_NO_COMPATIBLE_ITEMS"));
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
            )
            .with_message_key("SOURCE_NO_COMPATIBLE_ITEMS"));
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
    pub upstream: bool,
    pub final_edit: bool,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_ids: Vec<String>,
    /// Stable UI mapping key.  `reason` remains technical context for logs,
    /// while clients use this key for the localized explanation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_key: Option<String>,
    pub scope: ConfigurationIssueScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guided_value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_value: Option<Value>,
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
    let source_conflicts = collect_source_value_conflicts(kind, snapshots, &values);
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
    let conflicts = source_conflicts
        .into_iter()
        .map(|(path, mut conflict)| {
            conflict.effective_value = semantic_path_value(&merged, &path).ok().flatten().cloned();
            conflict
        })
        .collect();
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

type SourceConflictMap = BTreeMap<SemanticPath, ConfigurationConflict>;

/// Detect same-level Source conflicts independently from the deterministic
/// preview merge.  The preview remains useful while a conflict is being
/// repaired, but the returned blocking issues prevent Save/Validate/Apply.
/// Comparing the original source documents (instead of the progressively
/// merged preview) also keeps both owning source ids available to the UI.
fn collect_source_value_conflicts(
    kind: ProgramKind,
    snapshots: &[SourceSnapshot],
    values: &[Value],
) -> SourceConflictMap {
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
    let mut locations = conflicts.into_iter().collect::<Vec<_>>();
    locations.sort_by(|left, right| {
        left.0
            .len()
            .cmp(&right.0.len())
            .then_with(|| left.0.cmp(&right.0))
    });
    let mut aggregated = Vec::<(SemanticPath, ConfigurationConflict)>::new();
    for (path, conflict) in locations {
        if let Some((_, parent)) = aggregated
            .iter_mut()
            .find(|(parent_path, _)| path.starts_with(parent_path))
        {
            for source_id in conflict.source_ids {
                if !parent.source_ids.contains(&source_id) {
                    parent.source_ids.push(source_id);
                }
            }
        } else {
            aggregated.push((path, conflict));
        }
    }
    aggregated.into_iter().collect()
}

fn compare_source_values(
    kind: ProgramKind,
    left: &Value,
    right: &Value,
    path: &mut SemanticPath,
    left_source: &str,
    right_source: &str,
    conflicts: &mut SourceConflictMap,
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
        _ => insert_source_value_conflict(path, left, right, left_source, right_source, conflicts),
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
    conflicts: &mut SourceConflictMap,
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
    path: &[SemanticPathSegment],
    left: &Value,
    right: &Value,
    left_source: &str,
    right_source: &str,
    conflicts: &mut SourceConflictMap,
) {
    let path_key = path.to_vec();
    if let Some(conflict) = conflicts.get_mut(&path_key) {
        for source_id in [left_source, right_source] {
            if !conflict.source_ids.iter().any(|id| id == source_id) {
                conflict.source_ids.push(source_id.to_owned());
            }
        }
        return;
    }
    conflicts.insert(
        path_key,
        ConfigurationConflict {
            semantic_path: display_semantic_path(path),
            reason: format!(
                "Configuration sources {left_source} and {right_source} provide different values"
            ),
            severity: ConflictSeverity::Error,
            source_ids: vec![left_source.to_owned(), right_source.to_owned()],
            message_key: Some("SOURCE_VALUE_CONFLICT".into()),
            scope: ConfigurationIssueScope::sources(right_source.to_owned()),
            source_value: Some(left.clone()),
            guided_value: None,
            user_value: Some(right.clone()),
            effective_value: None,
        },
    );
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
            upstream: false,
            final_edit: false,
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
                    upstream: false,
                    final_edit: false,
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
                    upstream: false,
                    final_edit: false,
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
    Upstream,
    FinalEdit,
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
                            upstream: false,
                            final_edit: false,
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
                                upstream: false,
                                final_edit: false,
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
                                upstream: false,
                                final_edit: false,
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
        upstream: false,
        final_edit: false,
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
        result.upstream |= entry.upstream;
        result.final_edit |= entry.final_edit;
    }
    result
}

fn mark_provenance_layer(entry: &mut ProvenanceEntry, layer: ProvenanceLayer) {
    match layer {
        ProvenanceLayer::Upstream => entry.upstream = true,
        ProvenanceLayer::FinalEdit => entry.final_edit = true,
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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

/// A value at a semantic path.  `Missing` is distinct from JSON `null` and
/// allows additions and deletions to participate in the same three-way merge
/// as ordinary scalar edits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", content = "value", rename_all = "camelCase")]
pub enum SemanticValue {
    Missing,
    Present(Value),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpstreamBasis {
    pub content_hash: String,
    pub candidate_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FinalEditEntry {
    pub edit_id: String,
    pub path: SemanticPath,
    pub original: SemanticValue,
    pub edited: SemanticValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdoptUpstreamChangeRequest {
    pub operation_id: String,
    pub expected_state_revision: u64,
    pub editor_session_id: Option<String>,
    pub expected_draft_revision: Option<u64>,
    pub edit_id: String,
    pub path: SemanticPath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdoptUpstreamReceipt {
    pub operation_id: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FinalMergeConflictKind {
    AddVsAdd,
    ModifyVsModify,
    DeleteVsModify,
    ModifyVsDelete,
    Sequence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FinalMergeConflict {
    pub conflict_id: String,
    pub semantic_path: String,
    #[serde(rename = "segments")]
    pub path: SemanticPath,
    pub kind: FinalMergeConflictKind,
    pub base_value: SemanticValue,
    pub upstream_value: SemanticValue,
    pub user_value: SemanticValue,
    pub can_merge: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedCandidateMarker {
    pub content_hash: String,
    pub candidate_generation: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FinalEditState {
    pub basis: UpstreamBasis,
    /// Canonical semantic documents used for deterministic three-way merge.
    /// They are state-internal; the UI receives only the sparse `edits` and
    /// `conflicts` projections below.
    pub base_content: String,
    pub edited_content: String,
    pub edits: Vec<FinalEditEntry>,
    pub conflicts: Vec<FinalMergeConflict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_candidate: Option<SavedCandidateMarker>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum FinalConflictResolution {
    AcceptUpstream,
    KeepMine,
    ManualEdit { value: SemanticValue },
}

#[derive(Debug, Clone)]
struct MergeLocation {
    segments: SemanticPath,
    conflict_type: String,
    can_combine: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FinalRebaseResult {
    pub document: Value,
    #[serde(default)]
    pub conflicts: Vec<FinalMergeConflict>,
    pub deterministic_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FinalEditorSession {
    pub session_id: String,
    pub draft_revision: u64,
    pub based_on_state_revision: u64,
    pub based_on_candidate_generation: u64,
    pub base_content: String,
    pub working_content: String,
    pub conflicts: Vec<FinalMergeConflict>,
    pub resolutions: BTreeMap<String, FinalConflictResolution>,
    pub unresolved_conflict_ids: Vec<String>,
    pub rebase_required: bool,
    pub updated_unix_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuidedIntent {
    pub values: BTreeMap<String, Value>,
}

/// Settings owned by the Details surface (currently the managed dashboard
/// integrations).  Keeping this in a separate store is important: these
/// values are not Common Guided settings and must never create an Intent-page
/// conflict or projection entry.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedIntegrationIntent {
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
    FinalOnly,
    NeedsAttention,
    LatestSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedSettingProjection {
    pub setting_id: String,
    pub saved_value: SemanticValue,
    pub effective_value: SemanticValue,
    pub can_use_saved_value: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedIntegrationProjection {
    pub integration_id: String,
    pub status: ManagedIntegrationStatus,
    pub effective_enabled: bool,
    pub settings: Vec<ManagedSettingProjection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_value: Option<Value>,
    #[serde(default)]
    pub final_paths: Vec<String>,
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
    FinalEdit,
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
    Ok(effective)
}

/// Synchronize the Dashboard form into its dedicated Managed Integration
/// intent store.
pub fn sync_managed_dashboard_intent(
    intent: &mut ManagedIntegrationIntent,
    managed: &ManagedConfigSpec,
) {
    intent
        .values
        .retain(|setting, _| !setting.starts_with(DASHBOARD_INTENT_PREFIX));
    if let Some(dashboard) = managed.sing_box_dashboard.as_ref() {
        intent.values.insert(
            "dashboard.singBoxApi.listenPort".into(),
            Value::from(dashboard.listen_port),
        );
        intent.values.insert(
            "dashboard.singBoxApi.updateInterval".into(),
            Value::String(dashboard.update_interval.clone()),
        );
    }
    if let Some(dashboard) = managed.sing_box_clash_dashboard.as_ref() {
        intent.values.insert(
            "dashboard.singBoxClash.listenPort".into(),
            Value::from(dashboard.listen_port),
        );
        if let Some(url) = &dashboard.download_url {
            intent.values.insert(
                "dashboard.singBoxClash.downloadUrl".into(),
                Value::String(url.clone()),
            );
        }
    }
    if let Some(dashboard) = managed.xray_dashboard.as_ref() {
        sync_xray_dashboard_intent(intent, dashboard);
    }
    if let Some(dashboard) = managed.mihomo_dashboard.as_ref() {
        intent.values.insert(
            "dashboard.mihomo.listenPort".into(),
            Value::from(dashboard.listen_port),
        );
        if let Some(url) = &dashboard.download_url {
            intent.values.insert(
                "dashboard.mihomo.downloadUrl".into(),
                Value::String(url.clone()),
            );
        }
    }
}

fn sync_xray_dashboard_intent(
    intent: &mut ManagedIntegrationIntent,
    dashboard: &XrayDashboardSpec,
) {
    intent.values.insert(
        "dashboard.xray.apiPort".into(),
        Value::from(dashboard.api_port),
    );
    intent.values.insert(
        "dashboard.xray.metricsPort".into(),
        Value::from(dashboard.metrics_port),
    );
}

fn apply_dashboard_intent(
    kind: ProgramKind,
    root: &mut Value,
    intent: &ManagedIntegrationIntent,
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

fn intent_port(intent: &ManagedIntegrationIntent, id: &str) -> Result<Option<u16>> {
    let Some(value) = intent.values.get(id) else {
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

fn intent_text(intent: &ManagedIntegrationIntent, id: &str) -> Result<Option<String>> {
    let Some(value) = intent.values.get(id) else {
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

fn apply_sing_box_dashboard_intent(
    root: &mut Value,
    intent: &ManagedIntegrationIntent,
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

fn apply_sing_box_clash_dashboard_intent(
    root: &mut Value,
    intent: &ManagedIntegrationIntent,
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

fn apply_xray_dashboard_intent(root: &mut Value, intent: &ManagedIntegrationIntent) -> Result<()> {
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

fn apply_mihomo_dashboard_intent(
    root: &mut Value,
    intent: &ManagedIntegrationIntent,
) -> Result<()> {
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
    final_edit: &FinalEditState,
) -> Vec<GuidedProjection> {
    let final_paths = final_edit
        .edits
        .iter()
        .map(|entry| entry.path.clone())
        .chain(
            final_edit
                .conflicts
                .iter()
                .map(|conflict| conflict.path.clone()),
        )
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
                final_paths
                    .iter()
                    .any(|final_path| path_overlaps(&semantic, final_path))
            });
            GuidedProjection {
                setting_id: descriptor.id.clone(),
                status: if overridden {
                    GuidedProjectionStatus::FinalEdit
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

fn diff_intent_operations(base: &Value, edited: &Value) -> Vec<IntentOperation> {
    let mut operations = Vec::new();
    diff_value(base, edited, &mut Vec::new(), &mut operations);
    operations
}

fn semantic_document_hash(value: &Value) -> String {
    hash_bytes(
        serde_json::to_vec(value)
            .expect("semantic JSON values are serializable")
            .as_slice(),
    )
}

fn semantic_value_at(
    root: &Value,
    path: &[SemanticPathSegment],
) -> std::result::Result<SemanticValue, Box<ConfigurationConflict>> {
    Ok(match semantic_path_value(root, path)? {
        Some(value) => SemanticValue::Present(value.clone()),
        None => SemanticValue::Missing,
    })
}

fn semantic_values_equal_at(
    left: &SemanticValue,
    right: &SemanticValue,
    path: &[SemanticPathSegment],
) -> bool {
    match (left, right) {
        (SemanticValue::Missing, SemanticValue::Missing) => true,
        (SemanticValue::Present(left), SemanticValue::Present(right)) => {
            values_semantically_equal(left, right, path)
        }
        _ => false,
    }
}

fn apply_semantic_value(
    root: &mut Value,
    path: &[SemanticPathSegment],
    value: &SemanticValue,
) -> std::result::Result<(), Box<ConfigurationConflict>> {
    match value {
        SemanticValue::Missing => delete_semantic_path(root, path),
        SemanticValue::Present(value) => {
            set_semantic_path_creating_anchors(root, path, value.clone())
        }
    }
}

fn final_edit_id(
    path: &[SemanticPathSegment],
    original: &SemanticValue,
    edited: &SemanticValue,
) -> String {
    let encoded = serde_json::to_vec(&(path, original, edited))
        .expect("Final editor values are serializable");
    format!("edit-{}", &hash_bytes(&encoded)[..16])
}

fn final_edit_entries(base: &Value, edited: &Value) -> Result<Vec<FinalEditEntry>> {
    let operations = diff_intent_operations(base, edited);
    let mut entries = BTreeMap::<String, FinalEditEntry>::new();
    for operation in operations {
        let path = intent_operation_path(&operation);
        let original = semantic_value_at(base, &path).map_err(|conflict| {
            CamelliaNexusError::new(ErrorCode::ConfigConflict, conflict.reason)
        })?;
        let value = semantic_value_at(edited, &path).map_err(|conflict| {
            CamelliaNexusError::new(ErrorCode::ConfigConflict, conflict.reason)
        })?;
        if semantic_values_equal_at(&original, &value, &path) {
            continue;
        }
        let key = display_semantic_path(&path);
        entries.insert(
            key,
            FinalEditEntry {
                edit_id: final_edit_id(&path, &original, &value),
                path,
                original,
                edited: value,
            },
        );
    }
    Ok(entries.into_values().collect())
}

fn final_conflict_kind(
    base: &SemanticValue,
    upstream: &SemanticValue,
    user: &SemanticValue,
    draft_kind: &str,
) -> FinalMergeConflictKind {
    if draft_kind == "order" || draft_kind == "ordered-sequence" {
        return FinalMergeConflictKind::Sequence;
    }
    match (base, upstream, user) {
        (SemanticValue::Missing, SemanticValue::Present(_), SemanticValue::Present(_)) => {
            FinalMergeConflictKind::AddVsAdd
        }
        (_, SemanticValue::Missing, SemanticValue::Present(_)) => {
            FinalMergeConflictKind::ModifyVsDelete
        }
        (_, SemanticValue::Present(_), SemanticValue::Missing) => {
            FinalMergeConflictKind::DeleteVsModify
        }
        _ => FinalMergeConflictKind::ModifyVsModify,
    }
}

fn final_merge_conflict(
    draft_conflict: &MergeLocation,
    original: &Value,
    user: &Value,
    updated: &Value,
) -> Result<FinalMergeConflict> {
    let base_value = semantic_value_at(original, &draft_conflict.segments)
        .map_err(|conflict| CamelliaNexusError::new(ErrorCode::ConfigConflict, conflict.reason))?;
    let upstream_value = semantic_value_at(updated, &draft_conflict.segments)
        .map_err(|conflict| CamelliaNexusError::new(ErrorCode::ConfigConflict, conflict.reason))?;
    let user_value = semantic_value_at(user, &draft_conflict.segments)
        .map_err(|conflict| CamelliaNexusError::new(ErrorCode::ConfigConflict, conflict.reason))?;
    Ok(FinalMergeConflict {
        conflict_id: hash_bytes(
            format!(
                "{}|{:?}|{:?}",
                display_semantic_path(&draft_conflict.segments),
                base_value,
                user_value
            )
            .as_bytes(),
        )[..16]
            .into(),
        semantic_path: display_semantic_path(&draft_conflict.segments),
        path: draft_conflict.segments.clone(),
        kind: final_conflict_kind(
            &base_value,
            &upstream_value,
            &user_value,
            &draft_conflict.conflict_type,
        ),
        base_value,
        upstream_value,
        user_value,
        can_merge: draft_conflict.can_combine,
    })
}

fn aggregate_final_conflicts(mut conflicts: Vec<FinalMergeConflict>) -> Vec<FinalMergeConflict> {
    conflicts.sort_by(|left, right| {
        left.path.len().cmp(&right.path.len()).then_with(|| {
            display_semantic_path(&left.path).cmp(&display_semantic_path(&right.path))
        })
    });
    let mut result = Vec::<FinalMergeConflict>::new();
    for conflict in conflicts {
        if result
            .iter()
            .any(|parent| conflict.path.starts_with(&parent.path))
        {
            continue;
        }
        result.push(conflict);
    }
    result
}

fn initialize_final_edit_state(
    state: &mut FinalEditState,
    format: ConfigurationFormat,
    upstream: &Value,
    generation: u64,
) -> Result<()> {
    let content = serialize_semantic_document(format, upstream)?;
    state.basis = UpstreamBasis {
        content_hash: semantic_document_hash(upstream),
        candidate_generation: generation,
    };
    state.base_content = content.clone();
    state.edited_content = content;
    state.edits.clear();
    state.conflicts.clear();
    state.saved_candidate = None;
    Ok(())
}

fn reconcile_final_edits(
    state: &mut FinalEditState,
    format: ConfigurationFormat,
    upstream: &Value,
    generation: u64,
) -> Result<Value> {
    if state.base_content.is_empty() || state.edited_content.is_empty() {
        initialize_final_edit_state(state, format, upstream, generation)?;
        return Ok(upstream.clone());
    }
    let current_hash = semantic_document_hash(upstream);
    let original = parse_semantic_document(format, state.base_content.as_bytes())?;
    let user = parse_semantic_document(format, state.edited_content.as_bytes())?;
    let (mut document, mut conflicts) = if state.basis.content_hash == current_hash {
        (user.clone(), Vec::new())
    } else {
        let rebased = rebase_final_document(&original, &user, upstream);
        (rebased.document, rebased.conflicts)
    };

    // Unresolved conflicts keep the user's value out of the effective
    // candidate.  They survive further upstream refreshes until one side
    // converges or the user explicitly resolves the semantic path.
    for mut conflict in std::mem::take(&mut state.conflicts) {
        let updated = semantic_value_at(upstream, &conflict.path)
            .map_err(|value| CamelliaNexusError::new(ErrorCode::ConfigConflict, value.reason))?;
        if semantic_values_equal_at(&updated, &conflict.user_value, &conflict.path) {
            continue;
        }
        if semantic_values_equal_at(&updated, &conflict.base_value, &conflict.path) {
            apply_semantic_value(&mut document, &conflict.path, &conflict.user_value).map_err(
                |value| CamelliaNexusError::new(ErrorCode::ConfigConflict, value.reason),
            )?;
            continue;
        }
        conflict.upstream_value = updated;
        // The safe preview always follows the current upstream at an
        // unresolved path.
        apply_semantic_value(&mut document, &conflict.path, &conflict.upstream_value)
            .map_err(|value| CamelliaNexusError::new(ErrorCode::ConfigConflict, value.reason))?;
        conflicts.push(conflict);
    }
    let conflicts = aggregate_final_conflicts(conflicts);
    for conflict in &conflicts {
        apply_semantic_value(&mut document, &conflict.path, &conflict.upstream_value)
            .map_err(|value| CamelliaNexusError::new(ErrorCode::ConfigConflict, value.reason))?;
    }
    state.base_content = serialize_semantic_document(format, upstream)?;
    state.edited_content = serialize_semantic_document(format, &document)?;
    state.edits = final_edit_entries(upstream, &document)?;
    state.conflicts = conflicts;
    state.basis = UpstreamBasis {
        content_hash: current_hash,
        candidate_generation: generation,
    };
    Ok(document)
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

/// Performs a semantic three-way rebase of a Final editor document. The original
/// source snapshot, the user's working document and the newest source
/// snapshot are compared independently; formatting/key-order changes are
/// naturally ignored by the `Value` representation.  A conflict is emitted
/// only when both sides changed the same semantic unit differently.
pub fn rebase_final_document(
    original_base: &Value,
    user_document: &Value,
    updated_base: &Value,
) -> FinalRebaseResult {
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
    FinalRebaseResult {
        document,
        conflicts: aggregate_final_conflicts(
            conflicts
                .iter()
                .map(|conflict| {
                    final_merge_conflict(conflict, original_base, user_document, updated_base)
                        .expect(
                            "Merge locations refer to unambiguous paths in their input documents",
                        )
                })
                .collect(),
        ),
        deterministic_hash,
    }
}

pub fn resolve_final_editor_conflict(
    draft: &mut FinalEditorSession,
    format: ConfigurationFormat,
    conflict_id: &str,
    resolution: FinalConflictResolution,
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
    apply_semantic_value(
        &mut document,
        &conflict.path,
        &conflict_resolution_value(&conflict, &resolution),
    )
    .map_err(|conflict| CamelliaNexusError::new(ErrorCode::ConfigConflict, conflict.reason))?;
    draft.working_content = serialize_semantic_document(format, &document)?;
    draft
        .resolutions
        .insert(conflict.conflict_id.clone(), resolution);
    refresh_final_editor_conflicts(draft, format);
    Ok(())
}

pub fn refresh_final_editor_conflicts(draft: &mut FinalEditorSession, format: ConfigurationFormat) {
    let document = parse_semantic_document(format, draft.working_content.as_bytes()).ok();
    draft.unresolved_conflict_ids = draft
        .conflicts
        .iter()
        .filter(|conflict| {
            let Some(resolution) = draft.resolutions.get(&conflict.conflict_id) else {
                return true;
            };
            document.as_ref().is_none_or(|document| {
                !draft_conflict_resolution_matches(document, conflict, resolution).unwrap_or(false)
            })
        })
        .map(|conflict| conflict.conflict_id.clone())
        .collect();
}

fn conflict_resolution_value(
    conflict: &FinalMergeConflict,
    resolution: &FinalConflictResolution,
) -> SemanticValue {
    match resolution {
        FinalConflictResolution::KeepMine => conflict.user_value.clone(),
        FinalConflictResolution::AcceptUpstream => conflict.upstream_value.clone(),
        FinalConflictResolution::ManualEdit { value } => value.clone(),
    }
}

fn draft_conflict_resolution_matches(
    document: &Value,
    conflict: &FinalMergeConflict,
    resolution: &FinalConflictResolution,
) -> Result<bool> {
    let expected = conflict_resolution_value(conflict, resolution);
    let observed = semantic_value_at(document, &conflict.path)
        .map_err(|conflict| CamelliaNexusError::new(ErrorCode::ConfigConflict, conflict.reason))?;
    Ok(semantic_values_equal_at(
        &observed,
        &expected,
        &conflict.path,
    ))
}

/// Rebase editable text and unresolved choices using the same merge state as
/// saved Final edits. Invalid text remains recoverable without entering Desired.
pub fn rebase_final_editor_session(
    draft: &mut FinalEditorSession,
    format: ConfigurationFormat,
    updated_content: &str,
    state_revision: u64,
    generation: u64,
) -> Result<()> {
    refresh_final_editor_conflicts(draft, format);
    if draft.base_content == updated_content {
        draft.rebase_required = false;
        draft.based_on_state_revision = state_revision;
        draft.based_on_candidate_generation = generation;
        return Ok(());
    }
    let original = parse_semantic_document(format, draft.base_content.as_bytes())?;
    let upstream = parse_semantic_document(format, updated_content.as_bytes())?;
    if parse_semantic_document(format, draft.working_content.as_bytes()).is_err() {
        let mut changes = Vec::new();
        diff_value(&original, &upstream, &mut Vec::new(), &mut changes);
        draft.rebase_required = !changes.is_empty();
        if !draft.rebase_required {
            draft.base_content = updated_content.into();
            draft.based_on_state_revision = state_revision;
            draft.based_on_candidate_generation = generation;
        }
        return Ok(());
    }
    let mut edits = FinalEditState {
        basis: UpstreamBasis {
            content_hash: semantic_document_hash(&original),
            candidate_generation: draft.based_on_candidate_generation,
        },
        base_content: draft.base_content.clone(),
        edited_content: draft.working_content.clone(),
        edits: Vec::new(),
        conflicts: draft
            .conflicts
            .iter()
            .filter(|conflict| {
                draft
                    .unresolved_conflict_ids
                    .contains(&conflict.conflict_id)
            })
            .cloned()
            .collect(),
        saved_candidate: None,
    };
    reconcile_final_edits(&mut edits, format, &upstream, generation)?;
    draft.base_content = updated_content.into();
    draft.working_content = edits.edited_content;
    draft.conflicts = edits.conflicts;
    draft.resolutions.clear();
    draft.based_on_state_revision = state_revision;
    draft.based_on_candidate_generation = generation;
    draft.rebase_required = false;
    refresh_final_editor_conflicts(draft, format);
    Ok(())
}

fn semantic_path_value<'a>(
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

fn rebase_value(
    original: &Value,
    user: &Value,
    updated: &Value,
    path: &mut SemanticPath,
    conflicts: &mut Vec<MergeLocation>,
) -> Value {
    if values_semantically_equal(user, original, path) {
        return updated.clone();
    }
    if values_semantically_equal(updated, original, path)
        || values_semantically_equal(user, updated, path)
    {
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
                            rebase_added_value(user, updated, path, conflicts),
                        );
                    }
                    (Some(original), Some(user), Some(updated)) => {
                        result.insert(
                            key.clone(),
                            rebase_value(original, user, updated, path, conflicts),
                        );
                    }
                    (Some(original), Some(user), None)
                        if values_semantically_equal(original, user, path) => {}
                    (Some(original), None, Some(updated))
                        if values_semantically_equal(original, updated, path) => {}
                    (Some(original), Some(user), None) => {
                        add_draft_conflict(
                            original,
                            &Value::Null,
                            user,
                            path,
                            "modify-vs-delete",
                            conflicts,
                        );
                        // The unresolved preview follows the new upstream,
                        // which deleted this key. The user's value remains in
                        // the conflict projection for an explicit Keep mine.
                    }
                    (Some(original), None, Some(updated)) => {
                        add_draft_conflict(
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
                let common_order = |ids: &Vec<(String, String)>| {
                    ids.iter()
                        .filter(|id| {
                            original_ids.contains(id)
                                && user_ids.contains(id)
                                && updated_ids.contains(id)
                        })
                        .cloned()
                        .collect::<Vec<_>>()
                };
                let base_order = common_order(&original_ids);
                let user_order = common_order(&user_ids);
                let updated_order = common_order(&updated_ids);
                if user_order != base_order
                    && updated_order != base_order
                    && user_order != updated_order
                {
                    add_draft_conflict(
                        &Value::Array(original.to_vec()),
                        &Value::Array(updated.to_vec()),
                        &Value::Array(user.to_vec()),
                        path,
                        "order",
                        conflicts,
                    );
                }
                let mut identities = if user_order != base_order {
                    user_ids.clone()
                } else {
                    updated_ids.clone()
                };
                for identity in user_ids.iter().chain(updated_ids.iter()) {
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
                            result.push(rebase_added_value(user, updated, path, conflicts))
                        }
                        (Some(original), Some(user), Some(updated)) => {
                            result.push(rebase_value(original, user, updated, path, conflicts))
                        }
                        (Some(original), Some(user), None)
                            if values_semantically_equal(original, user, path) => {}
                        (Some(original), None, Some(updated))
                            if values_semantically_equal(original, updated, path) => {}
                        (Some(original), Some(user), None) => {
                            add_draft_conflict(
                                original,
                                &Value::Null,
                                user,
                                path,
                                "modify-vs-delete",
                                conflicts,
                            );
                            // Updated upstream removed the identity. Keep the
                            // user's element only in conflict metadata.
                        }
                        (Some(original), None, Some(updated)) => {
                            add_draft_conflict(
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
                add_draft_conflict(
                    &Value::Array(original.to_vec()),
                    &Value::Array(updated.to_vec()),
                    &Value::Array(user.to_vec()),
                    path,
                    "ordered-sequence",
                    conflicts,
                );
                Value::Array(updated.clone())
            }
        }
        _ => {
            add_draft_conflict(original, updated, user, path, "value", conflicts);
            updated.clone()
        }
    }
}

fn rebase_added_value(
    user: &Value,
    updated: &Value,
    path: &mut SemanticPath,
    conflicts: &mut Vec<MergeLocation>,
) -> Value {
    if values_semantically_equal(user, updated, path) {
        return updated.clone();
    }
    if user.is_object() && updated.is_object() {
        return rebase_value(&json!({}), user, updated, path, conflicts);
    }
    if let (Some(user), Some(updated)) = (user.as_array(), updated.as_array())
        && sequence_identities(user).is_some()
        && sequence_identities(updated).is_some()
    {
        return rebase_value(
            &json!([]),
            &Value::Array(user.clone()),
            &Value::Array(updated.clone()),
            path,
            conflicts,
        );
    }
    add_draft_conflict(&Value::Null, updated, user, path, "value", conflicts);
    updated.clone()
}

fn add_draft_conflict(
    original: &Value,
    updated: &Value,
    user: &Value,
    path: &[SemanticPathSegment],
    conflict_type: &str,
    conflicts: &mut Vec<MergeLocation>,
) {
    conflicts.push(MergeLocation {
        segments: path.to_vec(),
        conflict_type: conflict_type.to_owned(),
        can_combine: conflict_type == "value"
            && original.is_object()
            && updated.is_object()
            && user.is_object(),
    });
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

/// Applies an explicitly resolved Final edit even when an upstream update
/// removed one of its object/identity anchors. Only Keep mine or a validated
/// manual merge may recreate the minimum container chain required by the path.
fn set_semantic_path_creating_anchors(
    root: &mut Value,
    path: &[SemanticPathSegment],
    value: Value,
) -> std::result::Result<(), Box<ConfigurationConflict>> {
    fn apply(
        current: &mut Value,
        remaining: &[SemanticPathSegment],
        full_path: &[SemanticPathSegment],
        offset: usize,
        value: Value,
    ) -> std::result::Result<(), Box<ConfigurationConflict>> {
        let Some((segment, tail)) = remaining.split_first() else {
            *current = value;
            return Ok(());
        };
        match segment {
            SemanticPathSegment::Key { key } => {
                let object = current.as_object_mut().ok_or_else(|| {
                    semantic_path_conflict(
                        &full_path[..offset],
                        "A final decision cannot recreate an anchor through a non-object value",
                        Some(value.clone()),
                    )
                })?;
                if tail.is_empty() {
                    object.insert(key.clone(), value);
                    return Ok(());
                }
                let child = object.entry(key.clone()).or_insert_with(|| match tail[0] {
                    SemanticPathSegment::Key { .. } => Value::Object(Map::new()),
                    SemanticPathSegment::Identity { .. } => Value::Array(Vec::new()),
                });
                apply(child, tail, full_path, offset + 1, value)
            }
            SemanticPathSegment::Identity {
                field,
                value: identity,
            } => {
                let values = current.as_array_mut().ok_or_else(|| {
                    semantic_path_conflict(
                        &full_path[..offset],
                        "A final identity decision cannot recreate an anchor through a non-array value",
                        Some(value.clone()),
                    )
                })?;
                if tail.is_empty() {
                    if let Some(index) = identity_index(values, field, identity)? {
                        values[index] = value;
                    } else {
                        values.push(value);
                    }
                    return Ok(());
                }
                let index = if let Some(index) = identity_index(values, field, identity)? {
                    index
                } else {
                    if !matches!(tail[0], SemanticPathSegment::Key { .. }) {
                        return Err(semantic_path_conflict(
                            &full_path[..=offset],
                            "A nested identity anchor cannot be reconstructed safely",
                            Some(value),
                        ));
                    }
                    let mut identity_object = Map::new();
                    identity_object.insert(field.clone(), Value::String(identity.clone()));
                    values.push(Value::Object(identity_object));
                    values.len() - 1
                };
                apply(&mut values[index], tail, full_path, offset + 1, value)
            }
        }
    }

    if path.is_empty() {
        *root = value;
        return Ok(());
    }
    apply(root, path, path, 0, value)
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
                semantic_path_conflict(
                    path,
                    "Final edit delete target parent is not an object",
                    None,
                )
            })?;
            values.remove(key);
        }
        SemanticPathSegment::Identity {
            field,
            value: identity,
        } => {
            let values = parent.as_array_mut().ok_or_else(|| {
                semantic_path_conflict(
                    path,
                    "Final edit delete target parent is not an array",
                    None,
                )
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
                        semantic_path_conflict(
                            &path[..=offset],
                            "A Final edit object anchor no longer exists",
                            None,
                        )
                    })?;
            }
            SemanticPathSegment::Identity { field, value } => {
                let values = current.as_array_mut().ok_or_else(|| {
                    semantic_path_conflict(
                        &path[..=offset],
                        "A Final edit identity parent is no longer an array",
                        None,
                    )
                })?;
                let index = identity_index(values, field, value)?.ok_or_else(|| {
                    semantic_path_conflict(
                        &path[..=offset],
                        "A Final edit identity anchor no longer exists",
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
            source_ids: Vec::new(),
            message_key: Some("CONFIGURATION_IDENTITY_DUPLICATED".into()),
            scope: ConfigurationIssueScope::configuration(),
            source_value: None,
            guided_value: None,
            user_value: None,
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

fn semantic_path_conflict(
    path: &[SemanticPathSegment],
    reason: impl Into<String>,
    user_value: Option<Value>,
) -> Box<ConfigurationConflict> {
    Box::new(ConfigurationConflict {
        semantic_path: display_semantic_path(path),
        reason: reason.into(),
        severity: ConflictSeverity::Error,
        source_ids: Vec::new(),
        message_key: Some("FINAL_EDIT_CONFLICT".into()),
        scope: ConfigurationIssueScope::configuration(),
        source_value: None,
        guided_value: None,
        user_value,
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
pub struct ConfigurationDiagnosticLocation {
    pub semantic_path: String,
    pub document_path: Vec<String>,
}

impl ConfigurationDiagnosticLocation {
    pub fn from_pointer(pointer: &[String]) -> Self {
        let semantic_path = if pointer.is_empty() {
            "/".into()
        } else {
            format!(
                "/{}",
                pointer
                    .iter()
                    .map(|key| key.replace('~', "~0").replace('/', "~1"))
                    .collect::<Vec<_>>()
                    .join("/")
            )
        };
        Self {
            semantic_path,
            document_path: pointer.to_vec(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationDiagnostic {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<ConfigurationDiagnosticLocation>,
    pub message: String,
    /// Stable UI mapping key; the message is retained as technical context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_key: Option<String>,
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
    pub diagnostics: Vec<ConfigurationDiagnostic>,
    pub conflicts: Vec<ConfigurationConflict>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationState {
    pub kind: ProgramKind,
    pub format: ConfigurationFormat,
    pub state_revision: u64,
    pub generation: u64,
    pub compatibility_profile: CoreCompatibilityProfile,
    pub source_statuses: BTreeMap<String, SourceStatus>,
    pub source_snapshots: BTreeMap<String, SourceSnapshot>,
    pub base: ConfigurationCandidate,
    pub base_provenance: Vec<ProvenanceEntry>,
    pub provenance: Vec<ProvenanceEntry>,
    pub guided_intent: GuidedIntent,
    pub managed_intent: ManagedIntegrationIntent,
    pub upstream: UpstreamState,
    pub final_edit: FinalEditState,
    pub operation_receipts: Vec<ConfigurationOperationReceipt>,
    #[serde(default)]
    pub adopted_changes: Vec<AdoptUpstreamReceipt>,
    #[serde(default)]
    pub conflict_operations: Vec<ConfigurationConflictReceipt>,
    pub desired: ConfigurationCandidate,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editor_session: Option<FinalEditorSession>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied: Option<ConfigurationCandidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_known_good: Option<ConfigurationCandidate>,
}

impl ConfigurationState {
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
        let upstream =
            UpstreamState::new(parse_semantic_document(format, merge.content.as_bytes())?);
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
            kind,
            format,
            state_revision: generation,
            generation,
            compatibility_profile,
            source_statuses: BTreeMap::new(),
            source_snapshots: BTreeMap::new(),
            base: candidate.clone(),
            provenance: base_provenance.clone(),
            base_provenance,
            guided_intent: GuidedIntent::default(),
            managed_intent: ManagedIntegrationIntent::default(),
            upstream,
            final_edit: FinalEditState::default(),
            operation_receipts: Vec::new(),
            adopted_changes: Vec::new(),
            conflict_operations: Vec::new(),
            desired: candidate,
            editor_session: None,
            applied: None,
            last_known_good: None,
        })
    }

    pub fn upstream_document(&self) -> Result<Value> {
        let base = parse_semantic_document(self.format, self.base.content.as_bytes())?;
        self.upstream.clone().reconcile(
            self.kind,
            &base,
            &self.guided_intent,
            &self.managed_intent,
            SourceUpdateKind::UserEdit,
        )
    }

    pub fn claim_guided_setting(&mut self, setting: &str) -> Result<()> {
        let before = self.upstream.clone();
        self.upstream.claim_intent(self.kind, setting)?;
        if before != self.upstream {
            self.state_revision = self.state_revision.saturating_add(1);
        }
        Ok(())
    }

    pub fn claim_managed_setting(&mut self, setting: &str) -> Result<()> {
        let before = self.upstream.clone();
        self.upstream.claim_details(self.kind, setting)?;
        if before != self.upstream {
            self.state_revision = self.state_revision.saturating_add(1);
        }
        Ok(())
    }

    pub fn upstream_content(&self) -> Result<String> {
        serialize_semantic_document(self.format, &self.upstream_document()?)
    }

    pub fn adopt_upstream_change(
        &mut self,
        request: AdoptUpstreamChangeRequest,
        created_unix_ms: u64,
    ) -> Result<bool> {
        let request_hash = hash_bytes(&serde_json::to_vec(&request).expect("serializable request"));
        if let Some(recorded) = self
            .adopted_changes
            .iter()
            .find(|recorded| recorded.operation_id == request.operation_id)
        {
            if recorded.request_hash == request_hash {
                return Ok(false);
            }
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Operation identity belongs to a different change",
            )
            .with_message_key("CONFIGURATION_OPERATION_MISMATCH"));
        }
        if uuid::Uuid::parse_str(&request.operation_id).is_err() {
            return Err(CamelliaNexusError::invalid_spec(
                "Invalid configuration operation identity",
            ));
        }
        if self
            .operation_receipts
            .iter()
            .any(|receipt| receipt.request.operation_id == request.operation_id)
            || self
                .conflict_operations
                .iter()
                .any(|receipt| receipt.request.operation_id == request.operation_id)
        {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Operation identity belongs to a different request",
            )
            .with_message_key("CONFIGURATION_OPERATION_MISMATCH"));
        }
        if request.expected_state_revision != self.state_revision {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Configuration state changed",
            )
            .with_message_key("CONFIGURATION_STATE_STALE"));
        }
        let draft_matches = match &self.editor_session {
            Some(draft) => {
                request.editor_session_id.as_deref() == Some(&draft.session_id)
                    && request.expected_draft_revision == Some(draft.draft_revision)
            }
            None => request
                .expected_draft_revision
                .is_none_or(|revision| revision == 0),
        };
        if !draft_matches {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Configuration draft changed",
            )
            .with_message_key("CONFIGURATION_DRAFT_STALE"));
        }
        if self.editor_session.as_ref().is_some_and(|draft| {
            draft.rebase_required
                || !draft.unresolved_conflict_ids.is_empty()
                || draft.working_content != self.desired.content
        }) {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Finish or discard the current edit first",
            )
            .with_message_key("CONFIGURATION_DRAFT_UNCOMMITTED"));
        }
        let edit = self
            .final_edit
            .edits
            .iter()
            .find(|edit| edit.edit_id == request.edit_id && edit.path == request.path)
            .ok_or_else(|| {
                CamelliaNexusError::new(ErrorCode::ConfigConflict, "Final change has changed")
                    .with_message_key("CONFIGURATION_STATE_STALE")
            })?;
        if self.final_edit.conflicts.iter().any(|conflict| {
            edit.path.starts_with(&conflict.path) || conflict.path.starts_with(&edit.path)
        }) {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Resolve this conflict before adopting the updated value",
            )
            .with_message_key("FINAL_EDIT_CONFLICT"));
        }
        let upstream = self.upstream_document()?;
        let current = semantic_value_at(&upstream, &edit.path).map_err(|conflict| {
            CamelliaNexusError::new(ErrorCode::ConfigConflict, conflict.reason)
        })?;
        if !semantic_values_equal_at(&current, &edit.original, &edit.path) {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "The updated value has changed; review it again",
            )
            .with_message_key("CONFIGURATION_STATE_STALE"));
        }
        let mut next = self.clone();
        let mut document =
            parse_semantic_document(next.format, next.final_edit.edited_content.as_bytes())?;
        apply_semantic_value(&mut document, &request.path, &current).map_err(|conflict| {
            CamelliaNexusError::new(ErrorCode::ConfigConflict, conflict.reason)
        })?;
        next.final_edit.base_content = serialize_semantic_document(next.format, &upstream)?;
        next.final_edit.edited_content = serialize_semantic_document(next.format, &document)?;
        next.final_edit.edits = final_edit_entries(&upstream, &document)?;
        next.final_edit.basis.content_hash = semantic_document_hash(&upstream);
        next.final_edit.saved_candidate = None;
        next.rebuild_desired(created_unix_ms)?;
        next.state_revision = next
            .state_revision
            .max(self.state_revision.saturating_add(1));
        next.adopted_changes.push(AdoptUpstreamReceipt {
            operation_id: request.operation_id,
            request_hash,
        });
        const RECEIPT_LIMIT: usize = 64;
        if next.adopted_changes.len() > RECEIPT_LIMIT {
            next.adopted_changes
                .drain(..next.adopted_changes.len() - RECEIPT_LIMIT);
        }
        *self = next;
        Ok(true)
    }

    pub fn resolve_final_conflict(
        &mut self,
        conflict_id: &str,
        resolution: FinalConflictResolution,
        created_unix_ms: u64,
    ) -> Result<()> {
        let upstream = self.upstream_document()?;
        let mut document =
            parse_semantic_document(self.format, self.final_edit.edited_content.as_bytes())?;
        let conflict = self
            .final_edit
            .conflicts
            .iter()
            .find(|conflict| conflict.conflict_id == conflict_id)
            .cloned()
            .ok_or_else(|| {
                CamelliaNexusError::new(
                    ErrorCode::NotFound,
                    "Final configuration conflict was not found",
                )
            })?;
        match resolution {
            FinalConflictResolution::AcceptUpstream => {
                apply_semantic_value(&mut document, &conflict.path, &conflict.upstream_value)
            }
            FinalConflictResolution::KeepMine => {
                apply_semantic_value(&mut document, &conflict.path, &conflict.user_value)
            }
            FinalConflictResolution::ManualEdit { value } => {
                apply_semantic_value(&mut document, &conflict.path, &value)
            }
        }
        .map_err(|value| CamelliaNexusError::new(ErrorCode::ConfigConflict, value.reason))?;
        self.final_edit
            .conflicts
            .retain(|value| value.conflict_id != conflict_id);
        self.final_edit.base_content = serialize_semantic_document(self.format, &upstream)?;
        self.final_edit.edited_content = serialize_semantic_document(self.format, &document)?;
        self.final_edit.edits = final_edit_entries(&upstream, &document)?;
        self.final_edit.basis = UpstreamBasis {
            content_hash: semantic_document_hash(&upstream),
            candidate_generation: self.generation,
        };
        self.final_edit.saved_candidate = None;
        self.rebuild_desired(created_unix_ms)
    }

    pub fn rebuild_desired(&mut self, created_unix_ms: u64) -> Result<()> {
        self.rebuild_desired_with_source_update(created_unix_ms, SourceUpdateKind::UserEdit)
    }

    pub fn rebuild_desired_with_source_update(
        &mut self,
        created_unix_ms: u64,
        source_update: SourceUpdateKind,
    ) -> Result<()> {
        let previous_content = self.desired.content.clone();
        let previously_saved = self.candidate_is_saved();
        let previous_conflicts = self.desired.conflicts.clone();
        let previous_profile_hash = self.desired.compatibility_profile_hash.clone();
        let previous_final_edit = self.final_edit.clone();
        let previous_upstream = self.upstream.clone();
        let base = parse_semantic_document(self.format, self.base.content.as_bytes())?;
        let upstream_document = self.upstream.reconcile(
            self.kind,
            &base,
            &self.guided_intent,
            &self.managed_intent,
            source_update,
        )?;
        let upstream_provenance = apply_provenance_layer(
            &base,
            &upstream_document,
            &self.base_provenance,
            ProvenanceLayer::Upstream,
        );
        let effective = reconcile_final_edits(
            &mut self.final_edit,
            self.format,
            &upstream_document,
            self.generation,
        )?;
        let mut conflicts = self
            .final_edit
            .conflicts
            .iter()
            .map(|conflict| ConfigurationConflict {
                semantic_path: display_semantic_path(&conflict.path),
                reason: "The upstream configuration and final editor changed the same path".into(),
                severity: ConflictSeverity::Error,
                source_ids: Vec::new(),
                message_key: Some("FINAL_EDIT_CONFLICT".into()),
                scope: ConfigurationIssueScope {
                    surface: ConfigurationSurface::Configuration,
                    owner_id: Some(conflict.conflict_id.clone()),
                },
                source_value: match &conflict.upstream_value {
                    SemanticValue::Present(value) => Some(value.clone()),
                    SemanticValue::Missing => None,
                },
                guided_value: None,
                user_value: match &conflict.user_value {
                    SemanticValue::Present(value) => Some(value.clone()),
                    SemanticValue::Missing => None,
                },
                effective_value: match &conflict.upstream_value {
                    SemanticValue::Present(value) => Some(value.clone()),
                    SemanticValue::Missing => None,
                },
            })
            .collect::<Vec<_>>();
        self.provenance = apply_provenance_layer(
            &upstream_document,
            &effective,
            &upstream_provenance,
            ProvenanceLayer::FinalEdit,
        );
        // Preserve blocking issues produced while merging Sources. Rebuilding
        // downstream layers must not make a source conflict disappear.
        conflicts.extend(self.base.conflicts.clone());
        let previous_document = parse_semantic_document(self.format, previous_content.as_bytes())?;
        let content = if diff_intent_operations(&previous_document, &effective).is_empty() {
            previous_content.clone()
        } else {
            serialize_semantic_document(self.format, &effective)?
        };
        let final_edit_changed = previous_final_edit != self.final_edit;
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
        if !final_edit_changed && !candidate_changed && previous_upstream == self.upstream {
            // Re-entering a tab, refreshing an unchanged source, or repeating
            // the same Guided/Details value is a read-equivalent operation.
            // Keep generation and native evidence stable instead of creating a
            // phantom revision that can invalidate an otherwise valid Apply.
            return Ok(());
        }
        let content_changed = previous_content != content
            || previous_profile_hash != self.compatibility_profile.profile_hash;
        if content_changed {
            self.generation = self.generation.saturating_add(1);
        }
        if previously_saved && previous_content == content {
            self.final_edit.saved_candidate = Some(SavedCandidateMarker {
                content_hash: self.desired.revision.content_hash.clone(),
                candidate_generation: self.generation,
            });
        }
        self.state_revision = self.state_revision.saturating_add(1);
        self.final_edit.basis.candidate_generation = self.generation;
        if !candidate_changed {
            return Ok(());
        }
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

    pub fn replace_final_from_edited(&mut self, edited: &[u8], created_unix_ms: u64) -> Result<()> {
        if !self.final_edit.conflicts.is_empty() {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Resolve final configuration conflicts before saving",
            )
            .with_message_key("FINAL_EDIT_CONFLICT"));
        }
        let upstream = self.upstream_document()?;
        let edited = parse_semantic_document(self.format, edited)?;
        validate_static_candidate(self.kind, &edited)?;
        let next = FinalEditState {
            basis: UpstreamBasis {
                content_hash: semantic_document_hash(&upstream),
                candidate_generation: self.generation,
            },
            base_content: serialize_semantic_document(self.format, &upstream)?,
            edited_content: serialize_semantic_document(self.format, &edited)?,
            edits: final_edit_entries(&upstream, &edited)?,
            conflicts: Vec::new(),
            saved_candidate: self.final_edit.saved_candidate.clone(),
        };
        if self.final_edit == next {
            return Ok(());
        }
        let previous_revision = self.state_revision;
        self.final_edit = next;
        self.rebuild_desired(created_unix_ms)?;
        if self.state_revision == previous_revision {
            self.state_revision = self.state_revision.saturating_add(1);
        }
        Ok(())
    }

    pub fn mark_candidate_saved(&mut self) -> Result<()> {
        let document = parse_semantic_document(self.format, self.desired.content.as_bytes())?;
        validate_static_candidate(self.kind, &document)?;
        if !self.final_edit.conflicts.is_empty()
            || self
                .desired
                .conflicts
                .iter()
                .any(|conflict| conflict.severity == ConflictSeverity::Error)
        {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Resolve blocking conflicts before saving the candidate",
            )
            .with_message_key("CONFIGURATION_BLOCKING_CONFLICT"));
        }
        let marker = SavedCandidateMarker {
            content_hash: self.desired.revision.content_hash.clone(),
            candidate_generation: self.generation,
        };
        if self.final_edit.saved_candidate.as_ref() != Some(&marker) {
            self.final_edit.saved_candidate = Some(marker);
            self.state_revision = self.state_revision.saturating_add(1);
        }
        Ok(())
    }

    pub fn candidate_is_saved(&self) -> bool {
        self.final_edit
            .saved_candidate
            .as_ref()
            .is_some_and(|marker| {
                marker.content_hash == self.desired.revision.content_hash
                    && marker.candidate_generation == self.generation
            })
    }

    pub fn mark_validation(
        &mut self,
        valid: bool,
        diagnostics: Vec<ConfigurationDiagnostic>,
        evidence: Option<CoreValidationEvidence>,
    ) -> Result<()> {
        if !self.candidate_is_saved() {
            return Err(CamelliaNexusError::new(
                ErrorCode::InvalidState,
                "Save the current candidate before native validation",
            )
            .with_message_key("CONFIGURATION_CANDIDATE_UNSAVED"));
        }
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
                evidence.validates_candidate(
                    binary_sha256,
                    &self.compatibility_profile.profile_hash,
                    &self.desired.revision.content_hash,
                    self.generation,
                )
            }) {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigConflict,
                    "Native validation evidence does not match the exact binary, profile, and candidate",
                ).with_message_key("CORE_VALIDATION_EVIDENCE_STALE"));
            }
        }
        let next_validation = if valid {
            CandidateValidationStatus::Valid
        } else {
            CandidateValidationStatus::Invalid
        };
        let next_evidence = valid.then_some(evidence).flatten();
        let unchanged = self.desired.validation == next_validation
            && self.desired.diagnostics == diagnostics
            && match (&self.desired.validation_evidence, &next_evidence) {
                (None, None) => true,
                (Some(current), Some(next)) => {
                    current.binary_sha256 == next.binary_sha256
                        && current.profile_hash == next.profile_hash
                        && current.config_hash == next.config_hash
                        && current.candidate_generation == next.candidate_generation
                        && current.validator_contract_revision == next.validator_contract_revision
                        && current.native_accepted == next.native_accepted
                }
                _ => false,
            };
        if unchanged {
            return Ok(());
        }
        self.desired.validation = next_validation;
        self.desired.diagnostics = diagnostics;
        self.desired.validation_evidence = next_evidence;
        self.state_revision = self.state_revision.saturating_add(1);
        Ok(())
    }

    pub fn ensure_apply_ready(&self) -> Result<()> {
        if !self.candidate_is_saved() {
            return Err(CamelliaNexusError::new(
                ErrorCode::InvalidState,
                "Only a saved configuration candidate can be applied",
            )
            .with_message_key("CONFIGURATION_CANDIDATE_UNSAVED"));
        }
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
                evidence.validates_candidate(
                    binary_sha256,
                    &self.compatibility_profile.profile_hash,
                    &self.desired.revision.content_hash,
                    self.generation,
                )
            })
        {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Native validation evidence does not match the exact binary, profile, and candidate",
            ).with_message_key("CORE_VALIDATION_EVIDENCE_STALE"));
        }
        Ok(())
    }

    pub fn mark_applied(&mut self) -> Result<()> {
        self.ensure_apply_ready()?;
        let already_applied = self.applied.as_ref() == Some(&self.desired)
            && self.last_known_good.as_ref() == Some(&self.desired);
        if already_applied {
            return Ok(());
        }
        self.applied = Some(self.desired.clone());
        self.last_known_good = Some(self.desired.clone());
        self.state_revision = self.state_revision.saturating_add(1);
        Ok(())
    }

    pub fn view(&self) -> ConfigurationStateView {
        let base = parse_semantic_document(self.format, self.base.content.as_bytes())
            .unwrap_or(Value::Null);
        let upstream = self.upstream_document().unwrap_or_else(|_| base.clone());
        let effective = parse_semantic_document(self.format, self.desired.content.as_bytes())
            .unwrap_or(Value::Null);
        let workspace = build_configuration_workspace(self);
        ConfigurationStateView {
            kind: self.kind,
            format: self.format,
            state_revision: self.state_revision,
            generation: self.generation,
            compatibility_profile: self.compatibility_profile.clone(),
            core_admission: None,
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
                &upstream,
                &self.guided_intent,
                &self.final_edit,
            ),
            managed_integrations: project_managed_integrations(
                self.kind,
                &effective,
                &upstream,
                &self.managed_intent,
                &self.final_edit,
                &self.desired.conflicts,
            ),
            workspace,
        }
    }
}

fn build_configuration_workspace(state: &ConfigurationState) -> ConfigurationWorkspaceView {
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
    let final_conflicts = state
        .desired
        .conflicts
        .iter()
        .filter(|conflict| {
            matches!(
                conflict.message_key.as_deref(),
                Some("FINAL_EDIT_CONFLICT") | Some("CONFIGURATION_IDENTITY_DUPLICATED")
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut blockers = build_configuration_gate_blockers(
        state,
        &source_conflicts,
        &layer_conflicts,
        &final_conflicts,
    );
    let candidate_saved = state.candidate_is_saved();
    let draft_needs_choice = state
        .editor_session
        .as_ref()
        .is_some_and(|draft| draft.rebase_required || !draft.unresolved_conflict_ids.is_empty());
    if draft_needs_choice {
        blockers.push(ConfigurationGateBlocker {
            code: "FINAL_EDIT_CONFLICT".into(),
            details: None,
            message_key: "FINAL_EDIT_CONFLICT".into(),
            scope: ConfigurationIssueScope {
                surface: ConfigurationSurface::Configuration,
                owner_id: None,
            },
            semantic_path: None,
            blocks: vec![
                ConfigurationGate::Save,
                ConfigurationGate::Validate,
                ConfigurationGate::Apply,
            ],
            recovery_action: ConfigurationRecoveryAction::OpenFinalConfiguration,
        });
    }
    if !candidate_saved
        && !blockers
            .iter()
            .any(|blocker| blocker.blocks.contains(&ConfigurationGate::Save))
    {
        blockers.push(ConfigurationGateBlocker {
            code: "CONFIGURATION_CANDIDATE_UNSAVED".into(),
            details: None,
            message_key: "CONFIGURATION_CANDIDATE_UNSAVED".into(),
            scope: ConfigurationIssueScope {
                surface: ConfigurationSurface::Configuration,
                owner_id: None,
            },
            semantic_path: None,
            blocks: vec![ConfigurationGate::Validate, ConfigurationGate::Apply],
            recovery_action: ConfigurationRecoveryAction::OpenFinalConfiguration,
        });
    }
    if state.ensure_apply_ready().is_err()
        && !blockers
            .iter()
            .any(|blocker| blocker.blocks.contains(&ConfigurationGate::Apply))
    {
        let code = match state.desired.validation {
            CandidateValidationStatus::Pending => "CORE_VALIDATION_REQUIRED",
            CandidateValidationStatus::Invalid => "CONFIGURATION_INVALID",
            CandidateValidationStatus::Valid => "CORE_VALIDATION_EVIDENCE_STALE",
        };
        blockers.push(ConfigurationGateBlocker {
            code: code.into(),
            details: None,
            message_key: code.into(),
            scope: ConfigurationIssueScope {
                surface: ConfigurationSurface::Configuration,
                owner_id: None,
            },
            semantic_path: None,
            blocks: vec![ConfigurationGate::Apply],
            recovery_action: ConfigurationRecoveryAction::ReviewCandidate,
        });
    }
    let can_apply = candidate_saved
        && state.ensure_apply_ready().is_ok()
        && blockers
            .iter()
            .all(|blocker| !blocker.blocks.contains(&ConfigurationGate::Apply));
    let can_validate = blockers
        .iter()
        .all(|blocker| !blocker.blocks.contains(&ConfigurationGate::Validate));
    let can_save = blockers
        .iter()
        .all(|blocker| !blocker.blocks.contains(&ConfigurationGate::Save));
    let has_blocking_conflict = state
        .desired
        .conflicts
        .iter()
        .any(|conflict| conflict.severity == ConflictSeverity::Error);
    let edit_status =
        if draft_needs_choice || has_blocking_conflict || !state.final_edit.conflicts.is_empty() {
            FinalEditStatus::Conflict
        } else if state.final_edit.edits.is_empty() {
            FinalEditStatus::Clean
        } else {
            FinalEditStatus::Modified
        };
    let applied_matches = state.applied.as_ref().is_some_and(|applied| {
        applied.revision.content_hash == state.desired.revision.content_hash
            && applied.compatibility_profile_hash == state.desired.compatibility_profile_hash
    });
    let candidate_status = if !candidate_saved {
        CandidateStatus::Unsaved
    } else if applied_matches {
        CandidateStatus::Applied
    } else {
        match state.desired.validation {
            CandidateValidationStatus::Pending => CandidateStatus::PendingValidation,
            CandidateValidationStatus::Invalid => CandidateStatus::Invalid,
            CandidateValidationStatus::Valid => CandidateStatus::Validated,
        }
    };
    let issues_for_path = |path: &[SemanticPathSegment]| {
        let semantic_path = display_semantic_path(path);
        state
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
            .map(|conflict| ConfigurationEditorIssue {
                id: format!(
                    "{}:{}",
                    conflict
                        .message_key
                        .as_deref()
                        .unwrap_or("CONFIGURATION_CONFLICT"),
                    conflict.semantic_path
                ),
                code: conflict
                    .message_key
                    .clone()
                    .unwrap_or_else(|| "CONFIGURATION_CONFLICT".into()),
                message_key: conflict
                    .message_key
                    .clone()
                    .unwrap_or_else(|| "CONFIGURATION_CONFLICT".into()),
                semantic_path: conflict.semantic_path.clone(),
                severity: conflict.severity,
                blocking: conflict.severity == ConflictSeverity::Error,
            })
            .collect::<Vec<_>>()
    };
    let changes = state
        .final_edit
        .edits
        .iter()
        .map(|entry| FinalChangeProjection {
            edit_id: entry.edit_id.clone(),
            semantic_path: display_semantic_path(&entry.path),
            segments: entry.path.clone(),
            kind: match (&entry.original, &entry.edited) {
                (SemanticValue::Missing, SemanticValue::Present(_)) => FinalChangeKind::Added,
                (SemanticValue::Present(_), SemanticValue::Missing) => FinalChangeKind::Deleted,
                _ => FinalChangeKind::Modified,
            },
            upstream_value: entry.original.clone(),
            final_value: entry.edited.clone(),
            issues: issues_for_path(&entry.path),
        })
        .collect();
    let conflicts = state
        .final_edit
        .conflicts
        .iter()
        .map(|conflict| {
            project_configuration_conflict(ConfigurationConflictOrigin::Candidate, conflict)
        })
        .chain(state.editor_session.iter().flat_map(|draft| {
            draft
                .conflicts
                .iter()
                .filter(|conflict| {
                    !draft.rebase_required
                        && draft
                            .unresolved_conflict_ids
                            .contains(&conflict.conflict_id)
                })
                .map(|conflict| {
                    project_configuration_conflict(ConfigurationConflictOrigin::Draft, conflict)
                })
        }))
        .collect();
    let editor = ConfigurationEditorView {
        document: ConfigurationEditorDocument {
            content: state.desired.content.clone(),
            revision: state.desired.revision.clone(),
        },
        edit_status,
        candidate_status,
        changes,
        conflicts,
        diagnostics: state.desired.diagnostics.clone(),
        blockers,
        can_save,
        can_validate,
        can_apply,
    };
    ConfigurationWorkspaceView { editor }
}

fn build_configuration_gate_blockers(
    state: &ConfigurationState,
    source_conflicts: &[ConfigurationConflict],
    layer_conflicts: &[ConfigurationConflict],
    final_conflicts: &[ConfigurationConflict],
) -> Vec<ConfigurationGateBlocker> {
    let mut blockers = Vec::new();
    let mut add_conflict = |conflict: &ConfigurationConflict| {
        let code = conflict
            .message_key
            .clone()
            .unwrap_or_else(|| "CONFIGURATION_CONFLICT".into());
        let recovery_action = match code.as_str() {
            "SOURCE_VALUE_CONFLICT" => ConfigurationRecoveryAction::ResolveSourceConflict,
            "LAYER_OWNERSHIP_CONFLICT" => ConfigurationRecoveryAction::ResolveLayerConflict,
            "FINAL_EDIT_CONFLICT" => ConfigurationRecoveryAction::OpenFinalConfiguration,
            _ => ConfigurationRecoveryAction::ReviewCandidate,
        };
        blockers.push(ConfigurationGateBlocker {
            code: code.clone(),
            details: None,
            message_key: code,
            scope: conflict.scope.clone(),
            semantic_path: Some(conflict.semantic_path.clone()),
            blocks: vec![
                ConfigurationGate::Save,
                ConfigurationGate::Validate,
                ConfigurationGate::Apply,
            ],
            recovery_action,
        });
    };
    for conflict in source_conflicts
        .iter()
        .chain(layer_conflicts)
        .chain(final_conflicts)
    {
        if conflict.severity == ConflictSeverity::Error {
            add_conflict(conflict);
        }
    }
    for diagnostic in &state.desired.diagnostics {
        let code = diagnostic
            .message_key
            .clone()
            .unwrap_or_else(|| diagnostic.code.clone());
        let (blocks, recovery_action) = if diagnostic.scope.surface == ConfigurationSurface::Sources
        {
            (
                vec![
                    ConfigurationGate::Save,
                    ConfigurationGate::Validate,
                    ConfigurationGate::Apply,
                ],
                ConfigurationRecoveryAction::ResolveSourceConflict,
            )
        } else {
            match code.as_str() {
                "CORE_VALIDATION_EVIDENCE_STALE"
                | "CORE_PROFILE_MISMATCH"
                | "CORE_TARGET_CHANGED" => (
                    vec![ConfigurationGate::Apply],
                    ConfigurationRecoveryAction::ValidateCandidate,
                ),
                "CORE_INVALID" | "CONFIG_INVALID" | "CONFIGURATION_INVALID" => (
                    vec![ConfigurationGate::Apply],
                    ConfigurationRecoveryAction::ReviewCandidate,
                ),
                _ => (
                    vec![ConfigurationGate::Apply],
                    ConfigurationRecoveryAction::ReviewCandidate,
                ),
            }
        };
        blockers.push(ConfigurationGateBlocker {
            code,
            details: (diagnostic.scope.surface == ConfigurationSurface::Configuration)
                .then(|| diagnostic.details.clone())
                .flatten(),
            message_key: diagnostic
                .message_key
                .clone()
                .unwrap_or_else(|| "CONFIGURATION_DIAGNOSTIC".into()),
            scope: diagnostic.scope.clone(),
            semantic_path: diagnostic
                .location
                .as_ref()
                .map(|location| location.semantic_path.clone()),
            blocks,
            recovery_action,
        });
    }
    blockers
}

fn project_managed_integrations(
    kind: ProgramKind,
    effective: &Value,
    upstream: &Value,
    managed: &ManagedIntegrationIntent,
    final_edit: &FinalEditState,
    conflicts: &[ConfigurationConflict],
) -> Vec<ManagedIntegrationProjection> {
    let integration_ids: &[&str] = match kind {
        ProgramKind::SingBox => &["dashboard.singBoxApi", "dashboard.singBoxClash"],
        ProgramKind::Xray => &["dashboard.xray"],
        ProgramKind::Mihomo => &["dashboard.mihomo"],
        ProgramKind::Generic => &[],
    };
    let targets = managed_ownership_targets(kind);
    let mut rendered = json!({});
    let _ = apply_dashboard_intent(kind, &mut rendered, managed);
    integration_ids
        .iter()
        .map(|integration_id| {
            let integration_targets = targets
                .iter()
                .filter(|(_, integration, _)| integration == integration_id)
                .collect::<Vec<_>>();
            let final_paths = final_edit
                .edits
                .iter()
                .map(|entry| entry.path.clone())
                .chain(
                    final_edit
                        .conflicts
                        .iter()
                        .map(|conflict| conflict.path.clone()),
                )
                .filter(|final_path| {
                    integration_targets
                        .iter()
                        .any(|(_, _, target)| managed_path_overlaps(target, final_path))
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
            for final_conflict in &final_edit.conflicts {
                let decision_path = final_conflict.path.clone();
                if integration_targets
                    .iter()
                    .any(|(_, _, target)| managed_path_overlaps(target, &decision_path))
                {
                    issue_ids.push(format!(
                        "FINAL_EDIT_CONFLICT:{}",
                        display_semantic_path(&decision_path)
                    ));
                }
            }
            let intent_value = managed
                .values
                .iter()
                .find(|(key, _)| key.starts_with(&format!("{integration_id}.")))
                .map(|(_, value)| value.clone());
            let effective_enabled = integration_targets
                .iter()
                .filter(|(setting, _, _)| setting.ends_with("Port"))
                .filter_map(|(setting, _, _)| managed_setting_path(kind, setting))
                .any(|target| {
                    semantic_path_value(effective, &target)
                        .ok()
                        .flatten()
                        .is_some_and(|value| !value.is_null() && value.as_str() != Some(""))
                });
            let settings = integration_targets
                .iter()
                .map(|(setting, _, _)| *setting)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .filter_map(|setting| {
                    let path = managed_setting_path(kind, setting)?;
                    let effective_value = semantic_value_at(effective, &path).ok()?;
                    let effective_value = match effective_value {
                        SemanticValue::Present(Value::String(ref address))
                            if setting.ends_with("Port") && address.starts_with("127.0.0.1:") =>
                        {
                            address
                                .rsplit_once(':')
                                .and_then(|(_, port)| port.parse::<u16>().ok())
                                .map(|port| SemanticValue::Present(json!(port)))
                                .unwrap_or(effective_value)
                        }
                        value => value,
                    };
                    let saved_value = managed
                        .values
                        .get(setting)
                        .cloned()
                        .map(SemanticValue::Present)
                        .unwrap_or(SemanticValue::Missing);
                    let can_use_saved_value = saved_value != SemanticValue::Missing
                        && !semantic_values_equal_at(
                            &semantic_value_at(&rendered, &path).ok()?,
                            &semantic_value_at(upstream, &path).ok()?,
                            &path,
                        );
                    Some(ManagedSettingProjection {
                        setting_id: setting.into(),
                        saved_value,
                        effective_value,
                        can_use_saved_value,
                    })
                })
                .collect::<Vec<_>>();
            let status = if intent_value.is_none() && !issue_ids.is_empty() {
                ManagedIntegrationStatus::NeedsAttention
            } else if intent_value.is_some() && (!issue_ids.is_empty() || !final_paths.is_empty()) {
                ManagedIntegrationStatus::Overridden
            } else if settings.iter().any(|setting| setting.can_use_saved_value) {
                ManagedIntegrationStatus::LatestSettings
            } else if intent_value.is_some() {
                ManagedIntegrationStatus::Explicit
            } else if !final_paths.is_empty() && effective_enabled {
                ManagedIntegrationStatus::FinalOnly
            } else if effective_enabled {
                ManagedIntegrationStatus::LatestSettings
            } else if !effective_enabled && (intent_value.is_some() || !final_paths.is_empty()) {
                ManagedIntegrationStatus::NeedsAttention
            } else {
                ManagedIntegrationStatus::Inactive
            };
            ManagedIntegrationProjection {
                integration_id: (*integration_id).into(),
                status,
                effective_enabled,
                settings,
                intent_value,
                final_paths,
                issue_ids,
            }
        })
        .collect()
}

fn managed_setting_path(kind: ProgramKind, setting: &str) -> Option<SemanticPath> {
    let leaf = match setting {
        "dashboard.singBoxApi.listenPort" => "listen_port",
        "dashboard.singBoxApi.updateInterval" => "update_interval",
        "dashboard.singBoxClash.listenPort" => "external_controller",
        "dashboard.singBoxClash.downloadUrl" => "external_ui_download_url",
        "dashboard.xray.apiPort" | "dashboard.xray.metricsPort" => "listen",
        "dashboard.mihomo.listenPort" => "external-controller",
        "dashboard.mihomo.downloadUrl" => "external-ui-url",
        _ => return None,
    };
    managed_ownership_targets(kind)
        .into_iter()
        .find_map(|(owner, _, path)| {
            (owner == setting
                && matches!(path.last(), Some(SemanticPathSegment::Key { key }) if key == leaf))
            .then_some(path)
        })
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

/// A managed leaf overlaps a Final editor decision made at the same path or
/// at an ancestor. Descendant extension fields remain independent.
fn managed_path_overlaps(
    target: &[SemanticPathSegment],
    final_path: &[SemanticPathSegment],
) -> bool {
    final_path.len() <= target.len()
        && final_path
            .iter()
            .zip(target)
            .all(|(left, right)| left == right)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigurationGate {
    Save,
    Validate,
    Apply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigurationRecoveryAction {
    OpenCompatibility,
    ResolveSourceConflict,
    ResolveLayerConflict,
    OpenFinalConfiguration,
    ValidateCandidate,
    ReviewCandidate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationGateBlocker {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    pub message_key: String,
    pub scope: ConfigurationIssueScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_path: Option<String>,
    #[serde(default)]
    pub blocks: Vec<ConfigurationGate>,
    pub recovery_action: ConfigurationRecoveryAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationEditorIssue {
    pub id: String,
    pub code: String,
    pub message_key: String,
    pub semantic_path: String,
    pub severity: ConflictSeverity,
    pub blocking: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationEditorDocument {
    pub content: String,
    pub revision: ConfigurationRevision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FinalEditStatus {
    Clean,
    Modified,
    Conflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CandidateStatus {
    Unsaved,
    PendingValidation,
    Invalid,
    Validated,
    Applied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FinalChangeKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FinalChangeProjection {
    pub edit_id: String,
    pub semantic_path: String,
    pub segments: SemanticPath,
    pub kind: FinalChangeKind,
    pub upstream_value: SemanticValue,
    pub final_value: SemanticValue,
    #[serde(default)]
    pub issues: Vec<ConfigurationEditorIssue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FinalConflictProjection {
    pub reference: ConfigurationConflictReference,
    pub conflict_id: String,
    pub semantic_path: String,
    pub segments: SemanticPath,
    pub kind: FinalMergeConflictKind,
    pub base_value: SemanticValue,
    pub upstream_value: SemanticValue,
    pub user_value: SemanticValue,
    pub can_merge: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationEditorView {
    pub document: ConfigurationEditorDocument,
    pub edit_status: FinalEditStatus,
    pub candidate_status: CandidateStatus,
    #[serde(default)]
    pub changes: Vec<FinalChangeProjection>,
    #[serde(default)]
    pub conflicts: Vec<FinalConflictProjection>,
    pub diagnostics: Vec<ConfigurationDiagnostic>,
    #[serde(default)]
    pub blockers: Vec<ConfigurationGateBlocker>,
    pub can_save: bool,
    pub can_validate: bool,
    pub can_apply: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationWorkspaceView {
    pub editor: ConfigurationEditorView,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationStateView {
    pub kind: ProgramKind,
    pub format: ConfigurationFormat,
    pub state_revision: u64,
    pub generation: u64,
    pub compatibility_profile: CoreCompatibilityProfile,
    pub core_admission: Option<crate::CoreAdmissionReport>,
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
    pub workspace: ConfigurationWorkspaceView,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationWorkspaceSnapshot {
    pub state: ConfigurationStateView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_session: Option<FinalEditorSession>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_result: Option<ConfigurationOperationResult>,
}

pub fn parse_semantic_document(format: ConfigurationFormat, content: &[u8]) -> Result<Value> {
    let value = match format {
        ConfigurationFormat::Jsonc => {
            serde_json::from_slice(&normalize_jsonc(content)).map_err(|error| {
                CamelliaNexusError::new(ErrorCode::ConfigInvalid, "Configuration is not valid JSON")
                    .with_message_key("CONFIGURATION_SYNTAX_INVALID")
                    .with_details(format!("line={}; column={}", error.line(), error.column()))
            })?
        }
        ConfigurationFormat::Yaml => {
            let yaml: serde_yaml_ng::Value =
                serde_yaml_ng::from_slice(content).map_err(|error| {
                    CamelliaNexusError::new(
                        ErrorCode::ConfigInvalid,
                        "Configuration is not valid YAML",
                    )
                    .with_message_key("CONFIGURATION_SYNTAX_INVALID")
                    .with_details(error.location().map_or_else(
                        || "format=yaml".into(),
                        |location| {
                            format!("line={}; column={}", location.line(), location.column())
                        },
                    ))
                })?;
            serde_json::to_value(yaml).map_err(|_| {
                CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    "YAML configuration contains unsupported non-string keys",
                )
                .with_message_key("CONFIGURATION_SYNTAX_INVALID")
            })?
        }
    };
    ensure_root_mapping(&value)?;
    Ok(value)
}

fn validate_static_candidate(kind: ProgramKind, root: &Value) -> Result<()> {
    fn path_key(parent: &str, key: &str) -> String {
        format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
    }

    fn known_port_path(kind: ProgramKind, parent: &str, key: &str) -> bool {
        matches!(
            key,
            "listen_port" | "server_port" | "api_port" | "metrics_port"
        ) || key.ends_with("-port")
            || (key == "port"
                && (kind == ProgramKind::Mihomo
                    || parent.starts_with("/inbounds")
                    || parent.starts_with("/outbounds")))
    }

    fn invalid(path: &str, details: impl Into<String>) -> CamelliaNexusError {
        CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "Configuration candidate failed static validation",
        )
        .with_message_key("CONFIGURATION_STATIC_INVALID")
        .with_details(format!("{path}: {}", details.into()))
    }

    fn walk(kind: ProgramKind, value: &Value, path: &str) -> Result<()> {
        match value {
            Value::Object(object) => {
                for (key, child) in object {
                    let child_path = path_key(path, key);
                    if known_port_path(kind, path, key) {
                        let valid = child
                            .as_u64()
                            .is_some_and(|port| (1..=65_535).contains(&port));
                        if !valid {
                            return Err(invalid(
                                &child_path,
                                "port must be an integer between 1 and 65535",
                            ));
                        }
                    }
                    walk(kind, child, &child_path)?;
                }
            }
            Value::Array(values) => {
                let mut identities = BTreeSet::new();
                for (index, child) in values.iter().enumerate() {
                    let child_path = if let Some((field, identity)) = semantic_identity(child) {
                        if !identities.insert((field.clone(), identity.clone())) {
                            return Err(invalid(
                                &format!("{path}[{field}={identity}]"),
                                "semantic identity is duplicated",
                            ));
                        }
                        format!("{path}[{field}={identity}]")
                    } else {
                        format!("{path}/{index}")
                    };
                    walk(kind, child, &child_path)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    if !root.is_object() {
        return Err(invalid("/", "configuration root must be an object"));
    }
    walk(kind, root, "")
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

    #[test]
    fn syntax_diagnostics_do_not_echo_invalid_values_or_mapping_keys() {
        for (format, content) in [
            (
                ConfigurationFormat::Jsonc,
                "{\"fixture-private-token\": invalid}",
            ),
            (
                ConfigurationFormat::Yaml,
                "fixture-private-token: 1\nfixture-private-token: 2",
            ),
            (ConfigurationFormat::Yaml, "? [fixture-private-token]\n: 1"),
        ] {
            let error = parse_semantic_document(format, content.as_bytes()).unwrap_err();
            assert_eq!(
                error.message_key.as_deref(),
                Some("CONFIGURATION_SYNTAX_INVALID")
            );
            assert!(
                !serde_json::to_string(&error)
                    .unwrap()
                    .contains("fixture-private-token")
            );
        }
    }

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
            candidate_generation: state.generation,
            validator_contract_revision: crate::CORE_IMPLEMENTATION_REVISION.into(),
            native_accepted: true,
            validated_unix_ms: 1,
        }
    }

    fn replace_test_base(state: &mut ConfigurationState, value: Value, created_unix_ms: u64) {
        let content = serialize_semantic_document(state.format, &value).expect("serialize base");
        state.base = ConfigurationCandidate {
            revision: ConfigurationRevision::new(
                state.generation.saturating_add(1),
                &content,
                created_unix_ms,
            ),
            content,
            compatibility_profile_hash: state.compatibility_profile.profile_hash.clone(),
            validation: CandidateValidationStatus::Pending,
            validation_evidence: None,
            diagnostics: Vec::new(),
            conflicts: Vec::new(),
        };
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
                && conflict.source_ids == ["a".to_owned(), "b".to_owned()]
        }));
        assert!(result.conflicts.iter().any(|conflict| {
            conflict.semantic_path == "/outbounds[tag=same]/type"
                && conflict.message_key.as_deref() == Some("SOURCE_VALUE_CONFLICT")
        }));
    }

    #[test]
    fn source_conflict_names_every_contributing_source_on_one_path() {
        let result = merge_configuration_sources(
            ProgramKind::Xray,
            &[
                snapshot(ProgramKind::Xray, "a", r#"{"log":{"loglevel":"info"}}"#),
                snapshot(ProgramKind::Xray, "b", r#"{"log":{"loglevel":"debug"}}"#),
                snapshot(ProgramKind::Xray, "c", r#"{"log":{"loglevel":"warning"}}"#),
            ],
        )
        .expect("merge");
        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].semantic_path, "/log/loglevel");
        assert_eq!(
            result.conflicts[0].source_ids,
            ["a".to_owned(), "b".to_owned(), "c".to_owned()]
        );
        assert_eq!(
            result.conflicts[0].effective_value,
            Some(Value::from("warning"))
        );
    }

    #[test]
    fn source_conflict_preview_tracks_identity_replacement_and_parent_conflicts() {
        let replaced = merge_configuration_sources(
            ProgramKind::Mihomo,
            &[
                snapshot(
                    ProgramKind::Mihomo,
                    "a",
                    "proxies:\n  - name: p\n    server: first",
                ),
                snapshot(
                    ProgramKind::Mihomo,
                    "b",
                    "proxies:\n  - name: p\n    server: second",
                ),
                snapshot(ProgramKind::Mihomo, "c", "proxies:\n  - name: p"),
            ],
        )
        .expect("merge");
        assert_eq!(replaced.conflicts.len(), 1);
        assert_eq!(
            replaced.conflicts[0].semantic_path,
            "/proxies[name=p]/server"
        );
        assert_eq!(replaced.conflicts[0].effective_value, None);

        let parent = merge_configuration_sources(
            ProgramKind::Mihomo,
            &[
                snapshot(ProgramKind::Mihomo, "a", "log:\n  level: info"),
                snapshot(ProgramKind::Mihomo, "b", "log:\n  level: debug"),
                snapshot(ProgramKind::Mihomo, "c", "log: null"),
            ],
        )
        .expect("merge");
        assert_eq!(parent.conflicts.len(), 1);
        assert_eq!(parent.conflicts[0].semantic_path, "/log");
        assert_eq!(parent.conflicts[0].effective_value, Some(Value::Null));
        assert_eq!(
            parent.conflicts[0].source_ids,
            ["a".to_owned(), "c".to_owned(), "b".to_owned()]
        );
    }

    #[test]
    fn source_conflicts_keep_distinct_structural_paths_with_similar_labels() {
        let result = merge_configuration_sources(
            ProgramKind::Mihomo,
            &[
                snapshot(
                    ProgramKind::Mihomo,
                    "a",
                    r#"{"proxies":[{"name":"edge","server":"first"}],"proxies[name=edge]":{"server":"first"}}"#,
                ),
                snapshot(
                    ProgramKind::Mihomo,
                    "b",
                    r#"{"proxies":[{"name":"edge","server":"second"}],"proxies[name=edge]":{"server":"second"}}"#,
                ),
            ],
        )
        .expect("merge");
        assert_eq!(result.conflicts.len(), 2);
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
    fn semantic_three_way_rebase_merges_disjoint_changes_and_detects_same_path_conflict() {
        let base = serde_json::json!({"a":1,"b":1});
        let user = serde_json::json!({"a":9,"b":1});
        let updated = serde_json::json!({"a":1,"b":2});
        let result = rebase_final_document(&base, &user, &updated);
        assert!(result.conflicts.is_empty());
        assert_eq!(result.document, serde_json::json!({"a":9,"b":2}));

        let user = serde_json::json!({"a":3,"b":1});
        let updated = serde_json::json!({"a":2,"b":1});
        let result = rebase_final_document(&base, &user, &updated);
        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].semantic_path, "/a");
        // An unresolved conflict is safe-by-default: the effective preview
        // follows the updated upstream while retaining Mine in conflict
        // metadata for an explicit Keep mine resolution.
        assert_eq!(result.document["a"], 2);
        assert_eq!(
            result.conflicts[0].user_value,
            SemanticValue::Present(json!(3))
        );
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
        let result = rebase_final_document(&base, &user, &updated);
        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(
            result.conflicts[0].path,
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
        assert_eq!(result.document["outbounds"][0]["server"], "new");
        assert_eq!(
            result.conflicts[0].user_value,
            SemanticValue::Present(json!("mine"))
        );
        assert_eq!(result.document["outbounds"][2]["tag"], "c");
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
        let rejected = SourceSnapshot::parse_share(
            "share",
            "Share links",
            &CoreTargetIdentity::unknown(ProgramKind::Mihomo, None),
            b"vmess://unsupported",
            2,
        )
        .unwrap_err();
        assert_eq!(
            rejected.message_key.as_deref(),
            Some("SOURCE_NO_COMPATIBLE_ITEMS")
        );
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
        state.mark_candidate_saved().expect("save candidate");
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
            .mark_candidate_saved()
            .expect("save changed candidate");
        state
            .mark_validation(
                false,
                vec![ConfigurationDiagnostic {
                    location: None,
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
    fn format_only_rebuild_preserves_saved_bytes_and_retarget_requires_only_new_validation() {
        let kind = ProgramKind::Xray;
        let content = r#"{"log":{"loglevel":"info"}}"#;
        let mut merge =
            merge_configuration_sources(kind, &[snapshot(kind, "source", content)]).unwrap();
        merge.content = content.into();
        merge.content_hash = hash_bytes(content.as_bytes());
        let mut state =
            ConfigurationState::from_merge(kind, 1, 1, merge, compatibility_profile(kind)).unwrap();
        state.mark_candidate_saved().unwrap();
        state
            .mark_validation(true, Vec::new(), Some(validation_evidence(&state)))
            .unwrap();
        state.mark_applied().unwrap();
        let generation = state.generation;
        let applied = state.applied.clone();
        let evidence = state.desired.validation_evidence.clone();
        state.rebuild_desired(2).unwrap();
        assert_eq!(state.desired.content, content);
        assert_eq!(state.generation, generation);
        assert_eq!(state.desired.validation_evidence, evidence);

        state.compatibility_profile.profile_hash = "different-profile".into();
        state.rebuild_desired(3).unwrap();
        assert_eq!(state.desired.content, content);
        assert!(state.candidate_is_saved());
        assert_eq!(state.desired.validation, CandidateValidationStatus::Pending);
        assert!(state.desired.validation_evidence.is_none());
        assert_eq!(state.generation, generation + 1);
        assert_eq!(state.applied, applied);
        assert_eq!(state.last_known_good, applied);
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
        state.mark_candidate_saved().expect("save candidate");
        state
            .mark_validation(true, Vec::new(), Some(validation_evidence(&state)))
            .expect("validation");
        state
            .desired
            .validation_evidence
            .as_mut()
            .expect("evidence")
            .config_hash = "stale-config-hash".into();

        assert!(!state.view().workspace.editor.can_apply);
        let error = state.mark_applied().expect_err("stale evidence must fail");
        assert_eq!(error.code, ErrorCode::ConfigConflict);
        assert!(state.applied.is_none());
        assert!(state.last_known_good.is_none());
    }

    #[test]
    fn returning_to_identical_content_requires_current_generation_evidence() {
        let kind = ProgramKind::SingBox;
        let merge = merge_configuration_sources(
            kind,
            &[snapshot(kind, "source", r#"{"log":{"level":"info"}}"#)],
        )
        .unwrap();
        let mut state =
            ConfigurationState::from_merge(kind, 1, 1, merge, compatibility_profile(kind)).unwrap();
        state.mark_candidate_saved().unwrap();
        let old_evidence = validation_evidence(&state);
        state
            .mark_validation(true, Vec::new(), Some(old_evidence.clone()))
            .unwrap();
        state.mark_applied().unwrap();
        let applied = state.applied.clone();
        state.guided_intent.set("logging.level", json!("debug"));
        state.rebuild_desired(2).unwrap();
        state.guided_intent.set("logging.level", json!("info"));
        state.rebuild_desired(3).unwrap();
        state.mark_candidate_saved().unwrap();
        assert_eq!(
            state.desired.revision.content_hash,
            old_evidence.config_hash
        );
        assert_ne!(state.generation, old_evidence.candidate_generation);
        let error = state
            .mark_validation(true, Vec::new(), Some(old_evidence))
            .unwrap_err();
        assert_eq!(
            error.message_key.as_deref(),
            Some("CORE_VALIDATION_EVIDENCE_STALE")
        );
        assert_eq!(state.applied, applied);
        assert_eq!(state.last_known_good, applied);
        assert!(!state.view().workspace.editor.can_apply);
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
            &FinalEditState::default(),
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
    fn guided_projection_keeps_the_upstream_value_when_final_configuration_edits_the_path() {
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
            .set("logging.level", Value::String("fatal".into()));
        state.rebuild_desired(2).expect("Guided edit");
        state
            .replace_final_from_edited(br#"{"log":{"level":"error"}}"#, 3)
            .expect("Final configuration edit");

        let projection = state
            .view()
            .guided_projection
            .into_iter()
            .find(|projection| projection.setting_id == "logging.level")
            .expect("logging projection");

        assert_eq!(projection.status, GuidedProjectionStatus::FinalEdit);
        assert_eq!(projection.value, Some(Value::String("fatal".into())));
        assert_eq!(projection.intent_value, Some(Value::String("fatal".into())));
        assert_eq!(
            parse_semantic_document(state.format, state.desired.content.as_bytes())
                .expect("desired")["log"]["level"],
            Value::String("error".into())
        );
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
        let mut intent = ManagedIntegrationIntent::default();
        sync_managed_dashboard_intent(&mut intent, &managed);
        let mut sing_box = serde_json::json!({
            "services": [{"tag":"user-service"}],
            "experimental": {"clash_api": {"secret":"keep"}}
        });
        apply_dashboard_intent(ProgramKind::SingBox, &mut sing_box, &intent)
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
        let mut xray =
            serde_json::json!({"api":{"services":["UserService"]},"custom":{"keep":true}});
        apply_dashboard_intent(ProgramKind::Xray, &mut xray, &intent)
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
        let mut mihomo = serde_json::json!({"secret":"keep"});
        apply_dashboard_intent(ProgramKind::Mihomo, &mut mihomo, &intent)
            .expect("mihomo dashboard intent");
        assert_eq!(mihomo["secret"], "keep");
        assert_eq!(mihomo["external-controller"], "127.0.0.1:9092");
        assert_eq!(mihomo["external-ui"], "camellia-nexus-mihomo-dashboard");
    }

    #[test]
    fn common_intent_rejects_managed_integration_settings() {
        let mut guided = GuidedIntent::default();
        guided.set("dashboard.singBoxApi.listenPort", Value::from(9090));

        let error = apply_guided_intent(ProgramKind::SingBox, &serde_json::json!({}), &guided)
            .expect_err("managed setting must not enter Common Intent");

        assert_eq!(error.code, ErrorCode::ConfigInvalid);
        assert!(error.message.contains("Unsupported Guided setting"));
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
        assert!(diff_intent_operations(&base, &edited).is_empty());

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
    }

    #[test]
    fn final_editor_static_validation_rejects_invalid_ports_and_duplicate_identities() {
        let merge = merge_configuration_sources(
            ProgramKind::Xray,
            &[snapshot(
                ProgramKind::Xray,
                "base",
                r#"{"inbounds":[{"tag":"local","port":1080}]}"#,
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
        let original_generation = state.generation;
        let original_content = state.desired.content.clone();

        for invalid_document in [
            r#"{"inbounds":[{"tag":"local","port":70000}]}"#,
            r#"{"inbounds":[{"tag":"local","port":"not-a-port"}]}"#,
            r#"{"inbounds":[{"tag":"local","port":1080},{"tag":"local","port":1081}]}"#,
        ] {
            let error = state
                .replace_final_from_edited(invalid_document.as_bytes(), 2)
                .expect_err("invalid candidate");
            assert_eq!(error.code, ErrorCode::ConfigInvalid);
            assert_eq!(
                error.message_key.as_deref(),
                Some("CONFIGURATION_STATIC_INVALID")
            );
            assert_eq!(state.generation, original_generation);
            assert_eq!(state.desired.content, original_content);
            assert!(state.final_edit.edits.is_empty());
        }

        state
            .replace_final_from_edited(
                br#"{"inbounds":[{"tag":"local","port":1080}],"future_extension":{"port":"opaque"}}"#,
                3,
            )
            .expect("unknown extensions remain available to the native validator");
        assert_eq!(state.desired.validation, CandidateValidationStatus::Pending);
    }

    #[test]
    fn disabled_managed_integration_is_not_influenced_by_sibling_intent() {
        let merge = merge_configuration_sources(
            ProgramKind::SingBox,
            &[snapshot(ProgramKind::SingBox, "base", r#"{}"#)],
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
                sing_box_dashboard: Some(SingBoxDashboardSpec {
                    listen_port: 9090,
                    update_interval: "1d".into(),
                }),
                sing_box_clash_dashboard: Some(SingBoxClashDashboardSpec {
                    listen_port: 9091,
                    download_url: None,
                }),
                ..ManagedConfigSpec::default()
            },
        );
        state.rebuild_desired(2).expect("enable integrations");

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
        state.rebuild_desired(3).expect("disable native dashboard");
        let native = state
            .view()
            .managed_integrations
            .into_iter()
            .find(|projection| projection.integration_id == "dashboard.singBoxApi")
            .expect("native projection");
        assert_eq!(native.status, ManagedIntegrationStatus::Inactive);
        assert!(!native.effective_enabled);
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
        state.mark_candidate_saved().expect("save candidate");
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
        state.mark_candidate_saved().expect("save candidate");
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
            .mark_candidate_saved()
            .expect("save initial candidate");
        state
            .mark_validation(true, Vec::new(), Some(validation_evidence(&state)))
            .expect("initial validation");
        state.mark_applied().expect("initial apply");
        let applied = state.applied.clone();
        state.desired.validation = CandidateValidationStatus::Invalid;
        state.desired.validation_evidence = None;
        state.desired.diagnostics = vec![ConfigurationDiagnostic {
            location: None,
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
                message_key: None,
                observed_unix_ms: Some(2),
            },
        );
        let generation = state.generation;

        state.rebuild_desired(3).expect("source recovery rebuild");

        assert_eq!(state.generation, generation);
        assert_eq!(state.desired.validation, CandidateValidationStatus::Pending);
        assert!(state.desired.diagnostics.is_empty());
        assert_eq!(state.applied, applied);
        assert_eq!(state.last_known_good, applied);
    }

    #[test]
    fn workspace_final_document_is_the_exact_desired_candidate() {
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
        state.desired.content = "{\n  \"log\": { \"loglevel\": \"warning\" }\n}\n".into();

        let view = state.view();
        assert_eq!(
            view.workspace.editor.document.content,
            state.desired.content
        );
        assert_eq!(
            view.workspace.editor.document.revision, state.desired.revision,
            "the editor must use the same candidate revision as Desired"
        );
        assert_eq!(view.workspace.editor.edit_status, FinalEditStatus::Clean);
    }

    #[test]
    fn workspace_gate_blockers_preserve_repair_and_exact_validation_paths() {
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

        let unsaved = state.view().workspace.editor;
        assert!(unsaved.can_save);
        assert!(!unsaved.can_validate);
        assert!(!unsaved.can_apply);
        assert!(unsaved.blockers.iter().any(|blocker| {
            blocker.code == "CONFIGURATION_CANDIDATE_UNSAVED"
                && blocker.blocks == vec![ConfigurationGate::Validate, ConfigurationGate::Apply]
        }));

        let mut source_invalid_state = state.clone();
        source_invalid_state.desired.validation = CandidateValidationStatus::Invalid;
        source_invalid_state.desired.diagnostics = vec![ConfigurationDiagnostic {
            location: None,
            code: "SOURCE_INVALID".into(),
            message: "source invalid".into(),
            message_key: Some("SOURCE_INVALID".into()),
            scope: ConfigurationIssueScope::sources("source"),
            details: None,
        }];
        let source_invalid = source_invalid_state.view().workspace.editor;
        assert!(!source_invalid.can_save);
        assert!(!source_invalid.can_validate);
        assert!(!source_invalid.can_apply);
        assert!(source_invalid.blockers.iter().any(|blocker| {
            blocker.code == "SOURCE_INVALID"
                && blocker.blocks
                    == vec![
                        ConfigurationGate::Save,
                        ConfigurationGate::Validate,
                        ConfigurationGate::Apply,
                    ]
                && blocker.recovery_action == ConfigurationRecoveryAction::ResolveSourceConflict
        }));
        assert!(
            !source_invalid
                .blockers
                .iter()
                .any(|blocker| blocker.code == "CONFIGURATION_CANDIDATE_UNSAVED")
        );

        state.mark_candidate_saved().expect("save candidate");
        let pending = state.view().workspace.editor;
        assert!(pending.can_save);
        assert!(pending.can_validate);
        assert!(!pending.can_apply);
        assert!(pending.blockers.iter().any(|blocker| {
            blocker.code == "CORE_VALIDATION_REQUIRED"
                && blocker.blocks == vec![ConfigurationGate::Apply]
        }));

        state
            .mark_validation(true, Vec::new(), Some(validation_evidence(&state)))
            .expect("validation");
        state
            .desired
            .validation_evidence
            .as_mut()
            .expect("evidence")
            .config_hash = "stale".into();
        let stale = state.view().workspace.editor;
        assert!(stale.can_save);
        assert!(stale.can_validate);
        assert!(!stale.can_apply);
        assert!(stale.blockers.iter().any(|blocker| {
            blocker.code == "CORE_VALIDATION_EVIDENCE_STALE"
                && blocker.blocks == vec![ConfigurationGate::Apply]
        }));
    }

    #[test]
    fn final_edits_follow_disjoint_upstream_changes_without_a_decision_prompt() {
        let merge = merge_configuration_sources(
            ProgramKind::Xray,
            &[snapshot(ProgramKind::Xray, "source", r#"{"a":1,"b":1}"#)],
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
            .replace_final_from_edited(br#"{"a":9,"b":1}"#, 2)
            .expect("final edit");

        replace_test_base(
            &mut state,
            json!({"a": 1, "b": 2, "longSourceField": "x".repeat(40_000)}),
            3,
        );
        state.rebuild_desired(3).expect("upstream rebase");

        let desired = parse_semantic_document(state.format, state.desired.content.as_bytes())
            .expect("desired");
        assert_eq!(desired["a"], 9);
        assert_eq!(desired["b"], 2);
        assert_eq!(
            desired["longSourceField"].as_str().map(str::len),
            Some(40_000)
        );
        assert!(state.final_edit.conflicts.is_empty());
        assert_eq!(state.final_edit.edits.len(), 1);
        assert_eq!(
            state.view().workspace.editor.edit_status,
            FinalEditStatus::Modified
        );
        assert_eq!(
            state.view().workspace.editor.document.content,
            state.desired.content
        );
    }

    #[test]
    fn saved_source_value_masked_by_final_delete_requires_only_a_path_scoped_adoption() {
        let kind = ProgramKind::SingBox;
        let merge = merge_configuration_sources(
            kind,
            &[snapshot(
                kind,
                "source",
                r#"{"log":{"level":"info","timestamp":true},"inbounds":[],"outbounds":[]}"#,
            )],
        )
        .unwrap();
        let mut state =
            ConfigurationState::from_merge(kind, 1, 1, merge, compatibility_profile(kind)).unwrap();
        state.applied = Some(state.desired.clone());
        state.last_known_good = state.applied.clone();
        state
            .replace_final_from_edited(
                br#"{"log":{"level":"warn"},"inbounds":[],"outbounds":[]}"#,
                2,
            )
            .unwrap();
        let applied = state.applied.clone();
        let lkg = state.last_known_good.clone();
        let before = state.clone();
        state.rebuild_desired(3).unwrap();
        assert_eq!(state, before, "identical Source content is not a new write");
        let edit = state
            .final_edit
            .edits
            .iter()
            .find(|entry| display_semantic_path(&entry.path) == "/log/timestamp")
            .unwrap()
            .clone();
        let request = AdoptUpstreamChangeRequest {
            operation_id: "06acdb34-66f8-4db1-a562-321779bc9ca0".into(),
            expected_state_revision: state.state_revision,
            editor_session_id: None,
            expected_draft_revision: None,
            edit_id: edit.edit_id,
            path: edit.path,
        };
        assert!(state.adopt_upstream_change(request.clone(), 4).unwrap());
        let effective =
            parse_semantic_document(state.format, state.desired.content.as_bytes()).unwrap();
        assert_eq!(effective["log"]["timestamp"], true);
        assert_eq!(effective["log"]["level"], "warn");
        assert_eq!(state.applied, applied);
        assert_eq!(state.last_known_good, lkg);
        assert!(
            !state
                .final_edit
                .edits
                .iter()
                .any(|entry| display_semantic_path(&entry.path) == "/log/timestamp")
        );
        let after = state.clone();
        assert!(!state.adopt_upstream_change(request.clone(), 5).unwrap());
        assert_eq!(
            state, after,
            "retrying the same operation is read-equivalent"
        );
        let mismatched = AdoptUpstreamChangeRequest {
            edit_id: "another-edit".into(),
            ..request.clone()
        };
        assert_eq!(
            state
                .adopt_upstream_change(mismatched, 5)
                .unwrap_err()
                .message_key
                .as_deref(),
            Some("CONFIGURATION_OPERATION_MISMATCH")
        );
        let stale = AdoptUpstreamChangeRequest {
            operation_id: "fe2721b8-639f-40a2-ae1d-de6ed6df9424".into(),
            ..request
        };
        assert_eq!(
            state
                .adopt_upstream_change(stale, 6)
                .unwrap_err()
                .message_key
                .as_deref(),
            Some("CONFIGURATION_STATE_STALE")
        );
        assert_eq!(state, after);
    }

    #[test]
    fn adopting_upstream_does_not_discard_an_unfinished_final_editor_draft() {
        let kind = ProgramKind::SingBox;
        let merge = merge_configuration_sources(
            kind,
            &[snapshot(
                kind,
                "source",
                r#"{"log":{"level":"info","timestamp":true},"inbounds":[],"outbounds":[]}"#,
            )],
        )
        .unwrap();
        let mut state =
            ConfigurationState::from_merge(kind, 1, 1, merge, compatibility_profile(kind)).unwrap();
        state
            .replace_final_from_edited(
                br#"{"log":{"level":"info"},"inbounds":[],"outbounds":[]}"#,
                2,
            )
            .unwrap();
        let edit = state.final_edit.edits[0].clone();
        state.editor_session = Some(FinalEditorSession {
            session_id: "session".into(),
            draft_revision: 1,
            based_on_state_revision: state.state_revision,
            based_on_candidate_generation: state.generation,
            base_content: state.desired.content.clone(),
            working_content: "{unfinished".into(),
            conflicts: Vec::new(),
            resolutions: BTreeMap::new(),
            unresolved_conflict_ids: Vec::new(),
            rebase_required: true,
            updated_unix_ms: 3,
        });
        let before = state.clone();
        let request = AdoptUpstreamChangeRequest {
            operation_id: "cafce09d-7f38-41fc-b68c-a23310f96091".into(),
            expected_state_revision: state.state_revision,
            editor_session_id: Some("session".into()),
            expected_draft_revision: Some(1),
            edit_id: edit.edit_id,
            path: edit.path,
        };
        assert_eq!(
            state
                .adopt_upstream_change(request, 4)
                .unwrap_err()
                .message_key
                .as_deref(),
            Some("CONFIGURATION_DRAFT_UNCOMMITTED")
        );
        assert_eq!(state, before);
    }

    #[test]
    fn adopting_a_nonconflicting_path_preserves_another_unresolved_choice() {
        let kind = ProgramKind::Xray;
        let merge =
            merge_configuration_sources(kind, &[snapshot(kind, "source", r#"{"a":1,"b":1}"#)])
                .unwrap();
        let mut state =
            ConfigurationState::from_merge(kind, 1, 1, merge, compatibility_profile(kind)).unwrap();
        state
            .replace_final_from_edited(br#"{"a":9,"b":9}"#, 2)
            .unwrap();
        replace_test_base(&mut state, json!({"a":2,"b":1}), 3);
        state.rebuild_desired(3).unwrap();
        assert_eq!(state.final_edit.conflicts.len(), 1);
        let retained_conflict = state.final_edit.conflicts[0].clone();
        let edit = state
            .final_edit
            .edits
            .iter()
            .find(|entry| display_semantic_path(&entry.path) == "/b")
            .unwrap()
            .clone();
        let request = AdoptUpstreamChangeRequest {
            operation_id: "8e0145cd-3b0b-4680-9e3f-831ff5c0528d".into(),
            expected_state_revision: state.state_revision,
            editor_session_id: None,
            expected_draft_revision: None,
            edit_id: edit.edit_id,
            path: edit.path,
        };
        state.adopt_upstream_change(request, 4).unwrap();
        let effective =
            parse_semantic_document(state.format, state.desired.content.as_bytes()).unwrap();
        assert_eq!(effective, json!({"a":2,"b":1}));
        assert_eq!(state.final_edit.conflicts, vec![retained_conflict]);
        assert!(!state.view().workspace.editor.can_save);
    }

    #[test]
    fn source_timestamp_update_previews_latest_value_when_final_delete_conflicts() {
        let kind = ProgramKind::SingBox;
        let merge = merge_configuration_sources(
            kind,
            &[snapshot(
                kind,
                "source",
                r#"{"log":{"level":"info","timestamp":false},"inbounds":[],"outbounds":[]}"#,
            )],
        )
        .unwrap();
        let mut state =
            ConfigurationState::from_merge(kind, 1, 1, merge, compatibility_profile(kind)).unwrap();
        state
            .replace_final_from_edited(
                br#"{"log":{"level":"info"},"inbounds":[],"outbounds":[]}"#,
                2,
            )
            .unwrap();
        replace_test_base(
            &mut state,
            json!({"log":{"level":"info","timestamp":true},"inbounds":[],"outbounds":[]}),
            3,
        );
        state.rebuild_desired(3).unwrap();
        let effective =
            parse_semantic_document(state.format, state.desired.content.as_bytes()).unwrap();
        assert_eq!(effective["log"]["timestamp"], true);
        assert_eq!(state.final_edit.conflicts.len(), 1);
        let conflict = &state.final_edit.conflicts[0];
        assert_eq!(conflict.semantic_path, "/log/timestamp");
        assert_eq!(conflict.user_value, SemanticValue::Missing);
        assert_eq!(conflict.upstream_value, SemanticValue::Present(json!(true)));
        assert!(!state.view().workspace.editor.can_save);
    }

    #[test]
    fn latest_source_and_intent_writes_win_but_refresh_does_not_reclaim_paths() {
        let kind = ProgramKind::Xray;
        let merge = merge_configuration_sources(
            kind,
            &[snapshot(kind, "source", r#"{"log":{"loglevel":"info"}}"#)],
        )
        .unwrap();
        let mut state =
            ConfigurationState::from_merge(kind, 1, 1, merge, compatibility_profile(kind)).unwrap();
        state.guided_intent.set("logging.level", json!("debug"));
        state.rebuild_desired(2).unwrap();
        replace_test_base(&mut state, json!({"log":{"loglevel":"warning"}}), 3);
        state.rebuild_desired(3).unwrap();
        assert_eq!(
            state.upstream_document().unwrap()["log"]["loglevel"],
            "warning"
        );
        assert_eq!(
            state
                .view()
                .guided_projection
                .iter()
                .find(|value| value.setting_id == "logging.level")
                .unwrap()
                .value,
            Some(json!("warning"))
        );
        state.claim_guided_setting("logging.level").unwrap();
        state.rebuild_desired(4).unwrap();
        assert_eq!(
            state.upstream_document().unwrap()["log"]["loglevel"],
            "debug"
        );
        replace_test_base(
            &mut state,
            json!({"log":{"loglevel":"error"}, "newField":42}),
            5,
        );
        state
            .rebuild_desired_with_source_update(5, SourceUpdateKind::Refresh)
            .unwrap();
        assert_eq!(
            state.upstream_document().unwrap(),
            json!({"log":{"loglevel":"debug"}, "newField":42})
        );
        state.guided_intent.reset("logging.level");
        state.rebuild_desired(6).unwrap();
        assert_eq!(
            state.upstream_document().unwrap()["log"]["loglevel"],
            "error"
        );
        let unchanged = state.clone();
        state
            .rebuild_desired_with_source_update(7, SourceUpdateKind::Refresh)
            .unwrap();
        assert_eq!(state, unchanged);
    }

    #[test]
    fn invalid_draft_rebase_ignores_formatting_but_preserves_changed_upstream_basis() {
        for format in [ConfigurationFormat::Jsonc, ConfigurationFormat::Yaml] {
            let original = json!({"a":1,"b":1});
            let mut draft = FinalEditorSession {
                session_id: "draft".into(),
                draft_revision: 1,
                based_on_state_revision: 1,
                based_on_candidate_generation: 1,
                base_content: original.to_string(),
                working_content: "{unfinished".into(),
                conflicts: Vec::new(),
                resolutions: BTreeMap::new(),
                unresolved_conflict_ids: Vec::new(),
                rebase_required: false,
                updated_unix_ms: 1,
            };
            let formatted = serialize_semantic_document(format, &original).unwrap();
            rebase_final_editor_session(&mut draft, format, &formatted, 2, 1).unwrap();
            assert!(!draft.rebase_required);
            assert_eq!(draft.working_content, "{unfinished");
            assert_eq!(draft.base_content, formatted);
            assert_eq!(draft.based_on_state_revision, 2);
            assert_eq!(draft.draft_revision, 1);

            let updated = serialize_semantic_document(format, &json!({"a":1,"b":2})).unwrap();
            rebase_final_editor_session(&mut draft, format, &updated, 3, 2).unwrap();
            assert!(draft.rebase_required);
            assert_eq!(draft.working_content, "{unfinished");
            assert_eq!(draft.base_content, formatted);
            assert_eq!(draft.based_on_state_revision, 2);

            draft.working_content = r#"{"a":9,"b":1}"#.into();
            rebase_final_editor_session(&mut draft, format, &updated, 3, 2).unwrap();
            assert!(!draft.rebase_required);
            assert!(draft.conflicts.is_empty());
            assert_eq!(
                parse_semantic_document(format, draft.working_content.as_bytes()).unwrap(),
                json!({"a":9,"b":2})
            );
        }
    }

    #[test]
    fn invalid_draft_rebase_ignores_equivalent_dashboard_duration() {
        let content = |duration: &str| {
            format!(
                r#"{{"services":[{{"tag":"{MANAGED_SING_BOX_API_TAG}","dashboard":{{"update_interval":"{duration}"}}}}]}}"#
            )
        };
        let mut draft = FinalEditorSession {
            session_id: "draft".into(),
            draft_revision: 1,
            based_on_state_revision: 1,
            based_on_candidate_generation: 1,
            base_content: content("1d"),
            working_content: "{unfinished".into(),
            conflicts: Vec::new(),
            resolutions: BTreeMap::new(),
            unresolved_conflict_ids: Vec::new(),
            rebase_required: false,
            updated_unix_ms: 1,
        };
        let updated = content("24h0m0s");
        rebase_final_editor_session(&mut draft, ConfigurationFormat::Jsonc, &updated, 2, 1)
            .unwrap();
        assert!(!draft.rebase_required);
        assert_eq!(draft.base_content, updated);
        assert_eq!(draft.working_content, "{unfinished");
        assert_eq!(draft.based_on_state_revision, 2);
    }

    #[test]
    fn draft_conflicts_survive_repeated_updates_and_resolve_missing_values() {
        let mut draft = FinalEditorSession {
            session_id: "draft".into(),
            draft_revision: 1,
            based_on_state_revision: 1,
            based_on_candidate_generation: 1,
            base_content: r#"{"a":1,"b":1}"#.into(),
            working_content: r#"{"a":9,"b":1}"#.into(),
            conflicts: Vec::new(),
            resolutions: BTreeMap::new(),
            unresolved_conflict_ids: Vec::new(),
            rebase_required: false,
            updated_unix_ms: 1,
        };
        rebase_final_editor_session(
            &mut draft,
            ConfigurationFormat::Jsonc,
            r#"{"a":2,"b":1}"#,
            2,
            2,
        )
        .unwrap();
        let conflict_id = draft.conflicts[0].conflict_id.clone();
        rebase_final_editor_session(
            &mut draft,
            ConfigurationFormat::Jsonc,
            r#"{"a":3,"b":2}"#,
            3,
            3,
        )
        .unwrap();
        assert_eq!(draft.conflicts.len(), 1);
        assert_eq!(draft.conflicts[0].conflict_id, conflict_id);
        assert_eq!(
            draft.conflicts[0].user_value,
            SemanticValue::Present(json!(9))
        );
        assert_eq!(
            draft.conflicts[0].upstream_value,
            SemanticValue::Present(json!(3))
        );
        resolve_final_editor_conflict(
            &mut draft,
            ConfigurationFormat::Jsonc,
            &conflict_id,
            FinalConflictResolution::ManualEdit {
                value: SemanticValue::Missing,
            },
        )
        .unwrap();
        assert!(draft.unresolved_conflict_ids.is_empty());
        rebase_final_editor_session(
            &mut draft,
            ConfigurationFormat::Jsonc,
            r#"{"a":3,"b":4}"#,
            4,
            4,
        )
        .unwrap();
        assert_eq!(
            parse_semantic_document(ConfigurationFormat::Jsonc, draft.working_content.as_bytes())
                .unwrap(),
            json!({"b":4})
        );
        assert!(draft.conflicts.is_empty());
    }

    #[test]
    fn disjoint_deletion_and_identity_additions_merge_without_false_conflicts() {
        let result = rebase_final_document(
            &json!({"a":1,"b":1}),
            &json!({"a":1,"b":2}),
            &json!({"b":1}),
        );
        assert_eq!(result.document, json!({"b":2}));
        assert!(result.conflicts.is_empty());
        let result = rebase_final_document(
            &json!({"items":[]}),
            &json!({"items":[{"tag":"mine","value":1}]}),
            &json!({"items":[{"tag":"updated","value":2}]}),
        );
        assert!(result.conflicts.is_empty());
        assert_eq!(result.document["items"].as_array().unwrap().len(), 2);
        let result = rebase_final_document(&json!({}), &json!({"a":null}), &json!({"a":1}));
        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].base_value, SemanticValue::Missing);
        assert_eq!(
            result.conflicts[0].user_value,
            SemanticValue::Present(Value::Null)
        );
    }

    #[test]
    fn details_reclaims_only_changed_registered_fields_from_sources() {
        let kind = ProgramKind::Mihomo;
        let merge = merge_configuration_sources(kind, &[snapshot(kind, "source", "{}")]).unwrap();
        let mut state =
            ConfigurationState::from_merge(kind, 1, 1, merge, compatibility_profile(kind)).unwrap();
        state
            .managed_intent
            .values
            .insert("dashboard.mihomo.listenPort".into(), json!(9090));
        state.rebuild_desired(2).unwrap();
        replace_test_base(
            &mut state,
            json!({"external-controller":"127.0.0.1:9091", "external-ui":"user-ui", "unknown":42}),
            3,
        );
        state.rebuild_desired(3).unwrap();
        assert_eq!(
            state.upstream_document().unwrap()["external-controller"],
            "127.0.0.1:9091"
        );
        let projection = project_managed_integrations(
            kind,
            &state.upstream_document().unwrap(),
            &state.upstream_document().unwrap(),
            &state.managed_intent,
            &state.final_edit,
            &[],
        );
        let port = projection[0]
            .settings
            .iter()
            .find(|setting| setting.setting_id == "dashboard.mihomo.listenPort")
            .unwrap();
        assert_eq!(port.effective_value, SemanticValue::Present(json!(9091)));
        assert!(port.can_use_saved_value);
        state
            .claim_managed_setting("dashboard.mihomo.listenPort")
            .unwrap();
        state.rebuild_desired(4).unwrap();
        assert_eq!(
            state.upstream_document().unwrap()["external-controller"],
            "127.0.0.1:9090"
        );
        assert_eq!(state.upstream_document().unwrap()["external-ui"], "user-ui");
        let revision = state.state_revision;
        let generation = state.generation;
        state
            .claim_managed_setting("dashboard.mihomo.listenPort")
            .unwrap();
        state.rebuild_desired(4).unwrap();
        assert_eq!(state.state_revision, revision);
        assert_eq!(state.generation, generation);
        state
            .managed_intent
            .values
            .insert("dashboard.mihomo.listenPort".into(), json!(9092));
        state.rebuild_desired(4).unwrap();
        let upstream = state.upstream_document().unwrap();
        assert_eq!(upstream["external-controller"], "127.0.0.1:9092");
        assert_eq!(upstream["external-ui"], "user-ui");
        assert_eq!(upstream["unknown"], 42);
    }

    #[test]
    fn explicit_source_parent_delete_masks_earlier_intent_and_can_be_edited_again() {
        let kind = ProgramKind::Xray;
        let merge = merge_configuration_sources(
            kind,
            &[snapshot(kind, "source", r#"{"log":{"loglevel":"info"}}"#)],
        )
        .unwrap();
        let mut state =
            ConfigurationState::from_merge(kind, 1, 1, merge, compatibility_profile(kind)).unwrap();
        state.guided_intent.set("logging.level", json!("debug"));
        state.rebuild_desired(2).unwrap();
        replace_test_base(&mut state, json!({"unrelated":true}), 3);
        state.rebuild_desired(3).unwrap();
        assert!(state.upstream_document().unwrap().get("log").is_none());
        state.claim_guided_setting("logging.level").unwrap();
        state.rebuild_desired(4).unwrap();
        assert_eq!(
            state.upstream_document().unwrap(),
            json!({"log":{"loglevel":"debug"}, "unrelated":true})
        );
    }

    #[test]
    fn source_additions_do_not_claim_untouched_sibling_intent_paths() {
        let kind = ProgramKind::SingBox;
        let merge = merge_configuration_sources(kind, &[snapshot(kind, "source", "{}")]).unwrap();
        let mut state =
            ConfigurationState::from_merge(kind, 1, 1, merge, compatibility_profile(kind)).unwrap();
        state.guided_intent.set("logging.level", json!("debug"));
        state.rebuild_desired(2).unwrap();
        replace_test_base(&mut state, json!({"log":{"timestamp":true}}), 3);
        state.rebuild_desired(3).unwrap();
        assert_eq!(
            state.upstream_document().unwrap(),
            json!({"log":{"level":"debug","timestamp":true}})
        );
    }

    #[test]
    fn explicit_empty_source_container_masks_earlier_child_contributions() {
        let kind = ProgramKind::SingBox;
        let merge = merge_configuration_sources(kind, &[snapshot(kind, "source", "{}")]).unwrap();
        let mut state =
            ConfigurationState::from_merge(kind, 1, 1, merge, compatibility_profile(kind)).unwrap();
        state.guided_intent.set("logging.level", json!("debug"));
        state.rebuild_desired(2).unwrap();
        replace_test_base(&mut state, json!({"log": {}}), 3);
        state.rebuild_desired(3).unwrap();
        assert_eq!(state.upstream_document().unwrap(), json!({"log": {}}));
        state.claim_guided_setting("logging.level").unwrap();
        state.rebuild_desired(4).unwrap();
        assert_eq!(
            state.upstream_document().unwrap(),
            json!({"log": {"level": "debug"}})
        );
    }

    #[test]
    fn divergent_same_path_changes_block_every_candidate_gate_and_keep_applied() {
        let merge = merge_configuration_sources(
            ProgramKind::Xray,
            &[snapshot(
                ProgramKind::Xray,
                "source",
                r#"{"route":{"final":"base"}}"#,
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
        state.mark_candidate_saved().expect("save initial");
        state
            .mark_validation(true, Vec::new(), Some(validation_evidence(&state)))
            .expect("validate initial");
        state.mark_applied().expect("apply initial");
        let applied = state.applied.clone();
        state
            .replace_final_from_edited(br#"{"route":{"final":"mine"}}"#, 2)
            .expect("edit");

        replace_test_base(&mut state, json!({"route": {"final": "updated"}}), 3);
        state.rebuild_desired(3).expect("conflicting rebase");

        let preview = parse_semantic_document(state.format, state.desired.content.as_bytes())
            .expect("preview");
        assert_eq!(preview["route"]["final"], "updated");
        assert_eq!(state.final_edit.conflicts.len(), 1);
        assert_eq!(
            state.final_edit.conflicts[0].user_value,
            SemanticValue::Present(json!("mine"))
        );
        let editor = state.view().workspace.editor;
        assert_eq!(editor.edit_status, FinalEditStatus::Conflict);
        assert!(!editor.can_save);
        assert!(!editor.can_validate);
        assert!(!editor.can_apply);
        assert_eq!(state.applied, applied);
        assert_eq!(state.last_known_good, applied);
    }

    #[test]
    fn final_conflict_resolutions_are_path_scoped_reentrant_and_cancel_is_read_only() {
        fn conflicted_state() -> ConfigurationState {
            let merge = merge_configuration_sources(
                ProgramKind::Xray,
                &[snapshot(
                    ProgramKind::Xray,
                    "source",
                    r#"{"route":{"final":"base"},"log":{"loglevel":"info"}}"#,
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
                .replace_final_from_edited(
                    br#"{"route":{"final":"mine"},"log":{"loglevel":"debug"}}"#,
                    2,
                )
                .expect("edit");
            replace_test_base(
                &mut state,
                json!({"route": {"final": "updated"}, "log": {"loglevel": "info"}}),
                3,
            );
            state.rebuild_desired(3).expect("conflict");
            state
        }

        let original = conflicted_state();
        let conflict_id = original.final_edit.conflicts[0].conflict_id.clone();
        let mut mixed = original.clone();
        let conflict = mixed.final_edit.conflicts[0].clone();
        mixed.editor_session = Some(FinalEditorSession {
            session_id: uuid::Uuid::new_v4().to_string(),
            draft_revision: 1,
            based_on_state_revision: mixed.state_revision,
            based_on_candidate_generation: mixed.generation,
            base_content: mixed.desired.content.clone(),
            working_content: mixed.desired.content.clone(),
            conflicts: vec![conflict.clone()],
            resolutions: BTreeMap::new(),
            unresolved_conflict_ids: vec![conflict.conflict_id.clone()],
            rebase_required: false,
            updated_unix_ms: 1,
        });
        let projected = mixed.view().workspace.editor.conflicts;
        assert_eq!(projected.len(), 2);
        assert_ne!(projected[0].conflict_id, projected[1].conflict_id);
        assert_eq!(
            projected[0].reference.conflict_id,
            projected[1].reference.conflict_id
        );
        let session = mixed.editor_session.as_ref().unwrap();
        let draft_request = ResolveConfigurationConflictRequest {
            operation_id: uuid::Uuid::new_v4().to_string(),
            expected_state_revision: mixed.state_revision,
            editor_session_id: Some(session.session_id.clone()),
            expected_draft_revision: Some(session.draft_revision),
            action: ConfigurationConflictAction::Resolve {
                reference: projected[1].reference.clone(),
                resolution: FinalConflictResolution::KeepMine,
            },
        };
        mixed
            .resolve_configuration_conflict(draft_request, 4)
            .unwrap();
        assert_eq!(mixed.final_edit.conflicts.len(), 1);
        assert!(
            mixed
                .editor_session
                .as_ref()
                .unwrap()
                .unresolved_conflict_ids
                .is_empty()
        );
        let unchanged = original.clone();
        assert_eq!(original, unchanged, "Cancel performs no Core transaction");

        let mut accepted = original.clone();
        accepted
            .resolve_final_conflict(&conflict_id, FinalConflictResolution::AcceptUpstream, 4)
            .expect("accept updated");
        let accepted_value =
            parse_semantic_document(accepted.format, accepted.desired.content.as_bytes())
                .expect("accepted");
        assert_eq!(accepted_value["route"]["final"], "updated");
        assert_eq!(accepted_value["log"]["loglevel"], "debug");
        assert!(accepted.final_edit.conflicts.is_empty());

        let mut kept = original.clone();
        kept.resolve_final_conflict(&conflict_id, FinalConflictResolution::KeepMine, 4)
            .expect("keep mine");
        let kept_value =
            parse_semantic_document(kept.format, kept.desired.content.as_bytes()).expect("kept");
        assert_eq!(kept_value["route"]["final"], "mine");
        assert_eq!(kept_value["log"]["loglevel"], "debug");
        assert!(kept.final_edit.conflicts.is_empty());

        let mut merged = original;
        merged
            .resolve_final_conflict(
                &conflict_id,
                FinalConflictResolution::ManualEdit {
                    value: SemanticValue::Present(json!("merged")),
                },
                4,
            )
            .expect("manual merge");
        let merged_value =
            parse_semantic_document(merged.format, merged.desired.content.as_bytes())
                .expect("merged");
        assert_eq!(merged_value["route"]["final"], "merged");
        assert_eq!(merged_value["log"]["loglevel"], "debug");

        let mut transactional = conflicted_state();
        let conflict = transactional.final_edit.conflicts[0].clone();
        let operation_id = uuid::Uuid::new_v4().to_string();
        let request = ResolveConfigurationConflictRequest {
            operation_id: operation_id.clone(),
            expected_state_revision: transactional.state_revision,
            editor_session_id: None,
            expected_draft_revision: None,
            action: ConfigurationConflictAction::Resolve {
                reference: ConfigurationConflictReference::new(
                    ConfigurationConflictOrigin::Candidate,
                    &conflict,
                ),
                resolution: FinalConflictResolution::KeepMine,
            },
        };
        assert!(
            transactional
                .resolve_configuration_conflict(request.clone(), 5)
                .unwrap()
        );
        let committed = transactional.clone();
        assert!(
            !transactional
                .resolve_configuration_conflict(request.clone(), 6)
                .unwrap()
        );
        assert_eq!(transactional, committed);
        let mut mismatched = request;
        mismatched.action = ConfigurationConflictAction::Resolve {
            reference: ConfigurationConflictReference::new(
                ConfigurationConflictOrigin::Candidate,
                &conflict,
            ),
            resolution: FinalConflictResolution::AcceptUpstream,
        };
        assert_eq!(
            transactional
                .resolve_configuration_conflict(mismatched, 6)
                .unwrap_err()
                .message_key
                .as_deref(),
            Some("CONFIGURATION_OPERATION_MISMATCH"),
        );
        let undo = ResolveConfigurationConflictRequest {
            operation_id: uuid::Uuid::new_v4().to_string(),
            expected_state_revision: transactional.state_revision,
            editor_session_id: None,
            expected_draft_revision: None,
            action: ConfigurationConflictAction::Undo {
                resolution_operation_id: operation_id.clone(),
            },
        };
        assert!(
            transactional
                .resolve_configuration_conflict(undo, 7)
                .unwrap()
        );
        assert_eq!(transactional.final_edit.conflicts.len(), 1);
        assert_eq!(
            transactional.final_edit.conflicts[0].semantic_path,
            conflict.semantic_path
        );
        let redo = ResolveConfigurationConflictRequest {
            operation_id: uuid::Uuid::new_v4().to_string(),
            expected_state_revision: transactional.state_revision,
            editor_session_id: None,
            expected_draft_revision: None,
            action: ConfigurationConflictAction::Redo {
                resolution_operation_id: operation_id,
            },
        };
        assert!(
            transactional
                .resolve_configuration_conflict(redo, 8)
                .unwrap()
        );
        assert!(transactional.final_edit.conflicts.is_empty());
        let output = parse_semantic_document(
            transactional.format,
            transactional.desired.content.as_bytes(),
        )
        .unwrap();
        assert_eq!(output["route"]["final"], "mine");
        assert_eq!(output["log"]["loglevel"], "debug");
    }

    #[test]
    fn parent_delete_and_upstream_child_change_produce_one_parent_conflict() {
        let base = json!({"experimental": {"clash_api": {"external_ui": "ui", "secret": "old"}}});
        let mine = json!({});
        let updated =
            json!({"experimental": {"clash_api": {"external_ui": "new-ui", "secret": "old"}}});

        let result = rebase_final_document(&base, &mine, &updated);

        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].semantic_path, "/experimental");
        assert_eq!(result.document, updated);
    }

    #[test]
    fn repeated_save_is_idempotent_and_unsaved_candidates_cannot_validate_or_apply() {
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
        let evidence = validation_evidence(&state);
        let validation_error = state
            .mark_validation(true, Vec::new(), Some(evidence))
            .expect_err("unsaved validation must fail");
        assert_eq!(validation_error.code, ErrorCode::InvalidState);
        assert_eq!(
            state
                .mark_applied()
                .expect_err("unsaved apply must fail")
                .code,
            ErrorCode::InvalidState
        );

        state.mark_candidate_saved().expect("first save");
        let generation = state.generation;
        let revision = state.state_revision;
        state.mark_candidate_saved().expect("repeat save");
        assert_eq!(state.generation, generation);
        assert_eq!(state.state_revision, revision);
    }
}
