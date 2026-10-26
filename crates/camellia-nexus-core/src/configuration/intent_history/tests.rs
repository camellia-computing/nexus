use super::*;

fn fixture(
    kind: ProgramKind,
    document: Value,
) -> (crate::CoreCapabilityProfile, ConfigurationState) {
    let profile = super::super::intent_catalog::tests::latest(kind);
    let state = super::super::intent_catalog::tests::state(&profile, document);
    (profile, state)
}

fn save(
    before: &ConfigurationState,
    after: &mut ConfigurationState,
    id: &str,
) -> IntentHistoryPayload {
    let payload = IntentHistoryPayload::between(before, after)
        .unwrap()
        .unwrap();
    let bytes = serde_json::to_vec(&payload).unwrap();
    after
        .intent_history
        .append(IntentHistoryRecord {
            record_id: id.into(),
            payload_ref: id.into(),
            payload_hash: hash_bytes(&bytes),
            payload_bytes: bytes.len() as u64,
            label_key: "Intent change".into(),
            subject_key: "Logging".into(),
            created_unix_ms: 4,
            undone: false,
        })
        .unwrap();
    payload
}

fn set(
    state: &mut ConfigurationState,
    profile: &crate::CoreCapabilityProfile,
    key: &str,
    value: Value,
) {
    state
        .apply_intent_edit(
            profile,
            &IntentCompiledEdit::Set {
                setting_id: key.into(),
                value,
            },
            5,
        )
        .unwrap();
}

fn value(state: &ConfigurationState, path: &str) -> Value {
    parse_semantic_document(state.format, state.desired.content.as_bytes())
        .unwrap()
        .pointer(path)
        .unwrap()
        .clone()
}

#[test]
fn linked_removal_undo_restores_the_object_and_references_without_replaying_other_edits() {
    let (profile, mut state) = fixture(
        ProgramKind::SingBox,
        json!({
            "log": {"level": "info"},
            "dns": {"servers": [{"type": "udp", "tag": "first", "server": "1.1.1.1"}, {"type": "udp", "tag": "second", "server": "9.9.9.9"}], "final": "first"},
            "outbounds": [{"type": "direct", "tag": "direct", "domain_resolver": {"server": "first", "strategy": "prefer_ipv4"}}]
        }),
    );
    state.applied = Some(state.desired.clone());
    state.last_known_good = state.applied.clone();
    let initial = state.clone();
    let mut view = state.view();
    project_intent_objects(&profile, &state, &mut view).unwrap();
    let object = view
        .intent_objects
        .iter()
        .find(|object| object.label == "first")
        .unwrap();
    let change = super::super::intent_catalog::tests::registered_edit(
        &profile,
        &state,
        IntentCompiledEdit::RemoveObject {
            object_id: object.object_id.clone(),
            expected_hash: object.content_hash.clone(),
            replacements: BTreeMap::new(),
        },
    );
    let mut request = IntentPreviewRequest {
        expected_state_revision: state.state_revision,
        editor_session_id: None,
        expected_draft_revision: None,
        change,
    };
    let plan = state.preview_intent(&profile, &request).unwrap();
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
    state = state
        .prepare_intent_change(&profile, &request, 2)
        .unwrap()
        .state;
    let payload = save(&initial, &mut state, "linked-removal");
    set(&mut state, &profile, "logging.level", json!("debug"));
    let reopened: ConfigurationState =
        serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    let request = IntentPreviewRequest {
        expected_state_revision: reopened.state_revision,
        editor_session_id: None,
        expected_draft_revision: None,
        change: ConfigurationIntentAction::Undo {
            record_id: "linked-removal".into(),
            choices: BTreeMap::new(),
        },
    };
    let restored = reopened
        .prepare_intent_change_with_history(&profile, &request, Some(&payload), 6)
        .unwrap();
    assert!(restored.plan.can_commit, "{:?}", restored.plan.blockers);
    assert_eq!(value(&restored.state, "/dns/final"), "first");
    assert_eq!(
        value(&restored.state, "/outbounds/0/domain_resolver/server"),
        "first"
    );
    assert_eq!(
        value(&restored.state, "/dns/servers")
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(value(&restored.state, "/log/level"), "debug");
    assert_eq!(restored.state.applied, initial.applied);
    assert_eq!(restored.state.last_known_good, initial.last_known_good);
    let request = IntentPreviewRequest {
        expected_state_revision: restored.state.state_revision,
        editor_session_id: None,
        expected_draft_revision: None,
        change: ConfigurationIntentAction::Redo {
            record_id: "linked-removal".into(),
            choices: BTreeMap::new(),
        },
    };
    let redone = restored
        .state
        .prepare_intent_change_with_history(&profile, &request, Some(&payload), 7)
        .unwrap();
    assert!(redone.plan.can_commit, "{:?}", redone.plan.blockers);
    assert_eq!(value(&redone.state, "/dns/final"), "second");
    assert_eq!(
        value(&redone.state, "/dns/servers")
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(value(&redone.state, "/log/level"), "debug");
}

#[test]
fn mihomo_dns_removal_and_durable_undo_preserve_the_other_servers_and_dns_options() {
    let knowledge = crate::embedded_core_knowledge().unwrap();
    for release in &knowledge.program(ProgramKind::Mihomo).unwrap().releases {
        let profile =
            super::super::intent_catalog::tests::profile(ProgramKind::Mihomo, &release.version);
        let mut state = super::super::intent_catalog::tests::state(
            &profile,
            json!({
                "log-level":"info",
                "dns": {"enable":true, "nameserver":["1.1.1.1", "tls://9.9.9.9"], "extension":{"kept":true}}
            }),
        );
        state.applied = Some(state.desired.clone());
        state.last_known_good = state.applied.clone();
        let original = state.clone();
        let mut view = state.view();
        project_intent_objects(&profile, &state, &mut view).unwrap();
        let object = view
            .intent_objects
            .iter()
            .find(|object| object.values.get("server") == Some(&json!("1.1.1.1")))
            .unwrap();
        let request = IntentPreviewRequest {
            expected_state_revision: state.state_revision,
            editor_session_id: None,
            expected_draft_revision: None,
            change: super::super::intent_catalog::tests::registered_edit(
                &profile,
                &state,
                IntentCompiledEdit::RemoveObject {
                    object_id: object.object_id.clone(),
                    expected_hash: object.content_hash.clone(),
                    replacements: BTreeMap::new(),
                },
            ),
        };
        let prepared = state.prepare_intent_change(&profile, &request, 2).unwrap();
        assert!(
            prepared.plan.can_commit,
            "{}: {:?}",
            release.tag, prepared.plan.blockers
        );
        assert!(prepared.plan.reference_choices.is_empty());
        state = prepared.state;
        assert_eq!(value(&state, "/dns/nameserver"), json!(["tls://9.9.9.9"]));
        let payload = save(&original, &mut state, "remove-dns");
        set(&mut state, &profile, "logging.level", json!("debug"));
        let reopened: ConfigurationState =
            serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        let restored = reopened
            .prepare_intent_change_with_history(
                &profile,
                &IntentPreviewRequest {
                    expected_state_revision: reopened.state_revision,
                    editor_session_id: None,
                    expected_draft_revision: None,
                    change: ConfigurationIntentAction::Undo {
                        record_id: "remove-dns".into(),
                        choices: BTreeMap::new(),
                    },
                },
                Some(&payload),
                6,
            )
            .unwrap();
        assert!(
            restored.plan.can_commit,
            "{}: {:?}",
            release.tag, restored.plan.blockers
        );
        assert_eq!(value(&restored.state, "/dns"), value(&original, "/dns"));
        assert_eq!(value(&restored.state, "/log-level"), "debug");
        assert_eq!(restored.state.applied, original.applied);
        assert_eq!(restored.state.last_known_good, original.last_known_good);
    }
}

#[test]
fn restart_undo_and_redo_restore_contributions_not_the_whole_workspace() {
    let (profile, mut state) = fixture(
        ProgramKind::SingBox,
        json!({"log":{"level":"info","timestamp":true}}),
    );
    state.applied = Some(state.desired.clone());
    state.last_known_good = state.applied.clone();
    let before = state.clone();
    set(&mut state, &profile, "logging.level", json!("debug"));
    let payload = save(&before, &mut state, "first");
    set(&mut state, &profile, "logging.timestamp", json!(false));
    let restarted: ConfigurationState =
        serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    let (undone, conflicts) = payload
        .prepare(
            &restarted,
            "first",
            IntentHistoryDirection::Undo,
            &BTreeMap::new(),
            7,
        )
        .unwrap();
    assert!(conflicts.is_empty());
    assert_eq!(value(&undone, "/log/level"), json!("info"));
    assert_eq!(value(&undone, "/log/timestamp"), json!(false));
    assert!(!undone.guided_intent.values.contains_key("logging.level"));
    assert_eq!(undone.applied, before.applied);
    assert_eq!(undone.last_known_good, before.last_known_good);
    let (redone, conflicts) = payload
        .prepare(
            &undone,
            "first",
            IntentHistoryDirection::Redo,
            &BTreeMap::new(),
            8,
        )
        .unwrap();
    assert!(conflicts.is_empty());
    assert_eq!(value(&redone, "/log/level"), json!("debug"));
    assert_eq!(value(&redone, "/log/timestamp"), json!(false));
    assert!(
        payload
            .prepare(
                &redone,
                "first",
                IntentHistoryDirection::Redo,
                &BTreeMap::new(),
                9
            )
            .is_err()
    );
}

#[test]
fn history_choices_bind_the_compared_values_but_survive_unrelated_changes() {
    let (profile, mut state) = fixture(
        ProgramKind::SingBox,
        json!({"log":{"level":"info","timestamp":true}}),
    );
    state.applied = Some(state.desired.clone());
    state.last_known_good = state.applied.clone();
    set(&mut state, &profile, "logging.level", json!("warn"));
    let before = state.clone();
    set(&mut state, &profile, "logging.level", json!("debug"));
    let payload = save(&before, &mut state, "first");
    for direction in [IntentHistoryDirection::Undo, IntentHistoryDirection::Redo] {
        let mut current = state.clone();
        if direction == IntentHistoryDirection::Redo {
            current = payload
                .prepare(
                    &current,
                    "first",
                    IntentHistoryDirection::Undo,
                    &BTreeMap::new(),
                    6,
                )
                .unwrap()
                .0;
        }
        current.base.content = r#"{"log":{"level":"error","timestamp":true}}"#.into();
        current
            .rebuild_desired_with_source_update(7, SourceUpdateKind::UserEdit)
            .unwrap();
        let conflicts = payload
            .prepare(&current, "first", direction, &BTreeMap::new(), 8)
            .unwrap()
            .1;
        assert_eq!(conflicts.len(), 1);
        let choices = BTreeMap::from([(
            conflicts[0].conflict_id.clone(),
            IntentHistoryChoice::Restore,
        )]);
        set(&mut current, &profile, "logging.timestamp", json!(false));
        let (restored, fresh) = payload
            .prepare(&current, "first", direction, &choices, 9)
            .unwrap();
        assert_eq!(fresh[0].conflict_id, conflicts[0].conflict_id);
        assert_eq!(value(&restored, "/log/timestamp"), false);
        current.base.content = r#"{"log":{"level":"fatal","timestamp":true}}"#.into();
        current
            .rebuild_desired_with_source_update(10, SourceUpdateKind::Refresh)
            .unwrap();
        let fresh = payload
            .prepare(&current, "first", direction, &BTreeMap::new(), 11)
            .unwrap()
            .1;
        assert_ne!(fresh[0].conflict_id, conflicts[0].conflict_id);
        assert_eq!(
            payload
                .prepare(&current, "first", direction, &choices, 12)
                .unwrap_err()
                .message_key
                .as_deref(),
            Some("INTENT_PLAN_STALE")
        );
        assert_eq!(value(&current, "/log/level"), "fatal");
        assert_eq!(current.applied, state.applied);
        assert_eq!(current.last_known_good, state.last_known_good);
    }
}

#[test]
fn later_overlapping_edits_need_a_local_choice_and_keep_current_is_read_equivalent() {
    let (profile, mut state) = fixture(ProgramKind::SingBox, json!({"log":{"level":"info"}}));
    set(&mut state, &profile, "logging.level", json!("warn"));
    let before = state.clone();
    set(&mut state, &profile, "logging.level", json!("debug"));
    let payload = save(&before, &mut state, "first");
    set(&mut state, &profile, "logging.level", json!("error"));
    let (unchanged, conflicts) = payload
        .prepare(
            &state,
            "first",
            IntentHistoryDirection::Undo,
            &BTreeMap::new(),
            6,
        )
        .unwrap();
    assert_eq!(unchanged, state);
    assert_eq!(conflicts.len(), 1);
    let choice = BTreeMap::from([(
        conflicts[0].conflict_id.clone(),
        IntentHistoryChoice::KeepCurrent,
    )]);
    let (kept, _) = payload
        .prepare(&state, "first", IntentHistoryDirection::Undo, &choice, 7)
        .unwrap();
    assert_eq!(value(&kept, "/log/level"), json!("error"));
    assert_eq!(kept.generation, state.generation);
    let choice = BTreeMap::from([(
        conflicts[0].conflict_id.clone(),
        IntentHistoryChoice::Restore,
    )]);
    let (restored, _) = payload
        .prepare(&state, "first", IntentHistoryDirection::Undo, &choice, 7)
        .unwrap();
    assert_eq!(value(&restored, "/log/level"), json!("warn"));
}

#[test]
fn shared_listener_changes_undo_together_and_history_projection_contains_no_values() {
    let (profile, mut state) = fixture(
        ProgramKind::Mihomo,
        json!({"mixed-port":7890,"port":7891,"allow-lan":false,"bind-address":"127.0.0.1","log-level":"info"}),
    );
    let mut view = state.view();
    project_intent_objects(&profile, &state, &mut view).unwrap();
    let object = view
        .intent_objects
        .iter()
        .find(|item| item.values.get("protocol") == Some(&json!("mixed")))
        .unwrap();
    let before = state.clone();
    state
        .apply_intent_edit(
            &profile,
            &IntentCompiledEdit::UpdateObject {
                object_id: object.object_id.clone(),
                expected_hash: object.content_hash.clone(),
                values: BTreeMap::from([
                    ("access".into(), json!("lan")),
                    ("username".into(), json!("fixture-user")),
                    ("password".into(), json!("fixture-secret")),
                ]),
            },
            4,
        )
        .unwrap();
    let payload = save(&before, &mut state, "listener");
    assert!(
        serde_json::to_string(&payload)
            .unwrap()
            .contains("fixture-secret")
    );
    let projection = serde_json::to_string(&state.intent_history).unwrap();
    assert!(!projection.contains("fixture-secret"));
    set(&mut state, &profile, "logging.level", json!("debug"));
    let (undone, conflicts) = payload
        .prepare(
            &state,
            "listener",
            IntentHistoryDirection::Undo,
            &BTreeMap::new(),
            7,
        )
        .unwrap();
    assert!(conflicts.is_empty());
    assert_eq!(value(&undone, "/allow-lan"), json!(false));
    assert_eq!(value(&undone, "/bind-address"), json!("127.0.0.1"));
    assert_eq!(value(&undone, "/mixed-port"), json!(7890));
    assert_eq!(value(&undone, "/port"), json!(7891));
    assert_eq!(value(&undone, "/log-level"), json!("debug"));
    assert!(
        parse_semantic_document(undone.format, undone.desired.content.as_bytes())
            .unwrap()
            .get("authentication")
            .is_none()
    );
}

#[test]
fn overlap_preview_shows_the_actual_source_value_revealed_by_undo() {
    let (profile, mut state) = fixture(ProgramKind::SingBox, json!({"log":{"level":"info"}}));
    let before = state.clone();
    set(&mut state, &profile, "logging.level", json!("debug"));
    let payload = save(&before, &mut state, "first");
    state.base.content = r#"{"log":{"level":"error","timestamp":true}}"#.into();
    state.rebuild_desired(6).unwrap();
    let snapshot = state.clone();
    let (unresolved, conflicts) = payload
        .prepare(
            &state,
            "first",
            IntentHistoryDirection::Undo,
            &BTreeMap::new(),
            7,
        )
        .unwrap();
    assert_eq!(unresolved, snapshot);
    assert_eq!(conflicts.len(), 1);
    assert_eq!(
        conflicts[0].restoring,
        vec![Some(SemanticValue::Present(json!("error")))]
    );
    let choices = BTreeMap::from([(
        conflicts[0].conflict_id.clone(),
        IntentHistoryChoice::Restore,
    )]);
    let (restored, _) = payload
        .prepare(&state, "first", IntentHistoryDirection::Undo, &choices, 8)
        .unwrap();
    assert_eq!(value(&restored, "/log/level"), json!("error"));
    assert_eq!(value(&restored, "/log/timestamp"), json!(true));
    assert!(!restored.guided_intent.values.contains_key("logging.level"));
    assert_eq!(restored.applied, snapshot.applied);
    assert_eq!(restored.last_known_good, snapshot.last_known_good);
}

#[test]
fn history_is_bounded_and_new_changes_end_the_redo_branch() {
    let mut history = IntentHistory::default();
    for index in 0..70 {
        history
            .append(IntentHistoryRecord {
                record_id: index.to_string(),
                payload_ref: index.to_string(),
                payload_hash: "a".repeat(64),
                payload_bytes: 1024 * 1024,
                label_key: "Intent change".into(),
                subject_key: "Logging".into(),
                created_unix_ms: index,
                undone: false,
            })
            .unwrap();
    }
    assert_eq!(history.records.len(), 64);
    assert_eq!(history.records[0].record_id, "6");
    history.complete("69", IntentHistoryDirection::Undo);
    assert!(
        history
            .summaries()
            .iter()
            .find(|record| record.record_id == "69")
            .unwrap()
            .can_redo
    );
    history
        .append(IntentHistoryRecord {
            record_id: "new".into(),
            payload_ref: "new".into(),
            payload_hash: "a".repeat(64),
            payload_bytes: 1024,
            label_key: "Intent change".into(),
            subject_key: "Logging".into(),
            created_unix_ms: 90,
            undone: false,
        })
        .unwrap();
    assert!(history.redo_order.is_empty());
    assert!(history.record("69").is_err());
}

#[test]
fn repeated_equivalent_setting_produces_no_history_payload() {
    let (profile, mut state) = fixture(ProgramKind::SingBox, json!({"log":{"level":"info"}}));
    set(&mut state, &profile, "logging.level", json!("debug"));
    let before = state.clone();
    set(&mut state, &profile, "logging.level", json!("debug"));
    assert!(
        IntentHistoryPayload::between(&before, &state)
            .unwrap()
            .is_none()
    );
}

#[test]
fn undoing_object_creation_cannot_leave_later_references_unresolved() {
    let (profile, mut state) = fixture(ProgramKind::SingBox, json!({"dns": {"servers": []}}));
    let initial = state.clone();
    let create = super::super::intent_catalog::tests::registered_edit(
        &profile,
        &state,
        IntentCompiledEdit::CreateObject {
            object_kind: IntentObjectKind::DnsServer,
            values: BTreeMap::from([("protocol".into(), json!("system"))]),
        },
    );
    state.update_intent(&profile, &create, 2).unwrap();
    let payload = save(&initial, &mut state, "create-dns");
    let tag = state.upstream_document().unwrap()["dns"]["servers"][0]["tag"].clone();
    set(&mut state, &profile, "dns.final", tag);
    let before = state.clone();
    let request = IntentPreviewRequest {
        expected_state_revision: state.state_revision,
        editor_session_id: None,
        expected_draft_revision: None,
        change: ConfigurationIntentAction::Undo {
            record_id: "create-dns".into(),
            choices: BTreeMap::new(),
        },
    };
    let blocked = state
        .prepare_intent_change_with_history(&profile, &request, Some(&payload), 6)
        .unwrap();
    assert!(!blocked.plan.can_commit);
    assert_eq!(
        blocked.plan.blockers[0].message_key,
        "CONFIGURATION_REFERENCE_MISSING"
    );
    assert_eq!(blocked.state, before);
    assert_eq!(blocked.state.applied, initial.applied);
    assert_eq!(blocked.state.last_known_good, initial.last_known_good);
}

#[test]
fn follow_and_undo_preserve_targets_used_by_references_resolved_after_parsing() {
    let (profile, mut state) = fixture(ProgramKind::SingBox, json!({"dns":{"servers":[]}}));
    let initial = state.clone();
    let create = super::super::intent_catalog::tests::registered_edit(
        &profile,
        &state,
        IntentCompiledEdit::CreateObject {
            object_kind: IntentObjectKind::DnsServer,
            values: BTreeMap::from([("protocol".into(), json!("system"))]),
        },
    );
    state.update_intent(&profile, &create, 2).unwrap();
    let payload = save(&initial, &mut state, "create-dns");
    let tag = state.upstream_document().unwrap()["dns"]["servers"][0]["tag"].clone();
    state.base.content = serialize_semantic_document(state.format, &json!({
        "dns":{"servers":[]},
        "outbounds":[{"type":"direct", "tag":"direct", "domain_resolver":{"server":tag,"strategy":"prefer_ipv4"}}]
    })).unwrap();
    state
        .rebuild_desired_with_source_update(3, SourceUpdateKind::UserEdit)
        .unwrap();
    let original = state.clone();
    let mut view = state.view();
    project_intent_objects(&profile, &state, &mut view).unwrap();
    let object = view
        .intent_objects
        .iter()
        .find(|object| object.kind == IntentObjectKind::DnsServer)
        .unwrap();
    let follow = super::super::intent_catalog::tests::registered_edit(
        &profile,
        &state,
        IntentCompiledEdit::FollowObject {
            object_id: object.object_id.clone(),
            expected_hash: object.content_hash.clone(),
        },
    );
    for change in [
        follow,
        ConfigurationIntentAction::Undo {
            record_id: "create-dns".into(),
            choices: BTreeMap::new(),
        },
    ] {
        let history = matches!(change, ConfigurationIntentAction::Undo { .. }).then_some(&payload);
        let rejected = state
            .prepare_intent_change_with_history(
                &profile,
                &IntentPreviewRequest {
                    expected_state_revision: state.state_revision,
                    editor_session_id: None,
                    expected_draft_revision: None,
                    change,
                },
                history,
                4,
            )
            .unwrap();
        assert!(!rejected.plan.can_commit);
        assert_eq!(
            rejected.plan.blockers[0].message_key,
            "INTENT_TARGET_IN_USE"
        );
        assert_eq!(rejected.state, original);
        assert_eq!(rejected.state.applied, initial.applied);
        assert_eq!(rejected.state.last_known_good, initial.last_known_good);
    }
}

#[test]
fn undo_rechecks_the_current_platform_before_restoring_an_enabled_feature() {
    let windows = super::super::intent_catalog::tests::latest(ProgramKind::SingBox);
    let linux = crate::CoreCapabilityProfile::resolve(
        ProgramKind::SingBox,
        &crate::CoreProbeReport::from_program_output(ProgramKind::SingBox, &format!(
            "sing-box version {}\nEnvironment: go1.26.0 linux/amd64\nTags: with_gvisor,with_quic",
            windows.baseline_tag.trim_start_matches('v'),
        )),
        &crate::CoreBinaryFingerprint { sha256: windows.binary_sha256.clone(), size: 1, modified_unix_ms: 1 },
    ).unwrap();
    let mut state = super::super::intent_catalog::tests::state(
        &linux,
        json!({
            "inbounds":[{"type":"tun","tag":"adapter","auto_route":true,"auto_redirect":true}],
        }),
    );
    let catalog = configuration_intent_catalog(&linux).unwrap();
    let node = catalog
        .nodes
        .iter()
        .find(|node| {
            node.native_name == "auto_redirect"
                && node.pattern.first().is_some_and(|key| key == "inbounds")
        })
        .unwrap();
    let context = configuration_intent_context(&linux, &state, Some(&node.node_id)).unwrap();
    let before = state.clone();
    state
        .update_intent(
            &linux,
            &ConfigurationIntentAction::EditNode {
                node_id: node.node_id.clone(),
                reference_id: context.values[0].reference_id.clone(),
                value: SemanticValue::Present(json!(false)),
            },
            2,
        )
        .unwrap();
    let payload = save(&before, &mut state, "redirect");
    assert!(
        windows
            .assess(&state.upstream_document().unwrap())
            .unwrap()
            .issues
            .is_empty()
    );
    let request = IntentPreviewRequest {
        expected_state_revision: state.state_revision,
        editor_session_id: None,
        expected_draft_revision: None,
        change: ConfigurationIntentAction::Undo {
            record_id: "redirect".into(),
            choices: BTreeMap::new(),
        },
    };
    let blocked = state
        .prepare_intent_change_with_history(&windows, &request, Some(&payload), 6)
        .unwrap();
    assert!(!blocked.plan.can_commit);
    assert_eq!(
        blocked.plan.blockers[0].message_key,
        "CONFIGURATION_PLATFORM_UNSUPPORTED"
    );
    assert_eq!(blocked.state, state);
}
