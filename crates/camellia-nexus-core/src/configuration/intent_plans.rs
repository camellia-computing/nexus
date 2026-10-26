use super::*;
use crate::CoreCapabilityProfile;

const MAX_INTENT_INPUT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentPreviewRequest {
    pub expected_state_revision: u64,
    pub editor_session_id: Option<String>,
    pub expected_draft_revision: Option<u64>,
    pub change: ConfigurationIntentAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IntentEffectRole {
    Explicit,
    Related,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentPlannedEffect {
    pub semantic_path: String,
    pub role: IntentEffectRole,
    pub before: Option<SemanticValue>,
    pub after: Option<SemanticValue>,
    pub protected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentPlanBlocker {
    pub message_key: String,
    pub recovery_action: ConfigurationRecoveryAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentReferenceChoice {
    pub choice_id: String,
    pub label: String,
    pub semantic_path: String,
    pub current_target: String,
    pub targets: Vec<IntentTarget>,
    pub selected_target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentChangePlan {
    pub plan_hash: String,
    pub effect_hash: String,
    pub capability_profile_hash: String,
    pub state_revision: u64,
    pub draft_revision: Option<u64>,
    pub effects: Vec<IntentPlannedEffect>,
    pub affected_object_ids: Vec<String>,
    pub notice_keys: Vec<String>,
    pub blockers: Vec<IntentPlanBlocker>,
    pub history_conflicts: Vec<IntentHistoryConflict>,
    pub reference_choices: Vec<IntentReferenceChoice>,
    pub can_commit: bool,
}

pub struct PreparedIntentChange {
    pub plan: IntentChangePlan,
    pub state: ConfigurationState,
}

impl ConfigurationState {
    pub fn intent_change_subject(
        &self,
        profile: &CoreCapabilityProfile,
        change: &ConfigurationIntentAction,
    ) -> Result<String> {
        let node_id = match change {
            ConfigurationIntentAction::EditNode { node_id, .. }
            | ConfigurationIntentAction::FollowNode { node_id, .. }
            | ConfigurationIntentAction::ObjectNode { node_id, .. } => node_id,
            ConfigurationIntentAction::Undo { record_id, .. }
            | ConfigurationIntentAction::Redo { record_id, .. } => {
                return Ok(self.intent_history.record(record_id)?.subject_key.clone());
            }
        };
        let catalog = configuration_intent_catalog(profile)?;
        let node = catalog
            .node(node_id)
            .ok_or_else(|| plan_error("INTENT_CONTEXT_UNAVAILABLE"))?;
        Ok(guided_setting_descriptors(self.kind)
            .into_iter()
            .find(|setting| node.setting_id.as_ref() == Some(&setting.id))
            .map(|setting| setting.label)
            .unwrap_or_else(|| node.native_name.clone()))
    }

    pub fn preview_intent(
        &self,
        profile: &CoreCapabilityProfile,
        request: &IntentPreviewRequest,
    ) -> Result<IntentChangePlan> {
        Ok(self.prepare_intent_change(profile, request, 0)?.plan)
    }

    /// Pure planning on a private copy. No caller can observe partially compiled native edits.
    pub fn prepare_intent_change(
        &self,
        profile: &CoreCapabilityProfile,
        request: &IntentPreviewRequest,
        now: u64,
    ) -> Result<PreparedIntentChange> {
        self.prepare_intent_change_with_history(profile, request, None, now)
    }

    pub fn prepare_intent_change_with_history(
        &self,
        profile: &CoreCapabilityProfile,
        request: &IntentPreviewRequest,
        history: Option<&IntentHistoryPayload>,
        now: u64,
    ) -> Result<PreparedIntentChange> {
        if request.expected_state_revision != self.state_revision {
            return Err(plan_error("CONFIGURATION_STATE_STALE"));
        }
        match &self.editor_session {
            Some(draft)
                if request.editor_session_id.as_deref() != Some(&draft.session_id)
                    || request.expected_draft_revision != Some(draft.draft_revision) =>
            {
                return Err(plan_error("CONFIGURATION_DRAFT_STALE"));
            }
            None if request
                .expected_draft_revision
                .is_some_and(|revision| revision != 0) =>
            {
                return Err(plan_error("CONFIGURATION_DRAFT_STALE"));
            }
            _ => {}
        }
        if serde_json::to_vec(&request.change)?.len() > MAX_INTENT_INPUT_BYTES {
            return Err(plan_error("INTENT_INPUT_TOO_LARGE"));
        }
        let catalog = configuration_intent_catalog(profile)?;
        let before = self.upstream_document()?;
        let mut next = self.clone();
        let mut blockers = Vec::new();
        let mut history_conflicts = Vec::new();
        let reference_choices = match &request.change {
            ConfigurationIntentAction::ObjectNode {
                operation:
                    IntentObjectOperation::Remove {
                        object_id,
                        expected_hash,
                        replacements,
                    },
                ..
            } => self
                .intent_removal_choices(profile, object_id, expected_hash, replacements)
                .map(|(choices, _)| choices)
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let changed = match &request.change {
            ConfigurationIntentAction::Undo { record_id, choices }
            | ConfigurationIntentAction::Redo { record_id, choices } => {
                let direction = if matches!(&request.change, ConfigurationIntentAction::Undo { .. })
                {
                    IntentHistoryDirection::Undo
                } else {
                    IntentHistoryDirection::Redo
                };
                let payload = history.ok_or_else(|| plan_error("INTENT_HISTORY_UNAVAILABLE"))?;
                let (restored, conflicts) =
                    payload.prepare(self, record_id, direction, choices, now)?;
                history_conflicts = conflicts;
                if history_conflicts
                    .iter()
                    .any(|conflict| !choices.contains_key(&conflict.conflict_id))
                {
                    Err(plan_error("INTENT_HISTORY_OVERLAP"))
                } else {
                    next = restored;
                    Ok(())
                }
            }
            _ => next.update_intent(profile, &request.change, now),
        };
        if let Err(error) = changed {
            let message_key = error
                .message_key
                .unwrap_or_else(|| "INTENT_VALUE_INVALID".into());
            let recovery_action = match message_key.as_str() {
                "INTENT_REFERENCE_CHOICE_REQUIRED" | "INTENT_REFERENCE_CHOICE_STALE" => {
                    ConfigurationRecoveryAction::ReviewCandidate
                }
                _ => ConfigurationRecoveryAction::OpenFinalConfiguration,
            };
            blockers.push(IntentPlanBlocker {
                message_key,
                recovery_action,
            });
            next = self.clone();
        }
        let after = next.upstream_document()?;
        let changes = diff_intent_operations(&before, &after);
        if history.is_some() {
            let previous_issues = intent_catalog::candidate_assessment(profile, self)?.issues;
            let descriptor = profile.reviewed_descriptor()?;
            for issue in intent_catalog::candidate_assessment(profile, &next)?.issues {
                if changes.iter().any(|operation| {
                    let path = intent_operation_path(operation);
                    let directly_changed = path.iter()
                        .zip(&issue.path)
                        .all(|(segment, actual)| match segment {
                            SemanticPathSegment::Key { key } => key == actual,
                            SemanticPathSegment::Identity { .. } => actual.parse::<usize>().is_ok(),
                        });
                    let reference_target_changed = descriptor.semantic_rules.iter().any(|variant| {
                        variant.releases.contains(&profile.baseline_tag)
                            && variant.rule.id == issue.rule_id
                            && matches!(&variant.rule.constraint, crate::KnowledgeValueConstraint::Reference { targets, .. } if targets.iter().any(|target| path.iter().zip(&target.path).all(|(segment, actual)| matches!(segment, SemanticPathSegment::Key { key } if key == actual))))
                    }) && !previous_issues.iter().any(|previous| previous.rule_id == issue.rule_id && previous.path == issue.path && previous.code == issue.code);
                    directly_changed || reference_target_changed
                }) && !blockers
                    .iter()
                    .any(|blocker| blocker.message_key == issue.message_key)
                {
                    blockers.push(IntentPlanBlocker {
                        message_key: issue.message_key,
                        recovery_action: ConfigurationRecoveryAction::OpenFinalConfiguration,
                    });
                }
            }
            if blockers.is_empty() && intent_catalog::breaks_known_references(profile, self, &next)?
            {
                blockers.push(IntentPlanBlocker {
                    message_key: "INTENT_TARGET_IN_USE".into(),
                    recovery_action: ConfigurationRecoveryAction::OpenFinalConfiguration,
                });
            }
            if !blockers.is_empty() {
                next = self.clone();
            }
        }
        let effects = changes.iter().map(|operation| {
            let path = intent_operation_path(operation);
            let before = upstream::value_at(&before, &path)?;
            let after = upstream::value_at(&after, &path)?;
            let protected = path.iter().any(|part| matches!(part, SemanticPathSegment::Key {key} if intent_catalog::protected_key(key)))
                || [&before, &after].into_iter().any(|value| matches!(value, SemanticValue::Present(value) if intent_catalog::contains_protected_value(value)));
            Ok(IntentPlannedEffect {
                semantic_path: display_semantic_path(&path),
                role: effect_role(self.kind, &catalog, &request.change, &path),
                before: (!protected).then(|| redact_semantic(before)),
                after: (!protected).then(|| redact_semantic(after)),
                protected,
            })
        }).collect::<Result<Vec<_>>>()?;
        let mut notice_keys = Vec::new();
        let mut affected_object_ids = Vec::new();
        if self.kind == ProgramKind::Mihomo && changes.iter().any(|change| {
            let path = intent_operation_path(change);
            path.first().is_some_and(|part| matches!(part, SemanticPathSegment::Key {key} if ["allow-lan", "bind-address", "authentication"].contains(&key.as_str())))
        }) {
            notice_keys.push("INTENT_SHARED_LISTENER_SETTINGS".into());
            let view = intent_objects::intent_object_context_data(profile, self, &before)?;
            affected_object_ids = view.intent_objects.iter().filter(|object| object.kind == IntentObjectKind::Listener).map(|object| object.object_id.clone()).collect();
        }
        let effect_hash = hash_bytes(&serde_json::to_vec(&(
            &request.change,
            &changes,
            changed_entries(&self.guided_intent.values, &next.guided_intent.values),
            changed_entries(
                &self.guided_intent.path_edits,
                &next.guided_intent.path_edits,
            ),
            &blockers,
            &notice_keys,
            &affected_object_ids,
            &history_conflicts,
            &reference_choices,
        ))?);
        let plan_hash = hash_bytes(&serde_json::to_vec(&(
            request,
            &effect_hash,
            &catalog.catalog_hash,
        ))?);
        Ok(PreparedIntentChange {
            state: next,
            plan: IntentChangePlan {
                plan_hash,
                effect_hash,
                capability_profile_hash: profile.profile_hash.clone(),
                state_revision: self.state_revision,
                draft_revision: self
                    .editor_session
                    .as_ref()
                    .map(|draft| draft.draft_revision),
                effects,
                affected_object_ids,
                notice_keys,
                can_commit: blockers.is_empty(),
                blockers,
                history_conflicts,
                reference_choices,
            },
        })
    }
}

fn changed_entries<'a, T: PartialEq>(
    before: &'a BTreeMap<String, T>,
    after: &'a BTreeMap<String, T>,
) -> Vec<(&'a String, Option<&'a T>, Option<&'a T>)> {
    before
        .keys()
        .chain(after.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|key| {
            let previous = before.get(key);
            let next = after.get(key);
            (previous != next).then_some((key, previous, next))
        })
        .collect()
}

fn plan_error(key: &str) -> CamelliaNexusError {
    CamelliaNexusError::new(ErrorCode::ConfigConflict, "Review the current settings")
        .with_message_key(key)
}

fn redact_semantic(value: SemanticValue) -> SemanticValue {
    match value {
        SemanticValue::Present(value) => {
            SemanticValue::Present(intent_catalog::display_value(&value))
        }
        SemanticValue::Missing => SemanticValue::Missing,
    }
}

fn effect_role(
    program: ProgramKind,
    catalog: &IntentCatalog,
    change: &ConfigurationIntentAction,
    path: &SemanticPath,
) -> IntentEffectRole {
    if let ConfigurationIntentAction::ObjectNode {
        node_id,
        operation: IntentObjectOperation::Remove { .. },
    } = change
        && let Some(node) = catalog.node(node_id)
        && !node.pattern.iter().zip(path).all(|(key, part)| match part {
            SemanticPathSegment::Key { key: actual } => key == actual,
            SemanticPathSegment::Identity { .. } => key == "*",
        })
    {
        return IntentEffectRole::Related;
    }
    if let ConfigurationIntentAction::EditNode { node_id, .. } = change
        && let Some(node) = catalog.node(node_id)
        && !(node.pattern.len() == path.len()
            && node.pattern.iter().zip(path).all(|(key, part)| match part {
                SemanticPathSegment::Key { key: actual } => key == actual,
                SemanticPathSegment::Identity { .. } => key == "*",
            }))
    {
        return IntentEffectRole::Related;
    }
    // A shared native setting affects all listeners, not just the selected port.
    if program == ProgramKind::Mihomo && matches!(change, ConfigurationIntentAction::ObjectNode { operation: IntentObjectOperation::Create { object_kind: IntentObjectKind::Listener, .. } | IntentObjectOperation::Update { .. }, .. })
        && path.first().is_some_and(|part| matches!(part, SemanticPathSegment::Key {key} if ["allow-lan", "bind-address", "authentication"].contains(&key.as_str()))) {
        IntentEffectRole::Related
    } else { IntentEffectRole::Explicit }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(program: ProgramKind, value: Value) -> (CoreCapabilityProfile, ConfigurationState) {
        let profile = super::super::intent_catalog::tests::latest(program);
        let state = super::super::intent_catalog::tests::state(&profile, value);
        (profile, state)
    }

    #[test]
    fn referenced_object_removal_requires_explicit_targets_and_preserves_resolver_options() {
        let knowledge = crate::embedded_core_knowledge().unwrap();
        for release in &knowledge.program(ProgramKind::SingBox).unwrap().releases {
            let profile = super::super::intent_catalog::tests::profile(
                ProgramKind::SingBox,
                &release.version,
            );
            let mut state = super::super::intent_catalog::tests::state(
                &profile,
                json!({
                    "log": {"level": "info"},
                    "dns": {"servers": [{"type": "udp", "tag": "first", "server": "1.1.1.1"}, {"type": "udp", "tag": "second", "server": "9.9.9.9"}], "final": "first"},
                    "outbounds": [{"type": "direct", "tag": "direct", "domain_resolver": {"server": "first", "strategy": "prefer_ipv4"}}]
                }),
            );
            state.applied = Some(state.desired.clone());
            state.last_known_good = state.applied.clone();
            let original = state.clone();
            let context = intent_objects::intent_object_context_data(
                &profile,
                &state,
                &state.upstream_document().unwrap(),
            )
            .unwrap();
            let object = context
                .intent_objects
                .iter()
                .find(|object| object.label == "first")
                .unwrap();
            let mut request = request(
                &state,
                IntentCompiledEdit::RemoveObject {
                    object_id: object.object_id.clone(),
                    expected_hash: object.content_hash.clone(),
                    replacements: BTreeMap::new(),
                },
            );
            let blocked = state.prepare_intent_change(&profile, &request, 2).unwrap();
            assert!(!blocked.plan.can_commit);
            assert_eq!(
                blocked.plan.blockers[0].message_key,
                "INTENT_REFERENCE_CHOICE_REQUIRED"
            );
            assert_eq!(blocked.plan.reference_choices.len(), 2);
            assert_eq!(blocked.state, original);
            let choices = &blocked.plan.reference_choices;
            assert!(
                choices
                    .iter()
                    .all(|choice| choice.targets.len() == 1 && choice.targets[0].id == "second")
            );
            let ConfigurationIntentAction::ObjectNode {
                operation: IntentObjectOperation::Remove { replacements, .. },
                ..
            } = &mut request.change
            else {
                unreachable!()
            };
            *replacements = choices
                .iter()
                .map(|choice| (choice.choice_id.clone(), "second".into()))
                .collect();
            let changed = state.prepare_intent_change(&profile, &request, 3).unwrap();
            assert!(
                changed.plan.can_commit,
                "{}: {:?}",
                release.tag, changed.plan.blockers
            );
            assert_eq!(
                changed
                    .plan
                    .effects
                    .iter()
                    .filter(|effect| effect.role == IntentEffectRole::Related)
                    .count(),
                2
            );
            let updated = changed.state.upstream_document().unwrap();
            assert_eq!(updated["dns"]["servers"].as_array().unwrap().len(), 1);
            assert_eq!(updated["dns"]["final"], "second");
            assert_eq!(
                updated["outbounds"][0]["domain_resolver"],
                json!({"server": "second", "strategy": "prefer_ipv4"})
            );
            assert_eq!(changed.state.applied, original.applied);
            assert_eq!(changed.state.last_known_good, original.last_known_good);
            let ConfigurationIntentAction::ObjectNode {
                operation: IntentObjectOperation::Remove { replacements, .. },
                ..
            } = &mut request.change
            else {
                unreachable!()
            };
            replacements.insert("unregistered-choice".into(), "second".into());
            let rejected = state.prepare_intent_change(&profile, &request, 4).unwrap();
            assert!(!rejected.plan.can_commit);
            assert_eq!(
                rejected.plan.blockers[0].message_key,
                "INTENT_REFERENCE_CHOICE_STALE"
            );
            assert_eq!(rejected.state, original);
        }
    }

    #[test]
    fn multiple_replacements_in_ordered_rules_are_grouped_without_duplicating_or_reordering_entries()
     {
        let (profile, state) = fixture(
            ProgramKind::SingBox,
            json!({
                "dns": {
                    "servers": [{"type":"udp", "tag":"first", "server":"1.1.1.1"}, {"type":"udp", "tag":"second", "server":"9.9.9.9"}],
                "rules": [{"type":"logical", "mode":"or", "rules":[{"domain":["one.invalid"]}, {"domain":["two.invalid"]}], "action":"route", "server":"first", "extension":{"kept":true}}, {"domain":["three.invalid"], "action":"route", "server":"first"}, {"domain":["untouched.invalid"], "server":"second"}]
                }
            }),
        );
        let original = state.upstream_document().unwrap();
        let context =
            intent_objects::intent_object_context_data(&profile, &state, &original).unwrap();
        let object = context
            .intent_objects
            .iter()
            .find(|object| object.label == "first")
            .unwrap();
        let mut request = request(
            &state,
            IntentCompiledEdit::RemoveObject {
                object_id: object.object_id.clone(),
                expected_hash: object.content_hash.clone(),
                replacements: BTreeMap::new(),
            },
        );
        let plan = state.preview_intent(&profile, &request).unwrap();
        assert_eq!(plan.reference_choices.len(), 2, "{:?}", plan.blockers);
        let ConfigurationIntentAction::ObjectNode {
            operation: IntentObjectOperation::Remove { replacements, .. },
            ..
        } = &mut request.change
        else {
            unreachable!()
        };
        *replacements = plan
            .reference_choices
            .iter()
            .map(|choice| (choice.choice_id.clone(), "second".into()))
            .collect();
        let changed = state.prepare_intent_change(&profile, &request, 3).unwrap();
        assert!(changed.plan.can_commit, "{:?}", changed.plan.blockers);
        let mut expected = original.clone();
        expected["dns"]["servers"].as_array_mut().unwrap().remove(0);
        expected["dns"]["rules"][0]["server"] = json!("second");
        expected["dns"]["rules"][1]["server"] = json!("second");
        assert_eq!(changed.state.upstream_document().unwrap(), expected);
        assert_eq!(
            changed
                .state
                .guided_intent
                .path_edits
                .values()
                .filter(|edit| edit.path == key_path(&["dns", "rules"]))
                .count(),
            1
        );
    }

    #[test]
    fn xray_listener_removal_replaces_list_references_without_losing_selectors_or_rule_order() {
        let knowledge = crate::embedded_core_knowledge().unwrap();
        for release in &knowledge.program(ProgramKind::Xray).unwrap().releases {
            let profile =
                super::super::intent_catalog::tests::profile(ProgramKind::Xray, &release.version);
            let state = super::super::intent_catalog::tests::state(
                &profile,
                json!({
                    "inbounds": [
                        {"tag": "first", "protocol": "http", "listen": "127.0.0.1", "port": 9080, "settings": {}},
                        {"tag": "second", "protocol": "socks", "listen": "127.0.0.1", "port": 9081, "settings": {"udp": true}}
                    ],
                    "outbounds": [{"tag": "direct", "protocol": "freedom"}],
                    "routing": {"rules": [
                        {"type": "field", "inboundTag": ["first", "future-selector"], "outboundTag": "direct", "extension": {"kept": true}},
                        {"type": "field", "inboundTag": ["second"], "outboundTag": "direct"}
                    ]}
                }),
            );
            let original = state.upstream_document().unwrap();
            let context =
                intent_objects::intent_object_context_data(&profile, &state, &original).unwrap();
            let object = context
                .intent_objects
                .iter()
                .find(|object| object.label == "first")
                .unwrap();
            let mut request = request(
                &state,
                IntentCompiledEdit::RemoveObject {
                    object_id: object.object_id.clone(),
                    expected_hash: object.content_hash.clone(),
                    replacements: BTreeMap::new(),
                },
            );
            let plan = state.preview_intent(&profile, &request).unwrap();
            assert_eq!(
                plan.reference_choices.len(),
                1,
                "{}: {:?}",
                release.tag,
                plan.blockers
            );
            assert_eq!(
                plan.reference_choices[0]
                    .targets
                    .iter()
                    .map(|target| target.id.as_str())
                    .collect::<Vec<_>>(),
                ["second"]
            );
            let ConfigurationIntentAction::ObjectNode {
                operation: IntentObjectOperation::Remove { replacements, .. },
                ..
            } = &mut request.change
            else {
                unreachable!()
            };
            replacements.insert(plan.reference_choices[0].choice_id.clone(), "second".into());
            let changed = state.prepare_intent_change(&profile, &request, 2).unwrap();
            assert!(
                changed.plan.can_commit,
                "{}: {:?}",
                release.tag, changed.plan.blockers
            );
            let mut expected = original.clone();
            expected["inbounds"].as_array_mut().unwrap().remove(0);
            expected["routing"]["rules"][0]["inboundTag"] = json!(["second", "future-selector"]);
            assert_eq!(changed.state.upstream_document().unwrap(), expected);
            assert_eq!(changed.state.applied, state.applied);
            assert_eq!(changed.state.last_known_good, state.last_known_good);
        }
    }

    #[test]
    fn replacement_choices_bind_the_reference_value_and_reject_changed_targets_without_deleting() {
        let (profile, mut state) = fixture(
            ProgramKind::SingBox,
            json!({
                "dns": {"servers": [{"type":"local", "tag":"first"}, {"type":"local", "tag":"second"}], "final":"first"},
                "outbounds": [{"type":"direct", "tag":"direct", "domain_resolver": {"server":"first", "strategy":"prefer_ipv4"}}]
            }),
        );
        let context = intent_objects::intent_object_context_data(
            &profile,
            &state,
            &state.upstream_document().unwrap(),
        )
        .unwrap();
        let object = context
            .intent_objects
            .iter()
            .find(|object| object.label == "first")
            .unwrap();
        let edit = IntentCompiledEdit::RemoveObject {
            object_id: object.object_id.clone(),
            expected_hash: object.content_hash.clone(),
            replacements: BTreeMap::new(),
        };
        let plan = state
            .preview_intent(&profile, &request(&state, edit.clone()))
            .unwrap();
        let old_choices = plan
            .reference_choices
            .iter()
            .map(|choice| (choice.choice_id.clone(), "second".into()))
            .collect();
        let mut updated_source = state.upstream_document().unwrap();
        updated_source["outbounds"][0]["domain_resolver"]["strategy"] = json!("prefer_ipv6");
        state.base.content = serialize_semantic_document(state.format, &updated_source).unwrap();
        state
            .rebuild_desired_with_source_update(2, SourceUpdateKind::UserEdit)
            .unwrap();
        let current = state.clone();
        let mut request = request(&state, edit);
        let ConfigurationIntentAction::ObjectNode {
            operation: IntentObjectOperation::Remove { replacements, .. },
            ..
        } = &mut request.change
        else {
            unreachable!()
        };
        *replacements = old_choices;
        let rejected = state.prepare_intent_change(&profile, &request, 3).unwrap();
        assert!(!rejected.plan.can_commit);
        assert_eq!(
            rejected.plan.blockers[0].message_key,
            "INTENT_REFERENCE_CHOICE_STALE"
        );
        assert_eq!(
            rejected.plan.blockers[0].recovery_action,
            ConfigurationRecoveryAction::ReviewCandidate
        );
        assert_eq!(rejected.state, current);
        let ConfigurationIntentAction::ObjectNode {
            operation: IntentObjectOperation::Remove { replacements, .. },
            ..
        } = &mut request.change
        else {
            unreachable!()
        };
        replacements.clear();
        let refreshed = state.preview_intent(&profile, &request).unwrap();
        let ConfigurationIntentAction::ObjectNode {
            operation: IntentObjectOperation::Remove { replacements, .. },
            ..
        } = &mut request.change
        else {
            unreachable!()
        };
        *replacements = refreshed
            .reference_choices
            .iter()
            .map(|choice| (choice.choice_id.clone(), "second".into()))
            .collect();
        let changed = state.prepare_intent_change(&profile, &request, 4).unwrap();
        assert!(changed.plan.can_commit, "{:?}", changed.plan.blockers);
        assert_eq!(
            changed.state.upstream_document().unwrap()["outbounds"][0]["domain_resolver"],
            json!({"server":"second", "strategy":"prefer_ipv6"})
        );
    }

    #[test]
    fn following_a_created_dns_service_cannot_leave_another_setting_without_its_target() {
        let (profile, mut state) = fixture(ProgramKind::SingBox, json!({"dns":{"servers":[]}}));
        state = state
            .prepare_intent_change(
                &profile,
                &request(
                    &state,
                    IntentCompiledEdit::CreateObject {
                        object_kind: IntentObjectKind::DnsServer,
                        values: BTreeMap::from([
                            ("protocol".into(), json!("udp")),
                            ("server".into(), json!("1.1.1.1")),
                        ]),
                    },
                ),
                2,
            )
            .unwrap()
            .state;
        let context = intent_objects::intent_object_context_data(
            &profile,
            &state,
            &state.upstream_document().unwrap(),
        )
        .unwrap();
        let object = context
            .intent_objects
            .iter()
            .find(|object| object.kind == IntentObjectKind::DnsServer)
            .unwrap();
        state = state
            .prepare_intent_change(
                &profile,
                &request(
                    &state,
                    IntentCompiledEdit::Set {
                        setting_id: "dns.final".into(),
                        value: json!(object.label),
                    },
                ),
                3,
            )
            .unwrap()
            .state;
        let original = state.clone();
        let changed = state
            .prepare_intent_change(
                &profile,
                &request(
                    &state,
                    IntentCompiledEdit::FollowObject {
                        object_id: object.object_id.clone(),
                        expected_hash: object.content_hash.clone(),
                    },
                ),
                4,
            )
            .unwrap();
        assert!(!changed.plan.can_commit);
        assert_eq!(changed.plan.blockers[0].message_key, "INTENT_TARGET_IN_USE");
        assert_eq!(changed.state, original);
    }

    fn request(state: &ConfigurationState, change: IntentCompiledEdit) -> IntentPreviewRequest {
        IntentPreviewRequest {
            expected_state_revision: state.state_revision,
            editor_session_id: state
                .editor_session
                .as_ref()
                .map(|draft| draft.session_id.clone()),
            expected_draft_revision: state
                .editor_session
                .as_ref()
                .map(|draft| draft.draft_revision),
            change: super::super::intent_catalog::tests::registered_edit(
                &super::super::intent_catalog::tests::latest(state.kind),
                state,
                change,
            ),
        }
    }

    #[test]
    fn preview_is_deterministic_read_only_and_binds_the_current_workspace() {
        let (profile, state) = fixture(
            ProgramKind::SingBox,
            json!({"log":{"level":"info","timestamp":true}}),
        );
        let before = state.clone();
        let request = request(
            &state,
            IntentCompiledEdit::Set {
                setting_id: "logging.level".into(),
                value: json!("debug"),
            },
        );
        let first = state.preview_intent(&profile, &request).unwrap();
        assert_eq!(first, state.preview_intent(&profile, &request).unwrap());
        assert_eq!(state, before);
        assert!(first.can_commit);
        assert_eq!(first.effects.len(), 1);
        assert_eq!(first.effects[0].semantic_path, "/log/level");
        let prepared = state.prepare_intent_change(&profile, &request, 12).unwrap();
        assert_eq!(prepared.plan.plan_hash, first.plan_hash);
        assert_eq!(prepared.state.applied, state.applied);
        assert_eq!(prepared.state.last_known_good, state.last_known_good);
    }

    #[test]
    fn failed_plan_does_not_expose_partial_edits_or_secrets() {
        let (profile, state) = fixture(ProgramKind::SingBox, json!({}));
        let request = request(
            &state,
            IntentCompiledEdit::CreateObject {
                object_kind: IntentObjectKind::Listener,
                values: BTreeMap::from([
                    ("protocol".into(), json!("socks")),
                    ("port".into(), json!(70000)),
                    ("password".into(), json!("synthetic-secret")),
                ]),
            },
        );
        let prepared = state.prepare_intent_change(&profile, &request, 12).unwrap();
        assert_eq!(prepared.state, state);
        assert!(!prepared.plan.can_commit);
        assert!(
            !serde_json::to_string(&prepared.plan)
                .unwrap()
                .contains("synthetic-secret")
        );
    }

    #[test]
    fn object_creation_has_the_same_identity_in_preview_and_commit() {
        for kind in [ProgramKind::SingBox, ProgramKind::Xray, ProgramKind::Mihomo] {
            let (profile, state) = fixture(kind, json!({}));
            let preview_request = request(
                &state,
                IntentCompiledEdit::CreateObject {
                    object_kind: IntentObjectKind::Listener,
                    values: BTreeMap::from([
                        ("protocol".into(), json!("http")),
                        ("access".into(), json!("local")),
                        ("port".into(), json!(18080)),
                    ]),
                },
            );
            let first = state.preview_intent(&profile, &preview_request).unwrap();
            assert!(first.can_commit, "{kind:?}: {:?}", first.blockers);
            assert_eq!(
                first,
                state.preview_intent(&profile, &preview_request).unwrap()
            );
            let prepared = state
                .prepare_intent_change(&profile, &preview_request, 12)
                .unwrap();
            assert_eq!(first, prepared.plan);
            assert_eq!(state.applied, prepared.state.applied);
            assert_eq!(state.last_known_good, prepared.state.last_known_good);
            if kind != ProgramKind::Mihomo {
                let next = request(
                    &prepared.state,
                    IntentCompiledEdit::CreateObject {
                        object_kind: IntentObjectKind::Listener,
                        values: BTreeMap::from([
                            ("protocol".into(), json!("socks")),
                            ("access".into(), json!("local")),
                            ("port".into(), json!(18081)),
                        ]),
                    },
                );
                let added = prepared
                    .state
                    .prepare_intent_change(&profile, &next, 13)
                    .unwrap();
                assert!(added.plan.can_commit, "{:?}", added.plan.blockers);
                let document = added.state.upstream_document().unwrap();
                let entries = document["inbounds"].as_array().unwrap();
                assert_eq!(entries.len(), 2);
                assert_ne!(entries[0]["tag"], entries[1]["tag"]);
            }
        }
    }

    #[test]
    fn shared_listener_scope_is_related_and_lists_other_affected_entries() {
        let (profile, state) = fixture(
            ProgramKind::Mihomo,
            json!({"mixed-port":1080,"port":8080,"allow-lan":false}),
        );
        let mut view = state.view();
        project_intent_objects(&profile, &state, &mut view).unwrap();
        let object = &view.intent_objects[0];
        let request = request(
            &state,
            IntentCompiledEdit::UpdateObject {
                object_id: object.object_id.clone(),
                expected_hash: object.content_hash.clone(),
                values: BTreeMap::from([
                    ("access".into(), json!("lan")),
                    ("username".into(), json!("test")),
                    ("password".into(), json!("synthetic-secret")),
                ]),
            },
        );
        let plan = state.preview_intent(&profile, &request).unwrap();
        assert!(plan.can_commit, "{:?}", plan.blockers);
        assert_eq!(plan.affected_object_ids.len(), 2);
        assert_eq!(plan.notice_keys, ["INTENT_SHARED_LISTENER_SETTINGS"]);
        assert!(
            plan.effects
                .iter()
                .any(|effect| effect.role == IntentEffectRole::Related)
        );
        assert!(
            !serde_json::to_string(&plan)
                .unwrap()
                .contains("synthetic-secret")
        );
    }

    #[test]
    fn final_only_objects_are_not_partially_created_in_the_upstream() {
        for kind in [ProgramKind::SingBox, ProgramKind::Xray] {
            let (profile, mut state) = fixture(kind, json!({"inbounds":[]}));
            let object = if kind == ProgramKind::SingBox {
                json!({"type":"http","tag":"editor-only","listen":"127.0.0.1","listen_port":18080,"custom":{"preserved":true}})
            } else {
                json!({"protocol":"http","tag":"editor-only","listen":"127.0.0.1","port":18080,"settings":{"allowTransparent":false},"custom":{"preserved":true}})
            };
            let content =
                serialize_semantic_document(state.format, &json!({"inbounds":[object]})).unwrap();
            state
                .replace_final_from_edited(content.as_bytes(), 2)
                .unwrap();
            let before = state.clone();
            let mut view = state.view();
            project_intent_objects(&profile, &state, &mut view).unwrap();
            let entry = &view.intent_objects[0];
            assert!(!entry.editable);
            for change in [
                IntentCompiledEdit::UpdateObject {
                    object_id: entry.object_id.clone(),
                    expected_hash: entry.content_hash.clone(),
                    values: BTreeMap::from([("port".into(), json!(18081))]),
                },
                IntentCompiledEdit::RemoveObject {
                    object_id: entry.object_id.clone(),
                    expected_hash: entry.content_hash.clone(),
                    replacements: BTreeMap::new(),
                },
            ] {
                let prepared = state
                    .prepare_intent_change(&profile, &request(&state, change), 3)
                    .unwrap();
                assert!(
                    !prepared.plan.can_commit,
                    "{kind:?} must not synthesize an incomplete listener"
                );
                assert_eq!(
                    prepared.plan.blockers[0].message_key,
                    "INTENT_NATIVE_SHAPE_REQUIRES_EDITOR"
                );
                assert_eq!(prepared.state, before);
            }
            assert_eq!(state, before);
        }
    }

    #[test]
    fn impact_equivalence_excludes_unrelated_intent_edits_but_not_overlapping_values() {
        let (profile, mut state) = fixture(
            ProgramKind::SingBox,
            json!({"log":{"level":"info","timestamp":true}}),
        );
        let change = IntentCompiledEdit::Set {
            setting_id: "logging.level".into(),
            value: json!("debug"),
        };
        let first = state
            .preview_intent(&profile, &request(&state, change.clone()))
            .unwrap();
        state
            .apply_intent_edit(
                &profile,
                &IntentCompiledEdit::Set {
                    setting_id: "logging.timestamp".into(),
                    value: json!(false),
                },
                2,
            )
            .unwrap();
        let next = state
            .preview_intent(&profile, &request(&state, change.clone()))
            .unwrap();
        assert_eq!(first.effect_hash, next.effect_hash);
        assert_ne!(first.plan_hash, next.plan_hash);
        state
            .apply_intent_edit(
                &profile,
                &IntentCompiledEdit::Set {
                    setting_id: "logging.level".into(),
                    value: json!("error"),
                },
                3,
            )
            .unwrap();
        let overlapping = state
            .preview_intent(&profile, &request(&state, change))
            .unwrap();
        assert_ne!(next.effect_hash, overlapping.effect_hash);
    }

    #[test]
    fn ambiguous_or_noncanonical_wire_shapes_are_not_partially_rewritten() {
        for document in [
            json!({"Log":{"LogLevel":"info"}}),
            json!({"log":{"loglevel":"info"}, "Log":{"loglevel":"error"}}),
        ] {
            let (profile, state) = fixture(ProgramKind::Xray, document);
            let prepared = state
                .prepare_intent_change(
                    &profile,
                    &request(
                        &state,
                        IntentCompiledEdit::Set {
                            setting_id: "logging.level".into(),
                            value: json!("debug"),
                        },
                    ),
                    2,
                )
                .unwrap();
            assert_eq!(
                prepared.plan.blockers[0].message_key,
                "INTENT_NATIVE_SHAPE_REQUIRES_EDITOR"
            );
            assert_eq!(prepared.state, state);
        }
    }
}
