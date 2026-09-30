//! Evidence for configuration fields declared by reviewed source or the current binary.
//! A permissive schema is not evidence that an extension is decoded.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::{
    CamelliaNexusError, ConfigurationAssessmentEvidence, ConfigurationAssessmentIssue,
    ConfigurationSchemaDocument, CoreCapabilityProfile, ErrorCode, KnowledgeDeclaration,
    ProgramKind, Result, config_service::hash_bytes, embedded_core_knowledge,
};

const MAX_EVIDENCE_VISITS: usize = 32_768;
const MAX_EVIDENCE_DEPTH: usize = 64;

pub(crate) struct UndeclaredEntry {
    pub path: Vec<String>,
    pub evidence: ConfigurationAssessmentEvidence,
}

pub(crate) fn undeclared_entries(
    profile: &CoreCapabilityProfile,
    document: &Value,
) -> Result<Vec<UndeclaredEntry>> {
    let knowledge = embedded_core_knowledge()?;
    let descriptor = knowledge
        .program(profile.program)
        .ok_or_else(invalid_knowledge)?;
    let release = descriptor
        .releases
        .iter()
        .find(|release| release.tag == profile.baseline_tag)
        .ok_or_else(invalid_knowledge)?;
    let mut declarations: BTreeMap<&str, Vec<&KnowledgeDeclaration>> = BTreeMap::new();
    for variant in &descriptor.declarations {
        if variant.releases.contains(&profile.baseline_tag) {
            declarations
                .entry(&variant.declaration.id)
                .or_default()
                .push(&variant.declaration);
        }
    }
    let mut walker = FieldWalker {
        profile,
        descriptor,
        module_path: &release.module_path,
        declarations,
        visits: 0,
        entries: Vec::new(),
    };
    let encoding = FieldEncoding::native(profile.program);
    for id in &descriptor.configuration_roots {
        let root = walker.declaration(id).ok_or_else(invalid_knowledge)?;
        walker.walk_declaration(root, document, encoding, &mut Vec::new(), 0, true)?;
    }
    walker.walk_decoder_options(document)?;
    Ok(walker.entries)
}

struct FieldWalker<'a> {
    profile: &'a CoreCapabilityProfile,
    descriptor: &'a crate::ProgramKnowledgeDescriptor,
    module_path: &'a str,
    declarations: BTreeMap<&'a str, Vec<&'a KnowledgeDeclaration>>,
    visits: usize,
    entries: Vec<UndeclaredEntry>,
}

#[derive(Clone)]
struct DeclaredField<'a> {
    name: String,
    field: &'a crate::KnowledgeField,
    depth: usize,
    comparison: crate::KnowledgeFieldComparison,
    envelope: bool,
}

#[derive(Clone, Copy)]
struct FieldEncoding<'a> {
    tag: &'a str,
    comparison: crate::KnowledgeFieldComparison,
    field_names: bool,
    flatten_embedded: bool,
    nested_embedded: bool,
}

impl FieldEncoding<'_> {
    fn native(program: ProgramKind) -> Self {
        if program == ProgramKind::Mihomo {
            Self {
                tag: "yaml",
                comparison: crate::KnowledgeFieldComparison::Exact,
                field_names: true,
                flatten_embedded: true,
                nested_embedded: true,
            }
        } else {
            Self {
                tag: "json",
                comparison: crate::KnowledgeFieldComparison::AsciiCaseInsensitive,
                field_names: true,
                flatten_embedded: true,
                nested_embedded: true,
            }
        }
    }

    fn nested(self) -> Self {
        Self {
            field_names: true,
            flatten_embedded: self.nested_embedded,
            ..self
        }
    }

    fn custom_object_decoder(self, declaration: &KnowledgeDeclaration) -> bool {
        let methods: &[&str] = match self.tag {
            "json" => &["UnmarshalJSON", "UnmarshalJSONContext"],
            "yaml" => &["UnmarshalYAML"],
            _ => return false,
        };
        declaration
            .methods
            .iter()
            .any(|method| methods.contains(&method.as_str()))
    }
}

struct ObjectFields<'a> {
    declaration: &'a KnowledgeDeclaration,
    fields: Vec<DeclaredField<'a>>,
    complete: bool,
    encoding: FieldEncoding<'a>,
    layout: Option<&'a crate::KnowledgeDecodedObjectLayout>,
    discriminator: Option<&'a str>,
}

impl<'a> FieldWalker<'a> {
    fn walk_decoder_options(&mut self, document: &Value) -> Result<()> {
        use crate::core_build_constraints::{BuildCondition, evaluate_build_condition};
        let tags = self
            .descriptor
            .reported_build_tags_for(&self.profile.baseline_tag);
        let mut groups: BTreeMap<(&[String], &str), Vec<&crate::KnowledgeDecoderBinding>> =
            BTreeMap::new();
        for variant in &self.descriptor.decoder_bindings {
            let binding = &variant.binding;
            if variant.releases.contains(&self.profile.baseline_tag)
                && binding.object_layout.is_some()
            {
                groups
                    .entry((&binding.path, &binding.discriminator))
                    .or_default()
                    .push(binding);
            }
        }
        for ((pattern, discriminator), bindings) in groups {
            let mut selected = Vec::new();
            let case_insensitive = crate::ConfigurationFormat::for_kind(self.profile.program)
                == Some(crate::ConfigurationFormat::Jsonc);
            crate::configuration_assessment::select_paths(
                document,
                pattern,
                &mut Vec::new(),
                &mut selected,
                &mut self.visits,
                case_insensitive,
            )?;
            let selected = selected.iter().flat_map(|(path, value)| {
                let mut kinds: Vec<_> = crate::configuration_assessment::object_fields(
                    value,
                    discriminator,
                    case_insensitive,
                )
                .map(|(_, discriminator)| discriminator.as_str())
                .collect();
                if kinds.is_empty() {
                    kinds.push(None);
                }
                kinds
                    .into_iter()
                    .map(move |kind| (path.clone(), *value, kind))
            });
            for (mut path, value, kind) in selected {
                let matching: Vec<_> = bindings
                    .iter()
                    .filter(|binding| kind.is_some_and(|kind| binding.matches_discriminator(kind)))
                    .collect();
                if matching.is_empty() {
                    if self.entries.len() >= 128 {
                        return Err(evidence_limit());
                    }
                    self.entries.push(UndeclaredEntry {
                        path: path.clone(),
                        evidence: ConfigurationAssessmentEvidence::SourceBehavior(
                            bindings[0]
                                .evidence
                                .first()
                                .ok_or_else(invalid_knowledge)?
                                .clone(),
                        ),
                    });
                    continue;
                }
                let mut options = BTreeSet::new();
                let mut uncertain = false;
                for binding in matching {
                    match evaluate_build_condition(
                        self.profile.program,
                        self.profile.build.as_ref(),
                        &binding.build_constraint,
                        &tags,
                    ) {
                        BuildCondition::Satisfied
                            if binding.constructor_status
                                == crate::KnowledgeConstructorStatus::Registered =>
                        {
                            options.insert((
                                &binding.declaration,
                                &binding.options_path,
                                binding.encoding.as_str(),
                                binding.object_layout.as_ref().unwrap(),
                            ));
                        }
                        BuildCondition::Excluded => {}
                        _ => uncertain = true,
                    }
                }
                if uncertain || options.len() != 1 {
                    continue;
                }
                let (id, options_path, tag, layout) = options.into_iter().next().unwrap();
                if evaluate_build_condition(
                    self.profile.program,
                    self.profile.build.as_ref(),
                    &layout.build_constraint,
                    &tags,
                ) != BuildCondition::Satisfied
                {
                    if self.entries.len() >= 128 {
                        return Err(evidence_limit());
                    }
                    self.entries.push(UndeclaredEntry {
                        path: path.clone(),
                        evidence: ConfigurationAssessmentEvidence::SourceBehavior(
                            bindings[0]
                                .evidence
                                .first()
                                .ok_or_else(invalid_knowledge)?
                                .clone(),
                        ),
                    });
                    continue;
                }
                let encoding = FieldEncoding {
                    tag,
                    comparison: layout.key_comparison,
                    field_names: layout.root_field_names,
                    flatten_embedded: true,
                    nested_embedded: layout.nested_embedded,
                };
                let Some(declaration) = self.declaration(id) else {
                    continue;
                };
                let mut selected_options = Vec::new();
                crate::configuration_assessment::select_paths(
                    value,
                    options_path,
                    &mut path,
                    &mut selected_options,
                    &mut self.visits,
                    case_insensitive,
                )?;
                for (mut path, options) in selected_options {
                    if options_path.is_empty() {
                        self.walk_flat_object(
                            declaration,
                            layout,
                            encoding,
                            options,
                            &mut path,
                            discriminator,
                        )?;
                    } else {
                        self.walk_declaration(declaration, options, encoding, &mut path, 0, false)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn declaration(&self, id: &str) -> Option<&'a KnowledgeDeclaration> {
        use crate::core_build_constraints::{BuildCondition, evaluate_build_condition};
        let tags = self
            .descriptor
            .reported_build_tags_for(&self.profile.baseline_tag);
        let mut selected = None;
        for declaration in self.declarations.get(id)? {
            match evaluate_build_condition(
                self.profile.program,
                self.profile.build.as_ref(),
                &declaration.build_constraint,
                &tags,
            ) {
                BuildCondition::Satisfied if selected.is_none() => selected = Some(*declaration),
                BuildCondition::Excluded => {}
                _ => return None,
            }
        }
        selected
    }

    fn named(&self, shape: &crate::KnowledgeTypeShape) -> Option<&'a KnowledgeDeclaration> {
        use crate::KnowledgeTypeShape as Shape;
        match shape {
            Shape::Pointer { element } => self.named(element),
            Shape::Named { package, name } => {
                let package = package
                    .strip_prefix(self.module_path)
                    .and_then(|suffix| suffix.strip_prefix('/'))
                    .unwrap_or(package);
                self.declaration(&format!("{package}#{name}"))
            }
            _ => None,
        }
    }

    fn collect_fields(
        &mut self,
        declaration: &'a KnowledgeDeclaration,
        encoding: FieldEncoding<'a>,
        fields: &mut Vec<DeclaredField<'a>>,
        visiting: &mut BTreeSet<&'a str>,
        depth: usize,
    ) -> Result<bool> {
        use crate::KnowledgeTypeShape as Shape;
        visit(&mut self.visits, depth)?;
        if !visiting.insert(&declaration.id) {
            return Err(invalid_knowledge());
        }
        let complete = if matches!(declaration.shape, Shape::Structure) {
            let mut complete = true;
            for field in &declaration.fields {
                visit(&mut self.visits, depth)?;
                if !field.embedded && !field.name.chars().next().is_some_and(char::is_uppercase) {
                    continue;
                }
                let tag = field
                    .tags
                    .get(encoding.tag)
                    .map(String::as_str)
                    .unwrap_or("");
                let name = tag.split(',').next().unwrap_or("");
                if name == "-" {
                    continue;
                }
                if encoding.tag == "proxy" && tag.split(',').any(|part| part == "remain") {
                    continue;
                }
                let inline = match encoding.tag {
                    "json" => field.embedded && name.is_empty(),
                    "yaml" => name.is_empty() && tag.split(',').any(|part| part == "inline"),
                    "proxy" => {
                        (field.embedded && encoding.flatten_embedded)
                            || tag.split(',').any(|part| part == "squash")
                    }
                    _ => false,
                };
                if inline {
                    if let Some(nested) = self.named(&field.shape) {
                        if encoding.custom_object_decoder(nested) {
                            complete = false;
                        } else {
                            complete &= self.collect_fields(
                                nested,
                                encoding.nested(),
                                fields,
                                visiting,
                                depth + 1,
                            )?;
                        }
                    } else {
                        complete = false;
                    }
                } else {
                    if name.is_empty() && !encoding.field_names {
                        continue;
                    }
                    let name = if !name.is_empty() {
                        name.to_owned()
                    } else if encoding.tag == "yaml" {
                        field.name.to_lowercase()
                    } else {
                        field.name.clone()
                    };
                    fields.push(DeclaredField {
                        name,
                        field,
                        depth,
                        comparison: encoding.comparison,
                        envelope: false,
                    });
                }
            }
            complete
        } else if let Some(underlying) = self.named(&declaration.shape) {
            self.collect_fields(underlying, encoding, fields, visiting, depth + 1)?
        } else {
            false
        };
        visiting.remove(declaration.id.as_str());
        Ok(complete)
    }

    fn walk_declaration(
        &mut self,
        declaration: &'a KnowledgeDeclaration,
        value: &Value,
        encoding: FieldEncoding<'a>,
        path: &mut Vec<String>,
        depth: usize,
        root: bool,
    ) -> Result<()> {
        visit(&mut self.visits, depth)?;
        if !root && encoding.custom_object_decoder(declaration) {
            if let Some(decoder) = self.descriptor.value_decoders.iter().find_map(|variant| {
                (variant.releases.contains(&self.profile.baseline_tag)
                    && variant.decoder.declaration == declaration.id
                    && variant.decoder.encoding == encoding.tag)
                    .then_some(&variant.decoder)
            }) {
                use crate::core_build_constraints::{BuildCondition, evaluate_build_condition};
                let tags = self
                    .descriptor
                    .reported_build_tags_for(&self.profile.baseline_tag);
                if evaluate_build_condition(
                    self.profile.program,
                    self.profile.build.as_ref(),
                    &decoder.build_constraint,
                    &tags,
                ) != BuildCondition::Satisfied
                {
                    return self.undeclared(declaration, path);
                }
                if decoder.scalar_forms.iter().any(|form| match form {
                    crate::KnowledgeScalarForm::Boolean => value.is_boolean(),
                    crate::KnowledgeScalarForm::Null => value.is_null(),
                }) {
                    return Ok(());
                }
                if !value.is_object() {
                    return self.undeclared(declaration, path);
                }
                let object = self
                    .declaration(&decoder.object_declaration)
                    .ok_or_else(invalid_knowledge)?;
                return self.walk_declaration(
                    object,
                    value,
                    FieldEncoding {
                        comparison: decoder.key_comparison,
                        ..encoding
                    },
                    path,
                    depth + 1,
                    false,
                );
            }
            return Ok(());
        }
        if !matches!(declaration.shape, crate::KnowledgeTypeShape::Structure)
            && self.named(&declaration.shape).is_none()
        {
            return self.walk_shape(&declaration.shape, value, encoding, path, depth + 1);
        }
        if !value.is_object() {
            return Ok(());
        }
        let mut fields = Vec::new();
        let complete =
            self.collect_fields(declaration, encoding, &mut fields, &mut BTreeSet::new(), 0)?;
        if root && !complete {
            return Err(invalid_knowledge());
        }
        self.walk_object(
            &ObjectFields {
                declaration,
                fields,
                complete,
                encoding,
                layout: None,
                discriminator: None,
            },
            value,
            path,
            depth,
        )
    }

    fn walk_flat_object(
        &mut self,
        declaration: &'a KnowledgeDeclaration,
        layout: &'a crate::KnowledgeDecodedObjectLayout,
        encoding: FieldEncoding<'a>,
        value: &Value,
        path: &mut Vec<String>,
        discriminator: &'a str,
    ) -> Result<()> {
        let mut fields = Vec::new();
        let mut complete =
            self.collect_fields(declaration, encoding, &mut fields, &mut BTreeSet::new(), 0)?;
        for id in &layout.envelope_declarations {
            let envelope = self.declaration(id).ok_or_else(invalid_knowledge)?;
            let start = fields.len();
            complete &=
                self.collect_fields(envelope, encoding, &mut fields, &mut BTreeSet::new(), 0)?;
            for field in &mut fields[start..] {
                field.comparison = layout.envelope_key_comparison;
                field.envelope = true;
            }
        }
        self.walk_object(
            &ObjectFields {
                declaration,
                fields,
                complete,
                encoding,
                layout: Some(layout),
                discriminator: Some(discriminator),
            },
            value,
            path,
            0,
        )
    }

    fn walk_object(
        &mut self,
        definition: &ObjectFields<'a>,
        value: &Value,
        path: &mut Vec<String>,
        depth: usize,
    ) -> Result<()> {
        let Some(object) = value.as_object() else {
            return Ok(());
        };
        let encoding = definition.encoding;
        for (key, value) in object {
            visit(&mut self.visits, depth)?;
            if key.len() > 1024 {
                return Err(evidence_limit());
            }
            path.push(key.clone());
            if definition.discriminator == Some(key.as_str()) {
                path.pop();
                continue;
            }
            if let Some(shared) = definition.layout.and_then(|layout| {
                layout
                    .shared_options
                    .iter()
                    .find(|shared| shared.path.first() == Some(key))
            }) {
                let declaration = self
                    .declaration(&shared.declaration)
                    .ok_or_else(invalid_knowledge)?;
                let mut selected = Vec::new();
                crate::configuration_assessment::select_paths(
                    value,
                    &shared.path[1..],
                    path,
                    &mut selected,
                    &mut self.visits,
                    false,
                )?;
                for (mut selected_path, value) in selected {
                    if value.is_object() || value.is_null() {
                        self.walk_declaration(
                            declaration,
                            value,
                            encoding,
                            &mut selected_path,
                            depth + 1,
                            false,
                        )?;
                    } else {
                        self.undeclared(declaration, &selected_path)?;
                    }
                }
                path.pop();
                continue;
            }
            let mut matches: Vec<_> = definition
                .fields
                .iter()
                .filter(|field| field.comparison.matches(&field.name, key))
                .collect();
            // Excluded envelope keys never reach the protocol options decoder.
            if matches.iter().any(|field| field.envelope) {
                matches.retain(|field| field.envelope);
            }
            // An exact wire name takes precedence over a case-folded name.
            if matches.iter().any(|field| field.name == *key) {
                matches.retain(|field| field.name == *key);
            }
            matches.sort_by_key(|field| field.depth);
            if let Some(field) = matches.first() {
                if matches
                    .get(1)
                    .is_none_or(|other| other.depth != field.depth)
                {
                    self.walk_shape(
                        &field.field.shape,
                        value,
                        encoding.nested(),
                        path,
                        depth + 1,
                    )?;
                }
            } else if definition.complete {
                self.undeclared(definition.declaration, path)?;
            }
            path.pop();
        }
        Ok(())
    }

    fn undeclared(&mut self, declaration: &KnowledgeDeclaration, path: &[String]) -> Result<()> {
        if self.entries.len() >= 128 {
            return Err(evidence_limit());
        }
        self.entries.push(UndeclaredEntry {
            path: path.to_vec(),
            evidence: ConfigurationAssessmentEvidence::SourceDeclaration {
                source: declaration.source.clone(),
                declaration_hash: hash_bytes(&serde_json::to_vec(declaration)?),
            },
        });
        Ok(())
    }

    fn walk_shape(
        &mut self,
        shape: &'a crate::KnowledgeTypeShape,
        value: &Value,
        encoding: FieldEncoding<'a>,
        path: &mut Vec<String>,
        depth: usize,
    ) -> Result<()> {
        use crate::KnowledgeTypeShape as Shape;
        visit(&mut self.visits, depth)?;
        match shape {
            Shape::Pointer { element } => {
                self.walk_shape(element, value, encoding, path, depth + 1)?
            }
            Shape::Sequence { element } => {
                if let Some(items) = value.as_array() {
                    for (index, value) in items.iter().enumerate() {
                        path.push(index.to_string());
                        self.walk_shape(element, value, encoding, path, depth + 1)?;
                        path.pop();
                    }
                }
            }
            Shape::Named { .. } => {
                if let Some(declaration) = self.named(shape) {
                    self.walk_declaration(declaration, value, encoding, path, depth + 1, false)?;
                }
            }
            // Dynamic maps, generics and custom decoders require their own parsing adapters.
            _ => {}
        }
        Ok(())
    }
}

pub(crate) fn assess_entry_evidence(
    entries: Vec<UndeclaredEntry>,
    document: &Value,
    schema: Option<&ConfigurationSchemaDocument>,
) -> Result<Vec<ConfigurationAssessmentIssue>> {
    if entries.len() > 128 {
        return Err(evidence_limit());
    }
    let schema = schema
        .map(|schema| {
            if hash_bytes(schema.content.as_bytes()) != schema.content_hash {
                return Err(invalid_schema());
            }
            if schema.content.len() > crate::MAX_CONFIGURATION_SCHEMA_BYTES {
                return Err(evidence_limit());
            }
            serde_json::from_str::<Value>(&schema.content).map_err(|_| invalid_schema())
        })
        .transpose()?;
    let mut visits = 0;
    let mut issues = Vec::new();
    for entry in entries {
        let declared = if let Some(root) = schema.as_ref() {
            let (nodes, value) = schema_nodes_at_path(root, document, &entry.path, &mut visits)?;
            !nodes.is_empty() && declares_value(nodes, value, root, &mut visits, 0)?
        } else {
            false
        };
        if !declared {
            issues.push(ConfigurationAssessmentIssue {
                code: "CORE_CONFIGURATION_FIELD_UNCONFIRMED".into(),
                message_key: "CORE_CONFIGURATION_FIELD_UNCONFIRMED".into(),
                path: entry.path,
                rule_id: "configuration-entry-declaration".into(),
                evidence: entry.evidence,
            });
        }
    }
    Ok(issues)
}

fn schema_nodes_at_path<'a>(
    root: &'a Value,
    document: &'a Value,
    path: &[String],
    visits: &mut usize,
) -> Result<(Vec<&'a Value>, &'a Value)> {
    let mut nodes = vec![root];
    let mut value = document;
    for key in path {
        let expanded = expand_nodes(nodes, root, visits, 0)?;
        if expanded.iter().any(|node| node == &&Value::Bool(false)) {
            return Err(invalid_schema());
        }
        nodes = if let Some(object) = value.as_object() {
            value = object.get(key).ok_or_else(invalid_schema)?;
            expanded
                .iter()
                .filter_map(|node| node.get("properties")?.get(key))
                .collect()
        } else if let Some(items) = value.as_array() {
            let index = key.parse::<usize>().map_err(|_| invalid_schema())?;
            value = items.get(index).ok_or_else(invalid_schema)?;
            expanded
                .iter()
                .filter_map(|node| {
                    node.get("prefixItems")
                        .and_then(Value::as_array)
                        .and_then(|items| items.get(index))
                        .or_else(|| node.get("items"))
                })
                .collect()
        } else {
            return Err(invalid_schema());
        };
    }
    Ok((nodes, value))
}

// This extracts affirmative declarations, not JSON Schema validation results.
// Conditional or dynamic declarations need adapter evidence; native validation still checks values.
fn expand_nodes<'a>(
    nodes: Vec<&'a Value>,
    root: &'a Value,
    visits: &mut usize,
    depth: usize,
) -> Result<Vec<&'a Value>> {
    let mut expanded = Vec::new();
    for node in nodes {
        visit(visits, depth)?;
        expanded.push(node);
        if let Some(reference) = node.get("$ref") {
            let reference = reference.as_str().ok_or_else(invalid_schema)?;
            let pointer = reference.strip_prefix('#').ok_or_else(invalid_schema)?;
            if !pointer.is_empty() && !pointer.starts_with('/') {
                return Err(invalid_schema());
            }
            let target = root.pointer(pointer).ok_or_else(invalid_schema)?;
            expanded.extend(expand_nodes(vec![target], root, visits, depth + 1)?);
        }
        if let Some(items) = node.get("allOf") {
            let items = items.as_array().ok_or_else(invalid_schema)?;
            expanded.extend(expand_nodes(
                items.iter().collect(),
                root,
                visits,
                depth + 1,
            )?);
        }
    }
    Ok(expanded)
}

fn declares_value(
    nodes: Vec<&Value>,
    value: &Value,
    root: &Value,
    visits: &mut usize,
    depth: usize,
) -> Result<bool> {
    let nodes = expand_nodes(nodes, root, visits, depth)?;
    // Ambiguous alternatives cannot establish that a named field is decoded.
    if nodes.iter().any(|node| {
        !node.is_object()
            || [
                "$id",
                "$dynamicRef",
                "anyOf",
                "oneOf",
                "if",
                "then",
                "else",
                "not",
            ]
            .iter()
            .any(|key| node.get(key).is_some())
    }) {
        return Ok(false);
    }
    let mut typed = false;
    for node in &nodes {
        if let Some(expected) = node.get("type") {
            let matches = |kind: &str| match kind {
                "object" => value.is_object(),
                "array" => value.is_array(),
                "string" => value.is_string(),
                "boolean" => value.is_boolean(),
                "null" => value.is_null(),
                "number" => value.is_number(),
                "integer" => {
                    value.as_i64().is_some()
                        || value.as_u64().is_some()
                        || value
                            .as_f64()
                            .is_some_and(|number| number.is_finite() && number.fract() == 0.0)
                }
                _ => false,
            };
            let matched = expected.as_str().is_some_and(matches)
                || expected.as_array().is_some_and(|kinds| {
                    !kinds.is_empty()
                        && kinds.iter().all(|kind| {
                            kind.as_str().is_some_and(|kind| {
                                [
                                    "object", "array", "string", "boolean", "null", "number",
                                    "integer",
                                ]
                                .contains(&kind)
                            })
                        })
                        && kinds.iter().filter_map(Value::as_str).any(matches)
                });
            if !matched {
                return Ok(false);
            }
            typed = true;
        }
        if let Some(expected) = node.get("const") {
            if expected != value {
                return Ok(false);
            }
            typed = true;
        }
        if let Some(expected) = node.get("enum") {
            if !expected
                .as_array()
                .is_some_and(|items| items.contains(value))
            {
                return Ok(false);
            }
            typed = true;
        }
    }
    if !typed {
        return Ok(false);
    }
    if let Some(object) = value.as_object() {
        for (name, value) in object {
            let properties: Vec<_> = nodes
                .iter()
                .filter_map(|node| node.get("properties")?.get(name))
                .collect();
            if properties.is_empty() || !declares_value(properties, value, root, visits, depth + 1)?
            {
                return Ok(false);
            }
        }
    }
    if let Some(array) = value.as_array() {
        for (index, value) in array.iter().enumerate() {
            let items: Vec<_> = nodes
                .iter()
                .filter_map(|node| {
                    node.get("prefixItems")
                        .and_then(Value::as_array)
                        .and_then(|items| items.get(index))
                        .or_else(|| node.get("items"))
                })
                .collect();
            if items.is_empty() || !declares_value(items, value, root, visits, depth + 1)? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn visit(visits: &mut usize, depth: usize) -> Result<()> {
    *visits += 1;
    if *visits > MAX_EVIDENCE_VISITS || depth > MAX_EVIDENCE_DEPTH {
        return Err(evidence_limit());
    }
    Ok(())
}

fn invalid_knowledge() -> CamelliaNexusError {
    CamelliaNexusError::new(
        ErrorCode::InvalidSpec,
        "Configuration entry declaration is incomplete",
    )
    .with_message_key("CORE_KNOWLEDGE_INVALID")
}
fn invalid_schema() -> CamelliaNexusError {
    CamelliaNexusError::new(
        ErrorCode::ConfigurationSchemaInvalid,
        "Configuration field evidence is invalid",
    )
    .with_message_key("CORE_CONFIGURATION_SCHEMA_UNCONFIRMED")
}
fn evidence_limit() -> CamelliaNexusError {
    CamelliaNexusError::new(
        ErrorCode::ConfigInvalid,
        "Configuration field evidence limit exceeded",
    )
    .with_message_key("CONFIGURATION_ASSESSMENT_LIMIT")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ConfigurationSchemaSource, CoreBinaryFingerprint, CoreProbeReport, JsonSchemaDialect,
    };
    use serde_json::json;

    fn profile(program: ProgramKind, version: &str) -> CoreCapabilityProfile {
        let output = match program {
            ProgramKind::SingBox => format!("sing-box version {version}"),
            ProgramKind::Mihomo => format!("Mihomo Meta v{version}"),
            ProgramKind::Xray => format!("Xray {version} (Custom)"),
            ProgramKind::Generic => unreachable!(),
        };
        profile_from_output(program, &output)
    }

    fn profile_from_output(program: ProgramKind, output: &str) -> CoreCapabilityProfile {
        CoreCapabilityProfile::resolve(
            program,
            &CoreProbeReport::from_program_output(program, output),
            &CoreBinaryFingerprint {
                sha256: "b".repeat(64),
                size: 1,
                modified_unix_ms: 1,
            },
        )
        .unwrap()
    }

    #[test]
    fn reviewed_scalar_object_decoder_checks_each_patch_and_requires_extension_evidence() {
        let descriptor = embedded_core_knowledge()
            .unwrap()
            .program(ProgramKind::SingBox)
            .unwrap();
        for release in &descriptor.releases {
            let profile = profile_from_output(
                ProgramKind::SingBox,
                &format!(
                    "sing-box version {}\nEnvironment: go1.26.0 linux/amd64\nTags: \n",
                    release.version
                ),
            );
            for value in [
                json!(true),
                json!(false),
                Value::Null,
                json!({}),
                json!({"enabled":true,"version":2}),
                json!({"ENABLED":false,"VERSION":1}),
            ] {
                let document = json!({"outbounds":[{"type":"socks","udp_over_tcp":value}]});
                assert!(
                    undeclared_entries(&profile, &document).unwrap().is_empty(),
                    "{} {value}",
                    release.tag
                );
            }
            for (value, suffix) in [
                (json!({"extension":true}), vec!["extension"]),
                (json!("true"), vec![]),
                (json!([]), vec![]),
                (json!(1), vec![]),
            ] {
                let document = json!({"outbounds":[{"type":"socks","udp_over_tcp":value}]});
                let entries = undeclared_entries(&profile, &document).unwrap();
                let expected = [vec!["outbounds", "0", "udp_over_tcp"], suffix].concat();
                assert_eq!(entries.len(), 1, "{} {value}", release.tag);
                assert_eq!(entries[0].path, expected, "{}", release.tag);
                assert_eq!(
                    assess_entry_evidence(entries, &document, None)
                        .unwrap()
                        .len(),
                    1
                );
            }
            let document =
                json!({"outbounds":[{"type":"socks","udp_over_tcp":{"extension":true}}]});
            let content = json!({"properties":{"outbounds":{"type":"array","items":{"properties":{
                "udp_over_tcp":{"type":"object","properties":{"extension":{"type":"boolean"}}}
            }}}}})
            .to_string();
            let schema = ConfigurationSchemaDocument {
                content_hash: hash_bytes(content.as_bytes()),
                content,
                dialect: JsonSchemaDialect::Draft202012,
                source: ConfigurationSchemaSource::ProgramBinary,
            };
            assert!(
                assess_entry_evidence(
                    undeclared_entries(&profile, &document).unwrap(),
                    &document,
                    Some(&schema)
                )
                .unwrap()
                .is_empty()
            );
            let decoder = descriptor
                .value_decoders
                .iter()
                .find(|entry| entry.releases.contains(&release.tag))
                .unwrap();
            if decoder.decoder.build_constraint.contains("go1.") {
                for environment in ["", "Environment: go1.19.0 linux/amd64\n"] {
                    let unknown = profile_from_output(
                        ProgramKind::SingBox,
                        &format!(
                            "sing-box version {}\n{environment}Tags: \n",
                            release.version
                        ),
                    );
                    let document = json!({"outbounds":[{"type":"socks","udp_over_tcp":true}]});
                    let entries = undeclared_entries(&unknown, &document).unwrap();
                    assert_eq!(entries.len(), 1, "{} {environment}", release.tag);
                    assert_eq!(entries[0].path, ["outbounds", "0", "udp_over_tcp"]);
                }
            }
        }
    }

    #[test]
    fn registered_sing_box_objects_check_public_protocol_and_nested_fields() {
        let descriptor = embedded_core_knowledge()
            .unwrap()
            .program(ProgramKind::SingBox)
            .unwrap();
        for release in &descriptor.releases {
            let profile = profile_from_output(
                ProgramKind::SingBox,
                &format!("sing-box version {}\nTags: \n", release.version),
            );
            let valid = json!({"inbounds":[{"type":"socks","tag":"input","listen_port":1080}],
                "outbounds":[{"type":"vless","tag":"output","SERVER":"127.0.0.1","server_port":1081,"multiplex":{"ENABLED":false}}]});
            assert!(
                undeclared_entries(&profile, &valid).unwrap().is_empty(),
                "{}",
                release.tag
            );
            for (candidate, expected) in [
                (
                    json!({"outbounds":[{"type":"socks","TAG":"fixture"}]}),
                    vec!["outbounds", "0", "TAG"],
                ),
                (
                    json!({"outbounds":[{"type":"socks","privateExtension":true}]}),
                    vec!["outbounds", "0", "privateExtension"],
                ),
                (
                    json!({"outbounds":[{"type":"vless","multiplex":{"privateExtension":true}}]}),
                    vec!["outbounds", "0", "multiplex", "privateExtension"],
                ),
                (
                    json!({"inbounds":[{"type":"socks","UDPFragmentDefault":true}]}),
                    vec!["inbounds", "0", "UDPFragmentDefault"],
                ),
                (
                    json!({"outbounds":[{"type":"unregistered-fixture"}]}),
                    vec!["outbounds", "0"],
                ),
                (
                    json!({"outbounds":[{"tag":"missing-type"}]}),
                    vec!["outbounds", "0"],
                ),
            ] {
                let entries = undeclared_entries(&profile, &candidate).unwrap();
                assert_eq!(entries.len(), 1, "{} {expected:?}", release.tag);
                assert_eq!(entries[0].path, expected, "{}", release.tag);
            }
            let binding = descriptor
                .decoder_bindings_for(&release.tag, "outbounds", "socks")
                .next()
                .unwrap();
            if !binding
                .object_layout
                .as_ref()
                .unwrap()
                .build_constraint
                .is_empty()
            {
                for suffix in ["", "\nTags: without_contextjson\n"] {
                    let unknown = profile_from_output(
                        ProgramKind::SingBox,
                        &format!("sing-box version {}{suffix}", release.version),
                    );
                    let entries = undeclared_entries(
                        &unknown,
                        &json!({"outbounds":[{"type":"socks","server":"127.0.0.1"}]}),
                    )
                    .unwrap();
                    assert_eq!(entries.len(), 1, "{}", release.tag);
                    assert_eq!(entries[0].path, ["outbounds", "0"]);
                }
            }
        }
    }

    #[test]
    fn flat_envelope_keys_are_exact_without_changing_protocol_or_nested_matching() {
        let descriptor = embedded_core_knowledge()
            .unwrap()
            .program(ProgramKind::SingBox)
            .unwrap();
        for release in &descriptor.releases {
            let profile = profile(ProgramKind::SingBox, &release.version);
            let mut declarations: BTreeMap<&str, Vec<&KnowledgeDeclaration>> = BTreeMap::new();
            for variant in &descriptor.declarations {
                if variant.releases.contains(&release.tag) {
                    declarations
                        .entry(&variant.declaration.id)
                        .or_default()
                        .push(&variant.declaration);
                }
            }
            let mut walker = FieldWalker {
                profile: &profile,
                descriptor,
                module_path: &release.module_path,
                declarations,
                visits: 0,
                entries: Vec::new(),
            };
            let layout = crate::KnowledgeDecodedObjectLayout {
                build_constraint: String::new(),
                envelope_declarations: vec!["option#_Outbound".into()],
                envelope_key_comparison: crate::KnowledgeFieldComparison::Exact,
                shared_options: vec![],
                key_comparison: crate::KnowledgeFieldComparison::AsciiCaseInsensitive,
                root_field_names: true,
                nested_embedded: true,
            };
            let options = walker.declaration("option#VLESSOutboundOptions").unwrap();
            for (candidate, expected) in [
                (
                    json!({"type":"vless", "tag":"fixture", "SERVER":"127.0.0.1", "server_port":1080,
                    "bind_interface":"fixture", "multiplex":{"ENABLED":true}}),
                    Vec::<&str>::new(),
                ),
                (json!({"type":"vless", "TAG":"fixture"}), vec!["TAG"]),
                (json!({"TYPE":"vless"}), vec!["TYPE"]),
                (
                    json!({"type":"vless", "UDPBindPort":1080}),
                    vec!["UDPBindPort"],
                ),
                (
                    json!({"type":"vless", "multiplex":{"privateExtension":true}}),
                    vec!["multiplex", "privateExtension"],
                ),
            ] {
                walker.entries.clear();
                walker
                    .walk_flat_object(
                        options,
                        &layout,
                        FieldEncoding::native(ProgramKind::SingBox),
                        &candidate,
                        &mut vec![],
                        "type",
                    )
                    .unwrap();
                if expected.is_empty() {
                    assert!(walker.entries.is_empty(), "{}", release.tag);
                } else {
                    assert_eq!(walker.entries.len(), 1, "{} {expected:?}", release.tag);
                    assert_eq!(walker.entries[0].path, expected, "{}", release.tag);
                }
            }
            // A protocol member with the same name is decoded only when the envelope did not remove the key.
            let mut colliding_options = options.clone();
            let mut field = walker
                .declaration("option#_Outbound")
                .unwrap()
                .fields
                .iter()
                .find(|field| field.name == "Tag")
                .unwrap()
                .clone();
            field.shape = crate::KnowledgeTypeShape::Named {
                package: "option".into(),
                name: "ServerOptions".into(),
            };
            colliding_options.fields.push(field);
            for (key, count) in [("tag", 0), ("TAG", 1)] {
                walker.entries.clear();
                walker
                    .walk_flat_object(
                        &colliding_options,
                        &layout,
                        FieldEncoding::native(ProgramKind::SingBox),
                        &json!({key:{"privateExtension":true}}),
                        &mut vec![],
                        "type",
                    )
                    .unwrap();
                assert_eq!(walker.entries.len(), count, "{} {key}", release.tag);
                if count == 1 {
                    assert_eq!(walker.entries[0].path, [key, "privateExtension"]);
                }
            }
        }
    }

    fn extension_issue(
        document: &Value,
        schema: Option<Value>,
    ) -> Result<Vec<ConfigurationAssessmentIssue>> {
        let descriptor = embedded_core_knowledge()?
            .program(ProgramKind::SingBox)
            .unwrap();
        let profile = profile(
            ProgramKind::SingBox,
            &descriptor.releases.last().unwrap().version,
        );
        let schema = schema.map(|value| {
            let content = serde_json::to_string(&value).unwrap();
            ConfigurationSchemaDocument {
                source: ConfigurationSchemaSource::ProgramBinary,
                dialect: JsonSchemaDialect::Draft202012,
                content_hash: hash_bytes(content.as_bytes()),
                content,
            }
        });
        assess_entry_evidence(
            undeclared_entries(&profile, document)?,
            document,
            schema.as_ref(),
        )
    }

    #[test]
    fn all_maintained_patches_resolve_their_native_root_declarations() {
        for program in &embedded_core_knowledge().unwrap().programs {
            for release in &program.releases {
                let profile = profile(program.program, &release.version);
                let candidate = match program.program {
                    ProgramKind::SingBox => json!({"log": {}, "outbounds": [], "route": {}}),
                    ProgramKind::Mihomo => {
                        json!({"proxies": [], "proxy-providers": {}, "rules": []})
                    }
                    ProgramKind::Xray => json!({"log": {}, "outbounds": [], "routing": {}}),
                    ProgramKind::Generic => unreachable!(),
                };
                assert!(
                    undeclared_entries(&profile, &candidate).unwrap().is_empty(),
                    "{}",
                    release.tag
                );
                let entries = undeclared_entries(
                    &profile,
                    &json!({"privateExtension": "private-user-value"}),
                )
                .unwrap();
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].path, ["privateExtension"]);
                assert!(
                    !serde_json::to_string(&entries[0].evidence)
                        .unwrap()
                        .contains("private-user-value")
                );
            }
        }
    }

    #[test]
    fn nested_fields_are_checked_against_each_patch_without_rejecting_dynamic_keys() {
        for program in &embedded_core_knowledge().unwrap().programs {
            for release in &program.releases {
                let profile = profile(program.program, &release.version);
                let (valid, unknown, path) = match program.program {
                    ProgramKind::SingBox => (
                        json!({"log": {"level": "info"}, "route": {"rules": []}}),
                        json!({"log": {"privateExtension": true}}),
                        vec!["log", "privateExtension"],
                    ),
                    ProgramKind::Mihomo => (
                        json!({"dns": {"enable": true, "fallback-filter": {"geoip": true},
                            "nameserver-policy": {"private.example": "1.1.1.1"}}}),
                        json!({"dns": {"fallback-filter": {"privateExtension": true}}}),
                        vec!["dns", "fallback-filter", "privateExtension"],
                    ),
                    ProgramKind::Xray => (
                        json!({"inbounds": [{"protocol": "socks", "port": 1080,
                            "listen": "127.0.0.1", "sniffing": {"enabled": true}}]}),
                        json!({"inbounds": [{"protocol": "socks", "sniffing": {"privateExtension": true}}]}),
                        vec!["inbounds", "0", "sniffing", "privateExtension"],
                    ),
                    ProgramKind::Generic => unreachable!(),
                };
                assert!(
                    undeclared_entries(&profile, &valid).unwrap().is_empty(),
                    "{}",
                    release.tag
                );
                let entries = undeclared_entries(&profile, &unknown).unwrap();
                assert_eq!(entries.len(), 1, "{}", release.tag);
                assert_eq!(entries[0].path, path, "{}", release.tag);
            }
        }
    }

    #[test]
    fn nested_binary_evidence_must_match_the_full_object_and_array_path() {
        let candidate = json!({"http_clients": [{"tag": "client", "privateExtension": true}]});
        // Schema traversal uses the document path, including literal object keys and array indexes.
        let schema = json!({"$defs": {"Client": {"type": "object", "properties": {
            "privateExtension": {"type": "boolean"}
        }}}, "properties": {"http_clients": {"type": "array", "items": {"$ref": "#/$defs/Client"}}}});
        let path = vec!["http_clients".into(), "0".into(), "privateExtension".into()];
        let (nodes, value) = schema_nodes_at_path(&schema, &candidate, &path, &mut 0).unwrap();
        assert!(declares_value(nodes, value, &schema, &mut 0, 0).unwrap());
        let unrelated = json!({"properties": {"privateExtension": {"type": "boolean"}}});
        let (nodes, _) = schema_nodes_at_path(&unrelated, &candidate, &path, &mut 0).unwrap();
        assert!(nodes.is_empty());
        let conditional = json!({"properties": {"http_clients": {"items": {
            "anyOf": [{"properties": {"privateExtension": {"type": "boolean"}}}, {}]
        }}}});
        let (nodes, _) = schema_nodes_at_path(&conditional, &candidate, &path, &mut 0).unwrap();
        assert!(nodes.is_empty());
        let candidate = json!({"log": {"privateExtension": true}});
        assert!(
            extension_issue(
                &candidate,
                Some(json!({"properties": {"log": {"properties": {
                    "privateExtension": {"type": "boolean"}
                }}}}))
            )
            .unwrap()
            .is_empty()
        );
        assert_eq!(
            extension_issue(&candidate, Some(unrelated)).unwrap().len(),
            1
        );
    }

    #[test]
    fn protocol_options_use_the_selected_decoder_and_keep_array_locations_private() {
        let descriptor = embedded_core_knowledge()
            .unwrap()
            .program(ProgramKind::Xray)
            .unwrap();
        for release in &descriptor.releases {
            let profile = profile(ProgramKind::Xray, &release.version);
            let valid = json!({"inbounds": [{"protocol": "socks", "settings": {
                "auth": "password", "accounts": [{"user": "private-user", "pass": "private-value"}]
            }}]});
            assert!(undeclared_entries(&profile, &valid).unwrap().is_empty());
            let candidate = json!({"inbounds": [{"protocol": "socks", "settings": {
                "accounts": [{"user": "private-user", "privateExtension": "private-value"}]
            }}]});
            let entries = undeclared_entries(&profile, &candidate).unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(
                entries[0].path,
                [
                    "inbounds",
                    "0",
                    "settings",
                    "accounts",
                    "0",
                    "privateExtension"
                ]
            );
            assert!(matches!(&entries[0].evidence,
                ConfigurationAssessmentEvidence::SourceDeclaration { source, .. }
                    if source.symbol == "SocksAccount"));
            assert!(
                !serde_json::to_string(&entries[0].evidence)
                    .unwrap()
                    .contains("private-value")
            );
        }
    }

    #[test]
    fn json_case_variants_do_not_bypass_protocol_field_checks() {
        let descriptor = embedded_core_knowledge()
            .unwrap()
            .program(ProgramKind::Xray)
            .unwrap();
        let profile = profile(
            ProgramKind::Xray,
            &descriptor.releases.last().unwrap().version,
        );
        let candidate = json!({"INBOUNDS": [{"PROTOCOL": "SoCkS", "SETTINGS": {
            "ACCOUNTS": [{"privateExtension": true}]
        }}]});
        let entries = undeclared_entries(&profile, &candidate).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].path,
            [
                "INBOUNDS",
                "0",
                "SETTINGS",
                "ACCOUNTS",
                "0",
                "privateExtension"
            ]
        );
    }

    #[test]
    fn mihomo_proxy_layout_keeps_shared_and_embedded_fields_in_their_own_decoders() {
        let descriptor = embedded_core_knowledge()
            .unwrap()
            .program(ProgramKind::Mihomo)
            .unwrap();
        for release in &descriptor.releases {
            let profile = profile(ProgramKind::Mihomo, &release.version);
            let candidate = json!({"proxies": [{"type":"socks5", "name":"fixture", "server":"127.0.0.1", "port":1080,
                "skip_cert_verify":true, "INTERFACE_NAME":"fixture-interface",
                "smux":{"enabled":false, "max_connections":2}
            }]});
            assert!(
                undeclared_entries(&profile, &candidate).unwrap().is_empty(),
                "{}",
                release.tag
            );
            for (candidate, path) in [
                (
                    json!({"proxies":[{"type":"socks5", "privateExtension":"private-fixture-value"}]}),
                    vec!["proxies", "0", "privateExtension"],
                ),
                (
                    json!({"proxies":[{"type":"socks5", "smux":{"privateExtension":true}}]}),
                    vec!["proxies", "0", "smux", "privateExtension"],
                ),
                (
                    json!({"proxies":[{"type":"socks5", "SMUX":{"enabled":false}}]}),
                    vec!["proxies", "0", "SMUX"],
                ),
                (
                    json!({"proxies":[{"type":"socks5", "smux":"unparsed-value"}]}),
                    vec!["proxies", "0", "smux"],
                ),
                (
                    json!({"proxies":[{"type":"socks5", "ProviderName":"private-provider"}]}),
                    vec!["proxies", "0", "ProviderName"],
                ),
            ] {
                let entries = undeclared_entries(&profile, &candidate).unwrap();
                assert_eq!(entries.len(), 1, "{}", release.tag);
                assert_eq!(entries[0].path, path, "{}", release.tag);
            }
        }
    }

    #[test]
    fn unregistered_decoders_require_explicit_evidence_for_the_whole_object() {
        for (kind, document, path) in [
            (
                ProgramKind::Mihomo,
                json!({"proxies":[{"type":"private-protocol", "extension":true}]}),
                vec!["proxies", "0"],
            ),
            (
                ProgramKind::Xray,
                json!({"outbounds":[{"protocol":"private-protocol", "settings":{"extension":true}}]}),
                vec!["outbounds", "0"],
            ),
            (
                ProgramKind::Mihomo,
                json!({"proxies":[{"Type":"socks5", "extension":true}]}),
                vec!["proxies", "0"],
            ),
            (
                ProgramKind::Mihomo,
                json!({"proxies":[{"type":null, "extension":true}]}),
                vec!["proxies", "0"],
            ),
            (
                ProgramKind::Xray,
                json!({"outbounds":[{"protocol":42, "settings":{"extension":true}}]}),
                vec!["outbounds", "0"],
            ),
            (
                ProgramKind::Xray,
                json!({"outbounds":[{"settings":{"extension":true}}]}),
                vec!["outbounds", "0"],
            ),
        ] {
            let descriptor = embedded_core_knowledge().unwrap().program(kind).unwrap();
            let profile = profile(kind, &descriptor.releases.last().unwrap().version);
            let entries = undeclared_entries(&profile, &document).unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].path, path);
            assert!(
                !serde_json::to_string(&entries[0].evidence)
                    .unwrap()
                    .contains("private-protocol")
            );
            assert_eq!(
                assess_entry_evidence(entries, &document, None)
                    .unwrap()
                    .len(),
                1
            );
        }
    }

    #[test]
    fn permissive_schema_and_success_without_schema_do_not_prove_a_field() {
        let candidate = json!({"privateExtension": "private-user-value"});
        let variants = [
            None,
            Some(json!({"type": "object", "additionalProperties": true})),
            Some(json!({"type": "object", "additionalProperties": {"type": "string"}})),
            Some(json!({"properties": {"privateExtension": {}}})),
            Some(json!({"properties": {"privateExtension": true}})),
            Some(json!({"anyOf": [{"properties": {"privateExtension": {"type": "string"}}}, {}]})),
        ];
        for schema in variants {
            let issues = extension_issue(&candidate, schema).unwrap();
            assert_eq!(issues.len(), 1);
            assert_eq!(
                issues[0].message_key,
                "CORE_CONFIGURATION_FIELD_UNCONFIRMED"
            );
            assert!(
                !serde_json::to_string(&issues)
                    .unwrap()
                    .contains("private-user-value")
            );
        }
    }

    #[test]
    fn explicit_declarations_cover_the_actual_extension_structure_not_arbitrary_descendants() {
        let schema = json!({
            "$defs": {"Row": {"type": "object", "properties": {"enabled": {"type": "boolean"}}}},
            "allOf": [{"properties": {
                "privateExtension": {"type": "array", "items": {"$ref": "#/$defs/Row"}}
            }}]
        });
        assert!(
            extension_issue(
                &json!({"privateExtension": [{"enabled": true}]}),
                Some(schema.clone())
            )
            .unwrap()
            .is_empty()
        );
        for candidate in [
            json!({"privateExtension": [{"enabled": "not-a-boolean"}]}),
            json!({"privateExtension": [{"unconfirmed": true}]}),
            json!({"privateExtension": {}}),
        ] {
            assert_eq!(
                extension_issue(&candidate, Some(schema.clone()))
                    .unwrap()
                    .len(),
                1
            );
        }
        let schema = json!({"properties": {"privateExtension": {
            "type": "object", "additionalProperties": true
        }}});
        assert_eq!(
            extension_issue(
                &json!({"privateExtension": {"enabled": true}}),
                Some(schema)
            )
            .unwrap()
            .len(),
            1
        );
    }

    #[test]
    fn field_evidence_is_bounded_and_never_resolves_external_or_ambiguous_references() {
        for schema in [
            json!({"properties": {"privateExtension": {"$ref": "https://example.invalid/schema"}}}),
            json!({"properties": {"privateExtension": {"$ref": "#/$defs/missing"}}}),
            json!({"properties": {"privateExtension": {"$ref": 1}}}),
            json!({"$defs": {"Loop": {"$ref": "#/$defs/Loop"}}, "properties": {"privateExtension": {"$ref": "#/$defs/Loop"}}}),
        ] {
            assert!(extension_issue(&json!({"privateExtension": true}), Some(schema)).is_err());
        }
        let schema = json!({"properties": {"privateExtension": {
            "type": "boolean", "if": {}, "then": {}, "else": {}
        }}});
        assert_eq!(
            extension_issue(&json!({"privateExtension": true}), Some(schema))
                .unwrap()
                .len(),
            1
        );
        let mut candidate = serde_json::Map::new();
        for index in 0..129 {
            candidate.insert(format!("extension{index}"), Value::Null);
        }
        assert_eq!(
            extension_issue(&Value::Object(candidate), None)
                .unwrap_err()
                .message_key
                .as_deref(),
            Some("CONFIGURATION_ASSESSMENT_LIMIT"),
        );
    }

    #[test]
    fn source_codec_case_rules_do_not_mix_json_and_yaml_fields() {
        for (kind, candidate, expected) in [
            (ProgramKind::Xray, json!({"LOG":{}}), 0),
            (ProgramKind::SingBox, json!({"LOG":{}}), 0),
            (ProgramKind::Mihomo, json!({"LOG-LEVEL":"info"}), 1),
            (ProgramKind::Mihomo, json!({"rule":[]}), 1),
        ] {
            let descriptor = embedded_core_knowledge().unwrap().program(kind).unwrap();
            let profile = profile(kind, &descriptor.releases.last().unwrap().version);
            assert_eq!(
                undeclared_entries(&profile, &candidate).unwrap().len(),
                expected
            );
        }
    }

    #[test]
    fn only_codec_entry_methods_make_a_structure_opaque() {
        let descriptor = embedded_core_knowledge()
            .unwrap()
            .program(ProgramKind::SingBox)
            .unwrap();
        let mut declaration = descriptor.declarations[0].declaration.clone();
        for (methods, json, yaml) in [
            (vec!["UnmarshalJSONHelper"], false, false),
            (vec!["UnmarshalJSON"], true, false),
            (vec!["UnmarshalJSONContext"], true, false),
            (vec!["UnmarshalYAMLHelper"], false, false),
            (vec!["UnmarshalYAML"], false, true),
        ] {
            declaration.methods = methods.into_iter().map(String::from).collect();
            assert_eq!(
                FieldEncoding::native(ProgramKind::SingBox).custom_object_decoder(&declaration),
                json
            );
            assert_eq!(
                FieldEncoding::native(ProgramKind::Mihomo).custom_object_decoder(&declaration),
                yaml
            );
        }
    }
}
