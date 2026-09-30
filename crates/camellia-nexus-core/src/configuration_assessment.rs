//! Candidate-only checks backed by reviewed source behavior.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    CamelliaNexusError, CoreBinaryFingerprint, CoreProbeReport, ErrorCode,
    KnowledgeBehaviorEvidence, KnowledgeConstructorStatus, KnowledgeDecoderBinding,
    KnowledgeValueConstraint, ProgramKind, ProgramKnowledgeDescriptor, Result, assess_core_probe,
    config_service::hash_bytes,
    core_build_constraints::{BuildCondition, evaluate_build_condition},
    embedded_core_knowledge,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreCapabilityProfile {
    pub program: ProgramKind,
    pub baseline_tag: String,
    pub binary_sha256: String,
    pub build: Option<crate::CoreBuildObservation>,
    pub knowledge_hash: String,
    pub implementation_hash: String,
    pub profile_hash: String,
    pub rule_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "evidence",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ConfigurationAssessmentEvidence {
    SourceBehavior(KnowledgeBehaviorEvidence),
    SourceDeclaration {
        source: crate::KnowledgeSourceReference,
        declaration_hash: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationAssessmentIssue {
    pub code: String,
    pub message_key: String,
    pub path: Vec<String>,
    pub rule_id: String,
    pub evidence: ConfigurationAssessmentEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationAssessment {
    pub profile_hash: String,
    pub config_hash: String,
    pub issues: Vec<ConfigurationAssessmentIssue>,
}

pub fn assess_program_configuration(
    spec: &crate::ProgramSpec,
    content: &str,
) -> Result<Option<ConfigurationAssessment>> {
    let kind = spec.program_type.kind();
    let Some(format) = crate::ConfigurationFormat::for_kind(kind) else {
        return Ok(None);
    };
    let metadata = spec.executable.metadata().ok_or_else(missing_identity)?;
    let probe = metadata.probe.as_ref().ok_or_else(missing_identity)?;
    let profile = CoreCapabilityProfile::resolve(kind, probe, &metadata.fingerprint)?;
    let document = crate::parse_semantic_document(format, content.as_bytes())?;
    profile.assess(&document).map(Some)
}

fn missing_identity() -> CamelliaNexusError {
    CamelliaNexusError::new(
        ErrorCode::InvalidState,
        "Configuration assessment requires binary identity",
    )
    .with_message_key("CORE_VERSION_UNRECOGNIZED")
}

impl CoreCapabilityProfile {
    pub fn resolve(
        program: ProgramKind,
        probe: &CoreProbeReport,
        fingerprint: &CoreBinaryFingerprint,
    ) -> Result<Self> {
        fingerprint.validate()?;
        let admission = assess_core_probe(program, probe)?;
        let release = admission.require_admitted()?;
        let knowledge = embedded_core_knowledge()?;
        let descriptor = knowledge.program(program).ok_or_else(stale_profile)?;
        let rule_ids = descriptor
            .semantic_rules
            .iter()
            .filter(|variant| variant.releases.contains(&release.tag))
            .map(|variant| variant.rule.id.clone())
            .collect();
        let mut profile = Self {
            program,
            baseline_tag: release.tag.clone(),
            binary_sha256: fingerprint.sha256.clone(),
            build: probe.build.clone(),
            knowledge_hash: knowledge.content_hash.clone(),
            implementation_hash: crate::CORE_IMPLEMENTATION_REVISION.into(),
            profile_hash: String::new(),
            rule_ids,
        };
        profile.profile_hash = profile.compute_hash()?;
        Ok(profile)
    }

    fn compute_hash(&self) -> Result<String> {
        Ok(hash_bytes(&serde_json::to_vec(&(
            self.program,
            &self.baseline_tag,
            &self.binary_sha256,
            &self.build,
            &self.knowledge_hash,
            &self.implementation_hash,
            &self.rule_ids,
        ))?))
    }

    pub fn assess(&self, document: &Value) -> Result<ConfigurationAssessment> {
        let knowledge = embedded_core_knowledge()?;
        if self.knowledge_hash != knowledge.content_hash
            || self.implementation_hash != crate::CORE_IMPLEMENTATION_REVISION
            || self.profile_hash != self.compute_hash()?
        {
            return Err(stale_profile());
        }
        let descriptor = knowledge.program(self.program).ok_or_else(stale_profile)?;
        if !descriptor
            .releases
            .iter()
            .any(|release| release.tag == self.baseline_tag)
        {
            return Err(stale_profile());
        }
        let reported_tags = descriptor.reported_build_tags_for(&self.baseline_tag);
        let rules: Vec<_> = descriptor
            .semantic_rules
            .iter()
            .filter(|variant| variant.releases.contains(&self.baseline_tag))
            .map(|variant| &variant.rule)
            .collect();
        if rules.iter().map(|rule| &rule.id).ne(self.rule_ids.iter()) {
            return Err(stale_profile());
        }
        let mut issues = Vec::new();
        let mut visits = 0usize;
        self.assess_decoder_builds(descriptor, document, &mut issues, &mut visits)?;
        for rule in rules {
            let mut selected = Vec::new();
            select_paths(
                document,
                &rule.path,
                &mut Vec::new(),
                &mut selected,
                &mut visits,
                crate::ConfigurationFormat::for_kind(self.program)
                    == Some(crate::ConfigurationFormat::Jsonc),
            )?;
            for (path, value) in selected {
                let case_insensitive = self.program == ProgramKind::Xray;
                let parent = value_at_path(document, &path[..path.len().saturating_sub(1)]);
                let sibling_value = |name: &str| {
                    parent.and_then(|parent| {
                        object_fields(parent, name, case_insensitive)
                            .next()
                            .map(|(_, value)| value)
                    })
                };
                if !rule
                    .when
                    .iter()
                    .all(|predicate| match sibling_value(&predicate.sibling) {
                        Some(value) => predicate.values.contains(value),
                        None => predicate.allow_missing,
                    })
                {
                    continue;
                }
                let evidence = rule.evidence.first().ok_or_else(stale_profile)?;
                if let KnowledgeValueConstraint::Platform { activation, .. } = &rule.constraint
                    && !activation.is_active(value)
                {
                    continue;
                }
                if let KnowledgeValueConstraint::Requires { when_value, .. } = &rule.constraint
                    && value != when_value
                {
                    continue;
                }
                match evaluate_build_condition(
                    self.program,
                    self.build.as_ref(),
                    &rule.build_constraint,
                    &reported_tags,
                ) {
                    BuildCondition::Excluded => continue,
                    BuildCondition::Unconfirmed => {
                        push_issue(
                            &mut issues,
                            "CORE_BUILD_CAPABILITY_UNCONFIRMED",
                            path,
                            rule.id.clone(),
                            evidence,
                        )?;
                        continue;
                    }
                    BuildCondition::Satisfied => {}
                }
                let code = match &rule.constraint {
                    KnowledgeValueConstraint::Enum {
                        values,
                        allow_null,
                        case_insensitive,
                    } => {
                        let invalid = !(value.is_null() && *allow_null)
                            && !value.as_str().is_some_and(|value| {
                                values.iter().any(|allowed| {
                                    allowed == value
                                        || (*case_insensitive
                                            && allowed.eq_ignore_ascii_case(value))
                                })
                            });
                        invalid.then_some("CONFIGURATION_VALUE_NOT_ALLOWED")
                    }
                    KnowledgeValueConstraint::Platform { condition, .. } => {
                        match evaluate_build_condition(
                            self.program,
                            self.build.as_ref(),
                            condition,
                            &reported_tags,
                        ) {
                            BuildCondition::Satisfied => None,
                            BuildCondition::Excluded => Some("CONFIGURATION_PLATFORM_UNSUPPORTED"),
                            BuildCondition::Unconfirmed => {
                                Some("CORE_BUILD_CAPABILITY_UNCONFIRMED")
                            }
                        }
                    }
                    KnowledgeValueConstraint::Requires {
                        when_value,
                        sibling,
                        value: required,
                    } => (value == when_value && sibling_value(sibling) != Some(required))
                        .then_some("CONFIGURATION_SETTING_REQUIRED"),
                };
                if let Some(code) = code {
                    push_issue(&mut issues, code, path, rule.id.clone(), evidence)?;
                    if code != "CORE_BUILD_CAPABILITY_UNCONFIRMED"
                        && let Some(issue) = issues.last_mut()
                    {
                        issue.message_key = rule.message_key.clone();
                    }
                }
            }
        }
        Ok(ConfigurationAssessment {
            profile_hash: self.profile_hash.clone(),
            config_hash: hash_bytes(&serde_json::to_vec(document)?),
            issues,
        })
    }

    fn assess_decoder_builds(
        &self,
        descriptor: &ProgramKnowledgeDescriptor,
        document: &Value,
        issues: &mut Vec<ConfigurationAssessmentIssue>,
        visits: &mut usize,
    ) -> Result<()> {
        let reported_tags = descriptor.reported_build_tags_for(&self.baseline_tag);
        // Select each collection once; a binding is relevant only when the candidate uses it.
        let mut groups: BTreeMap<(&[String], &str), Vec<&KnowledgeDecoderBinding>> =
            BTreeMap::new();
        for variant in &descriptor.decoder_bindings {
            if variant.releases.contains(&self.baseline_tag) {
                let binding = &variant.binding;
                groups
                    .entry((&binding.path, &binding.discriminator))
                    .or_default()
                    .push(binding);
            }
        }
        for ((pattern, discriminator), bindings) in groups {
            let mut selected = Vec::new();
            let case_insensitive = crate::ConfigurationFormat::for_kind(self.program)
                == Some(crate::ConfigurationFormat::Jsonc);
            select_paths(
                document,
                pattern,
                &mut Vec::new(),
                &mut selected,
                visits,
                case_insensitive,
            )?;
            let selected = selected.iter().flat_map(|(path, value)| {
                object_fields(value, discriminator, case_insensitive).filter_map(
                    move |(name, value)| value.as_str().map(|kind| (path.clone(), name, kind)),
                )
            });
            for (mut path, discriminator_name, kind) in selected {
                let matching: Vec<_> = bindings
                    .iter()
                    .copied()
                    .filter(|binding| binding.matches_discriminator(kind))
                    .collect();
                if matching.is_empty() {
                    continue;
                }
                let conditions: Vec<_> = matching
                    .iter()
                    .map(|binding| {
                        (
                            binding,
                            evaluate_build_condition(
                                self.program,
                                self.build.as_ref(),
                                &binding.build_constraint,
                                &reported_tags,
                            ),
                        )
                    })
                    .collect();
                let usable = conditions.iter().any(|(binding, condition)| {
                    binding.constructor_status == KnowledgeConstructorStatus::Registered
                        && *condition == BuildCondition::Satisfied
                });
                let rejected = conditions.iter().any(|(binding, condition)| {
                    binding.constructor_status == KnowledgeConstructorStatus::Rejected
                        && *condition == BuildCondition::Satisfied
                });
                let uncertain = conditions
                    .iter()
                    .any(|(_, condition)| *condition == BuildCondition::Unconfirmed);
                let code = if usable && !rejected && !uncertain {
                    continue;
                } else if !usable && (rejected || !uncertain) {
                    "CORE_BUILD_CAPABILITY_UNAVAILABLE"
                } else {
                    "CORE_BUILD_CAPABILITY_UNCONFIRMED"
                };
                let binding = conditions
                    .iter()
                    .find(|(binding, condition)| {
                        binding.constructor_status == KnowledgeConstructorStatus::Rejected
                            && *condition == BuildCondition::Satisfied
                    })
                    .map(|(binding, _)| *binding)
                    .unwrap_or(&matching[0]);
                let evidence = binding.evidence.last().ok_or_else(stale_profile)?;
                path.push(discriminator_name.to_owned());
                push_issue(
                    issues,
                    code,
                    path,
                    format!("decoder:{}/{}", pattern.join("/"), binding.value),
                    evidence,
                )?;
            }
        }
        Ok(())
    }
}

fn value_at_path<'a>(mut value: &'a Value, path: &[String]) -> Option<&'a Value> {
    for segment in path {
        value = if let Some(array) = value.as_array() {
            array.get(segment.parse::<usize>().ok()?)?
        } else {
            value.get(segment)?
        };
    }
    Some(value)
}

fn push_issue(
    issues: &mut Vec<ConfigurationAssessmentIssue>,
    code: &str,
    path: Vec<String>,
    rule_id: String,
    evidence: &KnowledgeBehaviorEvidence,
) -> Result<()> {
    if issues.len() >= 128 {
        return Err(assessment_limit());
    }
    issues.push(ConfigurationAssessmentIssue {
        code: code.into(),
        message_key: code.into(),
        path,
        rule_id,
        evidence: ConfigurationAssessmentEvidence::SourceBehavior(evidence.clone()),
    });
    Ok(())
}

pub(crate) fn select_paths<'a>(
    value: &'a Value,
    pattern: &[String],
    path: &mut Vec<String>,
    selected: &mut Vec<(Vec<String>, &'a Value)>,
    visits: &mut usize,
    case_insensitive: bool,
) -> Result<()> {
    *visits += 1;
    if *visits > 32_768 {
        return Err(assessment_limit());
    }
    let Some((segment, rest)) = pattern.split_first() else {
        selected.push((path.clone(), value));
        return Ok(());
    };
    if segment == "*" {
        if let Some(items) = value.as_array() {
            for (index, item) in items.iter().enumerate() {
                path.push(index.to_string());
                select_paths(item, rest, path, selected, visits, case_insensitive)?;
                path.pop();
            }
        }
    } else {
        for (name, item) in object_fields(value, segment, case_insensitive) {
            path.push(name.to_owned());
            select_paths(item, rest, path, selected, visits, case_insensitive)?;
            path.pop();
        }
    }
    Ok(())
}

pub(crate) fn object_fields<'a>(
    value: &'a Value,
    key: &str,
    case_insensitive: bool,
) -> impl Iterator<Item = (&'a str, &'a Value)> {
    value
        .as_object()
        .into_iter()
        .flat_map(|object| object.iter())
        .filter(move |(name, _)| {
            name.as_str() == key || (case_insensitive && name.eq_ignore_ascii_case(key))
        })
        .map(|(name, value)| (name.as_str(), value))
}

fn assessment_limit() -> CamelliaNexusError {
    CamelliaNexusError::new(
        ErrorCode::ConfigInvalid,
        "Configuration assessment limit exceeded",
    )
    .with_message_key("CONFIGURATION_ASSESSMENT_LIMIT")
}

fn stale_profile() -> CamelliaNexusError {
    CamelliaNexusError::new(
        ErrorCode::ConfigInvalid,
        "Configuration capability profile is stale",
    )
    .with_message_key("CORE_PROFILE_MISMATCH")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn platform_profile(program: ProgramKind, version: &str, os: &str) -> CoreCapabilityProfile {
        let output = match program {
            ProgramKind::SingBox => format!(
                "sing-box version {version}\nEnvironment: go1.26.0 {os}/amd64\nTags: with_gvisor,with_quic"
            ),
            ProgramKind::Mihomo => {
                format!("Mihomo Meta v{version} {os} amd64 with go1.26.0\nUse tags: with_gvisor")
            }
            ProgramKind::Xray => format!(
                "Xray {version} (Xray, Penetrates Everything.) Custom (go1.26.0 {os}/amd64)"
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

    #[test]
    fn platform_rules_cover_each_maintained_patch_without_claiming_disabled_features() {
        let knowledge = embedded_core_knowledge().unwrap();
        for program in [ProgramKind::SingBox, ProgramKind::Mihomo, ProgramKind::Xray] {
            let descriptor = knowledge.program(program).unwrap();
            for release in &descriptor.releases {
                let (candidate, inactive) = match program {
                    ProgramKind::SingBox => (
                        json!({"inbounds":[{"type":"tun","auto_route":true,"auto_redirect":true}]}),
                        json!({"inbounds":[{"type":"tun","auto_redirect":false}]}),
                    ),
                    ProgramKind::Mihomo => (json!({"tproxy-port":12345}), json!({"tproxy-port":0})),
                    ProgramKind::Xray => (
                        json!({"outbounds":[{"streamSettings":{"sockopt":{"mark":123}}}]}),
                        json!({"outbounds":[{"streamSettings":{"sockopt":{"mark":0}}}]}),
                    ),
                    _ => unreachable!(),
                };
                let windows = platform_profile(program, &release.version, "windows");
                let issues = windows.assess(&candidate).unwrap().issues;
                assert_eq!(issues.len(), 1, "{program:?} {}", release.tag);
                assert_eq!(issues[0].code, "CONFIGURATION_PLATFORM_UNSUPPORTED");
                assert!(windows.assess(&inactive).unwrap().issues.is_empty());
                assert!(
                    platform_profile(program, &release.version, "linux")
                        .assess(&candidate)
                        .unwrap()
                        .issues
                        .is_empty()
                );
            }
        }
    }

    #[test]
    fn redirect_checks_require_an_enabled_tun_and_preserve_unknown_platform_evidence() {
        let knowledge = embedded_core_knowledge().unwrap();
        for program in [ProgramKind::SingBox, ProgramKind::Mihomo] {
            let descriptor = knowledge.program(program).unwrap();
            for release in &descriptor.releases {
                let declared = descriptor.semantic_rules.iter().any(|variant| {
                    variant.releases.contains(&release.tag)
                        && variant.rule.id.ends_with("auto-redirect.platform")
                });
                if !declared {
                    continue;
                }
                let candidate = if program == ProgramKind::SingBox {
                    json!({"inbounds":[{"type":"tun","auto_route":false,"auto_redirect":true}]})
                } else {
                    json!({"tun":{"enable":true,"auto-route":false,"auto-redirect":true}})
                };
                let linux = platform_profile(program, &release.version, "linux");
                let issues = linux.assess(&candidate).unwrap().issues;
                assert_eq!(issues.len(), 1, "{}", release.tag);
                assert_eq!(issues[0].message_key, "CONFIGURATION_AUTO_ROUTE_REQUIRED");
                let windows = platform_profile(program, &release.version, "windows");
                let issues = windows.assess(&candidate).unwrap().issues;
                assert_eq!(issues.len(), 1);
                assert_eq!(issues[0].message_key, "CONFIGURATION_PLATFORM_UNSUPPORTED");
                let inactive = if program == ProgramKind::SingBox {
                    json!({"inbounds":[{"type":"direct","auto_redirect":true}]})
                } else {
                    json!({"tun":{"enable":false,"auto-redirect":true}})
                };
                assert!(windows.assess(&inactive).unwrap().issues.is_empty());
                let mut unknown = windows.clone();
                unknown.build.as_mut().unwrap().operating_system = None;
                unknown.profile_hash = unknown.compute_hash().unwrap();
                let unknown_issues = unknown.assess(&candidate).unwrap().issues;
                assert_eq!(unknown_issues[0].code, "CORE_BUILD_CAPABILITY_UNCONFIRMED");
                assert_eq!(
                    unknown_issues[0].message_key,
                    "CORE_BUILD_CAPABILITY_UNCONFIRMED"
                );
                let disabled = if program == ProgramKind::SingBox {
                    json!({"inbounds":[{"type":"tun","auto_redirect":false}]})
                } else {
                    json!({"tun":{"enable":true,"auto-redirect":false}})
                };
                assert!(unknown.assess(&disabled).unwrap().issues.is_empty());
            }
        }
    }

    #[test]
    fn xray_socket_rules_distinguish_freebsd_support_and_inactive_numbers() {
        let knowledge = embedded_core_knowledge().unwrap();
        for release in &knowledge.program(ProgramKind::Xray).unwrap().releases {
            let candidate = json!({"inbounds":[{"streamSettings":{"sockopt":{
                "mark":1,"tcpMaxSeg":1200,"tcpCongestion":"bbr","tcpUserTimeout":1000,"tcpWindowClamp":65535}}}]});
            assert_eq!(
                platform_profile(ProgramKind::Xray, &release.version, "windows")
                    .assess(&candidate)
                    .unwrap()
                    .issues
                    .len(),
                5
            );
            assert_eq!(
                platform_profile(ProgramKind::Xray, &release.version, "freebsd")
                    .assess(&candidate)
                    .unwrap()
                    .issues
                    .len(),
                4
            );
            assert!(
                platform_profile(ProgramKind::Xray, &release.version, "linux")
                    .assess(&candidate)
                    .unwrap()
                    .issues
                    .is_empty()
            );
            let inactive = json!({"inbounds":[{"streamSettings":{"sockopt":{
                "mark":0,"tcpMaxSeg":-1,"tcpCongestion":"","tcpUserTimeout":0,"tcpWindowClamp":-1}}}]});
            assert!(
                platform_profile(ProgramKind::Xray, &release.version, "windows")
                    .assess(&inactive)
                    .unwrap()
                    .issues
                    .is_empty()
            );
        }
    }

    fn profile() -> CoreCapabilityProfile {
        let knowledge = embedded_core_knowledge().unwrap();
        let baseline = knowledge
            .program(ProgramKind::Xray)
            .unwrap()
            .releases
            .last()
            .unwrap();
        CoreCapabilityProfile::resolve(
            ProgramKind::Xray,
            &CoreProbeReport::from_program_output(
                ProgramKind::Xray,
                &format!("Xray {} (Custom)", baseline.version),
            ),
            &CoreBinaryFingerprint {
                sha256: "a".repeat(64),
                size: 1,
                modified_unix_ms: 1,
            },
        )
        .unwrap()
    }

    fn build_profile(program: ProgramKind, suffix: &str) -> CoreCapabilityProfile {
        let knowledge = embedded_core_knowledge().unwrap();
        let version = &knowledge
            .program(program)
            .unwrap()
            .releases
            .last()
            .unwrap()
            .version;
        let header = match program {
            ProgramKind::SingBox => format!("sing-box version {version}"),
            ProgramKind::Mihomo => format!("Mihomo Meta v{version}"),
            _ => unreachable!(),
        };
        CoreCapabilityProfile::resolve(
            program,
            &CoreProbeReport::from_program_output(program, &format!("{header}\n{suffix}")),
            &CoreBinaryFingerprint {
                sha256: "b".repeat(64),
                size: 1,
                modified_unix_ms: 1,
            },
        )
        .unwrap()
    }

    #[test]
    fn only_configured_build_features_are_checked_and_unknown_is_not_absent() {
        let candidate = json!({"outbounds":[{"type":"tuic","tag":"private-user-label"}]});
        let usable = build_profile(ProgramKind::SingBox, "Tags: with_quic");
        assert!(usable.assess(&candidate).unwrap().issues.is_empty());
        let excluded = build_profile(ProgramKind::SingBox, "Tags: with_gvisor");
        let issues = excluded.assess(&candidate).unwrap().issues;
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, "CORE_BUILD_CAPABILITY_UNAVAILABLE");
        assert_eq!(issues[0].path, ["outbounds", "0", "type"]);
        assert!(
            !serde_json::to_string(&issues)
                .unwrap()
                .contains("private-user-label")
        );
        assert!(
            excluded
                .assess(&json!({"outbounds":[{"type":"direct"}]}))
                .unwrap()
                .issues
                .is_empty()
        );
        let unknown = build_profile(ProgramKind::SingBox, "");
        assert_eq!(
            unknown.assess(&candidate).unwrap().issues[0].code,
            "CORE_BUILD_CAPABILITY_UNCONFIRMED"
        );
        assert_ne!(usable.profile_hash, excluded.profile_hash);
        assert_ne!(excluded.profile_hash, unknown.profile_hash);
    }

    #[test]
    fn rejected_constructor_never_becomes_usable_from_the_version_or_tag_name() {
        let profile = build_profile(ProgramKind::SingBox, "Tags: with_wireguard");
        let issues = profile
            .assess(&json!({"outbounds":[{"type":"wireguard"}]}))
            .unwrap()
            .issues;
        assert_eq!(issues[0].code, "CORE_BUILD_CAPABILITY_UNAVAILABLE");

        let disabled = build_profile(ProgramKind::Mihomo, "Use tags: no_tailscale");
        let issues = disabled
            .assess(&json!({"proxies":[{"type":"tailscale","name":"test"}]}))
            .unwrap()
            .issues;
        assert_eq!(issues[0].code, "CORE_BUILD_CAPABILITY_UNAVAILABLE");
        assert!(matches!(
            &issues[0].evidence,
            ConfigurationAssessmentEvidence::SourceBehavior(evidence)
                if evidence.source.path.ends_with("tailscale_stub.go")
        ));
        let enabled = build_profile(ProgramKind::Mihomo, "Use tags: with_gvisor");
        assert!(
            enabled
                .assess(&json!({"proxies":[{"type":"tailscale"}]}))
                .unwrap()
                .issues
                .is_empty()
        );
        let incomplete = build_profile(ProgramKind::Mihomo, "");
        assert_eq!(
            incomplete
                .assess(&json!({"proxies":[{"type":"tailscale"}]}))
                .unwrap()
                .issues[0]
                .code,
            "CORE_BUILD_CAPABILITY_UNCONFIRMED"
        );
    }

    #[test]
    fn reviewed_enum_distinguishes_missing_null_empty_and_invalid_values() {
        let profile = profile();
        for value in [
            json!(null),
            json!(""),
            json!("reject"),
            json!("allow"),
            json!("skip"),
        ] {
            let document = json!({"outbounds": [{"mux": {"xudpProxyUDP443": value}}]});
            assert!(profile.assess(&document).unwrap().issues.is_empty());
        }
        assert!(
            profile
                .assess(&json!({"outbounds": [{"mux": {}}]}))
                .unwrap()
                .issues
                .is_empty()
        );
        for value in [
            json!("secret-invalid-value"),
            json!(true),
            json!(23),
            json!([]),
            json!({}),
        ] {
            let document = json!({"outbounds": [{"mux": {"xudpProxyUDP443": value}}]});
            let assessment = profile.assess(&document).unwrap();
            assert_eq!(assessment.issues.len(), 1);
            assert_eq!(
                assessment.issues[0].path,
                ["outbounds", "0", "mux", "xudpProxyUDP443"]
            );
            assert!(
                !serde_json::to_string(&assessment)
                    .unwrap()
                    .contains("secret-invalid-value")
            );
        }
    }

    #[test]
    fn profiles_are_bound_to_binary_and_knowledge_and_cannot_drop_rules() {
        let original = profile();
        let mut changed = original.clone();
        changed.binary_sha256 = "b".repeat(64);
        assert!(changed.assess(&json!({})).is_err());
        changed = original.clone();
        changed.knowledge_hash = "c".repeat(64);
        assert!(changed.assess(&json!({})).is_err());
        changed = original;
        changed.rule_ids.clear();
        changed.profile_hash = changed.compute_hash().unwrap();
        assert!(changed.assess(&json!({})).is_err());
    }

    #[test]
    fn json_path_case_variants_keep_semantic_and_build_checks_active() {
        let issues = profile()
            .assess(&json!({"OUTBOUNDS": [{"MUX": {"XUDPPROXYUDP443": true}}]}))
            .unwrap()
            .issues;
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, ["OUTBOUNDS", "0", "MUX", "XUDPPROXYUDP443"]);
        let excluded = build_profile(ProgramKind::SingBox, "Tags: with_gvisor");
        let issues = excluded
            .assess(&json!({"OUTBOUNDS": [{"TYPE": "tuic"}]}))
            .unwrap()
            .issues;
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, ["OUTBOUNDS", "0", "TYPE"]);
        assert_eq!(issues[0].code, "CORE_BUILD_CAPABILITY_UNAVAILABLE");
    }

    #[test]
    fn excessive_issue_count_fails_without_unbounded_output() {
        let document = json!({"outbounds": vec![json!({"mux": {"xudpProxyUDP443": false}}); 129]});
        let error = profile().assess(&document).unwrap_err();
        assert_eq!(
            error.message_key.as_deref(),
            Some("CONFIGURATION_ASSESSMENT_LIMIT")
        );
    }
}
