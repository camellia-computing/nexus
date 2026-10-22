use super::*;
use crate::configuration_field_evidence::{
    NativeInputAvailability, NativeInputSelector, native_input_nodes,
};
use crate::core_build_constraints::{BuildCondition, evaluate_build_condition};
use crate::{CoreCapabilityProfile, KnowledgeTypeShape};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(test)]
pub(crate) mod tests;

const MAX_CACHED_CATALOGS: usize = 16;
static CATALOGS: OnceLock<Mutex<BoundedArcCache<String, IntentCatalog>>> = OnceLock::new();
const MAX_CACHED_CONTEXTS: usize = 8;
type IntentContextCache = BoundedArcCache<(String, String), IntentContextIndex>;
static CONTEXTS: OnceLock<Mutex<IntentContextCache>> = OnceLock::new();

pub(super) struct BoundedArcCache<K, V> {
    entries: VecDeque<(K, Arc<V>)>,
    capacity: usize,
}

impl<K: PartialEq, V> BoundedArcCache<K, V> {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity,
        }
    }

    pub(super) fn get(&mut self, key: &K) -> Option<Arc<V>> {
        let position = self.entries.iter().position(|(entry, _)| entry == key)?;
        let entry = self.entries.remove(position)?;
        let value = Arc::clone(&entry.1);
        self.entries.push_back(entry);
        Some(value)
    }

    pub(super) fn insert(&mut self, key: K, value: Arc<V>) -> Arc<V> {
        if let Some(current) = self.get(&key) {
            return current;
        }
        if self.capacity > 0 {
            if self.entries.len() == self.capacity {
                self.entries.pop_front();
            }
            self.entries.push_back((key, Arc::clone(&value)));
        }
        value
    }
}

struct IntentContextIndex {
    document: Value,
    assessment: crate::ConfigurationAssessment,
    references: crate::configuration_references::ConfigurationReferenceIndex,
    identities: BTreeMap<(Vec<String>, String, String), usize>,
}

fn context_index(
    profile: &CoreCapabilityProfile,
    state: &ConfigurationState,
) -> Result<Arc<IntentContextIndex>> {
    let cache = CONTEXTS.get_or_init(|| Mutex::new(BoundedArcCache::new(MAX_CACHED_CONTEXTS)));
    context_index_with_cache(profile, state, cache)
}

fn context_index_with_cache(
    profile: &CoreCapabilityProfile,
    state: &ConfigurationState,
    cache: &Mutex<IntentContextCache>,
) -> Result<Arc<IntentContextIndex>> {
    let key = (
        profile.profile_hash.clone(),
        hash_bytes(state.desired.content.as_bytes()),
    );
    if let Some(index) = cache.lock().map_err(|_| catalog_unavailable())?.get(&key) {
        return Ok(index);
    }
    if state.desired.content.len() > crate::MAX_CONFIG_BYTES {
        return Err(catalog_unavailable());
    }
    let document = parse_semantic_document(state.format, state.desired.content.as_bytes())?;
    let references =
        crate::configuration_references::ConfigurationReferenceIndex::new(profile, &document)?;
    let assessment = profile.assess_with_references(&document, &references)?;
    let mut identities = BTreeMap::new();
    index_identities(&document, &mut Vec::new(), &mut identities, &mut 0)?;
    let index = Arc::new(IntentContextIndex {
        document,
        assessment,
        references,
        identities,
    });
    Ok(cache
        .lock()
        .map_err(|_| catalog_unavailable())?
        .insert(key, index))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IntentNodeDisposition {
    Guided,
    DetailsManaged,
    FinalEditor,
    Inapplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentTypeSelection {
    pub path: Vec<String>,
    pub field: String,
    pub value: String,
    pub comparison: crate::KnowledgeDiscriminatorComparison,
}

impl From<NativeInputSelector> for IntentTypeSelection {
    fn from(selector: NativeInputSelector) -> Self {
        Self {
            path: selector.path,
            field: selector.field,
            value: selector.value,
            comparison: selector.comparison,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentNode {
    pub node_id: String,
    pub parent_id: Option<String>,
    pub native_name: String,
    pub native_order: usize,
    pub pattern: Vec<String>,
    pub type_selections: Vec<IntentTypeSelection>,
    pub shape: KnowledgeTypeShape,
    pub control: Option<IntentNativeControl>,
    pub disposition: IntentNodeDisposition,
    pub reason_key: Option<String>,
    pub setting_id: Option<String>,
    pub object_kinds: Vec<IntentObjectKind>,
    pub object_protocol: Option<String>,
    pub semantic_rule_ids: Vec<String>,
    pub source: crate::KnowledgeSourceReference,
    pub source_commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentNativeControl {
    pub kind: GuidedControl,
    pub allowed_values: Vec<String>,
    pub nullable: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentCoverage {
    pub native_nodes: usize,
    pub guided_nodes: usize,
    pub details_managed_nodes: usize,
    pub final_editor_nodes: usize,
    pub inapplicable_nodes: usize,
    pub reviewed_relations: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentCatalog {
    pub catalog_hash: String,
    pub capability_profile_hash: String,
    pub program: ProgramKind,
    pub chapters: Vec<String>,
    pub nodes: Vec<IntentNode>,
    pub relations: Vec<IntentRelation>,
    pub coverage: IntentCoverage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentRelation {
    pub relation_id: String,
    pub node_ids: Vec<String>,
    pub related_node_ids: Vec<String>,
    pub constraint: crate::KnowledgeValueConstraint,
    pub when: Vec<crate::KnowledgeFieldPredicate>,
    pub message_key: String,
    pub evidence: Vec<IntentRelationEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentRelationEvidence {
    pub source: crate::KnowledgeSourceReference,
    pub source_commit: String,
    pub behavior_hash: String,
}

impl IntentCatalog {
    pub fn node(&self, node_id: &str) -> Option<&IntentNode> {
        self.nodes.iter().find(|node| node.node_id == node_id)
    }

    pub fn children<'a>(&'a self, node_id: &'a str) -> impl Iterator<Item = &'a IntentNode> {
        self.nodes
            .iter()
            .filter(move |node| node.parent_id.as_deref() == Some(node_id))
    }
}

/// Resolved once per immutable binary capability profile, without process or network I/O.
pub fn configuration_intent_catalog(profile: &CoreCapabilityProfile) -> Result<Arc<IntentCatalog>> {
    profile.reviewed_descriptor()?;
    let cache = CATALOGS.get_or_init(|| Mutex::new(BoundedArcCache::new(MAX_CACHED_CATALOGS)));
    if let Some(catalog) = cache
        .lock()
        .map_err(|_| catalog_unavailable())?
        .get(&profile.profile_hash)
    {
        return Ok(catalog);
    }
    let catalog = Arc::new(build_catalog(profile)?);
    Ok(cache
        .lock()
        .map_err(|_| catalog_unavailable())?
        .insert(profile.profile_hash.clone(), catalog))
}

fn catalog_unavailable() -> CamelliaNexusError {
    CamelliaNexusError::new(ErrorCode::InvalidState, "Intent catalog unavailable")
        .with_message_key("INTENT_CONTEXT_UNAVAILABLE")
}

fn build_catalog(profile: &CoreCapabilityProfile) -> Result<IntentCatalog> {
    let knowledge = crate::embedded_core_knowledge()?;
    let descriptor = knowledge
        .program(profile.program)
        .ok_or_else(catalog_unavailable)?;
    let release = descriptor
        .releases
        .iter()
        .find(|release| release.tag == profile.baseline_tag)
        .ok_or_else(catalog_unavailable)?;
    let settings = guided_setting_descriptors(profile.program);
    let managed: Vec<_> = managed_ownership_targets(profile.program)
        .into_iter()
        .map(|(_, _, path)| path_pattern(&path))
        .collect();
    let mut coverage = IntentCoverage::default();
    let mut nodes = Vec::new();
    for native in native_input_nodes(profile)? {
        let setting_id = settings.iter().find_map(|setting| {
            let path = intent::intent_setting_path(profile.program, &setting.id)?;
            (native.selectors.is_empty()
                && path
                    .iter()
                    .copied()
                    .eq(native.path.iter().map(String::as_str)))
            .then(|| setting.id.clone())
        });
        let object_kinds = object_adapters(profile.program, &native.path, &native.selectors);
        let object_protocol = if profile.program == ProgramKind::Mihomo {
            match native.name.as_str() {
                "port" => Some("http".into()),
                "socks-port" => Some("socks".into()),
                "mixed-port" => Some("mixed".into()),
                _ => None,
            }
        } else {
            None
        };
        let mut control =
            if object_kinds.is_empty() && !native_linked_field(profile.program, &native.path) {
                native_control(&native.shape)
            } else {
                None
            };
        let semantic_rule_ids = descriptor
            .semantic_rules
            .iter()
            .filter(|variant| {
                variant.releases.contains(&profile.baseline_tag) && variant.rule.path == native.path
            })
            .map(|variant| variant.rule.id.clone())
            .collect::<Vec<_>>();
        let mut availability = native.availability;
        let mut reference_field = false;
        for variant in descriptor.semantic_rules.iter().filter(|variant| {
            variant.releases.contains(&profile.baseline_tag) && variant.rule.path == native.path
        }) {
            if let crate::KnowledgeValueConstraint::Reference {
                form: crate::KnowledgeReferenceForm::Scalar,
                ..
            } = &variant.rule.constraint
            {
                reference_field = true;
                if let Some(control) = &mut control
                    && control.kind == GuidedControl::Text
                {
                    control.kind = GuidedControl::Select;
                }
            }
            if let crate::KnowledgeValueConstraint::Enum {
                values, allow_null, ..
            } = &variant.rule.constraint
                && variant.rule.when.is_empty()
                && matches!(
                    evaluate_build_condition(
                        profile.program,
                        profile.build.as_ref(),
                        &variant.rule.build_constraint,
                        &descriptor.reported_build_tags_for(&profile.baseline_tag)
                    ),
                    BuildCondition::Satisfied
                )
                && let Some(control) = &mut control
                && matches!(control.kind, GuidedControl::Text | GuidedControl::Select)
            {
                control.allowed_values = if control.kind == GuidedControl::Select {
                    control
                        .allowed_values
                        .iter()
                        .filter(|value| values.contains(value))
                        .cloned()
                        .collect()
                } else {
                    values.clone()
                };
                control.kind = GuidedControl::Select;
                control.nullable &= allow_null;
            }
            if let crate::KnowledgeValueConstraint::Platform { condition, .. } =
                &variant.rule.constraint
            {
                match evaluate_build_condition(
                    profile.program,
                    profile.build.as_ref(),
                    condition,
                    &descriptor.reported_build_tags_for(&profile.baseline_tag),
                ) {
                    BuildCondition::Excluded => {
                        availability = NativeInputAvailability::Excluded;
                        break;
                    }
                    BuildCondition::Unconfirmed
                        if availability == NativeInputAvailability::Available =>
                    {
                        availability = NativeInputAvailability::Unconfirmed;
                    }
                    _ => {}
                }
            }
        }
        if native.path.iter().any(|key| protected_key(key))
            && control
                .as_ref()
                .is_some_and(|control| control.kind != GuidedControl::Text)
        {
            control = None;
        }
        let (disposition, reason) = match availability {
            NativeInputAvailability::Excluded => (
                IntentNodeDisposition::Inapplicable,
                Some("INTENT_NODE_NOT_AVAILABLE"),
            ),
            NativeInputAvailability::Unconfirmed => (
                IntentNodeDisposition::FinalEditor,
                Some("INTENT_NODE_CAPABILITY_UNCONFIRMED"),
            ),
            NativeInputAvailability::Available
                if managed.iter().any(|path| native.path.starts_with(path)) =>
            {
                (
                    IntentNodeDisposition::DetailsManaged,
                    Some("INTENT_NODE_MANAGED_IN_DETAILS"),
                )
            }
            NativeInputAvailability::Available
                if !object_kinds.is_empty()
                    || native.complete
                        && (setting_id.is_some()
                            || (control.is_some()
                                && (!native_identity_field(&native.name) || reference_field))) =>
            {
                (IntentNodeDisposition::Guided, None)
            }
            NativeInputAvailability::Available => (
                IntentNodeDisposition::FinalEditor,
                Some(if native.complete {
                    "INTENT_NODE_EDIT_IN_FINAL"
                } else {
                    "INTENT_NODE_LAYOUT_UNCONFIRMED"
                }),
            ),
        };
        coverage.native_nodes += 1;
        match disposition {
            IntentNodeDisposition::Guided => coverage.guided_nodes += 1,
            IntentNodeDisposition::DetailsManaged => coverage.details_managed_nodes += 1,
            IntentNodeDisposition::FinalEditor => coverage.final_editor_nodes += 1,
            IntentNodeDisposition::Inapplicable => coverage.inapplicable_nodes += 1,
        }
        nodes.push(IntentNode {
            node_id: native.id,
            parent_id: native.parent_id,
            native_name: native.name,
            native_order: native.order,
            pattern: native.path,
            type_selections: native.selectors.into_iter().map(Into::into).collect(),
            shape: native.shape,
            control,
            disposition,
            reason_key: reason.map(str::to_owned),
            setting_id,
            object_kinds,
            object_protocol,
            semantic_rule_ids,
            source_commit: release
                .source_commit(&native.source)
                .ok_or_else(catalog_unavailable)?
                .into(),
            source: native.source,
        });
    }
    let relations = descriptor
        .semantic_rules
        .iter()
        .filter(|variant| variant.releases.contains(&profile.baseline_tag))
        .map(|variant| {
            let rule = &variant.rule;
            let node_ids = nodes
                .iter()
                .filter(|node| node.pattern == rule.path)
                .map(|node| node.node_id.clone())
                .collect();
            let mut related_paths: Vec<_> = rule
                .when
                .iter()
                .map(|condition| {
                    let mut path = rule.path.clone();
                    if let Some(last) = path.last_mut() {
                        *last = condition.sibling.clone();
                    }
                    path
                })
                .collect();
            if let crate::KnowledgeValueConstraint::Requires { sibling, .. } = &rule.constraint {
                let mut path = rule.path.clone();
                if let Some(last) = path.last_mut() {
                    *last = sibling.clone();
                }
                related_paths.push(path);
            }
            if let crate::KnowledgeValueConstraint::Reference { targets, .. } = &rule.constraint {
                related_paths.extend(targets.iter().map(|target| target.path.clone()));
            }
            let related_node_ids = nodes
                .iter()
                .filter(|node| related_paths.contains(&node.pattern))
                .map(|node| node.node_id.clone())
                .collect();
            let evidence = rule
                .evidence
                .iter()
                .map(|evidence| {
                    Ok(IntentRelationEvidence {
                        source: evidence.source.clone(),
                        source_commit: release
                            .source_commit(&evidence.source)
                            .ok_or_else(catalog_unavailable)?
                            .into(),
                        behavior_hash: evidence.body_hash.clone(),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(IntentRelation {
                relation_id: rule.id.clone(),
                node_ids,
                related_node_ids,
                constraint: rule.constraint.clone(),
                when: rule.when.clone(),
                message_key: rule.message_key.clone(),
                evidence,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    coverage.reviewed_relations = relations.len();
    let chapters = nodes
        .iter()
        .filter(|node| node.parent_id.is_none())
        .map(|node| node.node_id.clone())
        .collect();
    let catalog_hash = hash_bytes(&serde_json::to_vec(&(
        &profile.profile_hash,
        &nodes,
        &relations,
        &coverage,
    ))?);
    Ok(IntentCatalog {
        catalog_hash,
        capability_profile_hash: profile.profile_hash.clone(),
        program: profile.program,
        chapters,
        nodes,
        relations,
        coverage,
    })
}

fn native_identity_field(name: &str) -> bool {
    ["tag", "name", "id"].contains(&name)
}

fn native_linked_field(program: ProgramKind, path: &[String]) -> bool {
    path.last()
        .is_some_and(|key| ["type", "protocol", "auth"].contains(&key.as_str()))
        || (program == ProgramKind::Mihomo
            && path.first().is_some_and(|key| {
                [
                    "allow-lan",
                    "bind-address",
                    "skip-auth-prefixes",
                    "lan-allowed-ips",
                    "lan-disallowed-ips",
                ]
                .contains(&key.as_str())
            }))
}

fn native_control(shape: &KnowledgeTypeShape) -> Option<IntentNativeControl> {
    match shape {
        KnowledgeTypeShape::Pointer { element } => native_control(element).map(|mut control| {
            control.nullable = true;
            control
        }),
        KnowledgeTypeShape::Builtin { name } => Some(IntentNativeControl {
            kind: match name.as_str() {
                "bool" => GuidedControl::Toggle,
                "string" => GuidedControl::Text,
                "int" | "int8" | "int16" | "int32" | "int64" | "uint" | "uint8" | "uint16"
                | "uint32" | "uint64" | "float32" | "float64" => GuidedControl::Number,
                _ => return None,
            },
            allowed_values: Vec::new(),
            nullable: false,
        }),
        _ => None,
    }
}

fn path_pattern(path: &[SemanticPathSegment]) -> Vec<String> {
    path.iter()
        .map(|segment| match segment {
            SemanticPathSegment::Key { key } => key.clone(),
            SemanticPathSegment::Identity { .. } => "*".into(),
        })
        .collect()
}

fn object_adapters(
    program: ProgramKind,
    path: &[String],
    selectors: &[NativeInputSelector],
) -> Vec<IntentObjectKind> {
    let is_path = |parts: &[&str]| parts.iter().copied().eq(path.iter().map(String::as_str));
    if program == ProgramKind::Mihomo {
        if is_path(&["tun"]) {
            return vec![IntentObjectKind::Tun];
        }
        if is_path(&["mixed-port"]) || is_path(&["port"]) || is_path(&["socks-port"]) {
            return vec![IntentObjectKind::Listener];
        }
        if is_path(&["dns", "nameserver"]) {
            return vec![IntentObjectKind::DnsServer];
        }
        if is_path(&["rules"]) {
            return vec![IntentObjectKind::RouteRule];
        }
    } else {
        if is_path(&["inbounds"]) {
            return if program == ProgramKind::SingBox {
                vec![IntentObjectKind::Listener, IntentObjectKind::Tun]
            } else {
                vec![IntentObjectKind::Listener]
            };
        }
        if is_path(&["dns", "servers"]) {
            return vec![IntentObjectKind::DnsServer];
        }
        if is_path(&[
            if program == ProgramKind::SingBox {
                "route"
            } else {
                "routing"
            },
            "rules",
        ]) {
            return vec![IntentObjectKind::RouteRule];
        }
        if program == ProgramKind::SingBox
            && is_path(&["inbounds", "*"])
            && selectors.iter().any(|selector| selector.value == "tun")
        {
            return vec![IntentObjectKind::Tun];
        }
    }
    Vec::new()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentContext {
    pub node: Option<IntentNode>,
    pub children: Vec<IntentNode>,
    pub relations: Vec<IntentRelation>,
    pub related_nodes: Vec<IntentNode>,
    pub state_revision: u64,
    pub candidate_generation: u64,
    pub catalog_hash: String,
    pub values: Vec<IntentContextValue>,
    pub issues: Vec<IntentContextIssue>,
    pub default_value: Option<Value>,
    pub default_conditions: Vec<crate::KnowledgeDefaultCondition>,
    pub setting: Option<GuidedSettingDescriptor>,
    pub object_editors: Vec<IntentObjectContext>,
    pub targets: Vec<IntentTarget>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentObjectContext {
    pub node_id: String,
    pub descriptor: IntentObjectDescriptor,
    pub objects: Vec<IntentObjectProjection>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentContextValue {
    pub reference_id: String,
    pub semantic_path: String,
    pub present: bool,
    pub value: Option<Value>,
    pub protected: bool,
    pub can_follow: bool,
    pub intent_value: Option<SemanticValue>,
    pub summary: Option<IntentValueSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentValueSummary {
    pub kind: String,
    pub size: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentContextIssue {
    pub message_key: String,
    pub semantic_path: String,
}

/// Context never mutates the state and never includes protected field content.
pub fn configuration_intent_context(
    profile: &CoreCapabilityProfile,
    state: &ConfigurationState,
    node_id: Option<&str>,
) -> Result<IntentContext> {
    if profile.program != state.kind {
        return Err(catalog_unavailable());
    }
    let catalog = configuration_intent_catalog(profile)?;
    let node = node_id
        .map(|id| catalog.node(id).ok_or_else(catalog_unavailable))
        .transpose()?;
    let children: Vec<IntentNode> = catalog
        .nodes
        .iter()
        .filter(|child| child.parent_id.as_deref() == node_id)
        .cloned()
        .collect();
    let children = if children
        .iter()
        .any(|child| child.disposition == IntentNodeDisposition::Inapplicable)
    {
        let index = context_index(profile, state)?;
        let mut visible = Vec::with_capacity(children.len());
        for child in children {
            if child.disposition == IntentNodeDisposition::Inapplicable {
                let mut selected = Vec::new();
                crate::configuration_assessment::select_paths(
                    &index.document,
                    &child.pattern,
                    &mut Vec::new(),
                    &mut selected,
                    &mut 0,
                    state.format == ConfigurationFormat::Jsonc,
                )?;
                if !selected
                    .iter()
                    .any(|(path, _)| selections_match(&index.document, &child, path))
                {
                    continue;
                }
            }
            visible.push(child);
        }
        visible
    } else {
        children
    };
    let Some(node) = node else {
        return Ok(IntentContext {
            node: None,
            children,
            relations: Vec::new(),
            related_nodes: Vec::new(),
            state_revision: state.state_revision,
            candidate_generation: state.generation,
            catalog_hash: catalog.catalog_hash.clone(),
            values: Vec::new(),
            issues: Vec::new(),
            default_value: None,
            default_conditions: Vec::new(),
            setting: None,
            object_editors: Vec::new(),
            targets: Vec::new(),
        });
    };
    let index = context_index(profile, state)?;
    let document = &index.document;
    let selected = native_values(&index, node, state.format)?;
    let sensitive = node.pattern.iter().any(|key| protected_key(key));
    let mut values = selected
        .into_iter()
        .filter(|(path, _)| selections_match(document, node, path))
        .map(|(path, value)| {
            let stable_path = actual_semantic_path(&index, &path).ok();
            let semantic_path = stable_path
                .as_ref()
                .map(|path| display_semantic_path(path))
                .unwrap_or_else(|| {
                    format!(
                        "/{}",
                        path.iter()
                            .map(|part| part.replace('~', "~0").replace('/', "~1"))
                            .collect::<Vec<_>>()
                            .join("/")
                    )
                });
            let reference_id = native_reference(&index, node, &path, value)?;
            let protected = sensitive || value.is_some_and(contains_protected_value);
            let summary = match value {
                Some(Value::Array(items)) => Some(IntentValueSummary {
                    kind: "list".into(),
                    size: items.len(),
                }),
                Some(Value::Object(fields)) => Some(IntentValueSummary {
                    kind: "object".into(),
                    size: fields.len(),
                }),
                Some(Value::String(text)) if text.len() > 2048 => Some(IntentValueSummary {
                    kind: "text".into(),
                    size: text.chars().count(),
                }),
                _ => None,
            };
            Ok(IntentContextValue {
                reference_id,
                semantic_path,
                present: value.is_some(),
                value: value.filter(|_| !protected && summary.is_none()).cloned(),
                protected,
                can_follow: stable_path
                    .as_ref()
                    .is_some_and(|path| native_can_follow(state, node, path)),
                intent_value: stable_path
                    .as_ref()
                    .and_then(|path| native_saved_value(state, node, path)),
                summary,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if values.is_empty()
        && !node.pattern.iter().any(|part| part == "*")
        && node.type_selections.is_empty()
    {
        values.push(IntentContextValue {
            reference_id: missing_reference(node),
            semantic_path: format!("/{}", node.pattern.join("/")),
            present: false,
            value: None,
            protected: sensitive,
            can_follow: native_can_follow(
                state,
                node,
                &node
                    .pattern
                    .iter()
                    .map(|key| SemanticPathSegment::Key { key: key.clone() })
                    .collect(),
            ),
            summary: None,
            intent_value: native_saved_value(
                state,
                node,
                &node
                    .pattern
                    .iter()
                    .map(|key| SemanticPathSegment::Key { key: key.clone() })
                    .collect(),
            ),
        });
    }
    let descriptor = crate::embedded_core_knowledge()?
        .program(profile.program)
        .ok_or_else(catalog_unavailable)?;
    let rule = descriptor.semantic_rules.iter().find_map(|variant| {
        (variant.releases.contains(&profile.baseline_tag) && variant.rule.path == node.pattern)
            .then_some(&variant.rule)
    });
    let issues = index
        .assessment
        .issues
        .iter()
        .filter(|issue| pattern_matches(&node.pattern, &issue.path))
        .map(|issue| IntentContextIssue {
            message_key: issue.message_key.clone(),
            semantic_path: format!("/{}", issue.path.join("/")),
        })
        .collect();
    let adapter_nodes: Vec<_> = std::iter::once(node)
        .chain(children.iter())
        .filter(|node| {
            !node.object_kinds.is_empty() && node.disposition == IntentNodeDisposition::Guided
        })
        .collect();
    let mut object_editors = Vec::new();
    let mut handled_adapters = BTreeSet::new();
    let mut targets = Vec::new();
    if !adapter_nodes.is_empty() {
        let view = intent_objects::intent_object_context_data(profile, state, document)?;
        targets = view.intent_targets;
        for node in adapter_nodes {
            for descriptor in &view.intent_object_descriptors {
                if !node.object_kinds.contains(&descriptor.kind) {
                    continue;
                }
                if !handled_adapters.insert((
                    format!("{:?}", descriptor.kind),
                    node.object_protocol.clone(),
                )) {
                    continue;
                }
                let mut descriptor = descriptor.clone();
                if let Some(protocol) = &node.object_protocol {
                    descriptor.protocols.retain(|value| value == protocol);
                    descriptor.can_create &= !descriptor.protocols.is_empty();
                }
                let objects = view
                    .intent_objects
                    .iter()
                    .filter(|object| {
                        object.kind == descriptor.kind
                            && node.object_protocol.as_ref().is_none_or(|protocol| {
                                object.values.get("protocol").and_then(Value::as_str)
                                    == Some(protocol)
                            })
                    })
                    .cloned()
                    .collect();
                object_editors.push(IntentObjectContext {
                    node_id: node.node_id.clone(),
                    descriptor,
                    objects,
                });
            }
        }
    }
    let setting = if let Some(id) = &node.setting_id {
        intent::resolved_setting_descriptors(profile)?
            .into_iter()
            .find(|item| &item.id == id)
    } else {
        None
    };
    if setting
        .as_ref()
        .is_some_and(|setting| matches!(setting.id.as_str(), "routing.final" | "dns.final"))
        && targets.is_empty()
    {
        targets = intent_targets(state.kind, document);
    }
    let mut resolved_node = node.clone();
    if let Some(rule) = descriptor.semantic_rules.iter().find_map(|variant| {
        (variant.releases.contains(&profile.baseline_tag)
            && variant.rule.path == node.pattern
            && matches!(
                &variant.rule.constraint,
                crate::KnowledgeValueConstraint::Reference {
                    form: crate::KnowledgeReferenceForm::Scalar,
                    ..
                }
            ))
        .then_some(&variant.rule)
    }) && let crate::KnowledgeValueConstraint::Reference {
        builtin_values,
        allow_empty,
        ..
    } = &rule.constraint
    {
        let mut names = BTreeMap::<String, usize>::new();
        for name in index
            .references
            .targets(&rule.id)
            .iter()
            .map(|target| &target.name)
            .chain(builtin_values)
        {
            *names.entry(name.clone()).or_default() += 1;
        }
        let kind = match node.setting_id.as_deref() {
            Some("routing.final") => "connection",
            Some("dns.final") => "dns",
            _ => "reference",
        };
        targets = names
            .iter()
            .filter(|(name, count)| {
                **count == 1 && !contains_protected_value(&Value::String((*name).clone()))
            })
            .map(|(name, _)| IntentTarget {
                id: name.clone(),
                label: name.clone(),
                kind: kind.into(),
            })
            .collect();
        if let Some(control) = &mut resolved_node.control {
            control.allowed_values = targets.iter().map(|target| target.id.clone()).collect();
            if *allow_empty {
                control.allowed_values.insert(0, String::new());
            }
        }
    }
    let relations: Vec<_> = catalog
        .relations
        .iter()
        .filter(|relation| {
            relation.node_ids.contains(&node.node_id)
                || relation.related_node_ids.contains(&node.node_id)
        })
        .cloned()
        .collect();
    let related_ids = relations
        .iter()
        .flat_map(|relation| relation.node_ids.iter().chain(&relation.related_node_ids))
        .collect::<BTreeSet<_>>();
    let related_nodes = catalog
        .nodes
        .iter()
        .filter(|related| related.node_id != node.node_id && related_ids.contains(&related.node_id))
        .cloned()
        .collect();
    Ok(IntentContext {
        node: Some(resolved_node),
        children,
        relations,
        related_nodes,
        state_revision: state.state_revision,
        candidate_generation: state.generation,
        catalog_hash: catalog.catalog_hash.clone(),
        values,
        issues,
        default_value: rule.map(|rule| rule.default_value.clone()),
        default_conditions: rule
            .map(|rule| rule.default_when.clone())
            .unwrap_or_default(),
        setting,
        object_editors,
        targets,
    })
}

fn missing_reference(node: &IntentNode) -> String {
    hash_bytes(
        &serde_json::to_vec(&(&node.node_id, &node.pattern, SemanticValue::Missing))
            .expect("native reference"),
    )
}

fn native_owner(node: &IntentNode, path: &SemanticPath) -> String {
    format!(
        "{}:{}",
        node.node_id,
        hash_bytes(display_semantic_path(path).as_bytes())
    )
}

fn native_can_follow(state: &ConfigurationState, node: &IntentNode, path: &SemanticPath) -> bool {
    node.setting_id
        .as_ref()
        .is_some_and(|id| state.guided_intent.values.contains_key(id))
        || state
            .guided_intent
            .path_edits
            .contains_key(&native_owner(node, path))
}

fn native_saved_value(
    state: &ConfigurationState,
    node: &IntentNode,
    path: &SemanticPath,
) -> Option<SemanticValue> {
    let saved = state
        .guided_intent
        .path_edits
        .get(&native_owner(node, path))
        .map(|edit| edit.value.clone())
        .or_else(|| {
            node.setting_id.as_ref().and_then(|id| {
                state
                    .guided_intent
                    .values
                    .get(id)
                    .cloned()
                    .map(SemanticValue::Present)
            })
        });
    saved.filter(|value| {
        !path
            .iter()
            .any(|part| matches!(part, SemanticPathSegment::Key { key } if protected_key(key)))
            && !matches!(value, SemanticValue::Present(value) if contains_protected_value(value))
    })
}

impl ConfigurationState {
    pub(super) fn edit_native_object(
        &mut self,
        profile: &CoreCapabilityProfile,
        node_id: &str,
        operation: &IntentObjectOperation,
        now: u64,
    ) -> Result<()> {
        let catalog = configuration_intent_catalog(profile)?;
        let node = catalog.node(node_id).ok_or_else(catalog_unavailable)?;
        if node.disposition != IntentNodeDisposition::Guided || node.object_kinds.is_empty() {
            return Err(native_edit_error("INTENT_NATIVE_SHAPE_REQUIRES_EDITOR"));
        }
        let context = configuration_intent_context(profile, self, Some(node_id))?;
        let allowed = context.object_editors.iter().any(|editor| {
            editor.node_id == node_id
                && match operation {
                    IntentObjectOperation::Create {
                        object_kind,
                        values,
                    } => {
                        editor.descriptor.kind == *object_kind
                            && editor.descriptor.can_create
                            && node.object_protocol.as_ref().is_none_or(|protocol| {
                                values.get("protocol").and_then(Value::as_str) == Some(protocol)
                            })
                    }
                    IntentObjectOperation::Update {
                        object_id,
                        expected_hash,
                        ..
                    }
                    | IntentObjectOperation::Remove {
                        object_id,
                        expected_hash,
                        ..
                    }
                    | IntentObjectOperation::Follow {
                        object_id,
                        expected_hash,
                    } => editor.objects.iter().any(|object| {
                        &object.object_id == object_id && &object.content_hash == expected_hash
                    }),
                }
        });
        if !allowed {
            return Err(native_edit_error("INTENT_PLAN_STALE"));
        }
        if matches!(operation, IntentObjectOperation::Follow { .. }) {
            let previous = context_index(profile, self)?;
            let mut next = self.clone();
            next.apply_intent_edit(profile, &operation.change(), now)?;
            let updated = context_index(profile, &next)?;
            if previous
                .references
                .breaks_known_references(&updated.references)
                || updated.assessment.issues.iter().any(|issue| {
                    issue.code.starts_with("CONFIGURATION_REFERENCE_")
                        && !previous.assessment.issues.iter().any(|old| {
                            old.rule_id == issue.rule_id
                                && old.path == issue.path
                                && old.code == issue.code
                        })
                })
            {
                return Err(native_edit_error("INTENT_TARGET_IN_USE"));
            }
            *self = next;
            return Ok(());
        }
        self.apply_intent_edit(profile, &operation.change(), now)
    }

    pub(super) fn edit_native_intent(
        &mut self,
        profile: &CoreCapabilityProfile,
        node_id: &str,
        reference_id: &str,
        value: Option<&SemanticValue>,
        now: u64,
    ) -> Result<()> {
        let catalog = configuration_intent_catalog(profile)?;
        let node = catalog.node(node_id).ok_or_else(catalog_unavailable)?;
        if node.disposition != IntentNodeDisposition::Guided
            && !(node.disposition == IntentNodeDisposition::Inapplicable
                && matches!(value, Some(SemanticValue::Missing)))
            || node.control.is_none() && node.setting_id.is_none()
        {
            return Err(native_edit_error("INTENT_NATIVE_SHAPE_REQUIRES_EDITOR"));
        }
        let index = context_index(profile, self)?;
        let selected = native_values(&index, node, self.format)?;
        if !node.pattern.iter().any(|part| part == "*") && selected.len() > 1 {
            return Err(native_edit_error("INTENT_NATIVE_SHAPE_REQUIRES_EDITOR"));
        }
        let matching = selected
            .iter()
            .filter(|(path, current)| {
                selections_match(&index.document, node, path)
                    && native_reference(&index, node, path, *current)
                        .is_ok_and(|reference| reference == reference_id)
            })
            .collect::<Vec<_>>();
        let path = if matching.len() == 1 {
            actual_semantic_path(&index, &matching[0].0)?
        } else if selected.is_empty()
            && reference_id == missing_reference(node)
            && !node.pattern.iter().any(|part| part == "*")
            && node.type_selections.is_empty()
        {
            node.pattern
                .iter()
                .map(|key| SemanticPathSegment::Key { key: key.clone() })
                .collect()
        } else {
            return Err(native_edit_error("INTENT_PLAN_STALE"));
        };
        if node.pattern.iter().zip(&path).any(|(expected, part)| matches!(part, SemanticPathSegment::Key {key} if expected != "*" && expected != key)) {
            return Err(native_edit_error("INTENT_NATIVE_SHAPE_REQUIRES_EDITOR"));
        }
        let upstream = self.upstream_document()?;
        for (depth, segment) in path.iter().enumerate() {
            if matches!(segment, SemanticPathSegment::Identity { .. }) {
                let anchor = upstream::value_at(&upstream, &path[..=depth])?;
                let effective = upstream::value_at(&index.document, &path[..=depth])?;
                let (SemanticValue::Present(anchor), SemanticValue::Present(effective)) =
                    (anchor, effective)
                else {
                    return Err(native_edit_error("INTENT_NATIVE_SHAPE_REQUIRES_EDITOR"));
                };
                if ["type", "protocol"].iter().any(|key| {
                    let before = anchor.get(key);
                    let after = effective.get(key);
                    before != after
                        && !(self.kind == ProgramKind::Xray
                            && before
                                .and_then(Value::as_str)
                                .zip(after.and_then(Value::as_str))
                                .is_some_and(|(before, after)| before.eq_ignore_ascii_case(after)))
                }) {
                    return Err(native_edit_error("INTENT_NATIVE_SHAPE_REQUIRES_EDITOR"));
                }
            }
        }
        let actual_path = matching
            .first()
            .map(|(path, _)| path.clone())
            .unwrap_or_else(|| node.pattern.clone());
        let required = required_native_edits(
            profile,
            &catalog,
            node,
            &path,
            &actual_path,
            value,
            &upstream,
        )?;
        let mut affected = vec![actual_path];
        let mut next = self.clone();
        for (related, path, actual, value) in required {
            affected.push(actual);
            next.stage_native_edit(related, path, Some(SemanticValue::Present(value)))?;
        }
        if let Some(setting_id) = &node.setting_id {
            if !matches!(value, Some(SemanticValue::Missing)) {
                next.guided_intent
                    .path_edits
                    .remove(&native_owner(node, &path));
            }
            let change = match value {
                Some(SemanticValue::Present(value)) => IntentCompiledEdit::Set {
                    setting_id: setting_id.clone(),
                    value: value.clone(),
                },
                None => IntentCompiledEdit::Follow {
                    setting_id: setting_id.clone(),
                },
                Some(SemanticValue::Missing) => {
                    next.stage_native_edit(node, path.clone(), value.cloned())?;
                    next.rebuild_desired(now)?;
                    validate_native_impact(
                        profile,
                        &parse_semantic_document(next.format, next.desired.content.as_bytes())?,
                        &affected,
                    )?;
                    *self = next;
                    return Ok(());
                }
            };
            next.apply_intent_edit(profile, &change, now)?;
        } else {
            next.stage_native_edit(node, path, value.cloned())?;
            next.rebuild_desired(now)?;
        }
        validate_native_impact(
            profile,
            &parse_semantic_document(next.format, next.desired.content.as_bytes())?,
            &affected,
        )?;
        *self = next;
        Ok(())
    }

    fn stage_native_edit(
        &mut self,
        node: &IntentNode,
        path: SemanticPath,
        value: Option<SemanticValue>,
    ) -> Result<()> {
        if node.pattern.iter().any(|key| protected_key(key))
            && let Some(SemanticValue::Present(value)) = &value
            && !value.as_str().is_some_and(|text| !text.is_empty())
        {
            return Err(native_edit_error("INTENT_REQUIRED_VALUE"));
        }
        if let Some(SemanticValue::Present(value)) = &value
            && !native_value_matches(&node.shape, value)
        {
            return Err(native_edit_error("INTENT_VALUE_INVALID"));
        }
        let owner = native_owner(node, &path);
        match value {
            Some(value) => {
                let edit = IntentPathEdit {
                    path,
                    value,
                    list_edit: None,
                };
                if self.guided_intent.path_edits.get(&owner) == Some(&edit) {
                    self.upstream.claim_intent_path(&owner, &edit)?;
                } else {
                    self.guided_intent.path_edits.insert(owner, edit);
                }
            }
            None => {
                self.guided_intent.path_edits.remove(&owner);
            }
        }
        Ok(())
    }
}

type RequiredNativeEdit<'a> = (&'a IntentNode, SemanticPath, Vec<String>, Value);

fn required_native_edits<'a>(
    profile: &CoreCapabilityProfile,
    catalog: &'a IntentCatalog,
    node: &'a IntentNode,
    primary_path: &SemanticPath,
    actual: &[String],
    value: Option<&SemanticValue>,
    upstream: &Value,
) -> Result<Vec<RequiredNativeEdit<'a>>> {
    let Some(SemanticValue::Present(value)) = value else {
        return Ok(Vec::new());
    };
    let descriptor = profile.reviewed_descriptor()?;
    let tags = descriptor.reported_build_tags_for(&profile.baseline_tag);
    let mut document = upstream.clone();
    apply_semantic_value(
        &mut document,
        primary_path,
        &SemanticValue::Present(value.clone()),
    )
    .map_err(|_| native_edit_error("INTENT_NATIVE_SHAPE_REQUIRES_EDITOR"))?;
    let mut queue = VecDeque::from([(node, primary_path.clone(), actual.to_vec(), value.clone())]);
    let mut edits: Vec<RequiredNativeEdit<'a>> = Vec::new();
    while let Some((node, path, actual, value)) = queue.pop_front() {
        for variant in &descriptor.semantic_rules {
            let rule = &variant.rule;
            let crate::KnowledgeValueConstraint::Requires {
                when_value,
                sibling,
                value: required,
            } = &rule.constraint
            else {
                continue;
            };
            if !variant.releases.contains(&profile.baseline_tag)
                || rule.path != node.pattern
                || &value != when_value
            {
                continue;
            }
            let parent = upstream::value_at(&document, &path[..path.len().saturating_sub(1)])?;
            if !rule.when.iter().all(|predicate| match &parent {
                SemanticValue::Present(parent) => parent
                    .get(&predicate.sibling)
                    .map_or(predicate.allow_missing, |value| {
                        predicate.values.contains(value)
                    }),
                SemanticValue::Missing => predicate.allow_missing,
            }) {
                continue;
            }
            match evaluate_build_condition(
                profile.program,
                profile.build.as_ref(),
                &rule.build_constraint,
                &tags,
            ) {
                BuildCondition::Excluded => continue,
                BuildCondition::Unconfirmed => {
                    return Err(native_edit_error("CORE_BUILD_CAPABILITY_UNCONFIRMED"));
                }
                BuildCondition::Satisfied => {}
            }
            let mut sibling_path = path.clone();
            *sibling_path.last_mut().ok_or_else(catalog_unavailable)? = SemanticPathSegment::Key {
                key: sibling.clone(),
            };
            if upstream::value_at(&document, &sibling_path)?
                == SemanticValue::Present(required.clone())
            {
                continue;
            }
            // Conflicting requirements cannot silently replace an explicit user choice.
            if sibling_path == *primary_path
                || edits
                    .iter()
                    .any(|(_, previous, _, _)| previous == &sibling_path)
            {
                return Err(native_edit_error(&rule.message_key));
            }
            let mut sibling_actual = actual.clone();
            *sibling_actual.last_mut().ok_or_else(catalog_unavailable)? = sibling.clone();
            let mut pattern = rule.path.clone();
            *pattern.last_mut().ok_or_else(catalog_unavailable)? = sibling.clone();
            let matching = catalog
                .nodes
                .iter()
                .filter(|related| {
                    related.pattern == pattern
                        && selections_match(&document, related, &sibling_actual)
                })
                .collect::<Vec<_>>();
            let [related] = matching.as_slice() else {
                return Err(native_edit_error("INTENT_LINKED_EDIT_UNAVAILABLE"));
            };
            if related.disposition != IntentNodeDisposition::Guided || related.control.is_none()
                || !native_value_matches(&related.shape, required) || contains_protected_value(required)
                || sibling_path.iter().any(|segment| matches!(segment, SemanticPathSegment::Key { key } if protected_key(key)))
                || edits.len() >= 128 {
                return Err(native_edit_error("INTENT_LINKED_EDIT_UNAVAILABLE"));
            }
            apply_semantic_value(
                &mut document,
                &sibling_path,
                &SemanticValue::Present(required.clone()),
            )
            .map_err(|_| native_edit_error("INTENT_LINKED_EDIT_UNAVAILABLE"))?;
            edits.push((
                related,
                sibling_path.clone(),
                sibling_actual.clone(),
                required.clone(),
            ));
            queue.push_back((related, sibling_path, sibling_actual, required.clone()));
        }
    }
    Ok(edits)
}

fn validate_native_impact(
    profile: &CoreCapabilityProfile,
    document: &Value,
    affected: &[Vec<String>],
) -> Result<()> {
    let descriptor = profile.reviewed_descriptor()?;
    for issue in profile.assess(document)?.issues {
        let relevant = affected.iter().any(|path| {
            path.starts_with(&issue.path) || issue.path.starts_with(path)
                || (path[..path.len().saturating_sub(1)] == issue.path[..issue.path.len().saturating_sub(1)]
                    && descriptor.semantic_rules.iter().any(|variant| {
                        let rule = &variant.rule;
                        variant.releases.contains(&profile.baseline_tag) && rule.id == issue.rule_id
                            && (rule.when.iter().any(|condition| path.last() == Some(&condition.sibling))
                                || matches!(&rule.constraint, crate::KnowledgeValueConstraint::Requires { sibling, .. } if path.last() == Some(sibling)))
                    }))
        });
        if relevant {
            return Err(native_edit_error(&issue.message_key));
        }
    }
    Ok(())
}

fn index_identities(
    document: &Value,
    path: &mut Vec<String>,
    identities: &mut BTreeMap<(Vec<String>, String, String), usize>,
    visits: &mut usize,
) -> Result<()> {
    *visits += 1;
    if *visits > 32768 || path.len() > 64 {
        return Err(catalog_unavailable());
    }
    match document {
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                if let Some((field, value)) = semantic_identity(item) {
                    *identities.entry((path.clone(), field, value)).or_default() += 1;
                }
                path.push(index.to_string());
                index_identities(item, path, identities, visits)?;
                path.pop();
            }
        }
        Value::Object(fields) => {
            for (key, value) in fields {
                path.push(key.clone());
                index_identities(value, path, identities, visits)?;
                path.pop();
            }
        }
        _ => {}
    }
    Ok(())
}

fn native_reference(
    index: &IntentContextIndex,
    node: &IntentNode,
    actual: &[String],
    value: Option<&Value>,
) -> Result<String> {
    let path = actual_semantic_path(index, actual)
        .map(|path| display_semantic_path(&path))
        .unwrap_or_else(|_| {
            format!(
                "/{}",
                actual
                    .iter()
                    .map(|part| part.replace('~', "~0").replace('/', "~1"))
                    .collect::<Vec<_>>()
                    .join("/")
            )
        });
    Ok(hash_bytes(&serde_json::to_vec(&(
        &node.node_id,
        path,
        value.is_some(),
        value,
    ))?))
}

fn actual_semantic_path(index: &IntentContextIndex, actual: &[String]) -> Result<SemanticPath> {
    let mut path = Vec::new();
    let mut current = &index.document;
    for (depth, key) in actual.iter().enumerate() {
        if let Some(items) = current.as_array() {
            let element = key
                .parse::<usize>()
                .ok()
                .and_then(|index| items.get(index))
                .ok_or_else(|| native_edit_error("INTENT_PLAN_STALE"))?;
            let (field, value) = semantic_identity(element)
                .ok_or_else(|| native_edit_error("INTENT_NATIVE_SHAPE_REQUIRES_EDITOR"))?;
            if index
                .identities
                .get(&(actual[..depth].to_vec(), field.clone(), value.clone()))
                != Some(&1)
            {
                return Err(native_edit_error("INTENT_OBJECT_AMBIGUOUS"));
            }
            path.push(SemanticPathSegment::Identity { field, value });
            current = element;
        } else {
            path.push(SemanticPathSegment::Key { key: key.clone() });
            if let Some(value) = current.get(key) {
                current = value;
            } else if depth + 1 != actual.len() || !current.is_object() {
                return Err(native_edit_error("INTENT_PLAN_STALE"));
            }
        }
    }
    Ok(path)
}

pub(super) fn native_edit_anchor(
    profile: &CoreCapabilityProfile,
    state: &ConfigurationState,
    actual: &[String],
) -> Result<(SemanticPath, Vec<String>)> {
    let index = context_index(profile, state)?;
    let mut path = Vec::new();
    let mut current = &index.document;
    for (depth, key) in actual.iter().enumerate() {
        if let Some(items) = current.as_array() {
            let element = key
                .parse::<usize>()
                .ok()
                .and_then(|index| items.get(index))
                .ok_or_else(|| native_edit_error("INTENT_PLAN_STALE"))?;
            let Some((field, value)) = semantic_identity(element) else {
                // An ordered sequence is one semantic value, not an index-addressed object.
                return Ok((path, actual[depth..].to_vec()));
            };
            if index
                .identities
                .get(&(actual[..depth].to_vec(), field.clone(), value.clone()))
                != Some(&1)
            {
                return Err(native_edit_error("INTENT_OBJECT_AMBIGUOUS"));
            }
            path.push(SemanticPathSegment::Identity { field, value });
            current = element;
        } else {
            path.push(SemanticPathSegment::Key { key: key.clone() });
            current = current
                .get(key)
                .ok_or_else(|| native_edit_error("INTENT_PLAN_STALE"))?;
        }
    }
    Ok((path, Vec::new()))
}

pub(super) fn native_removal_replacements(
    profile: &CoreCapabilityProfile,
    state: &ConfigurationState,
    path: &SemanticPath,
) -> Result<Vec<crate::configuration_references::ReferenceReplacement>> {
    let index = context_index(profile, state)?;
    if !index.references.is_referenced(&index.document, path)? {
        return Ok(Vec::new());
    }
    index
        .references
        .removal_replacements(profile, &index.document, path)
}

pub(super) fn breaks_known_references(
    profile: &CoreCapabilityProfile,
    previous: &ConfigurationState,
    updated: &ConfigurationState,
) -> Result<bool> {
    let before = context_index(profile, previous)?;
    let after = context_index(profile, updated)?;
    Ok(before.references.breaks_known_references(&after.references))
}

pub(super) fn candidate_assessment(
    profile: &CoreCapabilityProfile,
    state: &ConfigurationState,
) -> Result<crate::ConfigurationAssessment> {
    Ok(context_index(profile, state)?.assessment.clone())
}

fn native_values<'a>(
    index: &'a IntentContextIndex,
    node: &IntentNode,
    format: ConfigurationFormat,
) -> Result<Vec<(Vec<String>, Option<&'a Value>)>> {
    let mut present = Vec::new();
    crate::configuration_assessment::select_paths(
        &index.document,
        &node.pattern,
        &mut Vec::new(),
        &mut present,
        &mut 0,
        format == ConfigurationFormat::Jsonc,
    )?;
    let existing_parents = present
        .iter()
        .map(|(path, _)| path[..path.len().saturating_sub(1)].to_vec())
        .collect::<BTreeSet<_>>();
    let mut values: Vec<_> = present
        .into_iter()
        .map(|(path, value)| (path, Some(value)))
        .collect();
    if let Some(field) = node.pattern.last().filter(|field| *field != "*") {
        let mut parents = Vec::new();
        crate::configuration_assessment::select_paths(
            &index.document,
            &node.pattern[..node.pattern.len() - 1],
            &mut Vec::new(),
            &mut parents,
            &mut 0,
            format == ConfigurationFormat::Jsonc,
        )?;
        for (mut path, parent) in parents {
            if parent.is_object() && !existing_parents.contains(&path) {
                path.push(field.clone());
                values.push((path, None));
            }
        }
    }
    Ok(values)
}

fn native_value_matches(shape: &KnowledgeTypeShape, value: &Value) -> bool {
    match shape {
        KnowledgeTypeShape::Pointer { element } => {
            value.is_null() || native_value_matches(element, value)
        }
        KnowledgeTypeShape::Builtin { name } => match name.as_str() {
            "bool" => value.is_boolean(),
            "string" => value.is_string(),
            "int8" => value
                .as_i64()
                .is_some_and(|value| i8::try_from(value).is_ok()),
            "int16" => value
                .as_i64()
                .is_some_and(|value| i16::try_from(value).is_ok()),
            "int32" => value
                .as_i64()
                .is_some_and(|value| i32::try_from(value).is_ok()),
            "int" | "int64" => value.as_i64().is_some(),
            "uint8" => value
                .as_u64()
                .is_some_and(|value| u8::try_from(value).is_ok()),
            "uint16" => value
                .as_u64()
                .is_some_and(|value| u16::try_from(value).is_ok()),
            "uint32" => value
                .as_u64()
                .is_some_and(|value| u32::try_from(value).is_ok()),
            "uint" | "uint64" => value.as_u64().is_some(),
            "float32" => value
                .as_f64()
                .is_some_and(|value| (value as f32).is_finite()),
            "float64" => value.as_f64().is_some_and(f64::is_finite),
            _ => false,
        },
        _ => false,
    }
}

fn native_edit_error(key: &str) -> CamelliaNexusError {
    CamelliaNexusError::new(ErrorCode::ConfigInvalid, "Review this setting").with_message_key(key)
}

fn selections_match(document: &Value, node: &IntentNode, path: &[String]) -> bool {
    node.type_selections.iter().all(|selector| {
        let mut value = document;
        for (index, segment) in selector.path.iter().enumerate() {
            let segment = if segment == "*" {
                path.get(index).map(String::as_str).unwrap_or("")
            } else {
                segment
            };
            let Some(child) = (if let Some(items) = value.as_array() {
                segment
                    .parse::<usize>()
                    .ok()
                    .and_then(|index| items.get(index))
            } else {
                value.get(segment)
            }) else {
                return false;
            };
            value = child;
        }
        value
            .get(&selector.field)
            .and_then(Value::as_str)
            .is_some_and(|value| match selector.comparison {
                crate::KnowledgeDiscriminatorComparison::Exact => value == selector.value,
                crate::KnowledgeDiscriminatorComparison::AsciiCaseInsensitive => {
                    value.eq_ignore_ascii_case(&selector.value)
                }
            })
    })
}

fn pattern_matches(pattern: &[String], path: &[String]) -> bool {
    pattern.len() <= path.len()
        && pattern
            .iter()
            .zip(path)
            .all(|(expected, actual)| expected == "*" || expected == actual)
}

pub(super) fn protected_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().replace(['-', '_'], "").as_str(),
        "password"
            | "pass"
            | "secret"
            | "token"
            | "uuid"
            | "privatekey"
            | "key"
            | "authentication"
            | "accounts"
            | "users"
            | "clientid"
            | "clients"
            | "apikey"
            | "presharedkey"
            | "privatekeypath"
            | "authstr"
    )
}

pub(super) fn contains_protected_value(value: &Value) -> bool {
    match value {
        Value::Object(fields) => fields
            .iter()
            .any(|(key, value)| protected_key(key) || contains_protected_value(value)),
        Value::Array(items) => items.iter().any(contains_protected_value),
        Value::String(text) => {
            uuid::Uuid::parse_str(text).is_ok()
                || url::Url::parse(text).is_ok_and(|url| {
                    !url.username().is_empty()
                        || url.password().is_some()
                        || url.query_pairs().any(|(key, _)| protected_key(&key))
                })
        }
        _ => false,
    }
}

pub(super) fn display_value(value: &Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        if protected_key(key) {
                            json!({"protected":true,"isSet": !value.is_null()})
                        } else {
                            display_value(value)
                        },
                    )
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(display_value).collect()),
        Value::String(_) if contains_protected_value(value) => {
            json!({"protected":true,"isSet":true})
        }
        _ => value.clone(),
    }
}
