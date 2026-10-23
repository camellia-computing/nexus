//! Stable source identities, source capability declarations, and candidate validation evidence.

use std::collections::BTreeSet;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::{CamelliaNexusError, ErrorCode, ProgramKind, Result};

pub const CORE_IMPLEMENTATION_REVISION: &str = env!("NEXUS_CORE_IMPLEMENTATION_SHA256");
pub const CORE_BINARY_PROBE_REVISION: &str = CORE_IMPLEMENTATION_REVISION;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreBinaryFingerprint {
    pub sha256: String,
    pub size: u64,
    pub modified_unix_ms: u64,
}

impl CoreBinaryFingerprint {
    pub fn validate(&self) -> Result<()> {
        if !is_sha256(&self.sha256) {
            return Err(invalid_compatibility(
                "Core binary fingerprint must contain 64 lowercase hexadecimal characters",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreCliObservation {
    pub id: String,
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreProbeReport {
    pub revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normalized_version: Option<String>,
    pub prerelease: bool,
    pub has_build_metadata: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<crate::CoreBuildObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_issue: Option<crate::CoreProbeIdentityIssue>,
    #[serde(default)]
    pub cli_observations: Vec<CoreCliObservation>,
}

impl CoreProbeReport {
    pub fn from_program_output(program: ProgramKind, output: &str) -> Self {
        let version = crate::parse_program_version(program, output);
        let prerelease = version
            .as_ref()
            .is_some_and(|version| !version.pre.is_empty());
        let has_build_metadata = version
            .as_ref()
            .is_some_and(|version| !version.build.is_empty());
        let normalized_version =
            version.map(|version| format!("{}.{}.{}", version.major, version.minor, version.patch));
        let reported_version = normalized_version.as_ref().and_then(|version| {
            crate::core_probe::version_prefix(program).map(|prefix| format!("{prefix}{version}"))
        });
        let (build, reported_commit, identity_issue) =
            match crate::core_probe::parse_build_observation(program, output) {
                Ok((build, commit)) => (Some(build), commit, None),
                Err(issue) => (None, None, Some(issue)),
            };
        Self {
            revision: CORE_BINARY_PROBE_REVISION.into(),
            reported_version,
            normalized_version,
            prerelease,
            has_build_metadata,
            reported_commit,
            build,
            identity_issue,
            cli_observations: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.revision.trim().is_empty() {
            return Err(invalid_compatibility("Core probe revision cannot be empty"));
        }
        if self.build.as_ref().is_some_and(|build| !build.is_bounded()) {
            return Err(invalid_compatibility(
                "Core build observation exceeds its limits",
            ));
        }
        if self
            .reported_version
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 512)
        {
            return Err(invalid_compatibility(
                "Reported Core version must contain at most 512 non-empty bytes",
            ));
        }
        if let Some(version) = &self.normalized_version {
            let parsed = Version::parse(version).map_err(|_| {
                invalid_compatibility("Normalized Core version is not valid SemVer")
            })?;
            if !parsed.pre.is_empty() || !parsed.build.is_empty() {
                return Err(invalid_compatibility(
                    "Core version qualifiers must use structured flags",
                ));
            }
        } else if self.prerelease || self.has_build_metadata {
            return Err(invalid_compatibility(
                "Core version qualifiers require an identified version",
            ));
        }
        if self
            .reported_commit
            .as_ref()
            .is_some_and(|commit| !is_commit_sha(commit))
        {
            return Err(invalid_compatibility(
                "Reported Core commit must be an exact lowercase commit SHA",
            ));
        }
        let mut observations = BTreeSet::new();
        for observation in &self.cli_observations {
            if observation.id.trim().is_empty() || !observations.insert(&observation.id) {
                return Err(invalid_compatibility(
                    "Core CLI observations require unique non-empty ids",
                ));
            }
        }
        Ok(())
    }

    pub fn observes(&self, id: &str) -> Option<bool> {
        self.cli_observations
            .iter()
            .find(|observation| observation.id == id)
            .map(|observation| observation.available)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CoreCompatibilityBasis {
    BinaryReported,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CoreVersionCoordinate {
    Release {
        tag: String,
        normalized_version: String,
        commit_sha: String,
    },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreTargetIdentity {
    pub program: ProgramKind,
    pub coordinate: CoreVersionCoordinate,
    pub basis: CoreCompatibilityBasis,
    pub knowledge_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint_sha256: Option<String>,
}

impl CoreTargetIdentity {
    pub fn from_probe(
        program: ProgramKind,
        probe: &CoreProbeReport,
        fingerprint_sha256: Option<String>,
    ) -> Result<Self> {
        let admission = crate::assess_core_probe(program, probe)?;
        let coordinate = admission
            .baseline
            .map_or(CoreVersionCoordinate::Unknown, |release| {
                CoreVersionCoordinate::Release {
                    tag: release.tag,
                    normalized_version: release.version,
                    commit_sha: release.commit_sha,
                }
            });
        let target = Self {
            program,
            coordinate,
            basis: CoreCompatibilityBasis::BinaryReported,
            knowledge_hash: admission.knowledge_hash,
            reported_version: probe.reported_version.clone(),
            fingerprint_sha256,
        };
        target.validate()?;
        Ok(target)
    }

    pub fn unknown(program: ProgramKind, probe: Option<&CoreProbeReport>) -> Self {
        Self {
            program,
            coordinate: CoreVersionCoordinate::Unknown,
            basis: probe.map_or(CoreCompatibilityBasis::Unknown, |_| {
                CoreCompatibilityBasis::BinaryReported
            }),
            knowledge_hash: crate::embedded_core_knowledge()
                .map(|knowledge| knowledge.content_hash.clone())
                .unwrap_or_default(),
            reported_version: probe.and_then(|probe| probe.reported_version.clone()),
            fingerprint_sha256: None,
        }
    }

    /// Validates a retained observation without granting current capability authority.
    pub fn validate_observation(&self) -> Result<()> {
        if self.program == ProgramKind::Generic || !is_sha256(&self.knowledge_hash) {
            return Err(invalid_compatibility(
                "Core observations require a program kind and an exact knowledge digest",
            ));
        }
        if self
            .reported_version
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 512)
        {
            return Err(invalid_compatibility(
                "Reported version must contain at most 512 non-empty bytes",
            ));
        }
        if self
            .fingerprint_sha256
            .as_ref()
            .is_some_and(|value| !is_sha256(value))
        {
            return Err(invalid_compatibility(
                "Target fingerprint must be an exact SHA-256 digest",
            ));
        }
        if let CoreVersionCoordinate::Release {
            tag,
            normalized_version,
            commit_sha,
        } = &self.coordinate
        {
            let version = Version::parse(normalized_version)
                .map_err(|_| invalid_compatibility("Observed Core version is not valid SemVer"))?;
            if tag.is_empty()
                || tag.len() > 128
                || tag
                    .chars()
                    .any(|character| character.is_control() || character.is_whitespace())
                || !version.pre.is_empty()
                || !version.build.is_empty()
                || !is_commit_sha(commit_sha)
                || self.basis != CoreCompatibilityBasis::BinaryReported
            {
                return Err(invalid_compatibility("Core release observation is invalid"));
            }
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        self.validate_observation()?;
        let knowledge = crate::embedded_core_knowledge()?;
        let descriptor = knowledge
            .program(self.program)
            .ok_or_else(|| invalid_compatibility("Program has no source knowledge descriptor"))?;
        if self.knowledge_hash != knowledge.content_hash {
            return Err(invalid_compatibility(
                "Target knowledge digest does not match the reviewed source",
            ));
        }
        if let CoreVersionCoordinate::Release {
            tag,
            normalized_version,
            commit_sha,
        } = &self.coordinate
            && !descriptor.releases.iter().any(|release| {
                release.tag == *tag
                    && release.version == *normalized_version
                    && release.commit_sha == *commit_sha
            })
        {
            return Err(invalid_compatibility(
                "Target must bind an exact maintained stable release",
            ));
        }
        Ok(())
    }

    pub fn display_version(&self) -> String {
        self.reported_version
            .clone()
            .unwrap_or_else(|| match &self.coordinate {
                CoreVersionCoordinate::Release { tag, .. } => tag.clone(),
                CoreVersionCoordinate::Unknown => "Unrecognized program".into(),
            })
    }

    pub fn bind_fingerprint(mut self, fingerprint: &CoreBinaryFingerprint) -> Self {
        self.fingerprint_sha256 = Some(fingerprint.sha256.clone());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CoreFeatureId(String);

impl CoreFeatureId {
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.len() > 160
            || value.is_empty()
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        {
            return Err(invalid_compatibility("Core feature id is invalid"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CoreFeatureAvailability {
    SourceDeclared,
    SourceUnavailable,
    Unconfirmed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreFeatureDecision {
    pub feature_id: CoreFeatureId,
    pub availability: CoreFeatureAvailability,
    pub build_conditions: Vec<String>,
    pub evidence: Vec<crate::KnowledgeBehaviorEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreCompatibilityProfile {
    pub target: CoreTargetIdentity,
    pub profile_hash: String,
    pub decisions: Vec<CoreFeatureDecision>,
}

impl CoreCompatibilityProfile {
    pub fn resolve(target: &CoreTargetIdentity) -> Result<Self> {
        target.validate()?;
        let knowledge = crate::embedded_core_knowledge()?;
        let program = knowledge
            .program(target.program)
            .ok_or_else(|| invalid_compatibility("Program knowledge is unavailable"))?;
        let mut decisions = Vec::new();
        for (feature, registry_value) in &program.share_protocols {
            let bindings = match &target.coordinate {
                CoreVersionCoordinate::Release { tag, .. } => program
                    .decoder_bindings_for(tag, &program.outbound_collection, registry_value)
                    .collect::<Vec<_>>(),
                CoreVersionCoordinate::Unknown => Vec::new(),
            };
            let availability = if matches!(target.coordinate, CoreVersionCoordinate::Unknown) {
                CoreFeatureAvailability::Unconfirmed
            } else if bindings.iter().any(|binding| {
                binding.constructor_status == crate::KnowledgeConstructorStatus::Registered
            }) {
                CoreFeatureAvailability::SourceDeclared
            } else {
                CoreFeatureAvailability::SourceUnavailable
            };
            let mut evidence = Vec::new();
            for binding in &bindings {
                for item in &binding.evidence {
                    if !evidence.contains(item) {
                        evidence.push(item.clone());
                    }
                }
            }
            let build_conditions = bindings
                .iter()
                .filter(|binding| {
                    binding.constructor_status == crate::KnowledgeConstructorStatus::Registered
                })
                .map(|binding| binding.build_constraint.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            decisions.push(CoreFeatureDecision {
                feature_id: CoreFeatureId::parse(feature)?,
                availability,
                build_conditions,
                evidence,
            });
        }
        let profile_hash = crate::config_service::hash_bytes(&serde_json::to_vec(&(
            target,
            &decisions,
            &knowledge.content_hash,
            CORE_IMPLEMENTATION_REVISION,
        ))?);
        Ok(Self {
            target: target.clone(),
            profile_hash,
            decisions,
        })
    }

    pub fn decision(&self, id: &str) -> Option<&CoreFeatureDecision> {
        self.decisions
            .iter()
            .find(|decision| decision.feature_id.as_str() == id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreValidationEvidence {
    pub binary_sha256: String,
    pub profile_hash: String,
    pub config_hash: String,
    pub candidate_generation: u64,
    pub validator_contract_revision: String,
    pub native_accepted: bool,
    pub validated_unix_ms: u64,
}

impl CoreValidationEvidence {
    pub fn validates_candidate(
        &self,
        binary_sha256: &str,
        profile_hash: &str,
        config_hash: &str,
        generation: u64,
    ) -> bool {
        self.candidate_generation == generation
            && self.validates(binary_sha256, profile_hash, config_hash)
    }

    pub fn validates(&self, binary_sha256: &str, profile_hash: &str, config_hash: &str) -> bool {
        self.native_accepted
            && self.validator_contract_revision == CORE_IMPLEMENTATION_REVISION
            && self.binary_sha256 == binary_sha256
            && self.profile_hash == profile_hash
            && self.config_hash == config_hash
    }
}

fn is_commit_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}
fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}
fn invalid_compatibility(message: impl Into<String>) -> CamelliaNexusError {
    CamelliaNexusError::new(ErrorCode::InvalidSpec, message)
        .with_message_key("CORE_IDENTITY_INVALID")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_reports_keep_baseline_and_channel_without_native_text() {
        let knowledge = crate::embedded_core_knowledge().unwrap();
        for descriptor in &knowledge.programs {
            let baseline = descriptor.releases.last().unwrap();
            let prefix = crate::core_probe::version_prefix(descriptor.program).unwrap();
            for (suffix, prerelease, has_build_metadata, expected) in [
                ("", false, false, crate::CoreAdmissionStatus::Admitted),
                (
                    "+fixture-secret",
                    false,
                    true,
                    crate::CoreAdmissionStatus::Admitted,
                ),
                (
                    "-fixture-secret",
                    true,
                    false,
                    crate::CoreAdmissionStatus::Prerelease,
                ),
            ] {
                let output = format!(
                    "https://private-user:fixture-secret@private.example\n{prefix}{}{suffix} fixture-secret\nfixture-secret",
                    baseline.version
                );
                let probe = CoreProbeReport::from_program_output(descriptor.program, &output);
                let report = crate::assess_core_probe(descriptor.program, &probe).unwrap();
                assert_eq!(report.status, expected);
                assert_eq!(probe.normalized_version, Some(baseline.version.clone()));
                assert_eq!(probe.prerelease, prerelease);
                assert_eq!(probe.has_build_metadata, has_build_metadata);
                let target =
                    CoreTargetIdentity::from_probe(descriptor.program, &probe, None).unwrap();
                for encoded in [
                    serde_json::to_string(&probe).unwrap(),
                    serde_json::to_string(&target).unwrap(),
                    target.display_version(),
                ] {
                    for private_value in ["fixture-secret", "private-user", "private.example"] {
                        assert!(!encoded.contains(private_value));
                    }
                }
            }
            let unrecognized = CoreProbeReport::from_program_output(
                descriptor.program,
                &format!("{prefix}fixture-secret"),
            );
            assert!(unrecognized.reported_version.is_none());
            assert!(unrecognized.normalized_version.is_none());
            assert_eq!(
                crate::assess_core_probe(descriptor.program, &unrecognized)
                    .unwrap()
                    .status,
                crate::CoreAdmissionStatus::Unrecognized
            );
        }
    }

    #[test]
    fn each_maintained_release_resolves_exactly_without_claiming_origin_or_build_capability() {
        let knowledge = crate::embedded_core_knowledge().unwrap();
        for descriptor in &knowledge.programs {
            for baseline in &descriptor.releases {
                let prefix = match descriptor.program {
                    ProgramKind::SingBox => "sing-box version",
                    ProgramKind::Xray => "Xray",
                    ProgramKind::Mihomo => "Mihomo Meta",
                    ProgramKind::Generic => unreachable!(),
                };
                let probe = CoreProbeReport::from_program_output(
                    descriptor.program,
                    &format!("{prefix} {}", baseline.version),
                );
                let target = CoreTargetIdentity::from_probe(
                    descriptor.program,
                    &probe,
                    Some("a".repeat(64)),
                )
                .unwrap();
                assert_eq!(target.basis, CoreCompatibilityBasis::BinaryReported);
                assert_eq!(target.knowledge_hash, knowledge.content_hash);
                assert!(
                    matches!(&target.coordinate, CoreVersionCoordinate::Release { tag, commit_sha, .. }
                    if *tag == baseline.tag && *commit_sha == baseline.commit_sha)
                );
                let profile = CoreCompatibilityProfile::resolve(&target).unwrap();
                assert!(profile.decisions.iter().any(
                    |decision| decision.availability == CoreFeatureAvailability::SourceDeclared
                ));
                assert!(
                    profile
                        .decisions
                        .iter()
                        .all(|decision| decision.availability
                            != CoreFeatureAvailability::SourceDeclared
                            || !decision.evidence.is_empty())
                );
            }
        }
    }

    #[test]
    fn unrecognized_identity_has_no_source_capability_claim_and_cannot_be_forged() {
        let target = CoreTargetIdentity::unknown(ProgramKind::Xray, None);
        let profile = CoreCompatibilityProfile::resolve(&target).unwrap();
        assert!(
            profile
                .decisions
                .iter()
                .all(
                    |decision| decision.availability == CoreFeatureAvailability::Unconfirmed
                        && decision.evidence.is_empty()
                )
        );
        let mut forged = target.clone();
        forged.coordinate = CoreVersionCoordinate::Release {
            tag: "v999.0.0".into(),
            normalized_version: "999.0.0".into(),
            commit_sha: "a".repeat(40),
        };
        assert!(forged.validate().is_err());
        let mut stale = target;
        stale.knowledge_hash = "0".repeat(64);
        assert!(stale.validate_observation().is_ok());
        assert!(stale.validate().is_err());
    }

    #[test]
    fn retained_observations_are_bounded_and_never_grant_current_authority() {
        let knowledge = crate::embedded_core_knowledge().unwrap();
        let baseline = knowledge
            .program(ProgramKind::Xray)
            .unwrap()
            .releases
            .last()
            .unwrap();
        let probe = CoreProbeReport::from_program_output(
            ProgramKind::Xray,
            &format!("Xray {}", baseline.version),
        );
        let mut target =
            CoreTargetIdentity::from_probe(ProgramKind::Xray, &probe, Some("a".repeat(64)))
                .unwrap();
        target.knowledge_hash = "0".repeat(64);
        assert!(target.validate_observation().is_ok());
        assert!(CoreCompatibilityProfile::resolve(&target).is_err());
        let mut invalid = target.clone();
        invalid.knowledge_hash = "not-a-digest".into();
        assert!(invalid.validate_observation().is_err());
        for coordinate in [
            CoreVersionCoordinate::Release {
                tag: "v1.0.0".into(),
                normalized_version: "1.0.0-beta".into(),
                commit_sha: "a".repeat(40),
            },
            CoreVersionCoordinate::Release {
                tag: "v1.0.0".into(),
                normalized_version: "1.0.0".into(),
                commit_sha: "abc123".into(),
            },
            CoreVersionCoordinate::Release {
                tag: "v1.0.0\n".into(),
                normalized_version: "1.0.0".into(),
                commit_sha: "a".repeat(40),
            },
        ] {
            invalid = target.clone();
            invalid.coordinate = coordinate;
            assert!(invalid.validate_observation().is_err());
        }
    }

    #[test]
    fn native_acceptance_is_bound_to_exact_binary_profile_and_candidate() {
        let evidence = CoreValidationEvidence {
            binary_sha256: "a".repeat(64),
            profile_hash: "b".repeat(64),
            config_hash: "c".repeat(64),
            candidate_generation: 7,
            validator_contract_revision: CORE_IMPLEMENTATION_REVISION.into(),
            native_accepted: true,
            validated_unix_ms: 1,
        };
        assert!(evidence.validates(&"a".repeat(64), &"b".repeat(64), &"c".repeat(64)));
        assert!(evidence.validates_candidate(&"a".repeat(64), &"b".repeat(64), &"c".repeat(64), 7));
        assert!(!evidence.validates_candidate(
            &"a".repeat(64),
            &"b".repeat(64),
            &"c".repeat(64),
            8
        ));
        assert!(!evidence.validates(&"d".repeat(64), &"b".repeat(64), &"c".repeat(64)));
        assert!(!evidence.validates(&"a".repeat(64), &"d".repeat(64), &"c".repeat(64)));
        assert!(!evidence.validates(&"a".repeat(64), &"b".repeat(64), &"d".repeat(64)));
        let mut changed_implementation = evidence.clone();
        changed_implementation.validator_contract_revision = "0".repeat(64);
        assert!(!changed_implementation.validates(
            &"a".repeat(64),
            &"b".repeat(64),
            &"c".repeat(64)
        ));
        let mut rejected = evidence;
        rejected.native_accepted = false;
        assert!(!rejected.validates(&"a".repeat(64), &"b".repeat(64), &"c".repeat(64)));
    }
}
