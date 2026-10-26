use super::*;
use crate::{CoreBinaryFingerprint, CoreProbeReport, CoreTargetIdentity};

pub(crate) fn profile(program: ProgramKind, version: &str) -> CoreCapabilityProfile {
    profile_on(program, version, "windows")
}

fn profile_on(program: ProgramKind, version: &str, platform: &str) -> CoreCapabilityProfile {
    let output = match program {
        ProgramKind::SingBox => format!(
            "sing-box version {version}\nEnvironment: go1.26.0 {platform}/amd64\nTags: with_gvisor,with_quic"
        ),
        ProgramKind::Mihomo => {
            format!("Mihomo Meta v{version} {platform} amd64 with go1.26.0\nUse tags: with_gvisor")
        }
        ProgramKind::Xray => {
            format!(
                "Xray {version} (Xray, Penetrates Everything.) Custom (go1.26.0 {platform}/amd64)"
            )
        }
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

pub(crate) fn latest(program: ProgramKind) -> CoreCapabilityProfile {
    latest_on(program, "windows")
}

pub(crate) fn latest_on(program: ProgramKind, platform: &str) -> CoreCapabilityProfile {
    profile_on(
        program,
        &crate::embedded_core_knowledge()
            .unwrap()
            .program(program)
            .unwrap()
            .releases
            .last()
            .unwrap()
            .version,
        platform,
    )
}

pub(crate) fn state(profile: &CoreCapabilityProfile, document: Value) -> ConfigurationState {
    let target = CoreTargetIdentity::unknown(profile.program, None).bind_fingerprint(
        &CoreBinaryFingerprint {
            sha256: profile.binary_sha256.clone(),
            size: 1,
            modified_unix_ms: 1,
        },
    );
    let format = ConfigurationFormat::for_kind(profile.program).unwrap();
    let content = serialize_semantic_document(format, &document).unwrap();
    let source =
        SourceSnapshot::parse("source", "source", format, content.as_bytes(), 1, false).unwrap();
    ConfigurationState::from_merge(
        profile.program,
        1,
        1,
        merge_configuration_sources(profile.program, &[source]).unwrap(),
        CoreCompatibilityProfile::resolve(&target).unwrap(),
    )
    .unwrap()
}

pub(crate) fn registered_edit(
    profile: &CoreCapabilityProfile,
    state: &ConfigurationState,
    edit: IntentCompiledEdit,
) -> ConfigurationIntentAction {
    let catalog = configuration_intent_catalog(profile).unwrap();
    match edit {
        IntentCompiledEdit::Set { setting_id, value } => {
            let node = catalog
                .nodes
                .iter()
                .find(|node| node.setting_id.as_ref() == Some(&setting_id))
                .unwrap();
            let context =
                configuration_intent_context(profile, state, Some(&node.node_id)).unwrap();
            let reference_id = context.values[0].reference_id.clone();
            ConfigurationIntentAction::EditNode {
                node_id: node.node_id.clone(),
                reference_id,
                value: SemanticValue::Present(value),
            }
        }
        IntentCompiledEdit::Follow { setting_id } => {
            let node = catalog
                .nodes
                .iter()
                .find(|node| node.setting_id.as_ref() == Some(&setting_id))
                .unwrap();
            let context =
                configuration_intent_context(profile, state, Some(&node.node_id)).unwrap();
            ConfigurationIntentAction::FollowNode {
                node_id: node.node_id.clone(),
                reference_id: context.values[0].reference_id.clone(),
            }
        }
        IntentCompiledEdit::CreateObject {
            object_kind,
            values,
        } => {
            let node = catalog
                .nodes
                .iter()
                .find(|node| {
                    node.object_kinds.contains(&object_kind)
                        && node.disposition == IntentNodeDisposition::Guided
                        && node
                            .object_protocol
                            .as_ref()
                            .is_none_or(|protocol| values.get("protocol") == Some(&json!(protocol)))
                })
                .unwrap();
            ConfigurationIntentAction::ObjectNode {
                node_id: node.node_id.clone(),
                operation: IntentObjectOperation::Create {
                    object_kind,
                    values,
                },
            }
        }
        IntentCompiledEdit::UpdateObject {
            object_id,
            expected_hash,
            values,
        } => registered_object_edit(
            profile,
            state,
            &catalog,
            IntentObjectOperation::Update {
                object_id,
                expected_hash,
                values,
            },
        ),
        IntentCompiledEdit::RemoveObject {
            object_id,
            expected_hash,
            replacements,
        } => registered_object_edit(
            profile,
            state,
            &catalog,
            IntentObjectOperation::Remove {
                object_id,
                expected_hash,
                replacements,
            },
        ),
        IntentCompiledEdit::FollowObject {
            object_id,
            expected_hash,
        } => registered_object_edit(
            profile,
            state,
            &catalog,
            IntentObjectOperation::Follow {
                object_id,
                expected_hash,
            },
        ),
    }
}

fn registered_object_edit(
    profile: &CoreCapabilityProfile,
    state: &ConfigurationState,
    catalog: &IntentCatalog,
    operation: IntentObjectOperation,
) -> ConfigurationIntentAction {
    let object_id = match &operation {
        IntentObjectOperation::Update { object_id, .. }
        | IntentObjectOperation::Remove { object_id, .. }
        | IntentObjectOperation::Follow { object_id, .. } => object_id,
        _ => unreachable!(),
    };
    let node = catalog
        .nodes
        .iter()
        .filter(|node| {
            !node.object_kinds.is_empty() && node.disposition == IntentNodeDisposition::Guided
        })
        .find(|node| {
            configuration_intent_context(profile, state, Some(&node.node_id))
                .unwrap()
                .object_editors
                .iter()
                .any(|editor| {
                    editor
                        .objects
                        .iter()
                        .any(|object| &object.object_id == object_id)
                })
        })
        .unwrap();
    ConfigurationIntentAction::ObjectNode {
        node_id: node.node_id.clone(),
        operation,
    }
}

#[test]
fn existing_inapplicable_fields_remain_visible_and_can_be_removed_locally() {
    let profile = latest(ProgramKind::SingBox);
    let catalog = configuration_intent_catalog(&profile).unwrap();
    let node = catalog
        .nodes
        .iter()
        .find(|node| {
            node.native_name == "auto_redirect"
                && node.pattern.first().is_some_and(|part| part == "inbounds")
                && node.disposition == IntentNodeDisposition::Inapplicable
        })
        .unwrap();
    let mut configured = state(
        &profile,
        json!({"inbounds":[{"type":"tun","tag":"adapter","auto_redirect":true,"mtu":1500}]}),
    );
    let parent =
        configuration_intent_context(&profile, &configured, node.parent_id.as_deref()).unwrap();
    assert!(
        parent
            .children
            .iter()
            .any(|child| child.node_id == node.node_id)
    );
    let context = configuration_intent_context(&profile, &configured, Some(&node.node_id)).unwrap();
    assert!(context.values[0].present);
    assert!(!context.issues.is_empty());
    configured
        .update_intent(
            &profile,
            &ConfigurationIntentAction::EditNode {
                node_id: node.node_id.clone(),
                reference_id: context.values[0].reference_id.clone(),
                value: SemanticValue::Missing,
            },
            2,
        )
        .unwrap();
    let document =
        parse_semantic_document(configured.format, configured.desired.content.as_bytes()).unwrap();
    assert!(document["inbounds"][0].get("auto_redirect").is_none());
    assert_eq!(document["inbounds"][0]["mtu"], 1500);
    let parent =
        configuration_intent_context(&profile, &configured, node.parent_id.as_deref()).unwrap();
    assert!(
        !parent
            .children
            .iter()
            .any(|child| child.node_id == node.node_id)
    );
}

#[test]
fn intent_wire_writes_require_registered_nodes_and_explicit_operations() {
    for request in [
        json!({"action":"set","settingId":"logging.level","value":"debug"}),
        json!({"action":"follow","settingId":"logging.level"}),
        json!({"action":"createObject","objectKind":"listener","values":{"port":1080}}),
        json!({"action":"editNode","nodeId":"registered","referenceId":"bound","value":{"state":"present","value":true},"path":["other"]}),
    ] {
        assert!(serde_json::from_value::<ConfigurationIntentAction>(request).is_err());
    }
}

#[test]
fn every_maintained_patch_classifies_all_input_nodes_and_keeps_native_order() {
    let knowledge = crate::embedded_core_knowledge().unwrap();
    for kind in [ProgramKind::SingBox, ProgramKind::Xray, ProgramKind::Mihomo] {
        for release in &knowledge.program(kind).unwrap().releases {
            let profile = profile(kind, &release.version);
            let catalog = configuration_intent_catalog(&profile).unwrap();
            let native = native_input_nodes(&profile).unwrap();
            let descriptor = knowledge.program(kind).unwrap();
            let mut expected_roots = Vec::new();
            for root in &descriptor.configuration_roots {
                let mut id = root.clone();
                for _ in 0..64 {
                    let declaration = &descriptor
                        .declarations
                        .iter()
                        .find(|variant| {
                            variant.releases.contains(&release.tag) && variant.declaration.id == id
                        })
                        .unwrap()
                        .declaration;
                    if let KnowledgeTypeShape::Named { package, name } = &declaration.shape
                        && declaration.fields.is_empty()
                    {
                        id = format!("{package}#{name}");
                        continue;
                    }
                    for field in &declaration.fields {
                        if field.embedded {
                            continue;
                        }
                        let encoding = if kind == ProgramKind::Mihomo {
                            "yaml"
                        } else {
                            "json"
                        };
                        let key = field
                            .tags
                            .get(encoding)
                            .map(|tag| tag.split(',').next().unwrap())
                            .unwrap_or(&field.name);
                        if key != "-" && !key.is_empty() {
                            expected_roots.push(key.to_string());
                        }
                    }
                    break;
                }
            }
            assert_eq!(
                catalog
                    .nodes
                    .iter()
                    .filter(|node| node.parent_id.is_none())
                    .map(|node| node.native_name.clone())
                    .collect::<Vec<_>>(),
                expected_roots,
                "AST root order {kind:?} {}",
                release.tag
            );
            assert_eq!(
                catalog.nodes.len(),
                native.len(),
                "{kind:?} {}",
                release.tag
            );
            assert_eq!(
                catalog.coverage.native_nodes,
                catalog.coverage.guided_nodes
                    + catalog.coverage.details_managed_nodes
                    + catalog.coverage.final_editor_nodes
                    + catalog.coverage.inapplicable_nodes
            );
            let mut seen = BTreeSet::new();
            for node in &catalog.nodes {
                assert!(seen.insert(&node.node_id), "duplicate native node");
                assert_eq!(node.source_commit.len(), 40);
                if let Some(parent) = &node.parent_id {
                    assert!(
                        catalog.node(parent).is_some(),
                        "orphan {}",
                        node.native_name
                    );
                }
                assert!(
                    node.disposition == IntentNodeDisposition::Guided || node.reason_key.is_some()
                );
            }
            let names: Vec<_> = catalog
                .chapters
                .iter()
                .map(|id| catalog.node(id).unwrap().native_name.as_str())
                .collect();
            let root_fields: Vec<_> = native
                .iter()
                .filter(|node| node.parent_id.is_none())
                .map(|node| node.name.as_str())
                .collect();
            assert_eq!(names, root_fields);
            assert_eq!(
                catalog.coverage.reviewed_relations,
                knowledge
                    .program(kind)
                    .unwrap()
                    .semantic_rules
                    .iter()
                    .filter(|variant| variant.releases.contains(&release.tag))
                    .count()
            );
            for relation in &catalog.relations {
                assert!(!relation.evidence.is_empty());
                for id in relation.node_ids.iter().chain(&relation.related_node_ids) {
                    assert!(catalog.node(id).is_some());
                }
                for evidence in &relation.evidence {
                    assert_eq!(evidence.source_commit.len(), 40);
                    assert_eq!(evidence.behavior_hash.len(), 64);
                }
            }
            if kind == ProgramKind::SingBox {
                assert!(
                    names.iter().position(|name| *name == "log").unwrap()
                        < names.iter().position(|name| *name == "dns").unwrap()
                );
                assert!(
                    names.iter().position(|name| *name == "inbounds").unwrap()
                        < names.iter().position(|name| *name == "outbounds").unwrap()
                );
            }
        }
    }
}

#[test]
fn object_adapters_are_bound_to_the_registered_native_node_and_protocol() {
    let profile = latest(ProgramKind::Mihomo);
    let state = state(&profile, json!({"log-level":"info", "socks-port":7891}));
    let catalog = configuration_intent_catalog(&profile).unwrap();
    let http = catalog
        .nodes
        .iter()
        .find(|node| node.native_name == "port" && node.parent_id.is_none())
        .unwrap();
    let socks = catalog
        .nodes
        .iter()
        .find(|node| node.native_name == "socks-port" && node.parent_id.is_none())
        .unwrap();
    let context = configuration_intent_context(&profile, &state, Some(&http.node_id)).unwrap();
    assert_eq!(context.object_editors[0].descriptor.protocols, vec!["http"]);
    assert!(context.object_editors[0].objects.is_empty());
    let socks_context =
        configuration_intent_context(&profile, &state, Some(&socks.node_id)).unwrap();
    let object = &socks_context.object_editors[0].objects[0];
    let mut next = state.clone();
    let error = next
        .update_intent(
            &profile,
            &ConfigurationIntentAction::ObjectNode {
                node_id: http.node_id.clone(),
                operation: IntentObjectOperation::Remove {
                    object_id: object.object_id.clone(),
                    expected_hash: object.content_hash.clone(),
                    replacements: BTreeMap::new(),
                },
            },
            2,
        )
        .unwrap_err();
    assert_eq!(error.message_key.as_deref(), Some("INTENT_PLAN_STALE"));
    assert_eq!(next, state);
    next.update_intent(
        &profile,
        &ConfigurationIntentAction::ObjectNode {
            node_id: http.node_id.clone(),
            operation: IntentObjectOperation::Create {
                object_kind: IntentObjectKind::Listener,
                values: BTreeMap::from([
                    ("protocol".into(), json!("http")),
                    ("access".into(), json!("local")),
                    ("port".into(), json!(7890)),
                ]),
            },
        },
        2,
    )
    .unwrap();
    let document = parse_semantic_document(next.format, next.desired.content.as_bytes()).unwrap();
    assert_eq!(document["port"], 7890);
    assert_eq!(document["socks-port"], 7891);
}

#[test]
fn decoder_build_conditions_gate_native_protocol_fields() {
    let knowledge = crate::embedded_core_knowledge().unwrap();
    let descriptor = knowledge.program(ProgramKind::SingBox).unwrap();
    let release = descriptor
        .releases
        .iter()
        .find(|release| {
            descriptor.decoder_bindings.iter().any(|variant| {
                variant.releases.contains(&release.tag)
                    && variant
                        .binding
                        .object_layout
                        .as_ref()
                        .is_some_and(|layout| {
                            layout.build_constraint.contains("!without_contextjson")
                        })
            })
        })
        .unwrap();
    let profile = CoreCapabilityProfile::resolve(
        ProgramKind::SingBox,
        &CoreProbeReport::from_program_output(ProgramKind::SingBox, &format!(
            "sing-box version {}\nEnvironment: go1.26.0 windows/amd64\nTags: without_contextjson", release.version
        )),
        &CoreBinaryFingerprint { sha256: "b".repeat(64), size: 1, modified_unix_ms: 1 },
    ).unwrap();
    let catalog = configuration_intent_catalog(&profile).unwrap();
    let protocol = catalog
        .nodes
        .iter()
        .find(|node| node.pattern == ["inbounds", "*"] && node.native_name == "socks")
        .unwrap();
    assert_eq!(protocol.disposition, IntentNodeDisposition::Inapplicable);
    let fields = catalog.children(&protocol.node_id).collect::<Vec<_>>();
    assert!(!fields.is_empty());
    assert!(
        fields
            .iter()
            .all(|node| node.disposition == IntentNodeDisposition::Inapplicable)
    );
}

#[test]
fn native_container_context_exposes_reviewed_adapters_without_claiming_all_protocols() {
    for kind in [ProgramKind::SingBox, ProgramKind::Xray] {
        let profile = latest(kind);
        let state = state(&profile, json!({"inbounds":[], "dns":{"servers":[]}}));
        let root = configuration_intent_context(&profile, &state, None).unwrap();
        let inbounds = root
            .children
            .iter()
            .find(|node| node.native_name == "inbounds")
            .unwrap();
        let context =
            configuration_intent_context(&profile, &state, Some(&inbounds.node_id)).unwrap();
        assert_eq!(
            context
                .object_editors
                .iter()
                .filter(|editor| editor.descriptor.kind == IntentObjectKind::Listener)
                .count(),
            1
        );
        let dns = root
            .children
            .iter()
            .find(|node| node.native_name == "dns")
            .unwrap();
        let context = configuration_intent_context(&profile, &state, Some(&dns.node_id)).unwrap();
        assert_eq!(
            context
                .object_editors
                .iter()
                .filter(|editor| editor.descriptor.kind == IntentObjectKind::DnsServer)
                .count(),
            1
        );
    }
}

#[test]
fn large_object_contexts_share_parse_assessment_and_identity_index() {
    let profile = latest(ProgramKind::SingBox);
    let document = json!({"inbounds":(0..500).map(|index|json!({"type":"socks","tag":format!("local-{index}"),"listen_port":1080+index,"unknown":"preserved fixture text"})).collect::<Vec<_>>()});
    let state = state(&profile, document);
    assert!(state.desired.content.len() > 40_000);
    let start = std::time::Instant::now();
    let catalog = configuration_intent_catalog(&profile).unwrap();
    let node = catalog
        .nodes
        .iter()
        .find(|node| {
            node.pattern == ["inbounds", "*", "listen_port"]
                && node
                    .type_selections
                    .iter()
                    .any(|selection| selection.value == "socks")
        })
        .unwrap();
    let index = context_index(&profile, &state).unwrap();
    assert_eq!(index.document["inbounds"].as_array().unwrap().len(), 500);
    let cold = start.elapsed();
    let start = std::time::Instant::now();
    for _ in 0..4 {
        let context = configuration_intent_context(&profile, &state, Some(&node.node_id)).unwrap();
        assert_eq!(context.values.len(), 500);
        assert_eq!(
            context.values[499].semantic_path,
            "/inbounds[tag=local-499]/listen_port"
        );
    }
    eprintln!(
        "native Intent context: {} bytes, 500 entries, cold {:?}, four cached reads {:?}",
        state.desired.content.len(),
        cold,
        start.elapsed()
    );
}

#[test]
fn bounded_caches_reuse_recent_allocations_and_evict_least_recent_entries() {
    let mut cache = BoundedArcCache::new(2);
    let first = Arc::new(1);
    let second = Arc::new(2);
    cache.insert("first", Arc::clone(&first));
    cache.insert("second", Arc::clone(&second));
    assert!(Arc::ptr_eq(&first, &cache.get(&"first").unwrap()));
    cache.insert("third", Arc::new(3));
    assert!(cache.get(&"second").is_none());
    assert!(Arc::ptr_eq(&first, &cache.get(&"first").unwrap()));
    assert!(Arc::ptr_eq(&first, &cache.insert("first", Arc::new(9))));
    assert_eq!(cache.entries.len(), 2);
}

#[test]
fn mihomo_uses_yaml_names_instead_of_json_names() {
    let catalog = configuration_intent_catalog(&latest(ProgramKind::Mihomo)).unwrap();
    assert!(catalog.nodes.iter().any(|node| node.pattern == ["rules"]));
    assert!(!catalog.nodes.iter().any(|node| node.pattern == ["rule"]));
    assert!(
        catalog
            .nodes
            .iter()
            .any(|node| node.pattern == ["tun", "auto-redirect"])
    );
}

#[test]
fn contexts_are_read_only_and_protected_values_never_enter_the_projection() {
    let profile = latest(ProgramKind::SingBox);
    let state = state(
        &profile,
        json!({"inbounds":[{"type":"socks","tag":"local","listen":"127.0.0.1","listen_port":1080,"users":[{"username":"person","password":"synthetic-test-secret"}]}]}),
    );
    let before = state.clone();
    let catalog = configuration_intent_catalog(&profile).unwrap();
    let inbounds = catalog
        .nodes
        .iter()
        .find(|node| node.pattern == ["inbounds"] && node.type_selections.is_empty())
        .unwrap();
    let context = configuration_intent_context(&profile, &state, Some(&inbounds.node_id)).unwrap();
    let encoded = serde_json::to_string(&context).unwrap();
    assert!(!encoded.contains("synthetic-test-secret"));
    assert!(!encoded.contains("person"));
    assert!(context.values[0].protected);
    assert_eq!(context.values[0].summary.as_ref().unwrap().kind, "list");
    assert_eq!(state, before);
}

#[test]
fn protocol_context_selects_only_the_matching_native_objects() {
    let profile = latest(ProgramKind::SingBox);
    let state = state(
        &profile,
        json!({"inbounds":[{"type":"socks","tag":"socks","listen_port":1080},{"type":"http","tag":"http","listen_port":8080}]}),
    );
    let catalog = configuration_intent_catalog(&profile).unwrap();
    let field = catalog
        .nodes
        .iter()
        .find(|node| {
            node.pattern == ["inbounds", "*", "listen_port"]
                && node
                    .type_selections
                    .iter()
                    .any(|selector| selector.value == "socks")
        })
        .unwrap();
    let context = configuration_intent_context(&profile, &state, Some(&field.node_id)).unwrap();
    assert_eq!(context.values.len(), 1);
    assert_eq!(context.values[0].value, Some(json!(1080)));
}

#[test]
fn cached_catalog_reuses_the_same_immutable_allocation() {
    let profile = latest(ProgramKind::Xray);
    let first = configuration_intent_catalog(&profile).unwrap();
    let next = configuration_intent_catalog(&profile).unwrap();
    assert!(Arc::ptr_eq(&first, &next));
}

#[test]
fn reference_fields_offer_only_unambiguous_targets_and_reject_missing_targets_atomically() {
    for release in &crate::embedded_core_knowledge()
        .unwrap()
        .program(ProgramKind::Xray)
        .unwrap()
        .releases
    {
        let profile = profile(ProgramKind::Xray, &release.version);
        let initial = state(
            &profile,
            json!({"outbounds": [{"protocol": "freedom", "tag": "first", "streamSettings": {"sockopt": {"dialerProxy": "local", "tcpFastOpen": true}}}, {"protocol": "freedom", "tag": "local"}, {"protocol": "freedom", "tag": "other"}], "inbounds": [{"protocol": "socks", "tag": "not-an-outbound"}]}),
        );
        let catalog = configuration_intent_catalog(&profile).unwrap();
        let node = catalog
            .nodes
            .iter()
            .find(|node| {
                node.pattern == ["outbounds", "*", "streamSettings", "sockopt", "dialerProxy"]
            })
            .unwrap();
        assert_eq!(node.disposition, IntentNodeDisposition::Guided);
        let context =
            configuration_intent_context(&profile, &initial, Some(&node.node_id)).unwrap();
        assert_eq!(context.values.len(), 1);
        assert_eq!(
            context.node.unwrap().control.unwrap().allowed_values,
            vec!["", "first", "local", "other"]
        );
        let reference = context
            .values
            .iter()
            .find(|value| value.value == Some(json!("local")))
            .unwrap()
            .reference_id
            .clone();
        let request = IntentPreviewRequest {
            expected_state_revision: initial.state_revision,
            editor_session_id: None,
            expected_draft_revision: None,
            change: ConfigurationIntentAction::EditNode {
                node_id: node.node_id.clone(),
                reference_id: reference,
                value: SemanticValue::Present(json!("not-an-outbound")),
            },
        };
        let blocked = initial
            .prepare_intent_change(&profile, &request, 2)
            .unwrap();
        assert!(!blocked.plan.can_commit);
        assert_eq!(
            blocked.plan.blockers[0].message_key,
            "CONFIGURATION_REFERENCE_MISSING"
        );
        assert_eq!(blocked.state, initial);
        let mut request = request;
        if let ConfigurationIntentAction::EditNode { value, .. } = &mut request.change {
            *value = SemanticValue::Present(json!("other"));
        }
        let prepared = initial
            .prepare_intent_change(&profile, &request, 3)
            .unwrap();
        assert!(prepared.plan.can_commit, "{:?}", prepared.plan.blockers);
        let after = prepared.state.upstream_document().unwrap();
        assert_eq!(
            after["outbounds"][0]["streamSettings"]["sockopt"]["dialerProxy"],
            "other"
        );
        assert_eq!(
            after["outbounds"][0]["streamSettings"]["sockopt"]["tcpFastOpen"],
            true
        );
        let payload = IntentHistoryPayload::between(&initial, &prepared.state)
            .unwrap()
            .unwrap();
        let mut changed = prepared.state;
        let bytes = serde_json::to_vec(&payload).unwrap();
        changed
            .intent_history
            .append(IntentHistoryRecord {
                record_id: "target".into(),
                payload_ref: "protected-payload".into(),
                payload_hash: hash_bytes(&bytes),
                payload_bytes: bytes.len() as u64,
                label_key: "Changed configuration settings".into(),
                subject_key: "tag".into(),
                created_unix_ms: 3,
                undone: false,
            })
            .unwrap();
        let (restored, conflicts) = payload
            .prepare(
                &changed,
                "target",
                IntentHistoryDirection::Undo,
                &BTreeMap::new(),
                4,
            )
            .unwrap();
        assert!(conflicts.is_empty());
        assert_eq!(
            restored.upstream_document().unwrap(),
            initial.upstream_document().unwrap()
        );
        assert_eq!(restored.applied, initial.applied);
        assert_eq!(restored.last_known_good, initial.last_known_good);
        let document = json!({"outbounds": [{"protocol": "freedom", "tag": "first", "streamSettings": {"sockopt": {"dialerProxy": "local"}}}, {"protocol": "freedom", "tag": "local"}, {"protocol": "freedom", "tag": "local"}]});
        let ambiguous = state(&profile, document);
        let context =
            configuration_intent_context(&profile, &ambiguous, Some(&node.node_id)).unwrap();
        assert!(
            !context
                .node
                .unwrap()
                .control
                .unwrap()
                .allowed_values
                .contains(&"local".into())
        );
        assert!(
            context
                .issues
                .iter()
                .any(|issue| issue.message_key == "CONFIGURATION_REFERENCE_AMBIGUOUS")
        );
    }
}

#[test]
fn dns_removal_checks_the_completed_reference_graph_before_committing() {
    let profile = latest(ProgramKind::SingBox);
    for object_reference in [false, true] {
        let mut initial = state(
            &profile,
            json!({"dns": {"servers": [
                {"type":"local", "tag":"bootstrap"},
                {"type":"udp", "tag":"first", "server":"first.example.invalid", "domain_resolver":"second"},
                {"type":"udp", "tag":"second", "server":"second.example.invalid", "domain_resolver": if object_reference { json!({"server":"bootstrap", "strategy":"prefer_ipv4"}) } else { json!("bootstrap") }},
                {"type":"local", "tag":"safe"}
            ]}}),
        );
        initial.applied = Some(initial.desired.clone());
        initial.last_known_good = initial.applied.clone();
        let view = intent_objects::intent_object_context_data(
            &profile,
            &initial,
            &initial.upstream_document().unwrap(),
        )
        .unwrap();
        let object = view
            .intent_objects
            .iter()
            .find(|object| object.label == "bootstrap")
            .unwrap();
        let mut request = IntentPreviewRequest {
            expected_state_revision: initial.state_revision,
            editor_session_id: None,
            expected_draft_revision: None,
            change: registered_edit(
                &profile,
                &initial,
                IntentCompiledEdit::RemoveObject {
                    object_id: object.object_id.clone(),
                    expected_hash: object.content_hash.clone(),
                    replacements: BTreeMap::new(),
                },
            ),
        };
        let choices = initial
            .preview_intent(&profile, &request)
            .unwrap()
            .reference_choices;
        assert_eq!(choices.len(), 1);
        for (target, can_commit) in [("first", false), ("safe", true)] {
            let ConfigurationIntentAction::ObjectNode {
                operation: IntentObjectOperation::Remove { replacements, .. },
                ..
            } = &mut request.change
            else {
                unreachable!()
            };
            *replacements = BTreeMap::from([(choices[0].choice_id.clone(), target.into())]);
            let prepared = initial
                .prepare_intent_change(&profile, &request, 2)
                .unwrap();
            assert_eq!(
                prepared.plan.can_commit, can_commit,
                "{:?}",
                prepared.plan.blockers
            );
            if !can_commit {
                assert_eq!(prepared.plan.blockers[0].message_key, "INTENT_DNS_CYCLE");
                assert_eq!(prepared.state, initial);
            } else {
                let document = prepared.state.upstream_document().unwrap();
                assert_eq!(document["dns"]["servers"].as_array().unwrap().len(), 3);
                assert_eq!(
                    document["dns"]["servers"][1]["domain_resolver"],
                    if object_reference {
                        json!({"server":"safe", "strategy":"prefer_ipv4"})
                    } else {
                        json!("safe")
                    }
                );
            }
            assert_eq!(prepared.state.applied, initial.applied);
            assert_eq!(prepared.state.last_known_good, initial.last_known_good);
        }
    }
}

#[test]
fn removing_a_same_named_dns_service_checks_only_its_actual_users() {
    let profile = latest(ProgramKind::SingBox);
    for referenced in [false, true] {
        let mut document = json!({"outbounds": [{"type": "direct", "tag": "shared"}], "route": {"final": "shared"}, "dns": {"servers": [{"type": "local", "tag": "shared"}]}});
        if referenced {
            document["dns"]["final"] = json!("shared");
        }
        let initial = state(&profile, document);
        let view = intent_objects::intent_object_context_data(
            &profile,
            &initial,
            &initial.upstream_document().unwrap(),
        )
        .unwrap();
        let object = view
            .intent_objects
            .iter()
            .find(|object| object.kind == IntentObjectKind::DnsServer)
            .unwrap();
        let change = registered_edit(
            &profile,
            &initial,
            IntentCompiledEdit::RemoveObject {
                object_id: object.object_id.clone(),
                expected_hash: object.content_hash.clone(),
                replacements: BTreeMap::new(),
            },
        );
        let prepared = initial
            .prepare_intent_change(
                &profile,
                &IntentPreviewRequest {
                    expected_state_revision: initial.state_revision,
                    editor_session_id: None,
                    expected_draft_revision: None,
                    change,
                },
                2,
            )
            .unwrap();
        assert_eq!(prepared.plan.can_commit, !referenced);
        if referenced {
            assert_eq!(
                prepared.plan.blockers[0].message_key,
                "INTENT_TARGET_IN_USE"
            );
            assert_eq!(prepared.state, initial);
        } else {
            assert_eq!(
                prepared.state.upstream_document().unwrap()["outbounds"][0]["tag"],
                "shared"
            );
        }
        assert_eq!(prepared.state.applied, initial.applied);
        assert_eq!(prepared.state.last_known_good, initial.last_known_good);
    }
}

#[test]
fn reference_choices_do_not_expose_credentials_embedded_in_an_identity() {
    let profile = latest(ProgramKind::Xray);
    let private = "https://user:fixture-password@resolver.test";
    let state = state(
        &profile,
        json!({"outbounds": [{"protocol": "freedom", "tag": "first", "streamSettings": {"sockopt": {"dialerProxy": private}}}, {"protocol": "freedom", "tag": private}]}),
    );
    let catalog = configuration_intent_catalog(&profile).unwrap();
    let node = catalog
        .nodes
        .iter()
        .find(|node| node.pattern == ["outbounds", "*", "streamSettings", "sockopt", "dialerProxy"])
        .unwrap();
    let context = configuration_intent_context(&profile, &state, Some(&node.node_id)).unwrap();
    assert!(context.values[0].protected);
    assert_eq!(
        context
            .targets
            .iter()
            .map(|target| target.id.as_str())
            .collect::<Vec<_>>(),
        ["first"]
    );
    assert!(
        !serde_json::to_string(&context)
            .unwrap()
            .contains("fixture-password")
    );
}

#[test]
fn protected_native_strings_are_reversible_without_exposing_values_or_overwriting_siblings() {
    for kind in [ProgramKind::SingBox, ProgramKind::Xray, ProgramKind::Mihomo] {
        for release in &crate::embedded_core_knowledge()
            .unwrap()
            .program(kind)
            .unwrap()
            .releases
        {
            let profile = profile(kind, &release.version);
            let (document, pattern, protocol, pointer) = match kind {
                ProgramKind::SingBox => (
                    json!({"outbounds":[{"type":"shadowsocks","tag":"private","server":"127.0.0.1","server_port":443,"method":"aes-128-gcm","password":"fixture-original","extension":{"kept":true}}]}),
                    vec!["outbounds", "*", "password"],
                    "shadowsocks",
                    "/outbounds/0/password",
                ),
                ProgramKind::Xray => (
                    json!({"outbounds":[{"protocol":"trojan","tag":"private","settings":{"address":"127.0.0.1","port":443,"password":"fixture-original","extension":{"kept":true}}}]}),
                    vec!["outbounds", "*", "settings", "password"],
                    "trojan",
                    "/outbounds/0/settings/password",
                ),
                ProgramKind::Mihomo => (
                    json!({"proxies":[{"type":"ss","name":"private","server":"127.0.0.1","port":443,"cipher":"aes-128-gcm","password":"fixture-original","extension":{"kept":true}}]}),
                    vec!["proxies", "*", "password"],
                    "ss",
                    "/proxies/0/password",
                ),
                ProgramKind::Generic => unreachable!(),
            };
            let mut initial = state(&profile, document);
            initial.applied = Some(initial.desired.clone());
            initial.last_known_good = initial.applied.clone();
            let catalog = configuration_intent_catalog(&profile).unwrap();
            let node = catalog
                .nodes
                .iter()
                .find(|node| {
                    node.pattern == pattern
                        && node
                            .type_selections
                            .iter()
                            .any(|selection| selection.value == protocol)
                })
                .unwrap();
            assert_eq!(
                node.disposition,
                IntentNodeDisposition::Guided,
                "{kind:?} {}",
                release.tag
            );
            let context =
                configuration_intent_context(&profile, &initial, Some(&node.node_id)).unwrap();
            assert_eq!(context.values.len(), 1);
            assert!(context.values[0].protected);
            assert!(context.values[0].value.is_none());
            assert!(
                !serde_json::to_string(&context)
                    .unwrap()
                    .contains("fixture-original")
            );
            let request = IntentPreviewRequest {
                expected_state_revision: initial.state_revision,
                editor_session_id: None,
                expected_draft_revision: None,
                change: ConfigurationIntentAction::EditNode {
                    node_id: node.node_id.clone(),
                    reference_id: context.values[0].reference_id.clone(),
                    value: SemanticValue::Present(json!("fixture-replacement")),
                },
            };
            for invalid in [
                json!(""),
                Value::Null,
                json!(12),
                json!({"password":"fixture-invalid"}),
            ] {
                let mut invalid_request = request.clone();
                if let ConfigurationIntentAction::EditNode { value, .. } =
                    &mut invalid_request.change
                {
                    *value = SemanticValue::Present(invalid);
                }
                let preview = initial
                    .prepare_intent_change(&profile, &invalid_request, 2)
                    .unwrap();
                assert!(!preview.plan.can_commit);
                assert_eq!(preview.state, initial);
                assert!(
                    !serde_json::to_string(&preview.plan)
                        .unwrap()
                        .contains("fixture-invalid")
                );
            }
            let prepared = initial
                .prepare_intent_change(&profile, &request, 3)
                .unwrap();
            assert!(
                prepared.plan.can_commit,
                "{kind:?} {}: {:?}",
                release.tag, prepared.plan.blockers
            );
            assert_eq!(prepared.plan.effects.len(), 1);
            assert!(prepared.plan.effects[0].protected);
            assert!(prepared.plan.effects[0].before.is_none());
            assert!(prepared.plan.effects[0].after.is_none());
            assert!(
                !serde_json::to_string(&prepared.plan)
                    .unwrap()
                    .contains("fixture-")
            );
            let mut changed = prepared.state;
            let current =
                configuration_intent_context(&profile, &changed, Some(&node.node_id)).unwrap();
            let repeated = changed
                .prepare_intent_change(
                    &profile,
                    &IntentPreviewRequest {
                        expected_state_revision: changed.state_revision,
                        editor_session_id: None,
                        expected_draft_revision: None,
                        change: ConfigurationIntentAction::EditNode {
                            node_id: node.node_id.clone(),
                            reference_id: current.values[0].reference_id.clone(),
                            value: SemanticValue::Present(json!("fixture-replacement")),
                        },
                    },
                    3,
                )
                .unwrap();
            assert_eq!(repeated.state, changed);
            let payload = IntentHistoryPayload::between(&initial, &changed)
                .unwrap()
                .unwrap();
            let bytes = serde_json::to_vec(&payload).unwrap();
            changed
                .intent_history
                .append(IntentHistoryRecord {
                    record_id: "credential".into(),
                    payload_ref: "protected-payload".into(),
                    payload_hash: hash_bytes(&bytes),
                    payload_bytes: bytes.len() as u64,
                    label_key: "Changed configuration settings".into(),
                    subject_key: "password".into(),
                    created_unix_ms: 3,
                    undone: false,
                })
                .unwrap();
            let restarted: ConfigurationState =
                serde_json::from_slice(&serde_json::to_vec(&changed).unwrap()).unwrap();
            let (undone, conflicts) = payload
                .prepare(
                    &restarted,
                    "credential",
                    IntentHistoryDirection::Undo,
                    &BTreeMap::new(),
                    4,
                )
                .unwrap();
            assert!(conflicts.is_empty());
            assert_eq!(
                undone.upstream_document().unwrap(),
                initial.upstream_document().unwrap()
            );
            let (redone, conflicts) = payload
                .prepare(
                    &undone,
                    "credential",
                    IntentHistoryDirection::Redo,
                    &BTreeMap::new(),
                    5,
                )
                .unwrap();
            assert!(conflicts.is_empty());
            assert_eq!(
                redone.upstream_document().unwrap(),
                changed.upstream_document().unwrap()
            );
            assert_eq!(redone.applied, initial.applied);
            assert_eq!(redone.last_known_good, initial.last_known_good);
            let context =
                configuration_intent_context(&profile, &redone, Some(&node.node_id)).unwrap();
            assert!(context.values[0].intent_value.is_none());
            assert!(
                !serde_json::to_string(&redone.intent_history)
                    .unwrap()
                    .contains("fixture-")
            );
            let mut removed = redone;
            removed
                .update_intent(
                    &profile,
                    &ConfigurationIntentAction::EditNode {
                        node_id: node.node_id.clone(),
                        reference_id: context.values[0].reference_id.clone(),
                        value: SemanticValue::Missing,
                    },
                    6,
                )
                .unwrap();
            assert!(
                removed
                    .upstream_document()
                    .unwrap()
                    .pointer(pointer)
                    .is_none()
            );
            let context =
                configuration_intent_context(&profile, &removed, Some(&node.node_id)).unwrap();
            assert!(!context.values[0].present);
            assert!(context.values[0].protected);
            assert!(context.values[0].can_follow);
            removed
                .update_intent(
                    &profile,
                    &ConfigurationIntentAction::FollowNode {
                        node_id: node.node_id.clone(),
                        reference_id: context.values[0].reference_id.clone(),
                    },
                    7,
                )
                .unwrap();
            assert_eq!(
                removed.upstream_document().unwrap(),
                initial.upstream_document().unwrap()
            );
            assert_eq!(removed.applied, initial.applied);
            assert_eq!(removed.last_known_good, initial.last_known_good);
        }
    }
}

#[test]
fn registered_node_edits_bind_values_and_keep_missing_null_and_follow_distinct() {
    let profile = latest(ProgramKind::SingBox);
    let mut state = state(
        &profile,
        json!({"log":{"level":"info","timestamp":true,"unknown":"retained"}}),
    );
    let catalog = configuration_intent_catalog(&profile).unwrap();
    let node = catalog
        .nodes
        .iter()
        .find(|node| node.pattern == ["log", "timestamp"])
        .unwrap();
    let context = configuration_intent_context(&profile, &state, Some(&node.node_id)).unwrap();
    let reference = context.values[0].reference_id.clone();
    state
        .update_intent(
            &profile,
            &ConfigurationIntentAction::EditNode {
                node_id: node.node_id.clone(),
                reference_id: reference.clone(),
                value: SemanticValue::Missing,
            },
            3,
        )
        .unwrap();
    let document = parse_semantic_document(state.format, state.desired.content.as_bytes()).unwrap();
    assert!(document["log"].get("timestamp").is_none());
    assert_eq!(document["log"]["unknown"], json!("retained"));
    assert!(
        state
            .update_intent(
                &profile,
                &ConfigurationIntentAction::EditNode {
                    node_id: node.node_id.clone(),
                    reference_id: reference,
                    value: SemanticValue::Present(json!(false))
                },
                4
            )
            .is_err()
    );
    let context = configuration_intent_context(&profile, &state, Some(&node.node_id)).unwrap();
    assert!(!context.values[0].present);
    assert!(context.values[0].can_follow);
    state
        .update_intent(
            &profile,
            &ConfigurationIntentAction::FollowNode {
                node_id: node.node_id.clone(),
                reference_id: context.values[0].reference_id.clone(),
            },
            5,
        )
        .unwrap();
    assert_eq!(
        parse_semantic_document(state.format, state.desired.content.as_bytes()).unwrap()["log"]["timestamp"],
        json!(true)
    );
    let context = configuration_intent_context(&profile, &state, Some(&node.node_id)).unwrap();
    assert!(!context.values[0].can_follow);
}

#[test]
fn native_fields_reject_wrong_types_and_do_not_rewrite_siblings_or_missing_objects() {
    let profile = latest(ProgramKind::SingBox);
    let mut state = state(
        &profile,
        json!({"log":{"disabled":false,"unknown":{"preserve":1}}}),
    );
    let catalog = configuration_intent_catalog(&profile).unwrap();
    let node = catalog
        .nodes
        .iter()
        .find(|node| node.pattern == ["log", "disabled"])
        .unwrap();
    let context = configuration_intent_context(&profile, &state, Some(&node.node_id)).unwrap();
    let request = IntentPreviewRequest {
        expected_state_revision: state.state_revision,
        editor_session_id: None,
        expected_draft_revision: None,
        change: ConfigurationIntentAction::EditNode {
            node_id: node.node_id.clone(),
            reference_id: context.values[0].reference_id.clone(),
            value: SemanticValue::Present(Value::Null),
        },
    };
    let invalid = state.prepare_intent_change(&profile, &request, 3).unwrap();
    assert!(!invalid.plan.can_commit);
    assert_eq!(invalid.state, state);
    state
        .update_intent(
            &profile,
            &ConfigurationIntentAction::EditNode {
                node_id: node.node_id.clone(),
                reference_id: context.values[0].reference_id.clone(),
                value: SemanticValue::Present(json!(true)),
            },
            4,
        )
        .unwrap();
    let document = parse_semantic_document(state.format, state.desired.content.as_bytes()).unwrap();
    assert_eq!(document["log"]["disabled"], json!(true));
    assert_eq!(document["log"]["unknown"], json!({"preserve":1}));
}

#[test]
fn protocol_fields_select_stable_objects_and_refuse_duplicate_identities() {
    let profile = latest(ProgramKind::SingBox);
    let mut state = state(
        &profile,
        json!({"inbounds":[{"type":"socks","tag":"local","listen_port":1080,"unknown":1},{"type":"http","tag":"web","listen_port":8080}]}),
    );
    let catalog = configuration_intent_catalog(&profile).unwrap();
    let node = catalog
        .nodes
        .iter()
        .find(|node| {
            node.pattern == ["inbounds", "*", "listen_port"]
                && node
                    .type_selections
                    .iter()
                    .any(|selection| selection.value == "socks")
        })
        .unwrap();
    let context = configuration_intent_context(&profile, &state, Some(&node.node_id)).unwrap();
    state
        .update_intent(
            &profile,
            &ConfigurationIntentAction::EditNode {
                node_id: node.node_id.clone(),
                reference_id: context.values[0].reference_id.clone(),
                value: SemanticValue::Present(json!(1081)),
            },
            4,
        )
        .unwrap();
    let document = parse_semantic_document(state.format, state.desired.content.as_bytes()).unwrap();
    assert_eq!(document["inbounds"][0]["listen_port"], json!(1081));
    assert_eq!(document["inbounds"][0]["unknown"], json!(1));
    assert_eq!(document["inbounds"][1]["listen_port"], json!(8080));
    let mut duplicate = super::tests::state(
        &profile,
        json!({"inbounds":[{"type":"socks","tag":"local","listen_port":1080},{"type":"socks","tag":"local","listen_port":1081}]}),
    );
    let context = configuration_intent_context(&profile, &duplicate, Some(&node.node_id)).unwrap();
    assert_eq!(
        duplicate
            .update_intent(
                &profile,
                &ConfigurationIntentAction::EditNode {
                    node_id: node.node_id.clone(),
                    reference_id: context.values[0].reference_id.clone(),
                    value: SemanticValue::Present(json!(1082))
                },
                4
            )
            .unwrap_err()
            .message_key
            .as_deref(),
        Some("INTENT_OBJECT_AMBIGUOUS")
    );
}

#[test]
fn contexts_reuse_one_candidate_parse_and_refresh_revision_metadata() {
    let profile = latest(ProgramKind::Xray);
    let mut state = state(
        &profile,
        json!({"log":{"loglevel":"info"},"dns":{"queryStrategy":"UseIP"}}),
    );
    let cache = Mutex::new(IntentContextCache::new(MAX_CACHED_CONTEXTS));
    let first = context_index_with_cache(&profile, &state, &cache).unwrap();
    let catalog = configuration_intent_catalog(&profile).unwrap();
    let log = catalog
        .nodes
        .iter()
        .find(|node| node.pattern == ["log"])
        .unwrap();
    let context = configuration_intent_context(&profile, &state, Some(&log.node_id)).unwrap();
    state.state_revision += 1;
    let next = context_index_with_cache(&profile, &state, &cache).unwrap();
    assert!(Arc::ptr_eq(&first, &next));
    let newer = configuration_intent_context(&profile, &state, Some(&log.node_id)).unwrap();
    assert_eq!(context.values, newer.values);
    assert_eq!(newer.state_revision, context.state_revision + 1);
    state.desired.content = "{\"log\":{\"loglevel\":\"debug\"}}".into();
    let updated = context_index_with_cache(&profile, &state, &cache).unwrap();
    assert!(!Arc::ptr_eq(&first, &updated));
}

#[test]
fn root_context_discovers_chapters_without_reading_or_rewriting_values() {
    let profile = latest(ProgramKind::Mihomo);
    let state = state(
        &profile,
        json!({"dns":{"enable":true},"rules":["MATCH,DIRECT"]}),
    );
    let before = state.clone();
    let context = configuration_intent_context(&profile, &state, None).unwrap();
    assert!(context.node.is_none());
    assert!(
        context
            .children
            .iter()
            .any(|child| child.pattern == ["dns"])
    );
    assert!(context.values.is_empty());
    assert_eq!(state, before);
}

#[test]
fn context_hides_credentials_embedded_in_native_urls() {
    let profile = latest(ProgramKind::SingBox);
    let state = state(
        &profile,
        json!({"outbounds":[{"type":"http","tag":"test","server":"https://test:synthetic-password@example.test?token=synthetic-token"}]}),
    );
    let catalog = configuration_intent_catalog(&profile).unwrap();
    let node = catalog
        .nodes
        .iter()
        .find(|node| {
            node.pattern == ["outbounds", "*", "server"]
                && node
                    .type_selections
                    .iter()
                    .any(|selector| selector.value == "http")
        })
        .unwrap();
    let context = configuration_intent_context(&profile, &state, Some(&node.node_id)).unwrap();
    assert!(context.values[0].protected);
    assert!(context.values[0].value.is_none());
    let encoded = serde_json::to_string(&context).unwrap();
    assert!(!encoded.contains("synthetic-"));
}

#[test]
fn reviewed_required_settings_are_one_reversible_plan_for_each_stable_patch() {
    for kind in [ProgramKind::SingBox, ProgramKind::Mihomo] {
        let knowledge = crate::embedded_core_knowledge().unwrap();
        for release in &knowledge.program(kind).unwrap().releases {
            let profile = profile_on(kind, &release.version, "linux");
            let (document, pattern, dependency) = if kind == ProgramKind::SingBox {
                (
                    json!({"inbounds":[{"type":"tun","tag":"adapter","auto_redirect":false,"mtu":1500}]}),
                    vec!["inbounds", "*", "auto_redirect"],
                    "/inbounds/0/auto_route",
                )
            } else {
                (
                    json!({"tun":{"enable":true,"auto-redirect":false,"mtu":1500}}),
                    vec!["tun", "auto-redirect"],
                    "/tun/auto-route",
                )
            };
            if !knowledge
                .program(kind)
                .unwrap()
                .semantic_rules
                .iter()
                .any(|variant| {
                    variant.releases.contains(&release.tag)
                        && variant.rule.path == pattern
                        && matches!(
                            variant.rule.constraint,
                            crate::KnowledgeValueConstraint::Requires { .. }
                        )
                })
            {
                continue;
            }
            let mut state = state(&profile, document);
            state.applied = Some(state.desired.clone());
            state.last_known_good = state.applied.clone();
            let catalog = configuration_intent_catalog(&profile).unwrap();
            let node = catalog
                .nodes
                .iter()
                .find(|node| node.pattern == pattern)
                .unwrap();
            let context =
                configuration_intent_context(&profile, &state, Some(&node.node_id)).unwrap();
            let request = IntentPreviewRequest {
                expected_state_revision: state.state_revision,
                editor_session_id: None,
                expected_draft_revision: None,
                change: ConfigurationIntentAction::EditNode {
                    node_id: node.node_id.clone(),
                    reference_id: context.values[0].reference_id.clone(),
                    value: SemanticValue::Present(json!(true)),
                },
            };
            let prepared = state.prepare_intent_change(&profile, &request, 2).unwrap();
            assert!(
                prepared.plan.can_commit,
                "{kind:?} {}: {:?}",
                release.tag, prepared.plan.blockers
            );
            assert_eq!(prepared.plan.effects.len(), 2);
            assert_eq!(
                prepared
                    .plan
                    .effects
                    .iter()
                    .filter(|effect| effect.role == IntentEffectRole::Related)
                    .count(),
                1
            );
            let mut next = prepared.state;
            assert_eq!(next.generation, state.generation + 1);
            assert_eq!(
                parse_semantic_document(next.format, next.desired.content.as_bytes())
                    .unwrap()
                    .pointer(dependency),
                Some(&json!(true))
            );
            assert_eq!(next.applied, state.applied);
            assert_eq!(next.last_known_good, state.last_known_good);
            let payload = IntentHistoryPayload::between(&state, &next)
                .unwrap()
                .unwrap();
            let bytes = serde_json::to_vec(&payload).unwrap();
            next.intent_history
                .append(IntentHistoryRecord {
                    subject_key: "Automatic traffic redirection".into(),
                    record_id: "linked".into(),
                    payload_ref: "protected".into(),
                    payload_hash: hash_bytes(&bytes),
                    payload_bytes: bytes.len() as u64,
                    label_key: "Intent change".into(),
                    created_unix_ms: 2,
                    undone: false,
                })
                .unwrap();
            let (undone, conflicts) = payload
                .prepare(
                    &next,
                    "linked",
                    IntentHistoryDirection::Undo,
                    &BTreeMap::new(),
                    3,
                )
                .unwrap();
            assert!(conflicts.is_empty());
            assert_eq!(
                undone.upstream_document().unwrap(),
                state.upstream_document().unwrap()
            );
            let (redone, conflicts) = payload
                .prepare(
                    &undone,
                    "linked",
                    IntentHistoryDirection::Redo,
                    &BTreeMap::new(),
                    4,
                )
                .unwrap();
            assert!(conflicts.is_empty());
            assert_eq!(
                redone.upstream_document().unwrap(),
                next.upstream_document().unwrap()
            );
            let context =
                configuration_intent_context(&profile, &next, Some(&node.node_id)).unwrap();
            let mut repeat = request;
            repeat.expected_state_revision = next.state_revision;
            if let ConfigurationIntentAction::EditNode { reference_id, .. } = &mut repeat.change {
                *reference_id = context.values[0].reference_id.clone();
            }
            let repeated = next.prepare_intent_change(&profile, &repeat, 5).unwrap();
            assert_eq!(
                repeated.state, next,
                "repeated setting must not claim already effective dependencies"
            );
        }
    }
}

#[test]
fn native_field_selection_stays_on_the_same_identity_after_source_reorder() {
    let profile = latest(ProgramKind::SingBox);
    let mut state = state(
        &profile,
        json!({"inbounds":[
            {"type":"http","tag":"first","listen_port":8080},
            {"type":"http","tag":"second","listen_port":8081}
        ]}),
    );
    let catalog = configuration_intent_catalog(&profile).unwrap();
    let node = catalog
        .nodes
        .iter()
        .find(|node| {
            node.pattern == ["inbounds", "*", "listen_port"]
                && node
                    .type_selections
                    .iter()
                    .any(|selection| selection.value == "http")
        })
        .unwrap();
    let context = configuration_intent_context(&profile, &state, Some(&node.node_id)).unwrap();
    let chosen = context.values[0].clone();
    state.base.content = serialize_semantic_document(
        state.format,
        &json!({"inbounds":[
            {"type":"http","tag":"second","listen_port":8081},
            {"type":"http","tag":"first","listen_port":8080}
        ]}),
    )
    .unwrap();
    state.rebuild_desired(2).unwrap();
    let updated = configuration_intent_context(&profile, &state, Some(&node.node_id)).unwrap();
    assert_eq!(updated.values[1].semantic_path, chosen.semantic_path);
    assert_eq!(updated.values[1].reference_id, chosen.reference_id);
    state
        .update_intent(
            &profile,
            &ConfigurationIntentAction::EditNode {
                node_id: node.node_id.clone(),
                reference_id: chosen.reference_id,
                value: SemanticValue::Present(json!(8180)),
            },
            3,
        )
        .unwrap();
    let document = state.upstream_document().unwrap();
    assert_eq!(document["inbounds"][0]["listen_port"], 8081);
    assert_eq!(document["inbounds"][1]["listen_port"], 8180);
}

#[test]
fn required_native_settings_cannot_be_disabled_while_their_dependent_is_enabled() {
    for kind in [ProgramKind::SingBox, ProgramKind::Mihomo] {
        let profile = latest_on(kind, "linux");
        let (document, field) = if kind == ProgramKind::SingBox {
            (
                json!({"inbounds":[{"type":"tun","tag":"adapter","auto_redirect":true,"auto_route":true}]}),
                "auto_route",
            )
        } else {
            (
                json!({"tun":{"enable":true,"auto-redirect":true,"auto-route":true}}),
                "auto-route",
            )
        };
        let state = state(&profile, document);
        let catalog = configuration_intent_catalog(&profile).unwrap();
        let node = catalog
            .nodes
            .iter()
            .find(|node| node.native_name == field)
            .unwrap();
        let context = configuration_intent_context(&profile, &state, Some(&node.node_id)).unwrap();
        let request = IntentPreviewRequest {
            expected_state_revision: state.state_revision,
            editor_session_id: None,
            expected_draft_revision: None,
            change: ConfigurationIntentAction::EditNode {
                node_id: node.node_id.clone(),
                reference_id: context.values[0].reference_id.clone(),
                value: SemanticValue::Present(json!(false)),
            },
        };
        let prepared = state.prepare_intent_change(&profile, &request, 2).unwrap();
        assert!(!prepared.plan.can_commit, "{kind:?}");
        assert_eq!(prepared.state, state);
        assert_eq!(prepared.plan.blockers.len(), 1);
    }
}

#[test]
fn necessary_native_edits_are_platform_checked_without_blocking_unrelated_repairs() {
    for kind in [ProgramKind::SingBox, ProgramKind::Mihomo] {
        let document = if kind == ProgramKind::SingBox {
            json!({"log":{"level":"not-a-level"},"inbounds":[{"type":"tun","tag":"adapter","auto_redirect":false}]})
        } else {
            json!({"log-level":"not-a-level","tun":{"enable":true,"auto-redirect":false}})
        };
        let field = if kind == ProgramKind::SingBox {
            "auto_redirect"
        } else {
            "auto-redirect"
        };
        for platform in ["linux", "windows"] {
            let profile = latest_on(kind, platform);
            let state = state(&profile, document.clone());
            assert!(!profile.assess(&document).unwrap().issues.is_empty());
            let catalog = configuration_intent_catalog(&profile).unwrap();
            let node = catalog
                .nodes
                .iter()
                .find(|node| node.native_name == field)
                .unwrap();
            let context =
                configuration_intent_context(&profile, &state, Some(&node.node_id)).unwrap();
            let mut request = IntentPreviewRequest {
                expected_state_revision: state.state_revision,
                editor_session_id: None,
                expected_draft_revision: None,
                change: ConfigurationIntentAction::EditNode {
                    node_id: node.node_id.clone(),
                    reference_id: context.values[0].reference_id.clone(),
                    value: SemanticValue::Present(json!(true)),
                },
            };
            let prepared = state.prepare_intent_change(&profile, &request, 2).unwrap();
            assert_eq!(
                prepared.plan.can_commit,
                platform == "linux",
                "{kind:?} {platform}: {:?}",
                prepared.plan.blockers
            );
            if platform == "windows" {
                assert_eq!(prepared.state, state);
                if let ConfigurationIntentAction::EditNode { value, .. } = &mut request.change {
                    *value = SemanticValue::Missing;
                }
                let removal = state.prepare_intent_change(&profile, &request, 3).unwrap();
                assert!(
                    removal.plan.can_commit,
                    "{kind:?}: {:?}",
                    removal.plan.blockers
                );
                assert_eq!(removal.plan.effects.len(), 1);
            }
        }
    }
}

#[test]
fn a_final_only_protocol_switch_does_not_write_fields_to_the_previous_protocol() {
    let profile = latest(ProgramKind::SingBox);
    let mut state = state(
        &profile,
        json!({"inbounds":[{"type":"http","tag":"local","listen_port":8080}]}),
    );
    state
        .replace_final_from_edited(
            br#"{"inbounds":[{"type":"socks","tag":"local","listen_port":1080}]}"#,
            2,
        )
        .unwrap();
    let catalog = configuration_intent_catalog(&profile).unwrap();
    let node = catalog
        .nodes
        .iter()
        .find(|node| {
            node.pattern == ["inbounds", "*", "listen_port"]
                && node
                    .type_selections
                    .iter()
                    .any(|selection| selection.value == "socks")
        })
        .unwrap();
    let context = configuration_intent_context(&profile, &state, Some(&node.node_id)).unwrap();
    let request = IntentPreviewRequest {
        expected_state_revision: state.state_revision,
        editor_session_id: None,
        expected_draft_revision: None,
        change: ConfigurationIntentAction::EditNode {
            node_id: node.node_id.clone(),
            reference_id: context.values[0].reference_id.clone(),
            value: SemanticValue::Present(json!(1180)),
        },
    };
    let prepared = state.prepare_intent_change(&profile, &request, 3).unwrap();
    assert!(!prepared.plan.can_commit);
    assert_eq!(
        prepared.plan.blockers[0].message_key,
        "INTENT_NATIVE_SHAPE_REQUIRES_EDITOR"
    );
    assert_eq!(prepared.state, state);
}

#[test]
fn reviewed_native_enums_expose_the_exact_patch_options_and_reject_other_values() {
    let knowledge = crate::embedded_core_knowledge().unwrap();
    for release in &knowledge.program(ProgramKind::Xray).unwrap().releases {
        let profile = profile(ProgramKind::Xray, &release.version);
        let state = state(
            &profile,
            json!({"outbounds":[{"protocol":"freedom","tag":"direct","settings":{},"mux":{"enabled":true,"xudpProxyUDP443":"reject"}}]}),
        );
        let catalog = configuration_intent_catalog(&profile).unwrap();
        let node = catalog
            .nodes
            .iter()
            .find(|node| node.pattern == ["outbounds", "*", "mux", "xudpProxyUDP443"])
            .unwrap();
        let control = node.control.as_ref().unwrap();
        assert_eq!(control.kind, GuidedControl::Select);
        assert_eq!(control.allowed_values, ["", "reject", "allow", "skip"]);
        let context = configuration_intent_context(&profile, &state, Some(&node.node_id)).unwrap();
        let mut request = IntentPreviewRequest {
            expected_state_revision: state.state_revision,
            editor_session_id: None,
            expected_draft_revision: None,
            change: ConfigurationIntentAction::EditNode {
                node_id: node.node_id.clone(),
                reference_id: context.values[0].reference_id.clone(),
                value: SemanticValue::Present(json!("accept")),
            },
        };
        let blocked = state.prepare_intent_change(&profile, &request, 2).unwrap();
        assert!(!blocked.plan.can_commit);
        assert_eq!(blocked.state, state);
        if let ConfigurationIntentAction::EditNode { value, .. } = &mut request.change {
            *value = SemanticValue::Present(json!("allow"));
        }
        let accepted = state.prepare_intent_change(&profile, &request, 3).unwrap();
        assert!(accepted.plan.can_commit, "{:?}", accepted.plan.blockers);
        assert_eq!(
            accepted.state.upstream_document().unwrap()["outbounds"][0]["mux"]["xudpProxyUDP443"],
            "allow"
        );
    }
}

#[test]
fn missing_native_fields_can_be_added_removed_and_followed_without_recreating_objects() {
    let profile = latest(ProgramKind::SingBox);
    let catalog = configuration_intent_catalog(&profile).unwrap();
    let node = catalog
        .nodes
        .iter()
        .find(|node| {
            node.pattern == ["inbounds", "*", "listen_port"]
                && node
                    .type_selections
                    .iter()
                    .any(|selection| selection.value == "http")
        })
        .unwrap();
    for source_port in [None, Some(8080)] {
        let mut object = json!({"type":"http","tag":"local","unknown":{"keep":true}});
        if let Some(port) = source_port {
            object["listen_port"] = json!(port);
        }
        let mut state = state(
            &profile,
            json!({"inbounds":[object.clone(),{"type":"socks","tag":"other","listen_port":1080}]}),
        );
        let original = state.upstream_document().unwrap();
        let context = configuration_intent_context(&profile, &state, Some(&node.node_id)).unwrap();
        assert_eq!(context.values.len(), 1);
        assert_eq!(
            context.values[0].semantic_path,
            "/inbounds[tag=local]/listen_port"
        );
        assert_eq!(context.values[0].present, source_port.is_some());
        state
            .update_intent(
                &profile,
                &ConfigurationIntentAction::EditNode {
                    node_id: node.node_id.clone(),
                    reference_id: context.values[0].reference_id.clone(),
                    value: SemanticValue::Present(json!(8180)),
                },
                2,
            )
            .unwrap();
        let context = configuration_intent_context(&profile, &state, Some(&node.node_id)).unwrap();
        state
            .update_intent(
                &profile,
                &ConfigurationIntentAction::EditNode {
                    node_id: node.node_id.clone(),
                    reference_id: context.values[0].reference_id.clone(),
                    value: SemanticValue::Missing,
                },
                3,
            )
            .unwrap();
        let context = configuration_intent_context(&profile, &state, Some(&node.node_id)).unwrap();
        assert_eq!(context.values.len(), 1);
        assert!(!context.values[0].present);
        assert!(context.values[0].can_follow);
        assert_eq!(
            state.upstream_document().unwrap()["inbounds"][0]["unknown"],
            json!({"keep":true})
        );
        state
            .update_intent(
                &profile,
                &ConfigurationIntentAction::FollowNode {
                    node_id: node.node_id.clone(),
                    reference_id: context.values[0].reference_id.clone(),
                },
                4,
            )
            .unwrap();
        assert_eq!(state.upstream_document().unwrap(), original);
        let before = state.clone();
        let stale = state.update_intent(
            &profile,
            &ConfigurationIntentAction::EditNode {
                node_id: node.node_id.clone(),
                reference_id: context.values[0].reference_id.clone(),
                value: SemanticValue::Present(Value::Null),
            },
            5,
        );
        assert!(stale.is_err());
        assert_eq!(state, before);
    }
}

#[test]
fn curated_fields_follow_their_source_after_deletion_without_retaining_a_delete_owner() {
    for program in [ProgramKind::SingBox, ProgramKind::Xray, ProgramKind::Mihomo] {
        let profile = latest(program);
        let source = match program {
            ProgramKind::SingBox => json!({"log":{"level":"info","timestamp":true}}),
            ProgramKind::Xray => json!({"log":{"loglevel":"info","dnsLog":true}}),
            ProgramKind::Mihomo => json!({"log-level":"info","ipv6":true}),
            _ => unreachable!(),
        };
        let mut state = state(&profile, source.clone());
        let catalog = configuration_intent_catalog(&profile).unwrap();
        let node = catalog
            .nodes
            .iter()
            .find(|node| node.setting_id.as_deref() == Some("logging.level"))
            .unwrap();
        let context = configuration_intent_context(&profile, &state, Some(&node.node_id)).unwrap();
        state
            .update_intent(
                &profile,
                &ConfigurationIntentAction::EditNode {
                    node_id: node.node_id.clone(),
                    reference_id: context.values[0].reference_id.clone(),
                    value: SemanticValue::Missing,
                },
                2,
            )
            .unwrap();
        let context = configuration_intent_context(&profile, &state, Some(&node.node_id)).unwrap();
        assert!(!context.values[0].present);
        assert!(context.values[0].can_follow);
        assert_eq!(context.values[0].intent_value, Some(SemanticValue::Missing));
        state
            .update_intent(
                &profile,
                &ConfigurationIntentAction::FollowNode {
                    node_id: node.node_id.clone(),
                    reference_id: context.values[0].reference_id.clone(),
                },
                3,
            )
            .unwrap();
        assert_eq!(state.upstream_document().unwrap(), source);
        assert!(state.guided_intent.path_edits.is_empty());
        let context = configuration_intent_context(&profile, &state, Some(&node.node_id)).unwrap();
        assert!(!context.values[0].can_follow);
        let generation = state.generation;
        state
            .update_intent(
                &profile,
                &ConfigurationIntentAction::FollowNode {
                    node_id: node.node_id.clone(),
                    reference_id: context.values[0].reference_id.clone(),
                },
                4,
            )
            .unwrap();
        assert_eq!(state.generation, generation);
    }
}

#[test]
fn default_target_contexts_are_typed_unambiguous_and_reject_missing_targets() {
    let knowledge = crate::embedded_core_knowledge().unwrap();
    for release in &knowledge.program(ProgramKind::SingBox).unwrap().releases {
        let profile = profile(ProgramKind::SingBox, &release.version);
        let source = json!({"outbounds":[{"type":"direct","tag":"direct"},{"type":"direct","tag":"duplicate"},{"type":"direct","tag":"duplicate"},{"type":"direct","tag":""}],"dns":{"servers":[{"type":"local","tag":"system"},{"type":"local","tag":"duplicate-dns"},{"type":"local","tag":"duplicate-dns"}]},"unknown":{"preserved":true}});
        let initial = state(&profile, source.clone());
        let catalog = configuration_intent_catalog(&profile).unwrap();
        for (setting, selected, kind) in [
            ("routing.final", "direct", "connection"),
            ("dns.final", "system", "dns"),
        ] {
            let node = catalog
                .nodes
                .iter()
                .find(|node| node.setting_id.as_deref() == Some(setting))
                .unwrap();
            let context =
                configuration_intent_context(&profile, &initial, Some(&node.node_id)).unwrap();
            let targets = context
                .targets
                .iter()
                .filter(|target| target.kind == kind)
                .map(|target| target.id.as_str())
                .collect::<Vec<_>>();
            assert_eq!(targets, [selected]);
            let mut next = initial.clone();
            let action = ConfigurationIntentAction::EditNode {
                node_id: node.node_id.clone(),
                reference_id: context.values[0].reference_id.clone(),
                value: SemanticValue::Present(json!(if kind == "dns" {
                    "direct"
                } else {
                    "system"
                })),
            };
            assert_eq!(
                next.update_intent(&profile, &action, 2)
                    .unwrap_err()
                    .message_key
                    .as_deref(),
                Some("INTENT_TARGET_MISSING")
            );
            assert_eq!(next, initial);
            let action = ConfigurationIntentAction::EditNode {
                node_id: node.node_id.clone(),
                reference_id: context.values[0].reference_id.clone(),
                value: SemanticValue::Present(json!(selected)),
            };
            next.update_intent(&profile, &action, 3).unwrap();
            let effective = next.upstream_document().unwrap();
            assert_eq!(effective["unknown"], source["unknown"]);
            assert_eq!(effective["outbounds"], source["outbounds"]);
            assert_eq!(effective["dns"]["servers"], source["dns"]["servers"]);
        }
    }
}
