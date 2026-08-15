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
    embedded_core_upstream_manifest, normalize_jsonc,
};

pub const CONFIGURATION_STATE_SCHEMA_VERSION: u32 = 3;
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
        conflicts: Vec::new(),
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
                if !value.is_null() {
                    current.insert(key.clone(), value);
                    current_provenance.insert(key, provenance);
                }
            }
        }
    }
    Ok(())
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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawManualIntent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub based_on_revision: Option<ConfigurationRevision>,
    #[serde(default)]
    pub operations: Vec<IntentOperation>,
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
    apply_dashboard_intent(kind, &mut effective, intent)?;
    Ok(effective)
}

/// Synchronize the Dashboard form's semantic values into the same Guided intent store.
/// The form remains a presentation of these values; it no longer gets to mutate the generated
/// Core document directly. Source snapshots and Raw operations continue to rebase around them.
pub fn sync_managed_dashboard_intent(intent: &mut GuidedIntent, managed: &ManagedConfigSpec) {
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

fn sync_xray_dashboard_intent(intent: &mut GuidedIntent, dashboard: &XrayDashboardSpec) {
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
    intent: &GuidedIntent,
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

fn intent_port(intent: &GuidedIntent, id: &str) -> Result<Option<u16>> {
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

fn intent_text(intent: &GuidedIntent, id: &str) -> Result<Option<String>> {
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

fn apply_sing_box_dashboard_intent(root: &mut Value, intent: &GuidedIntent) -> Result<()> {
    let Some(port) = intent_port(intent, "dashboard.singBoxApi.listenPort")? else {
        return Ok(());
    };
    let update_interval =
        intent_text(intent, "dashboard.singBoxApi.updateInterval")?.unwrap_or_else(|| "1d".into());
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

fn apply_sing_box_clash_dashboard_intent(root: &mut Value, intent: &GuidedIntent) -> Result<()> {
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

fn apply_xray_dashboard_intent(root: &mut Value, intent: &GuidedIntent) -> Result<()> {
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

fn apply_mihomo_dashboard_intent(root: &mut Value, intent: &GuidedIntent) -> Result<()> {
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
    let raw_paths = raw
        .operations
        .iter()
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
                    GuidedProjectionStatus::Overridden
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

pub fn diff_raw_intent(base: &Value, edited: &Value) -> RawManualIntent {
    let mut operations = Vec::new();
    diff_value(base, edited, &mut Vec::new(), &mut operations);
    RawManualIntent {
        based_on_revision: None,
        operations,
    }
}

fn diff_value(
    base: &Value,
    edited: &Value,
    path: &mut SemanticPath,
    operations: &mut Vec<IntentOperation>,
) {
    if base == edited {
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
    for operation in &intent.operations {
        if let Err(conflict) = apply_intent_operation(&mut effective, operation) {
            conflicts.push(*conflict);
        }
    }
    (effective, conflicts)
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
    pub raw_intent: RawManualIntent,
    pub desired: ConfigurationCandidate,
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
            raw_intent: RawManualIntent::default(),
            desired: candidate,
            applied: None,
            last_known_good: None,
        })
    }

    pub fn rebuild_desired(&mut self, created_unix_ms: u64) -> Result<()> {
        let base = parse_semantic_document(self.format, self.base.content.as_bytes())?;
        let guided = apply_guided_intent(self.kind, &base, &self.guided_intent)?;
        let guided_provenance = apply_provenance_layer(
            &base,
            &guided,
            &self.base_provenance,
            ProvenanceLayer::Guided,
        );
        let (effective, mut conflicts) = apply_raw_intent(&guided, &self.raw_intent);
        self.provenance = apply_provenance_layer(
            &guided,
            &effective,
            &guided_provenance,
            ProvenanceLayer::Raw,
        );
        conflicts.extend(dashboard_raw_conflicts(
            self.kind,
            &self.guided_intent,
            &self.raw_intent,
        ));
        let content = serialize_semantic_document(self.format, &effective)?;
        self.generation = self.generation.saturating_add(1);
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
        let guided = apply_guided_intent(self.kind, &base, &self.guided_intent)?;
        let edited = parse_semantic_document(self.format, edited)?;
        self.raw_intent = diff_raw_intent(&guided, &edited);
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

    pub fn mark_applied(&mut self) -> Result<()> {
        if self.desired.validation != CandidateValidationStatus::Valid {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Only a valid Desired configuration can be applied",
            ));
        }
        if self.desired.validation_evidence.is_none() {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Applied configuration requires native validation evidence",
            ));
        }
        self.applied = Some(self.desired.clone());
        self.last_known_good = Some(self.desired.clone());
        Ok(())
    }

    pub fn view(&self) -> ConfigurationStateView {
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
            compatibility_references,
        }
    }
}

fn dashboard_raw_conflicts(
    kind: ProgramKind,
    guided: &GuidedIntent,
    raw: &RawManualIntent,
) -> Vec<ConfigurationConflict> {
    let targets: Vec<(&str, SemanticPath)> = match kind {
        ProgramKind::SingBox => [
            ("dashboard.singBoxApi.listenPort", key_path(&["services"])),
            (
                "dashboard.singBoxClash.listenPort",
                key_path(&["experimental", "clash_api"]),
            ),
        ]
        .into(),
        ProgramKind::Xray => [
            ("dashboard.xray.apiPort", key_path(&["api"])),
            ("dashboard.xray.metricsPort", key_path(&["metrics"])),
            ("dashboard.xray.apiPort", key_path(&["stats"])),
            ("dashboard.xray.apiPort", key_path(&["policy", "system"])),
        ]
        .into(),
        ProgramKind::Mihomo => [
            (
                "dashboard.mihomo.listenPort",
                key_path(&["external-controller"]),
            ),
            ("dashboard.mihomo.listenPort", key_path(&["external-ui"])),
            (
                "dashboard.mihomo.listenPort",
                key_path(&["external-ui-url"]),
            ),
        ]
        .into(),
        ProgramKind::Generic => Vec::new(),
    };
    targets
        .into_iter()
        .filter(|(setting, target)| {
            guided.values.contains_key(*setting)
                && raw
                    .operations
                    .iter()
                    .map(intent_operation_path)
                    .any(|raw_path| path_overlaps(target, &raw_path))
        })
        .map(|(setting, target)| ConfigurationConflict {
            semantic_path: display_semantic_path(&target),
            reason: "Dashboard Guided intent is overridden by Raw configuration".into(),
            severity: ConflictSeverity::Error,
            message_key: Some("CONFIGURATION_RAW_OVERRIDE".into()),
            source_value: None,
            guided_value: guided.values.get(setting).cloned(),
            raw_value: None,
            effective_value: None,
        })
        .collect()
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
    pub compatibility_references: Vec<crate::CoreCompatibilityReference>,
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
    fn raw_dashboard_overlap_blocks_a_silent_managed_dashboard_save() {
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
        sync_managed_dashboard_intent(
            &mut state.guided_intent,
            &ManagedConfigSpec {
                mihomo_dashboard: Some(MihomoDashboardSpec {
                    listen_port: 9092,
                    download_url: None,
                }),
                ..ManagedConfigSpec::default()
            },
        );
        state.raw_intent.operations.push(IntentOperation::Set {
            path: key_path(&["external-controller"]),
            value: Value::String("127.0.0.1:9999".into()),
        });

        state.rebuild_desired(2).expect("rebuild");

        assert_eq!(state.desired.validation, CandidateValidationStatus::Invalid);
        assert_eq!(state.desired.conflicts.len(), 1);
        assert_eq!(
            state.desired.conflicts[0].reason,
            "Dashboard Guided intent is overridden by Raw configuration"
        );
    }
}
