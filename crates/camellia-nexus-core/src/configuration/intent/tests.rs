use super::*;
use crate::{CoreBinaryFingerprint, CoreProbeReport, embedded_core_knowledge};

fn profile(program: ProgramKind, version: &str) -> CoreCapabilityProfile {
    let output = match program {
        ProgramKind::SingBox => format!(
            "sing-box version {version}\nEnvironment: go1.26.0 windows/amd64\nTags: with_gvisor,with_quic"
        ),
        ProgramKind::Mihomo => {
            format!("Mihomo Meta v{version} windows amd64 with go1.26.0\nUse tags: with_gvisor")
        }
        ProgramKind::Xray => {
            format!("Xray {version} (Xray, Penetrates Everything.) Custom (go1.26.0 windows/amd64)")
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

fn latest(program: ProgramKind) -> CoreCapabilityProfile {
    let knowledge = embedded_core_knowledge().unwrap();
    let release = &knowledge.program(program).unwrap().releases.last().unwrap();
    profile(program, &release.version)
}

#[test]
fn listener_authentication_edits_preserve_other_accounts_and_custom_fields() {
    for kind in [ProgramKind::SingBox, ProgramKind::Xray] {
        let profile = latest(kind);
        let original = if kind == ProgramKind::SingBox {
            json!({"inbounds":[{"tag":"local","type":"socks","listen":"127.0.0.1","listen_port":18080,"users":[{"username":"first","password":"synthetic-one","custom":true},{"username":"second","password":"synthetic-two"}]}]})
        } else {
            json!({"inbounds":[{"tag":"local","protocol":"socks","listen":"127.0.0.1","port":18080,"settings":{"auth":"password","accounts":[{"user":"first","pass":"synthetic-one","custom":true},{"user":"second","pass":"synthetic-two"}]}}]})
        };
        let mut state = state(&profile, original);
        let listener = objects(&profile, &state)
            .into_iter()
            .find(|object| object.kind == IntentObjectKind::Listener)
            .unwrap();
        update(
            &profile,
            &mut state,
            &listener,
            &[
                ("username", json!("first")),
                ("password", json!("synthetic-replaced")),
            ],
        );
        let document = state.upstream_document().unwrap();
        let (accounts, password_key) = if kind == ProgramKind::SingBox {
            (&document["inbounds"][0]["users"], "password")
        } else {
            (&document["inbounds"][0]["settings"]["accounts"], "pass")
        };
        assert_eq!(accounts.as_array().unwrap().len(), 2);
        assert_eq!(accounts[0][password_key], "synthetic-replaced");
        assert_eq!(accounts[0]["custom"], true);
        assert_eq!(accounts[1][password_key], "synthetic-two");
        let projection = serde_json::to_string(&objects(&profile, &state)).unwrap();
        assert!(!projection.contains("synthetic-"));
    }
}

#[test]
fn default_route_can_select_a_unique_outbound_added_in_final_configuration() {
    let profile = latest(ProgramKind::SingBox);
    let mut state = state(&profile, json!({"outbounds":[]}));
    state
        .replace_final_from_edited(
            br#"{"outbounds":[{"type":"direct","tag":"final-direct"}]}"#,
            2,
        )
        .unwrap();
    state
        .update_intent(
            &profile,
            &ConfigurationIntentAction::Set {
                setting_id: "routing.final".into(),
                value: json!("final-direct"),
            },
            3,
        )
        .unwrap();
    let document = parse_semantic_document(state.format, state.desired.content.as_bytes()).unwrap();
    assert_eq!(document["route"]["final"], "final-direct");
    assert_eq!(document["outbounds"][0]["tag"], "final-direct");
    assert!(state.final_edit.conflicts.is_empty());
}

#[test]
fn simple_rules_use_existing_targets_and_preserve_order_and_unrelated_fields() {
    for kind in [ProgramKind::SingBox, ProgramKind::Xray, ProgramKind::Mihomo] {
        let profile = latest(kind);
        let original = if kind == ProgramKind::Mihomo {
            json!({"rules":["DOMAIN,first.example,DIRECT"],"proxies":[],"keep":true})
        } else if kind == ProgramKind::SingBox {
            json!({"route":{"rules":[{"domain":["first.example"],"action":"route","outbound":"direct"}]},"outbounds":[{"tag":"direct","type":"direct"}],"keep":true})
        } else {
            json!({"routing":{"rules":[{"type":"field","domain":["full:first.example"],"outboundTag":"direct"}]},"outbounds":[{"tag":"direct","protocol":"freedom"}],"keep":true})
        };
        let mut state = state(&profile, original);
        let mut view = state.view();
        project_intent_objects(&profile, &state, &mut view).unwrap();
        assert!(
            view.intent_object_descriptors
                .iter()
                .any(|descriptor| descriptor.kind == IntentObjectKind::RouteRule
                    && descriptor.can_create)
        );
        let target = if kind == ProgramKind::Mihomo {
            "DIRECT"
        } else {
            "direct"
        };
        state
            .update_intent(
                &profile,
                &ConfigurationIntentAction::CreateObject {
                    object_kind: IntentObjectKind::RouteRule,
                    values: values(&[
                        ("match", json!("subdomain")),
                        ("value", json!("second.example")),
                        ("target", json!(target)),
                    ]),
                },
                2,
            )
            .unwrap();
        let original = objects(&profile, &state)
            .into_iter()
            .find(|object| {
                object.kind == IntentObjectKind::RouteRule
                    && object.values.get("value") == Some(&json!("first.example"))
            })
            .unwrap();
        update(
            &profile,
            &mut state,
            &original,
            &[("value", json!("changed.example"))],
        );
        let rules = objects(&profile, &state)
            .into_iter()
            .filter(|object| object.kind == IntentObjectKind::RouteRule)
            .collect::<Vec<_>>();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].values["value"], "changed.example");
        assert_eq!(rules[1].values["value"], "second.example");
        state
            .update_intent(
                &profile,
                &ConfigurationIntentAction::FollowObject {
                    object_id: rules[0].object_id.clone(),
                    expected_hash: rules[0].content_hash.clone(),
                },
                3,
            )
            .unwrap();
        assert!(
            objects(&profile, &state)
                .iter()
                .any(|object| object.values.get("value") == Some(&json!("first.example")))
        );
        assert_eq!(state.upstream_document().unwrap()["keep"], true);
        let created = objects(&profile, &state)
            .into_iter()
            .find(|object| object.values.get("value") == Some(&json!("second.example")))
            .unwrap();
        state
            .update_intent(
                &profile,
                &ConfigurationIntentAction::RemoveObject {
                    object_id: created.object_id,
                    expected_hash: created.content_hash,
                },
                4,
            )
            .unwrap();
        assert_eq!(
            objects(&profile, &state)
                .iter()
                .filter(|object| object.kind == IntentObjectKind::RouteRule)
                .count(),
            1
        );
    }
}

#[test]
fn mihomo_dns_update_remove_and_follow_do_not_capture_sibling_servers() {
    let profile = latest(ProgramKind::Mihomo);
    let mut state = state(
        &profile,
        json!({"dns":{"nameserver":["192.0.2.1","192.0.2.2"]}}),
    );
    let original = objects(&profile, &state).remove(0);
    update(
        &profile,
        &mut state,
        &original,
        &[("server", json!("192.0.2.5"))],
    );
    let current = objects(&profile, &state).remove(0);
    assert_eq!(original.object_id, current.object_id);
    let source = SourceSnapshot::parse(
        "source",
        "source",
        state.format,
        serialize_semantic_document(
            state.format,
            &json!({"dns":{"nameserver":["192.0.2.1","192.0.2.2","192.0.2.3"]}}),
        )
        .unwrap()
        .as_bytes(),
        3,
        false,
    )
    .unwrap();
    state.base.content = source.content;
    state
        .rebuild_desired_with_source_update(3, SourceUpdateKind::Refresh)
        .unwrap();
    assert_eq!(
        state.upstream_document().unwrap()["dns"]["nameserver"],
        json!(["192.0.2.5", "192.0.2.2", "192.0.2.3"])
    );
    let current = objects(&profile, &state).remove(0);
    state
        .update_intent(
            &profile,
            &ConfigurationIntentAction::RemoveObject {
                object_id: current.object_id,
                expected_hash: current.content_hash,
            },
            4,
        )
        .unwrap();
    let removed = objects(&profile, &state)
        .into_iter()
        .find(|object| object.removed)
        .unwrap();
    state
        .update_intent(
            &profile,
            &ConfigurationIntentAction::FollowObject {
                object_id: removed.object_id,
                expected_hash: removed.content_hash,
            },
            5,
        )
        .unwrap();
    assert_eq!(
        state.upstream_document().unwrap()["dns"]["nameserver"],
        json!(["192.0.2.1", "192.0.2.2", "192.0.2.3"])
    );
}

#[test]
fn catchall_rules_and_missing_targets_do_not_create_unreachable_guided_rules() {
    let profile = latest(ProgramKind::Mihomo);
    let mut state = state(&profile, json!({"rules":["MATCH,DIRECT"]}));
    let action = ConfigurationIntentAction::CreateObject {
        object_kind: IntentObjectKind::RouteRule,
        values: values(&[
            ("match", json!("domain")),
            ("value", json!("example.test")),
            ("target", json!("DIRECT")),
        ]),
    };
    let before = state.clone();
    assert_eq!(
        state
            .update_intent(&profile, &action, 2)
            .unwrap_err()
            .message_key
            .as_deref(),
        Some("INTENT_RULE_SHADOWED")
    );
    assert_eq!(state, before);
}

fn state(profile: &CoreCapabilityProfile, document: Value) -> ConfigurationState {
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

fn values(items: &[(&str, Value)]) -> BTreeMap<String, Value> {
    items
        .iter()
        .map(|(key, value)| ((*key).into(), value.clone()))
        .collect()
}

fn objects(
    profile: &CoreCapabilityProfile,
    state: &ConfigurationState,
) -> Vec<IntentObjectProjection> {
    let mut view = state.view();
    project_intent_objects(profile, state, &mut view).unwrap();
    view.intent_objects
}

fn update(
    profile: &CoreCapabilityProfile,
    state: &mut ConfigurationState,
    object: &IntentObjectProjection,
    fields: &[(&str, Value)],
) {
    state
        .update_intent(
            profile,
            &ConfigurationIntentAction::UpdateObject {
                object_id: object.object_id.clone(),
                expected_hash: object.content_hash.clone(),
                values: values(fields),
            },
            2,
        )
        .unwrap();
}

#[test]
fn each_stable_patch_has_program_specific_log_options_and_safe_listener_creation() {
    let knowledge = embedded_core_knowledge().unwrap();
    for kind in [ProgramKind::SingBox, ProgramKind::Xray, ProgramKind::Mihomo] {
        for release in &knowledge.program(kind).unwrap().releases {
            let profile = profile(kind, &release.version);
            let mut state = state(&profile, json!({}));
            let mut view = state.view();
            project_intent_capabilities(&profile, &mut view).unwrap();
            project_intent_objects(&profile, &state, &mut view).unwrap();
            let rule = view
                .intent_object_descriptors
                .iter()
                .find(|item| item.kind == IntentObjectKind::RouteRule)
                .unwrap();
            assert!(rule.can_create, "{} {kind:?}", release.tag);
            assert!(
                rule.fields
                    .iter()
                    .any(|field| field.key == "target" && field.required),
                "{} {kind:?}: rule target control",
                release.tag
            );
            if kind == ProgramKind::SingBox {
                let dns = view
                    .intent_object_descriptors
                    .iter()
                    .find(|item| item.kind == IntentObjectKind::DnsServer)
                    .unwrap();
                assert!(
                    dns.fields.iter().any(|field| field.key == "bootstrap"),
                    "{}: DNS resolver control",
                    release.tag
                );
            }
            let logging = view
                .guided_descriptors
                .iter()
                .find(|setting| setting.id == "logging.level")
                .unwrap();
            assert!(logging.available, "{} {kind:?}", release.tag);
            let options = match kind {
                ProgramKind::Mihomo => vec!["debug", "info", "warning", "error", "silent"],
                ProgramKind::Xray => vec!["debug", "info", "warning", "error", "none"],
                _ => vec!["trace", "debug", "info", "warn", "error", "fatal", "panic"],
            };
            assert_eq!(logging.allowed_values, options);
            let protocol = if kind == ProgramKind::Xray {
                "socks"
            } else {
                "mixed"
            };
            state
                .update_intent(
                    &profile,
                    &ConfigurationIntentAction::CreateObject {
                        object_kind: IntentObjectKind::Listener,
                        values: values(&[
                            ("protocol", json!(protocol)),
                            ("port", json!(7890)),
                            ("access", json!("local")),
                        ]),
                    },
                    2,
                )
                .unwrap();
            let document =
                parse_semantic_document(state.format, state.desired.content.as_bytes()).unwrap();
            if kind == ProgramKind::Mihomo {
                assert_eq!(document["bind-address"], "127.0.0.1");
                assert_eq!(document["allow-lan"], false);
            } else {
                assert_eq!(document["inbounds"][0]["listen"], "127.0.0.1");
            }
            assert!(state.applied.is_none());
            assert!(state.last_known_good.is_none());
        }
    }
}

#[test]
fn listener_edits_preserve_extensions_and_merge_with_final_editor_fields() {
    let profile = latest(ProgramKind::SingBox);
    let mut state = state(
        &profile,
        json!({"inbounds":[{"type":"mixed","tag":"local","listen":"127.0.0.1","listen_port":7890,"custom_option":{"keep":true}}]}),
    );
    state.replace_final_from_edited(br#"{"inbounds":[{"type":"mixed","tag":"local","listen":"127.0.0.1","listen_port":7890,"custom_option":{"keep":true},"sniff":true}]}"#, 2).unwrap();
    let object = objects(&profile, &state).remove(0);
    update(&profile, &mut state, &object, &[("port", json!(7891))]);
    let document = parse_semantic_document(state.format, state.desired.content.as_bytes()).unwrap();
    assert_eq!(document["inbounds"][0]["listen_port"], 7891);
    assert_eq!(document["inbounds"][0]["custom_option"]["keep"], true);
    assert_eq!(document["inbounds"][0]["sniff"], true);
    assert!(state.final_edit.conflicts.is_empty());
    assert_eq!(state.guided_intent.path_edits.len(), 1);
    let before = state.clone();
    let object = objects(&profile, &state).remove(0);
    update(&profile, &mut state, &object, &[("port", json!(7891))]);
    assert_eq!(
        state, before,
        "repeating the same effective value is not a new write"
    );
}

#[test]
fn lan_listener_requires_authentication_and_does_not_expose_secrets_in_projection() {
    for kind in [ProgramKind::SingBox, ProgramKind::Xray, ProgramKind::Mihomo] {
        let profile = latest(kind);
        let mut state = state(&profile, json!({}));
        let mut fields = values(&[
            ("protocol", json!("socks")),
            ("port", json!(7890)),
            ("access", json!("lan")),
        ]);
        assert_eq!(
            state
                .update_intent(
                    &profile,
                    &ConfigurationIntentAction::CreateObject {
                        object_kind: IntentObjectKind::Listener,
                        values: fields.clone()
                    },
                    2
                )
                .unwrap_err()
                .message_key
                .as_deref(),
            Some("INTENT_AUTH_REQUIRED")
        );
        fields.insert("username".into(), json!("fixture-user"));
        fields.insert("password".into(), json!("fixture-password"));
        state
            .update_intent(
                &profile,
                &ConfigurationIntentAction::CreateObject {
                    object_kind: IntentObjectKind::Listener,
                    values: fields,
                },
                2,
            )
            .unwrap();
        let object = objects(&profile, &state).remove(0);
        assert!(
            !serde_json::to_string(&object)
                .unwrap()
                .contains("fixture-password")
        );
        update(&profile, &mut state, &object, &[("port", json!(7891))]);
        let document =
            parse_semantic_document(state.format, state.desired.content.as_bytes()).unwrap();
        assert!(
            serde_json::to_string(&document)
                .unwrap()
                .contains("fixture-password")
        );
    }
}

#[test]
fn mihomo_port_edit_does_not_reclaim_shared_access_or_replace_other_fields() {
    let profile = latest(ProgramKind::Mihomo);
    let mut state = state(
        &profile,
        json!({"mixed-port":7890,"allow-lan":true,"bind-address":"0.0.0.0","authentication":["fixture-user:fixture-password"],"rules":["MATCH,DIRECT"]}),
    );
    let object = objects(&profile, &state).remove(0);
    update(&profile, &mut state, &object, &[("port", json!(7891))]);
    let document = parse_semantic_document(state.format, state.desired.content.as_bytes()).unwrap();
    assert_eq!(document["allow-lan"], true);
    assert_eq!(document["bind-address"], "0.0.0.0");
    assert_eq!(document["rules"], json!(["MATCH,DIRECT"]));
    assert_eq!(state.guided_intent.path_edits.len(), 1);
}

#[test]
fn dns_identity_update_preserves_url_and_server_options() {
    for kind in [ProgramKind::SingBox, ProgramKind::Xray] {
        let profile = latest(kind);
        let mut state = state(&profile, json!({}));
        state
            .update_intent(
                &profile,
                &ConfigurationIntentAction::CreateObject {
                    object_kind: IntentObjectKind::DnsServer,
                    values: values(&[
                        ("protocol", json!("https")),
                        ("server", json!("https://192.0.2.1:8443/dns-query")),
                    ]),
                },
                2,
            )
            .unwrap();
        let object = objects(&profile, &state).remove(0);
        assert_eq!(object.values["server"], "https://192.0.2.1:8443/dns-query");
        update(
            &profile,
            &mut state,
            &object,
            &[("serverPort", json!(8444))],
        );
        assert!(state.final_edit.conflicts.is_empty());
    }
}

#[test]
fn removed_source_listener_can_restore_following_without_deleting_siblings() {
    let profile = latest(ProgramKind::SingBox);
    let mut state = state(
        &profile,
        json!({"inbounds":[{"type":"mixed","tag":"one","listen":"127.0.0.1","listen_port":7890},{"type":"socks","tag":"two","listen":"127.0.0.1","listen_port":7891}]}),
    );
    let object = objects(&profile, &state).remove(0);
    state
        .update_intent(
            &profile,
            &ConfigurationIntentAction::RemoveObject {
                object_id: object.object_id.clone(),
                expected_hash: object.content_hash.clone(),
            },
            2,
        )
        .unwrap();
    let removed = objects(&profile, &state)
        .into_iter()
        .find(|entry| entry.removed)
        .unwrap();
    assert_eq!(removed.object_id, object.object_id);
    state
        .update_intent(
            &profile,
            &ConfigurationIntentAction::FollowObject {
                object_id: removed.object_id,
                expected_hash: removed.content_hash,
            },
            3,
        )
        .unwrap();
    assert_eq!(parse_semantic_document(state.format, state.desired.content.as_bytes()).unwrap()["inbounds"].as_array().unwrap().len(), 2);
    assert!(state.guided_intent.path_edits.is_empty());
}

#[test]
fn duplicate_object_identities_are_not_selectable_and_have_unique_ui_keys() {
    let profile = latest(ProgramKind::SingBox);
    let state = state(
        &profile,
        json!({"inbounds":[{"type":"mixed","tag":"same","listen_port":7890},{"type":"socks","tag":"same","listen_port":7891}]}),
    );
    let objects = objects(&profile, &state);
    assert_eq!(objects.len(), 2);
    assert!(objects.iter().all(|object| !object.editable));
    assert_ne!(objects[0].object_id, objects[1].object_id);
}

#[test]
fn object_writes_reclaim_only_requested_fields_and_background_refresh_keeps_new_fields() {
    let profile = latest(ProgramKind::SingBox);
    let mut state = state(
        &profile,
        json!({"inbounds":[{"type":"mixed","tag":"one","listen":"127.0.0.1","listen_port":7890}]}),
    );
    let object = objects(&profile, &state).remove(0);
    update(&profile, &mut state, &object, &[("port", json!(7891))]);
    state.base.content = serde_json::to_string(&json!({"inbounds":[{"type":"mixed","tag":"one","listen":"::1","listen_port":7892,"tcp_fast_open":true}]})).unwrap();
    state.rebuild_desired(3).unwrap();
    assert_eq!(
        state.upstream_document().unwrap()["inbounds"][0]["listen_port"],
        7892
    );
    let object = objects(&profile, &state).remove(0);
    update(&profile, &mut state, &object, &[("port", json!(7891))]);
    let document = state.upstream_document().unwrap();
    assert_eq!(document["inbounds"][0]["listen_port"], 7891);
    assert_eq!(document["inbounds"][0]["listen"], "::1");
    assert_eq!(document["inbounds"][0]["tcp_fast_open"], true);
    state.base.content = serde_json::to_string(&json!({"inbounds":[{"type":"mixed","tag":"one","listen":"::1","listen_port":7893,"tcp_fast_open":true,"udp_timeout":"5m"}]})).unwrap();
    state
        .rebuild_desired_with_source_update(4, SourceUpdateKind::Refresh)
        .unwrap();
    let document = state.upstream_document().unwrap();
    assert_eq!(document["inbounds"][0]["listen_port"], 7891);
    assert_eq!(document["inbounds"][0]["udp_timeout"], "5m");
}

#[test]
fn mihomo_dns_additions_do_not_freeze_source_servers() {
    let profile = latest(ProgramKind::Mihomo);
    let mut state = state(&profile, json!({"dns":{"nameserver":["192.0.2.1"]}}));
    state
        .update_intent(
            &profile,
            &ConfigurationIntentAction::CreateObject {
                object_kind: IntentObjectKind::DnsServer,
                values: values(&[("protocol", json!("udp")), ("server", json!("192.0.2.2"))]),
            },
            2,
        )
        .unwrap();
    state.base.content = serialize_semantic_document(
        state.format,
        &json!({"dns":{"nameserver":["192.0.2.1","192.0.2.3"]}}),
    )
    .unwrap();
    state
        .rebuild_desired_with_source_update(3, SourceUpdateKind::Refresh)
        .unwrap();
    assert_eq!(
        state.upstream_document().unwrap()["dns"]["nameserver"],
        json!(["192.0.2.1", "192.0.2.3", "192.0.2.2"])
    );
}

#[test]
fn mihomo_new_proxy_does_not_change_existing_shared_access_or_accounts() {
    let profile = latest(ProgramKind::Mihomo);
    let mut state = state(
        &profile,
        json!({"mixed-port":7890,"allow-lan":true,"bind-address":"0.0.0.0","authentication":["fixture-existing:fixture-pass"]}),
    );
    let before = state.clone();
    assert_eq!(
        state
            .update_intent(
                &profile,
                &ConfigurationIntentAction::CreateObject {
                    object_kind: IntentObjectKind::Listener,
                    values: values(&[
                        ("protocol", json!("http")),
                        ("port", json!(7891)),
                        ("access", json!("local"))
                    ])
                },
                2
            )
            .unwrap_err()
            .message_key
            .as_deref(),
        Some("INTENT_SHARED_ACCESS_CONFLICT")
    );
    assert_eq!(state, before);
    state
        .update_intent(
            &profile,
            &ConfigurationIntentAction::CreateObject {
                object_kind: IntentObjectKind::Listener,
                values: values(&[
                    ("protocol", json!("http")),
                    ("port", json!(7891)),
                    ("access", json!("lan")),
                    ("username", json!("fixture-new")),
                    ("password", json!("fixture-new-pass")),
                ]),
            },
            2,
        )
        .unwrap();
    assert_eq!(
        state.upstream_document().unwrap()["authentication"],
        json!([
            "fixture-existing:fixture-pass",
            "fixture-new:fixture-new-pass"
        ])
    );
}

#[test]
fn dns_bootstrap_and_reference_checks_are_explicit_and_reentrant() {
    let profile = latest(ProgramKind::SingBox);
    let mut state = state(
        &profile,
        json!({"dns":{"servers":[{"type":"udp","tag":"bootstrap","server":"192.0.2.1"}]}}),
    );
    let mut fields = values(&[
        ("protocol", json!("https")),
        (
            "server",
            json!("https://resolver.example.invalid/dns-query"),
        ),
    ]);
    assert_eq!(
        state
            .update_intent(
                &profile,
                &ConfigurationIntentAction::CreateObject {
                    object_kind: IntentObjectKind::DnsServer,
                    values: fields.clone()
                },
                2
            )
            .unwrap_err()
            .message_key
            .as_deref(),
        Some("INTENT_DNS_BOOTSTRAP_REQUIRED")
    );
    fields.insert("bootstrap".into(), json!("missing"));
    assert_eq!(
        state
            .update_intent(
                &profile,
                &ConfigurationIntentAction::CreateObject {
                    object_kind: IntentObjectKind::DnsServer,
                    values: fields.clone()
                },
                2
            )
            .unwrap_err()
            .message_key
            .as_deref(),
        Some("INTENT_TARGET_MISSING")
    );
    fields.insert("bootstrap".into(), json!("bootstrap"));
    state
        .update_intent(
            &profile,
            &ConfigurationIntentAction::CreateObject {
                object_kind: IntentObjectKind::DnsServer,
                values: fields,
            },
            2,
        )
        .unwrap();
    let object = objects(&profile, &state)
        .into_iter()
        .find(|object| object.label == "bootstrap")
        .unwrap();
    assert_eq!(
        state
            .update_intent(
                &profile,
                &ConfigurationIntentAction::RemoveObject {
                    object_id: object.object_id,
                    expected_hash: object.content_hash
                },
                3
            )
            .unwrap_err()
            .message_key
            .as_deref(),
        Some("INTENT_TARGET_IN_USE")
    );
}
