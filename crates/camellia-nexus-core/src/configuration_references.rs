//! Typed reference namespaces resolved from reviewed configuration relations.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;

use crate::{
    CoreCapabilityProfile, KnowledgeReferenceForm, KnowledgeReferenceTarget, KnowledgeSemanticRule,
    KnowledgeValueConstraint, Result,
    configuration::SemanticPathSegment,
    configuration_assessment::{object_fields, select_paths},
    core_build_constraints::{BuildCondition, evaluate_build_condition},
};

#[derive(Debug, Clone)]
pub(crate) struct ReferenceTarget {
    pub name: String,
    pub path: Vec<String>,
}

pub(crate) struct ReferenceReplacement {
    pub choice_id: String,
    pub path: Vec<String>,
    pub current_target: String,
    pub targets: Vec<String>,
    value: Value,
    form: KnowledgeReferenceForm,
    case_insensitive: bool,
}

impl ReferenceReplacement {
    pub(crate) fn with_target(&self, target: &str) -> Result<Value> {
        if !self.targets.iter().any(|available| available == target) {
            return Err(reference_edit_error("INTENT_REFERENCE_CHOICE_STALE"));
        }
        let mut value = self.value.clone();
        let selected = if let KnowledgeReferenceForm::ScalarOrObject { field } = &self.form
            && value.is_object()
        {
            let key = object_fields(&value, field, self.case_insensitive)
                .next()
                .map(|(key, _)| key.to_owned())
                .ok_or_else(|| reference_edit_error("CONFIGURATION_REFERENCE_INVALID"))?;
            value
                .get_mut(&key)
                .expect("Reviewed reference field exists")
        } else {
            &mut value
        };
        match selected {
            Value::String(name) if name == &self.current_target => *name = target.into(),
            Value::Array(names) if self.form == KnowledgeReferenceForm::Listable => {
                for name in names {
                    if name.as_str() == Some(&self.current_target) {
                        *name = Value::String(target.into());
                    }
                }
            }
            _ => return Err(reference_edit_error("CONFIGURATION_REFERENCE_INVALID")),
        }
        Ok(value)
    }
}

fn reference_edit_error(key: &str) -> crate::CamelliaNexusError {
    crate::CamelliaNexusError::new(
        crate::ErrorCode::ConfigConflict,
        "Review the related settings",
    )
    .with_message_key(key)
}

pub(crate) struct ConfigurationReferenceIndex {
    targets: BTreeMap<String, Arc<ReferenceNamespace>>,
    uses: BTreeMap<String, Vec<ReferenceUse>>,
}

struct ReferenceNamespace {
    targets: Vec<ReferenceTarget>,
    paths_by_name: BTreeMap<String, Vec<Vec<String>>>,
}

struct ReferenceUse {
    path: Vec<String>,
    names: Vec<String>,
    target_paths: Vec<Vec<String>>,
    code: Option<&'static str>,
}

impl ConfigurationReferenceIndex {
    pub(crate) fn new(profile: &CoreCapabilityProfile, document: &Value) -> Result<Self> {
        let descriptor = profile.reviewed_descriptor()?;
        let reported_tags = descriptor.reported_build_tags_for(&profile.baseline_tag);
        let case_insensitive = profile.program == crate::ProgramKind::Xray;
        let mut index = Self {
            targets: BTreeMap::new(),
            uses: BTreeMap::new(),
        };
        let mut cached_targets = BTreeMap::<String, Arc<ReferenceNamespace>>::new();
        let mut visits = 0;
        for variant in &descriptor.semantic_rules {
            let rule = &variant.rule;
            let KnowledgeValueConstraint::Reference { targets, .. } = &rule.constraint else {
                continue;
            };
            let build = evaluate_build_condition(
                profile.program,
                profile.build.as_ref(),
                &rule.build_constraint,
                &reported_tags,
            );
            if !variant.releases.contains(&profile.baseline_tag)
                || build == BuildCondition::Excluded
            {
                continue;
            }
            let key = serde_json::to_string(targets)?;
            let available = if let Some(available) = cached_targets.get(&key) {
                Arc::clone(available)
            } else {
                let targets = collect_targets(document, targets, case_insensitive, &mut visits)?;
                let mut paths_by_name = BTreeMap::<String, Vec<Vec<String>>>::new();
                for target in &targets {
                    paths_by_name
                        .entry(target.name.clone())
                        .or_default()
                        .push(target.path.clone());
                }
                let available = Arc::new(ReferenceNamespace {
                    targets,
                    paths_by_name,
                });
                cached_targets.insert(key, Arc::clone(&available));
                available
            };
            index
                .targets
                .insert(rule.id.clone(), Arc::clone(&available));
            let selected = select_references(document, rule, case_insensitive, &mut visits)?;
            for (path, value) in selected {
                if !rule_applies(rule, document, &path, case_insensitive) {
                    continue;
                }
                let code = if build == BuildCondition::Unconfirmed {
                    Some("CORE_BUILD_CAPABILITY_UNCONFIRMED")
                } else {
                    index.validation_code(rule, value, case_insensitive)
                };
                let mut names = Vec::new();
                let mut target_paths = Vec::new();
                for name in reference_names(&rule.constraint, value, case_insensitive)
                    .into_iter()
                    .flatten()
                {
                    reference_visit(&mut visits)?;
                    names.push(name.to_owned());
                    for target in available.paths_by_name.get(name).into_iter().flatten() {
                        reference_visit(&mut visits)?;
                        target_paths.push(target.clone());
                    }
                }
                index
                    .uses
                    .entry(rule.id.clone())
                    .or_default()
                    .push(ReferenceUse {
                        path,
                        names,
                        target_paths,
                        code,
                    });
            }
        }
        Ok(index)
    }

    pub(crate) fn targets(&self, rule_id: &str) -> &[ReferenceTarget] {
        self.targets
            .get(rule_id)
            .map(|namespace| namespace.targets.as_slice())
            .unwrap_or_default()
    }

    pub(crate) fn issues<'a>(
        &'a self,
        rule_id: &'a str,
    ) -> impl Iterator<Item = (&'a Vec<String>, &'static str)> {
        self.uses
            .get(rule_id)
            .into_iter()
            .flatten()
            .filter_map(|usage| usage.code.map(|code| (&usage.path, code)))
    }

    pub(crate) fn validation_code(
        &self,
        rule: &KnowledgeSemanticRule,
        value: &Value,
        case_insensitive: bool,
    ) -> Option<&'static str> {
        let KnowledgeValueConstraint::Reference {
            builtin_values,
            allow_unresolved,
            ..
        } = &rule.constraint
        else {
            return None;
        };
        let Some(names) = reference_names(&rule.constraint, value, case_insensitive) else {
            return Some("CONFIGURATION_REFERENCE_INVALID");
        };
        for name in names {
            let matches = self
                .targets
                .get(&rule.id)
                .and_then(|namespace| namespace.paths_by_name.get(name))
                .map_or(0, Vec::len)
                + builtin_values
                    .iter()
                    .filter(|value| value.as_str() == name)
                    .count();
            if matches > 1 {
                return Some("CONFIGURATION_REFERENCE_AMBIGUOUS");
            }
            if matches == 0 && !allow_unresolved {
                return Some("CONFIGURATION_REFERENCE_MISSING");
            }
        }
        None
    }

    pub(crate) fn is_referenced(
        &self,
        document: &Value,
        path: &[SemanticPathSegment],
    ) -> Result<bool> {
        let path = actual_path(document, path).ok_or_else(|| {
            crate::CamelliaNexusError::new(
                crate::ErrorCode::ConfigConflict,
                "Referenced entry changed",
            )
            .with_message_key("INTENT_OBJECT_STALE")
        })?;
        Ok(self.uses.values().flatten().any(|usage| {
            !usage.path.starts_with(&path)
                && usage.target_paths.iter().any(|target| target == &path)
        }))
    }

    pub(crate) fn breaks_known_references(&self, updated: &Self) -> bool {
        self.targets.iter().any(|(rule, previous)| {
            let Some(current) = updated.targets.get(rule) else {
                return false;
            };
            let removed = previous
                .paths_by_name
                .keys()
                .filter(|name| !current.paths_by_name.contains_key(*name))
                .collect::<std::collections::BTreeSet<_>>();
            !removed.is_empty()
                && updated
                    .uses
                    .get(rule)
                    .into_iter()
                    .flatten()
                    .any(|usage| usage.names.iter().any(|name| removed.contains(name)))
        })
    }

    pub(crate) fn removal_replacements(
        &self,
        profile: &CoreCapabilityProfile,
        document: &Value,
        semantic_path: &[SemanticPathSegment],
    ) -> Result<Vec<ReferenceReplacement>> {
        let path = actual_path(document, semantic_path)
            .ok_or_else(|| reference_edit_error("INTENT_OBJECT_STALE"))?;
        let descriptor = profile.reviewed_descriptor()?;
        let mut replacements = BTreeMap::new();
        let mut option_visits = 0;
        let mut projection_bytes = 0usize;
        for variant in &descriptor.semantic_rules {
            let rule = &variant.rule;
            let KnowledgeValueConstraint::Reference {
                form,
                builtin_values,
                ..
            } = &rule.constraint
            else {
                continue;
            };
            if !variant.releases.contains(&profile.baseline_tag) {
                continue;
            }
            let Some(namespace) = self.targets.get(&rule.id) else {
                continue;
            };
            let Some(removed) = namespace.targets.iter().find(|target| target.path == path) else {
                continue;
            };
            for usage in self
                .uses
                .get(&rule.id)
                .into_iter()
                .flatten()
                .filter(|usage| {
                    !usage.path.starts_with(&path)
                        && usage.target_paths.iter().any(|target| target == &path)
                })
            {
                if let Some(code) = usage.code {
                    return Err(reference_edit_error(code));
                }
                let value = usage
                    .path
                    .iter()
                    .try_fold(document, |value, key| match value {
                        Value::Object(fields) => fields.get(key),
                        Value::Array(items) => {
                            key.parse::<usize>().ok().and_then(|index| items.get(index))
                        }
                        _ => None,
                    })
                    .ok_or_else(|| reference_edit_error("INTENT_OBJECT_STALE"))?;
                let mut targets = namespace
                    .paths_by_name
                    .iter()
                    .filter(|(name, paths)| {
                        paths.len() == 1
                            && !paths[0].starts_with(&path)
                            && !usage.path.starts_with(&paths[0])
                            && !builtin_values.contains(name)
                    })
                    .map(|(name, _)| name)
                    .chain(
                        builtin_values
                            .iter()
                            .filter(|name| !namespace.paths_by_name.contains_key(*name)),
                    )
                    .map(|name| {
                        reference_visit(&mut option_visits)?;
                        projection_bytes = projection_bytes.saturating_add(name.len());
                        if projection_bytes > 4 * 1024 * 1024 {
                            return Err(reference_edit_error("CONFIGURATION_ASSESSMENT_LIMIT"));
                        }
                        Ok(name.clone())
                    })
                    .collect::<Result<Vec<_>>>()?;
                targets.sort();
                targets.dedup();
                let value_bytes = serde_json::to_vec(value)?;
                projection_bytes = projection_bytes.saturating_add(value_bytes.len());
                if projection_bytes > 4 * 1024 * 1024 {
                    return Err(reference_edit_error("CONFIGURATION_ASSESSMENT_LIMIT"));
                }
                let value_hash = crate::config_service::hash_bytes(&value_bytes);
                let choice_id = crate::config_service::hash_bytes(&serde_json::to_vec(&(
                    &profile.profile_hash,
                    &rule.id,
                    &usage.path,
                    value_hash,
                    &path,
                ))?);
                let replacement = ReferenceReplacement {
                    choice_id,
                    path: usage.path.clone(),
                    current_target: removed.name.clone(),
                    targets,
                    value: value.clone(),
                    form: form.clone(),
                    case_insensitive: profile.program == crate::ProgramKind::Xray,
                };
                if replacements
                    .insert(usage.path.clone(), replacement)
                    .is_some()
                {
                    return Err(reference_edit_error("CONFIGURATION_REFERENCE_AMBIGUOUS"));
                }
                if replacements.len() > 1024 {
                    return Err(reference_edit_error("CONFIGURATION_ASSESSMENT_LIMIT"));
                }
            }
        }
        Ok(replacements.into_values().collect())
    }
}

fn select_references<'a>(
    document: &'a Value,
    rule: &KnowledgeSemanticRule,
    case_insensitive: bool,
    visits: &mut usize,
) -> Result<Vec<(Vec<String>, &'a Value)>> {
    let mut selected = Vec::new();
    let KnowledgeValueConstraint::Reference {
        recursive_collection: Some(collection),
        ..
    } = &rule.constraint
    else {
        select_paths(
            document,
            &rule.path,
            &mut Vec::new(),
            &mut selected,
            visits,
            case_insensitive,
        )?;
        return Ok(selected);
    };
    let Some((field, parent_path)) = rule.path.split_last() else {
        return Ok(selected);
    };
    let mut parents = Vec::new();
    select_paths(
        document,
        parent_path,
        &mut Vec::new(),
        &mut parents,
        visits,
        case_insensitive,
    )?;
    parents.reverse();
    while let Some((path, parent)) = parents.pop() {
        reference_visit(visits)?;
        for (key, value) in object_fields(parent, field, case_insensitive) {
            let mut path = path.clone();
            path.push(key.into());
            selected.push((path, value));
        }
        for (key, items) in object_fields(parent, collection, case_insensitive) {
            if let Some(items) = items.as_array() {
                for (position, item) in items.iter().enumerate().rev() {
                    reference_visit(visits)?;
                    let mut path = path.clone();
                    path.extend([key.into(), position.to_string()]);
                    parents.push((path, item));
                }
            }
        }
    }
    Ok(selected)
}

fn actual_path(mut value: &Value, semantic: &[SemanticPathSegment]) -> Option<Vec<String>> {
    let mut path = Vec::with_capacity(semantic.len());
    for segment in semantic {
        match segment {
            SemanticPathSegment::Key { key } => {
                value = value.get(key)?;
                path.push(key.clone());
            }
            SemanticPathSegment::Identity {
                field,
                value: identity,
            } => {
                let mut matches =
                    value.as_array()?.iter().enumerate().filter(|(_, item)| {
                        item.get(field).and_then(Value::as_str) == Some(identity)
                    });
                let (position, item) = matches.next()?;
                if matches.next().is_some() {
                    return None;
                }
                value = item;
                path.push(position.to_string());
            }
        }
    }
    Some(path)
}

fn collect_targets(
    document: &Value,
    targets: &[KnowledgeReferenceTarget],
    case_insensitive: bool,
    visits: &mut usize,
) -> Result<Vec<ReferenceTarget>> {
    let mut available = Vec::new();
    for target in targets {
        let mut collections = Vec::new();
        select_paths(
            document,
            &target.path,
            &mut Vec::new(),
            &mut collections,
            visits,
            case_insensitive,
        )?;
        for (path, collection) in collections {
            if let (Some(identity), Some(items)) = (&target.identity, collection.as_array()) {
                for (position, item) in items.iter().enumerate() {
                    reference_visit(visits)?;
                    for (_, value) in object_fields(item, identity, case_insensitive) {
                        if let Some(name) = value.as_str().filter(|name| !name.is_empty()) {
                            let mut path = path.clone();
                            path.push(position.to_string());
                            available.push(ReferenceTarget {
                                name: name.into(),
                                path,
                            });
                        }
                    }
                }
            } else if target.identity.is_none()
                && let Some(items) = collection.as_object()
            {
                for name in items.keys().filter(|name| !name.is_empty()) {
                    reference_visit(visits)?;
                    let mut path = path.clone();
                    path.push(name.clone());
                    available.push(ReferenceTarget {
                        name: name.clone(),
                        path,
                    });
                }
            }
        }
    }
    Ok(available)
}

fn reference_visit(visits: &mut usize) -> Result<()> {
    *visits += 1;
    if *visits > 32_768 {
        return Err(crate::CamelliaNexusError::new(
            crate::ErrorCode::ConfigInvalid,
            "Configuration assessment limit exceeded",
        )
        .with_message_key("CONFIGURATION_ASSESSMENT_LIMIT"));
    }
    Ok(())
}

pub(crate) fn rule_applies(
    rule: &KnowledgeSemanticRule,
    document: &Value,
    path: &[String],
    case_insensitive: bool,
) -> bool {
    let parent = path[..path.len().saturating_sub(1)]
        .iter()
        .try_fold(document, |value, part| match value {
            Value::Object(fields) => fields.get(part),
            Value::Array(items) => part
                .parse::<usize>()
                .ok()
                .and_then(|position| items.get(position)),
            _ => None,
        });
    rule.when.iter().all(|condition| {
        match parent
            .and_then(|parent| object_fields(parent, &condition.sibling, case_insensitive).next())
        {
            Some((_, value)) => condition.values.contains(value),
            None => condition.allow_missing,
        }
    })
}

fn reference_names<'a>(
    constraint: &KnowledgeValueConstraint,
    value: &'a Value,
    case_insensitive: bool,
) -> Option<Vec<&'a str>> {
    let KnowledgeValueConstraint::Reference {
        form, allow_empty, ..
    } = constraint
    else {
        return None;
    };
    if value.is_null() {
        return allow_empty.then(Vec::new);
    }
    let value = if let KnowledgeReferenceForm::ScalarOrObject { field } = form
        && value.is_object()
    {
        let mut fields = object_fields(value, field, case_insensitive);
        match fields.next() {
            Some((_, value))
                if fields.next().is_none()
                    && value.as_str().is_some_and(|value| !value.is_empty()) =>
            {
                value
            }
            _ => return None,
        }
    } else {
        value
    };
    let names = match value {
        Value::String(value) => vec![value.as_str()],
        Value::Array(values) if *form == KnowledgeReferenceForm::Listable => values
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()?,
        _ => return None,
    };
    if names.iter().any(|name| name.is_empty()) && !allow_empty {
        return None;
    }
    Some(names.into_iter().filter(|name| !name.is_empty()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CoreBinaryFingerprint, CoreProbeReport, ProgramKind};
    use serde_json::json;

    fn profile(program: ProgramKind, version: &str) -> CoreCapabilityProfile {
        let output = match program {
            ProgramKind::SingBox => format!(
                "sing-box version {version}\nEnvironment: go1.26.0 windows/amd64\nTags: with_gvisor,with_quic"
            ),
            ProgramKind::Mihomo => {
                format!("Mihomo Meta v{version} windows amd64 with go1.26.0\nUse tags: with_gvisor")
            }
            ProgramKind::Xray => format!(
                "Xray {version} (Xray, Penetrates Everything.) Custom (go1.26.0 windows/amd64)"
            ),
            ProgramKind::Generic => unreachable!(),
        };
        CoreCapabilityProfile::resolve(
            program,
            &CoreProbeReport::from_program_output(program, &output),
            &CoreBinaryFingerprint {
                sha256: "a".repeat(64),
                size: 1,
                modified_unix_ms: 1,
            },
        )
        .unwrap()
    }

    fn maintained(program: ProgramKind) -> Vec<CoreCapabilityProfile> {
        crate::embedded_core_knowledge()
            .unwrap()
            .program(program)
            .unwrap()
            .releases
            .iter()
            .map(|release| profile(program, &release.version))
            .collect()
    }

    fn entry(collection: &[&str], name: &str) -> Vec<SemanticPathSegment> {
        collection
            .iter()
            .map(|key| SemanticPathSegment::Key { key: (*key).into() })
            .chain([SemanticPathSegment::Identity {
                field: "tag".into(),
                value: name.into(),
            }])
            .collect()
    }

    #[test]
    fn sing_box_reference_namespaces_do_not_confuse_same_named_objects() {
        for profile in maintained(ProgramKind::SingBox) {
            let document = json!({"inbounds": [{"tag": "shared"}], "outbounds": [{"tag": "shared"}], "dns": {"servers": [{"tag": "shared"}]}, "route": {"final": "shared"}});
            let index = ConfigurationReferenceIndex::new(&profile, &document).unwrap();
            assert!(
                index
                    .is_referenced(&document, &entry(&["outbounds"], "shared"))
                    .unwrap()
            );
            assert!(
                !index
                    .is_referenced(&document, &entry(&["inbounds"], "shared"))
                    .unwrap()
            );
            assert!(
                !index
                    .is_referenced(&document, &entry(&["dns", "servers"], "shared"))
                    .unwrap()
            );
            assert!(
                index
                    .issues("singBox.reference.route-final")
                    .next()
                    .is_none()
            );
            let mut dns = document.clone();
            dns["dns"]["final"] = json!("shared");
            let index = ConfigurationReferenceIndex::new(&profile, &dns).unwrap();
            assert!(
                index
                    .is_referenced(&dns, &entry(&["dns", "servers"], "shared"))
                    .unwrap()
            );
        }
    }

    #[test]
    fn replacement_choices_bound_the_total_expanded_target_list() {
        let profile = maintained(ProgramKind::SingBox).pop().unwrap();
        let document = json!({
            "dns": {"servers": (0..1280).map(|index| json!({"tag":format!("resolver-{index}")})).collect::<Vec<_>>()},
            "outbounds": (0..32).map(|index| json!({"tag":format!("connection-{index}"), "domain_resolver":"resolver-0"})).collect::<Vec<_>>()
        });
        let index = ConfigurationReferenceIndex::new(&profile, &document).unwrap();
        let error = match index.removal_replacements(
            &profile,
            &document,
            &entry(&["dns", "servers"], "resolver-0"),
        ) {
            Err(error) => error,
            Ok(_) => panic!("Repeated option lists must respect the shared expansion limit"),
        };
        assert_eq!(
            error.message_key.as_deref(),
            Some("CONFIGURATION_ASSESSMENT_LIMIT")
        );
    }

    #[test]
    fn nested_selectors_and_resolvers_track_native_namespaces_at_every_patch() {
        for profile in maintained(ProgramKind::SingBox) {
            let document = json!({"inbounds": [{"tag": "local"}, {"tag": "other"}], "outbounds": [{"tag": "local", "domain_resolver": {"server": "resolver"}}], "dns": {"servers": [{"tag": "resolver"}], "rules": [{"type": "logical", "rules": [{"type": "logical", "rules": [{"inbound": ["local", "not-present"]}]}]}]}});
            let index = ConfigurationReferenceIndex::new(&profile, &document).unwrap();
            assert!(
                index
                    .is_referenced(&document, &entry(&["inbounds"], "local"))
                    .unwrap()
            );
            assert!(
                !index
                    .is_referenced(&document, &entry(&["inbounds"], "other"))
                    .unwrap()
            );
            assert!(
                !index
                    .is_referenced(&document, &entry(&["outbounds"], "local"))
                    .unwrap()
            );
            assert!(
                index
                    .is_referenced(&document, &entry(&["dns", "servers"], "resolver"))
                    .unwrap()
            );
            assert!(
                index
                    .issues("singBox.reference.dns-inbound")
                    .next()
                    .is_none()
            );
            let mut changed = document.clone();
            changed["dns"]["rules"] = json!([]);
            changed["route"] =
                json!({"rules": [{"type": "logical", "rules": [{"inbound": "other"}]}]});
            let index = ConfigurationReferenceIndex::new(&profile, &changed).unwrap();
            assert!(
                !index
                    .is_referenced(&changed, &entry(&["inbounds"], "local"))
                    .unwrap()
            );
            assert!(
                index
                    .is_referenced(&changed, &entry(&["inbounds"], "other"))
                    .unwrap()
            );
        }
    }

    #[test]
    fn strict_references_report_missing_ambiguous_and_invalid_values() {
        for profile in maintained(ProgramKind::SingBox) {
            for (value, code) in [
                (json!("missing"), "CONFIGURATION_REFERENCE_MISSING"),
                (json!(["valid"]), "CONFIGURATION_REFERENCE_INVALID"),
                (json!(7), "CONFIGURATION_REFERENCE_INVALID"),
            ] {
                let document = json!({"outbounds": [{"tag": "valid"}], "route": {"final": value}});
                let index = ConfigurationReferenceIndex::new(&profile, &document).unwrap();
                assert_eq!(
                    index
                        .issues("singBox.reference.route-final")
                        .next()
                        .unwrap()
                        .1,
                    code
                );
                assert!(
                    profile
                        .assess(&document)
                        .unwrap()
                        .issues
                        .iter()
                        .any(|issue| issue.code == code)
                );
            }
            for value in [json!(null), json!(""), json!("valid")] {
                let index = ConfigurationReferenceIndex::new(
                    &profile,
                    &json!({"outbounds": [{"tag": "valid"}], "route": {"final": value}}),
                )
                .unwrap();
                assert!(
                    index
                        .issues("singBox.reference.route-final")
                        .next()
                        .is_none()
                );
            }
            let document = json!({"outbounds": [{"tag": "valid"}, {"tag": "valid"}], "route": {"final": "valid"}});
            let index = ConfigurationReferenceIndex::new(&profile, &document).unwrap();
            assert_eq!(
                index
                    .issues("singBox.reference.route-final")
                    .next()
                    .unwrap()
                    .1,
                "CONFIGURATION_REFERENCE_AMBIGUOUS"
            );
            assert_eq!(
                index
                    .is_referenced(&document, &entry(&["outbounds"], "valid"))
                    .unwrap_err()
                    .message_key
                    .as_deref(),
                Some("INTENT_OBJECT_STALE")
            );
        }
    }

    #[test]
    fn xray_proxy_target_and_routing_selectors_use_distinct_collections() {
        for profile in maintained(ProgramKind::Xray) {
            let document = json!({"Inbounds": [{"Tag": "shared"}], "Outbounds": [{"Tag": "first", "StreamSettings": {"Sockopt": {"DialerProxy": "shared"}}}, {"Tag": "shared"}], "routing": {"rules": [{"inboundTag": ["unknown"]}]}});
            let index = ConfigurationReferenceIndex::new(&profile, &document).unwrap();
            assert!(index.issues("xray.reference.dialer-proxy").next().is_none());
            assert!(
                index
                    .issues("xray.reference.inbound-match")
                    .next()
                    .is_none()
            );
            assert_eq!(
                index.uses["xray.reference.dialer-proxy"][0].target_paths,
                vec![vec![String::from("Outbounds"), String::from("1")]]
            );
            let mut document = document;
            document["Outbounds"][0]["StreamSettings"]["Sockopt"]["DialerProxy"] = json!("missing");
            let index = ConfigurationReferenceIndex::new(&profile, &document).unwrap();
            assert_eq!(
                index
                    .issues("xray.reference.dialer-proxy")
                    .next()
                    .unwrap()
                    .1,
                "CONFIGURATION_REFERENCE_MISSING"
            );
        }
    }

    #[test]
    fn mihomo_group_targets_follow_reviewed_builtin_variants() {
        for profile in maintained(ProgramKind::Mihomo) {
            let descriptor = profile.reviewed_descriptor().unwrap();
            let rule = &descriptor
                .semantic_rules
                .iter()
                .find(|variant| {
                    variant.releases.contains(&profile.baseline_tag)
                        && variant.rule.id == "mihomo.reference.group-proxies"
                })
                .unwrap()
                .rule;
            let KnowledgeValueConstraint::Reference { builtin_values, .. } = &rule.constraint
            else {
                unreachable!()
            };
            let document = json!({"proxies": [{"name": "node"}], "proxy-groups": [{"name": "choice", "proxies": ["node", "DIRECT"]}]});
            let index = ConfigurationReferenceIndex::new(&profile, &document).unwrap();
            assert!(index.issues(&rule.id).next().is_none());
            let mut changed = document.clone();
            changed["proxy-groups"][0]["proxies"] = json!(["PASS-RULE"]);
            let index = ConfigurationReferenceIndex::new(&profile, &changed).unwrap();
            assert_eq!(
                index.issues(&rule.id).next().is_none(),
                builtin_values.contains(&"PASS-RULE".into())
            );
            changed["proxy-groups"][0]["proxies"] = json!(["GLOBAL"]);
            assert_eq!(
                ConfigurationReferenceIndex::new(&profile, &changed)
                    .unwrap()
                    .issues(&rule.id)
                    .next()
                    .unwrap()
                    .1,
                "CONFIGURATION_REFERENCE_MISSING"
            );
            changed["proxies"] = json!([{"name": "DIRECT"}]);
            changed["proxy-groups"][0]["proxies"] = json!(["DIRECT"]);
            assert_eq!(
                ConfigurationReferenceIndex::new(&profile, &changed)
                    .unwrap()
                    .issues(&rule.id)
                    .next()
                    .unwrap()
                    .1,
                "CONFIGURATION_REFERENCE_AMBIGUOUS"
            );
        }
    }

    #[test]
    fn reference_form_requires_the_native_resolver_field_and_rejects_conflicting_aliases() {
        let constraint = KnowledgeValueConstraint::Reference {
            targets: vec![],
            form: KnowledgeReferenceForm::ScalarOrObject {
                field: "server".into(),
            },
            builtin_values: vec![],
            allow_empty: true,
            allow_unresolved: true,
            recursive_collection: None,
        };
        assert!(reference_names(&constraint, &json!({"strategy": "prefer_ipv4"}), false).is_none());
        assert!(reference_names(&constraint, &json!({"server": ""}), false).is_none());
        assert_eq!(
            reference_names(&constraint, &json!({"server": "dns"}), false),
            Some(vec!["dns"])
        );
        assert!(
            reference_names(
                &constraint,
                &json!({"server": "dns", "Server": "other"}),
                true
            )
            .is_none()
        );
        assert!(reference_names(&constraint, &json!(["dns"]), false).is_none());
    }

    #[test]
    fn reference_edges_and_recursive_walks_are_bounded() {
        let profile = maintained(ProgramKind::SingBox).pop().unwrap();
        let document = json!({"outbounds": (0..500).map(|_| json!({"tag": "same"})).collect::<Vec<_>>(), "route": {"rules": (0..500).map(|_| json!({"inbound": ["same"]})).collect::<Vec<_>>(), "final": "same"}, "inbounds": (0..500).map(|_| json!({"tag": "same"})).collect::<Vec<_>>()});
        assert_eq!(
            ConfigurationReferenceIndex::new(&profile, &document)
                .err()
                .unwrap()
                .message_key
                .as_deref(),
            Some("CONFIGURATION_ASSESSMENT_LIMIT")
        );
    }
}
