use super::*;
use crate::core_build_constraints::{BuildCondition, evaluate_build_condition};
use crate::{KnowledgeSourceReference, KnowledgeTypeShape};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum NativeInputAvailability {
    Available,
    Excluded,
    Unconfirmed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeInputSelector {
    pub path: Vec<String>,
    pub field: String,
    pub value: String,
    pub comparison: crate::KnowledgeDiscriminatorComparison,
}

#[derive(Debug, Clone)]
pub(crate) struct NativeInputNode {
    pub id: String,
    pub parent_id: Option<String>,
    pub path: Vec<String>,
    pub selectors: Vec<NativeInputSelector>,
    pub name: String,
    pub order: usize,
    pub shape: KnowledgeTypeShape,
    pub source: KnowledgeSourceReference,
    pub availability: NativeInputAvailability,
    pub complete: bool,
}

/// Enumerates the wire structures with exactly the same tag, embedding, alias,
/// and build selection as the configuration evidence walker.
pub(crate) fn native_input_nodes(profile: &CoreCapabilityProfile) -> Result<Vec<NativeInputNode>> {
    let walker = FieldWalker::for_profile(profile)?;
    let mut catalog = InputCollector {
        walker,
        nodes: Vec::new(),
        ids: BTreeSet::new(),
        availabilities: BTreeMap::new(),
        sibling_orders: BTreeMap::new(),
    };
    for id in &catalog.walker.descriptor.configuration_roots {
        let declaration = catalog
            .walker
            .declaration(id)
            .ok_or_else(invalid_knowledge)?;
        catalog.fields(
            declaration,
            FieldEncoding::native(profile.program),
            &[],
            &[],
            None,
            &mut BTreeSet::new(),
        )?;
    }
    for variant in &catalog.walker.descriptor.decoder_bindings {
        if !variant.releases.contains(&profile.baseline_tag) {
            continue;
        }
        let binding = &variant.binding;
        let selectors = vec![NativeInputSelector {
            path: binding.path.clone(),
            field: binding.discriminator.clone(),
            value: binding.value.clone(),
            comparison: binding.discriminator_comparison,
        }];
        let mut path = binding.path.clone();
        path.extend(binding.options_path.clone());
        let parent = catalog.parent_for(&path, &[]);
        let declaration = catalog.walker.declaration(&binding.declaration);
        let mut availability =
            if binding.constructor_status == crate::KnowledgeConstructorStatus::Rejected {
                NativeInputAvailability::Excluded
            } else {
                catalog.availability(&binding.build_constraint)
            };
        if let Some(layout) = &binding.object_layout {
            availability = availability.and(catalog.availability(&layout.build_constraint));
        }
        let source = binding
            .evidence
            .first()
            .ok_or_else(invalid_knowledge)?
            .source
            .clone();
        let id = catalog.push(NativeInputNode {
            id: input_id(&path, &selectors),
            parent_id: parent,
            path: path.clone(),
            selectors: selectors.clone(),
            name: binding.value.clone(),
            order: 0,
            shape: KnowledgeTypeShape::Structure,
            source,
            availability,
            complete: binding.object_layout.is_some() && declaration.is_some(),
        })?;
        let Some(layout) = &binding.object_layout else {
            continue;
        };
        let Some(declaration) = declaration else {
            continue;
        };
        let encoding = FieldEncoding {
            tag: &binding.encoding,
            comparison: layout.key_comparison,
            field_names: layout.root_field_names,
            flatten_embedded: true,
            nested_embedded: layout.nested_embedded,
        };
        for envelope in &layout.envelope_declarations {
            if let Some(envelope) = catalog.walker.declaration(envelope) {
                catalog.fields(
                    envelope,
                    encoding,
                    &path,
                    &selectors,
                    Some(&id),
                    &mut BTreeSet::new(),
                )?;
            }
        }
        catalog.fields(
            declaration,
            encoding,
            &path,
            &selectors,
            Some(&id),
            &mut BTreeSet::new(),
        )?;
        for shared in &layout.shared_options {
            let mut shared_path = path.clone();
            shared_path.extend(shared.path.clone());
            if let Some(shared_declaration) = catalog.walker.declaration(&shared.declaration) {
                let parent = catalog
                    .parent_for(&shared_path, &selectors)
                    .unwrap_or_else(|| id.clone());
                catalog.fields(
                    shared_declaration,
                    encoding.nested(),
                    &shared_path,
                    &selectors,
                    Some(&parent),
                    &mut BTreeSet::new(),
                )?;
            }
        }
    }
    Ok(catalog.nodes)
}

fn input_id(path: &[String], selectors: &[NativeInputSelector]) -> String {
    // Stable wire identity; release hashes and display labels are not node identities.
    format!(
        "native:{}",
        hash_bytes(&serde_json::to_vec(&(path, selectors)).expect("wire identity"))
    )
}

struct InputCollector<'a> {
    walker: FieldWalker<'a>,
    nodes: Vec<NativeInputNode>,
    ids: BTreeSet<String>,
    availabilities: BTreeMap<String, NativeInputAvailability>,
    sibling_orders: BTreeMap<Option<String>, usize>,
}

impl<'a> InputCollector<'a> {
    fn availability(&self, condition: &str) -> NativeInputAvailability {
        match evaluate_build_condition(
            self.walker.profile.program,
            self.walker.profile.build.as_ref(),
            condition,
            &self
                .walker
                .descriptor
                .reported_build_tags_for(&self.walker.profile.baseline_tag),
        ) {
            BuildCondition::Satisfied => NativeInputAvailability::Available,
            BuildCondition::Excluded => NativeInputAvailability::Excluded,
            BuildCondition::Unconfirmed => NativeInputAvailability::Unconfirmed,
        }
    }

    fn parent_for(&self, path: &[String], selectors: &[NativeInputSelector]) -> Option<String> {
        (0..path.len())
            .rev()
            .map(|length| input_id(&path[..length], selectors))
            .find(|id| self.ids.contains(id))
    }

    fn push(&mut self, mut node: NativeInputNode) -> Result<String> {
        if self.nodes.len() >= MAX_EVIDENCE_VISITS {
            return Err(evidence_limit());
        }
        let id = node.id.clone();
        if self.ids.insert(id.clone()) {
            if let Some(parent) = node
                .parent_id
                .as_ref()
                .and_then(|id| self.availabilities.get(id))
            {
                node.availability = node.availability.and(*parent);
            }
            self.availabilities.insert(id.clone(), node.availability);
            let order = self
                .sibling_orders
                .entry(node.parent_id.clone())
                .or_default();
            node.order = *order;
            *order += 1;
            self.nodes.push(node);
        }
        Ok(id)
    }

    fn fields(
        &mut self,
        declaration: &'a KnowledgeDeclaration,
        encoding: FieldEncoding<'a>,
        path: &[String],
        selectors: &[NativeInputSelector],
        parent: Option<&str>,
        visiting: &mut BTreeSet<&'a str>,
    ) -> Result<()> {
        if !visiting.insert(&declaration.id) {
            return Ok(());
        }
        let mut fields = Vec::new();
        let complete = self.walker.collect_fields(
            declaration,
            encoding,
            &mut fields,
            &mut BTreeSet::new(),
            path.len(),
        )?;
        for field in fields {
            let mut field_path = path.to_vec();
            field_path.push(field.name.clone());
            let id = self.push(NativeInputNode {
                id: input_id(&field_path, selectors),
                parent_id: parent.map(str::to_owned),
                path: field_path.clone(),
                selectors: selectors.to_vec(),
                name: field.name,
                order: 0,
                shape: field.field.shape.clone(),
                source: field.field.source.clone(),
                availability: self.availability(&declaration.build_constraint),
                complete,
            })?;
            self.shape(
                &field.field.shape,
                encoding.nested(),
                &field_path,
                selectors,
                &id,
                visiting,
            )?;
        }
        visiting.remove(declaration.id.as_str());
        Ok(())
    }

    fn shape(
        &mut self,
        shape: &'a KnowledgeTypeShape,
        encoding: FieldEncoding<'a>,
        path: &[String],
        selectors: &[NativeInputSelector],
        parent: &str,
        visiting: &mut BTreeSet<&'a str>,
    ) -> Result<()> {
        visit(&mut self.walker.visits, path.len())?;
        match shape {
            KnowledgeTypeShape::Pointer { element } => {
                self.shape(element, encoding, path, selectors, parent, visiting)
            }
            KnowledgeTypeShape::Sequence { element }
            | KnowledgeTypeShape::Mapping { element, .. } => {
                let mut items = path.to_vec();
                items.push("*".into());
                self.shape(element, encoding, &items, selectors, parent, visiting)
            }
            KnowledgeTypeShape::Named { .. } => {
                if let Some(declaration) = self.walker.named(shape) {
                    if encoding.custom_object_decoder(declaration) {
                        if let Some(decoder) = self
                            .walker
                            .descriptor
                            .value_decoders
                            .iter()
                            .find_map(|variant| {
                                (variant.releases.contains(&self.walker.profile.baseline_tag)
                                    && variant.decoder.declaration == declaration.id
                                    && variant.decoder.encoding == encoding.tag)
                                    .then_some(&variant.decoder)
                            })
                            && self.availability(&decoder.build_constraint)
                                == NativeInputAvailability::Available
                            && let Some(object) =
                                self.walker.declaration(&decoder.object_declaration)
                        {
                            return self.fields(
                                object,
                                FieldEncoding {
                                    comparison: decoder.key_comparison,
                                    ..encoding
                                },
                                path,
                                selectors,
                                Some(parent),
                                visiting,
                            );
                        }
                        // A custom decoder without a reviewed layout is intentionally a leaf.
                        return Ok(());
                    }
                    if matches!(declaration.shape, KnowledgeTypeShape::Structure)
                        || self.walker.named(&declaration.shape).is_some()
                    {
                        self.fields(
                            declaration,
                            encoding,
                            path,
                            selectors,
                            Some(parent),
                            visiting,
                        )
                    } else {
                        self.shape(
                            &declaration.shape,
                            encoding,
                            path,
                            selectors,
                            parent,
                            visiting,
                        )
                    }
                } else {
                    Ok(())
                }
            }
            // Generic, dynamic, and opaque inputs need a reviewed decoder to expose child nodes.
            _ => Ok(()),
        }
    }
}

impl NativeInputAvailability {
    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::Excluded, _) | (_, Self::Excluded) => Self::Excluded,
            (Self::Unconfirmed, _) | (_, Self::Unconfirmed) => Self::Unconfirmed,
            _ => Self::Available,
        }
    }
}
