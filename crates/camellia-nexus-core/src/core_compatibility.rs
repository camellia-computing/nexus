//! Offline Core release history and version-scoped feature decisions.
//!
//! Binary origin, a reported compatibility version, the reviewed upstream
//! catalog, and candidate-specific native validation are deliberately separate
//! evidence layers.  An unknown or catalog-mismatched build remains usable;
//! its feature decisions are `Unknown` and must be verified against the exact
//! binary before apply.

use std::{collections::BTreeSet, sync::OnceLock};

use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{CamelliaNexusError, ErrorCode, ProgramKind, Result};

pub const CORE_COMPATIBILITY_CATALOG_SCHEMA_VERSION: u32 = 1;
pub const CORE_COMPATIBILITY_EXTRACTOR_REVISION: &str = "core-history-v1-20260811";
pub const CORE_BINARY_PROBE_REVISION: &str = "core-binary-probe-v2-20260811";

const EMBEDDED_CATALOG: &str = include_str!("../core-compatibility-catalog.json");
static CATALOG: OnceLock<std::result::Result<CoreCompatibilityCatalog, String>> = OnceLock::new();

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_commit: Option<String>,
    #[serde(default)]
    pub cli_observations: Vec<CoreCliObservation>,
}

impl CoreProbeReport {
    pub fn from_reported_version(value: Option<String>) -> Self {
        let reported_version = value.filter(|value| !value.trim().is_empty());
        let normalized_version = reported_version
            .as_deref()
            .and_then(crate::parse_reported_core_version)
            .map(|version| version.to_string());
        Self {
            revision: CORE_BINARY_PROBE_REVISION.into(),
            reported_version,
            normalized_version,
            reported_commit: None,
            cli_observations: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.revision.trim().is_empty() {
            return Err(invalid_compatibility("Core probe revision cannot be empty"));
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
            Version::parse(version).map_err(|error| {
                invalid_compatibility("Normalized Core version is not valid SemVer")
                    .with_details(error.to_string())
            })?;
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(
    tag = "mode",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CoreCompatibilityPreference {
    #[default]
    Automatic,
    Release {
        tag: String,
    },
    Commit {
        commit_sha: String,
    },
    Unknown,
}

impl CoreCompatibilityPreference {
    pub fn validate_for_program(&self, program: ProgramKind) -> Result<()> {
        if program == ProgramKind::Generic {
            return if matches!(self, Self::Automatic) {
                Ok(())
            } else {
                Err(invalid_compatibility(
                    "Generic programs cannot select a Core compatibility target",
                ))
            };
        }
        let catalog = embedded_core_compatibility_catalog()?;
        let catalog_program = catalog.program(program).ok_or_else(|| {
            invalid_compatibility("No historical compatibility catalog exists for this program")
        })?;
        match self {
            Self::Automatic | Self::Unknown => Ok(()),
            Self::Release { tag } => {
                if tag.trim() != tag || tag.is_empty() || tag.len() > 128 {
                    return Err(invalid_compatibility(
                        "Selected Core release tag is invalid",
                    ));
                }
                if !catalog_program
                    .versions
                    .iter()
                    .any(|version| version.tag == *tag)
                {
                    return Err(invalid_compatibility(
                        "Selected Core release is not catalogued",
                    ));
                }
                Ok(())
            }
            Self::Commit { commit_sha } => {
                if !is_commit_sha(commit_sha) {
                    return Err(invalid_compatibility("Selected Core commit SHA is invalid"));
                }
                let known = catalog_program
                    .versions
                    .iter()
                    .any(|version| version.commit_sha == *commit_sha)
                    || crate::embedded_core_upstream_manifest()?
                        .program(program)
                        .is_some_and(|tracks| {
                            tracks.development.commit_sha == *commit_sha
                                || tracks.stable.commit_sha == *commit_sha
                        });
                if !known {
                    return Err(invalid_compatibility(
                        "Selected Core commit is not a catalogued release or tracked anchor",
                    ));
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CoreCompatibilityBasis {
    VerifiedOfficialArtifact,
    TrustedPackage,
    TrackedSource,
    BinaryReported,
    UserDeclared,
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
    Commit {
        commit_sha: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
    },
    Uncatalogued {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        normalized_version: Option<String>,
        reason: CoreUncataloguedReason,
    },
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CoreUncataloguedReason {
    NotFound,
    Ambiguous,
    FutureVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreTargetIdentity {
    pub program: ProgramKind,
    pub coordinate: CoreVersionCoordinate,
    pub basis: CoreCompatibilityBasis,
    pub catalog_revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint_sha256: Option<String>,
}

impl CoreTargetIdentity {
    pub fn from_reported(program: ProgramKind, reported_version: Option<String>) -> Result<Self> {
        let probe = CoreProbeReport::from_reported_version(reported_version);
        embedded_core_compatibility_catalog()?.resolve_target(
            program,
            &probe,
            &CoreCompatibilityPreference::Automatic,
            None,
        )
    }

    pub fn unknown(program: ProgramKind, probe: Option<&CoreProbeReport>) -> Self {
        Self {
            program,
            coordinate: probe
                .and_then(|probe| probe.normalized_version.clone())
                .map_or(CoreVersionCoordinate::Unknown, |normalized_version| {
                    CoreVersionCoordinate::Uncatalogued {
                        normalized_version: Some(normalized_version),
                        reason: CoreUncataloguedReason::NotFound,
                    }
                }),
            basis: probe.map_or(CoreCompatibilityBasis::Unknown, |_| {
                CoreCompatibilityBasis::BinaryReported
            }),
            catalog_revision: CORE_COMPATIBILITY_EXTRACTOR_REVISION.into(),
            reported_version: probe.and_then(|probe| probe.reported_version.clone()),
            fingerprint_sha256: None,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.program == ProgramKind::Generic {
            return Err(invalid_compatibility(
                "Generic programs cannot carry a Core compatibility target",
            ));
        }
        if self.catalog_revision != CORE_COMPATIBILITY_EXTRACTOR_REVISION {
            return Err(invalid_compatibility(
                "Core target catalog revision does not match this client",
            ));
        }
        if self
            .reported_version
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 512)
        {
            return Err(invalid_compatibility(
                "Core target reported version must contain at most 512 non-empty bytes",
            ));
        }
        if self
            .fingerprint_sha256
            .as_ref()
            .is_some_and(|value| !is_sha256(value))
        {
            return Err(invalid_compatibility(
                "Core target fingerprint must be an exact SHA-256 digest",
            ));
        }
        match &self.coordinate {
            CoreVersionCoordinate::Release {
                normalized_version,
                commit_sha,
                ..
            } => {
                Version::parse(normalized_version).map_err(|error| {
                    invalid_compatibility("Core target release version is invalid")
                        .with_details(error.to_string())
                })?;
                if !is_commit_sha(commit_sha) {
                    return Err(invalid_compatibility(
                        "Core target release requires an exact commit SHA",
                    ));
                }
            }
            CoreVersionCoordinate::Commit { commit_sha, .. } if !is_commit_sha(commit_sha) => {
                return Err(invalid_compatibility(
                    "Core target commit requires an exact lowercase commit SHA",
                ));
            }
            _ => {}
        }
        Ok(())
    }

    pub fn display_version(&self) -> String {
        if let Some(reported) = &self.reported_version {
            return reported.clone();
        }
        match &self.coordinate {
            CoreVersionCoordinate::Release { tag, .. } => tag.clone(),
            CoreVersionCoordinate::Commit { commit_sha, .. } => short_revision(commit_sha).into(),
            CoreVersionCoordinate::Uncatalogued {
                normalized_version: Some(version),
                ..
            } => version.clone(),
            CoreVersionCoordinate::Uncatalogued { .. } | CoreVersionCoordinate::Unknown => {
                "Unknown compatibility".into()
            }
        }
    }

    pub fn semantic_version(&self) -> Option<Version> {
        match &self.coordinate {
            CoreVersionCoordinate::Release {
                normalized_version, ..
            }
            | CoreVersionCoordinate::Uncatalogued {
                normalized_version: Some(normalized_version),
                ..
            } => Version::parse(normalized_version).ok(),
            _ => None,
        }
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
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CoreFeatureLifecycle {
    Active,
    Deprecated,
    Removed,
    Unreviewed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CoreEvidenceLevel {
    CatalogConfirmed,
    BinaryReported,
    UserDeclared,
    NativeAccepted,
    RuntimeConfirmed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreFeatureDecision {
    pub feature_id: CoreFeatureId,
    pub availability: CoreFeatureAvailability,
    pub lifecycle: CoreFeatureLifecycle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior_revision: Option<String>,
    pub evidence: CoreEvidenceLevel,
    pub attempt_allowed: bool,
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
        let catalog = embedded_core_compatibility_catalog()?;
        let mut decisions = Vec::with_capacity(catalog.semantic_features.len());
        let catalog_version = catalog
            .program(target.program)
            .and_then(|program| catalog_version_for_target(target, program));
        for feature in &catalog.semantic_features {
            let feature_id = CoreFeatureId::parse(feature.id.clone())?;
            let mut availability = feature.default_availability;
            let mut lifecycle = CoreFeatureLifecycle::Unreviewed;
            let mut behavior_revision = None;
            if let Some(program) = catalog.program(target.program) {
                // A user-declared exact commit that is also the commit of a
                // catalogued release inherits that release's semantic
                // decisions.  Branch-head and uncatalogued commits remain
                // Unknown unless a future catalog adds an explicit anchor.
                let target_version = catalog_version.clone();
                let mut selected_version: Option<Version> = None;
                for event in &feature.events {
                    let event_version = program
                        .versions
                        .iter()
                        .find(|version| version.tag == event.tag)
                        .and_then(|version| Version::parse(&version.normalized_version).ok());
                    if event.program == target.program
                        && event_version
                            .as_ref()
                            .zip(target_version.as_ref())
                            .is_some_and(|(event_version, target_version)| {
                                event_version <= target_version
                            })
                        && selected_version
                            .as_ref()
                            .is_none_or(|selected| event_version.as_ref() > Some(selected))
                    {
                        availability = event.availability;
                        lifecycle = event.lifecycle;
                        behavior_revision.clone_from(&event.behavior_revision);
                        selected_version = event_version;
                    }
                }
            }
            decisions.push(CoreFeatureDecision {
                feature_id,
                availability,
                lifecycle,
                behavior_revision,
                evidence: if catalog_version.is_some() {
                    CoreEvidenceLevel::CatalogConfirmed
                } else {
                    match target.basis {
                        CoreCompatibilityBasis::UserDeclared => CoreEvidenceLevel::UserDeclared,
                        _ => CoreEvidenceLevel::BinaryReported,
                    }
                },
                // Product policy: unknown and catalog-mismatched features remain
                // attemptable.  Native validation and LKG protection decide the
                // exact candidate; this flag is never a support claim.
                attempt_allowed: true,
            });
        }
        let profile_hash = profile_hash(target, &decisions)?;
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

fn catalog_version_for_target(
    target: &CoreTargetIdentity,
    program: &CoreCatalogProgram,
) -> Option<Version> {
    match &target.coordinate {
        CoreVersionCoordinate::Release {
            normalized_version, ..
        }
        | CoreVersionCoordinate::Uncatalogued {
            normalized_version: Some(normalized_version),
            ..
        } => Version::parse(normalized_version).ok(),
        CoreVersionCoordinate::Commit { commit_sha, .. } => program
            .versions
            .iter()
            .find(|version| version.commit_sha == *commit_sha)
            .and_then(|version| Version::parse(&version.normalized_version).ok()),
        CoreVersionCoordinate::Unknown
        | CoreVersionCoordinate::Uncatalogued {
            normalized_version: None,
            ..
        } => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreValidationEvidence {
    pub binary_sha256: String,
    pub profile_hash: String,
    pub config_hash: String,
    pub validator_contract_revision: String,
    pub native_accepted: bool,
    pub validated_unix_ms: u64,
}

impl CoreValidationEvidence {
    pub fn validates(&self, binary_sha256: &str, profile_hash: &str, config_hash: &str) -> bool {
        self.native_accepted
            && self.binary_sha256 == binary_sha256
            && self.profile_hash == profile_hash
            && self.config_hash == config_hash
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreCompatibilityCatalog {
    pub schema_version: u32,
    pub extractor_revision: String,
    pub source_snapshot_at: String,
    pub programs: Vec<CoreCatalogProgram>,
    pub semantic_features: Vec<CoreSemanticFeature>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreCatalogProgram {
    pub program: ProgramKind,
    pub repository: String,
    pub development_ref: String,
    pub versions: Vec<CoreCatalogVersion>,
    pub excluded_tags: Vec<String>,
    pub surface: Vec<CoreSurfaceFeature>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreCatalogVersion {
    pub tag: String,
    pub normalized_version: String,
    pub commit_sha: String,
    pub source_timestamp: String,
    pub source_url: String,
    pub prerelease: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreSurfaceFeature {
    pub id: String,
    pub encoding: String,
    pub name: String,
    pub go_field: String,
    pub source_path: String,
    pub source_line: String,
    pub events: Vec<CoreSurfaceEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreSurfaceEvent {
    pub version: String,
    pub tag: String,
    pub state: CoreSurfaceState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CoreSurfaceState {
    Present,
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreSemanticFeature {
    pub id: String,
    pub domain: String,
    pub default_availability: CoreFeatureAvailability,
    pub events: Vec<CoreSemanticFeatureEvent>,
    pub evidence_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreSemanticFeatureEvent {
    pub program: ProgramKind,
    pub tag: String,
    pub availability: CoreFeatureAvailability,
    pub lifecycle: CoreFeatureLifecycle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior_revision: Option<String>,
}

impl CoreCompatibilityCatalog {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CORE_COMPATIBILITY_CATALOG_SCHEMA_VERSION
            || self.extractor_revision != CORE_COMPATIBILITY_EXTRACTOR_REVISION
        {
            return Err(invalid_compatibility(
                "Embedded Core compatibility catalog has an unsupported revision",
            ));
        }
        validate_timestamp(&self.source_snapshot_at)?;
        let mut programs = BTreeSet::new();
        for program in &self.programs {
            if program.program == ProgramKind::Generic
                || !programs.insert(program_catalog_key(program.program))
            {
                return Err(invalid_compatibility(
                    "Core compatibility catalog has an invalid program set",
                ));
            }
            let (repository, development_ref) = expected_upstream(program.program)?;
            if program.repository != repository || program.development_ref != development_ref {
                return Err(invalid_compatibility(
                    "Core compatibility catalog repository/ref does not match product policy",
                ));
            }
            let mut exact_versions = BTreeSet::new();
            for version in &program.versions {
                Version::parse(&version.normalized_version).map_err(|error| {
                    invalid_compatibility("Core catalog contains an invalid normalized version")
                        .with_details(error.to_string())
                })?;
                if !is_commit_sha(&version.commit_sha)
                    || !exact_versions.insert((version.tag.as_str(), version.commit_sha.as_str()))
                {
                    return Err(invalid_compatibility(
                        "Core catalog contains an invalid or duplicate exact version",
                    ));
                }
                validate_timestamp(&version.source_timestamp)?;
                if !version
                    .source_url
                    .starts_with(&format!("https://github.com/{repository}/"))
                {
                    return Err(invalid_compatibility(
                        "Core catalog version URL does not belong to its repository",
                    ));
                }
            }
            let mut surface_ids = BTreeSet::new();
            for feature in &program.surface {
                CoreFeatureId::parse(feature.id.clone())?;
                if !surface_ids.insert(&feature.id) || feature.events.is_empty() {
                    return Err(invalid_compatibility(
                        "Core catalog contains a duplicate or eventless surface feature",
                    ));
                }
            }
        }
        if programs != BTreeSet::from(["mihomo", "singBox", "xray"]) {
            return Err(invalid_compatibility(
                "Core compatibility catalog must contain exactly three Core programs",
            ));
        }
        let mut semantic_ids = BTreeSet::new();
        for feature in &self.semantic_features {
            CoreFeatureId::parse(feature.id.clone())?;
            if !semantic_ids.insert(&feature.id) || feature.evidence_revision.trim().is_empty() {
                return Err(invalid_compatibility(
                    "Core compatibility catalog contains duplicate semantic features",
                ));
            }
        }
        Ok(())
    }

    pub fn program(&self, kind: ProgramKind) -> Option<&CoreCatalogProgram> {
        self.programs.iter().find(|program| program.program == kind)
    }

    pub fn resolve_target(
        &self,
        kind: ProgramKind,
        probe: &CoreProbeReport,
        preference: &CoreCompatibilityPreference,
        fingerprint_sha256: Option<String>,
    ) -> Result<CoreTargetIdentity> {
        probe.validate()?;
        let program = self.program(kind).ok_or_else(|| {
            invalid_compatibility("No historical compatibility catalog exists for this program")
        })?;
        let (coordinate, basis) = match preference {
            CoreCompatibilityPreference::Unknown => (
                CoreVersionCoordinate::Unknown,
                CoreCompatibilityBasis::Unknown,
            ),
            CoreCompatibilityPreference::Release { tag } => {
                let version = program
                    .versions
                    .iter()
                    .find(|version| &version.tag == tag)
                    .ok_or_else(|| {
                        invalid_compatibility("Selected Core release is not catalogued")
                    })?;
                (
                    release_coordinate(version),
                    CoreCompatibilityBasis::UserDeclared,
                )
            }
            CoreCompatibilityPreference::Commit { commit_sha } => {
                if !is_commit_sha(commit_sha) {
                    return Err(invalid_compatibility("Selected Core commit SHA is invalid"));
                }
                let known = program
                    .versions
                    .iter()
                    .any(|version| &version.commit_sha == commit_sha)
                    || crate::embedded_core_upstream_manifest()?
                        .program(kind)
                        .is_some_and(|tracks| {
                            tracks.development.commit_sha == *commit_sha
                                || tracks.stable.commit_sha == *commit_sha
                        });
                if !known {
                    return Err(invalid_compatibility(
                        "Selected Core commit is not a catalogued release or tracked anchor",
                    ));
                }
                (
                    CoreVersionCoordinate::Commit {
                        commit_sha: commit_sha.clone(),
                        branch: None,
                    },
                    CoreCompatibilityBasis::UserDeclared,
                )
            }
            CoreCompatibilityPreference::Automatic => resolve_automatic_coordinate(program, probe),
        };
        let target = CoreTargetIdentity {
            program: kind,
            coordinate,
            basis,
            catalog_revision: self.extractor_revision.clone(),
            reported_version: probe.reported_version.clone(),
            fingerprint_sha256,
        };
        target.validate()?;
        Ok(target)
    }
}

pub fn embedded_core_compatibility_catalog() -> Result<&'static CoreCompatibilityCatalog> {
    let result = CATALOG.get_or_init(|| {
        let catalog = serde_json::from_str::<CoreCompatibilityCatalog>(EMBEDDED_CATALOG)
            .map_err(|error| error.to_string())?;
        catalog.validate().map_err(|error| error.to_string())?;
        Ok(catalog)
    });
    result.as_ref().map_err(|details| {
        invalid_compatibility("Embedded Core compatibility catalog is invalid")
            .with_details(details.clone())
    })
}

fn resolve_automatic_coordinate(
    program: &CoreCatalogProgram,
    probe: &CoreProbeReport,
) -> (CoreVersionCoordinate, CoreCompatibilityBasis) {
    let Some(normalized) = probe.normalized_version.as_deref() else {
        return (
            CoreVersionCoordinate::Unknown,
            CoreCompatibilityBasis::Unknown,
        );
    };
    let matches = program
        .versions
        .iter()
        .filter(|version| version.normalized_version == normalized)
        .collect::<Vec<_>>();
    let unique_commits = matches
        .iter()
        .map(|version| version.commit_sha.as_str())
        .collect::<BTreeSet<_>>();
    if matches.len() == 1 || unique_commits.len() == 1 {
        return (
            release_coordinate(matches[0]),
            CoreCompatibilityBasis::BinaryReported,
        );
    }
    let reason = if matches.is_empty() {
        let reported = Version::parse(normalized).ok();
        let newest_catalogued = program
            .versions
            .iter()
            .filter_map(|version| Version::parse(&version.normalized_version).ok())
            .max();
        if reported
            .as_ref()
            .zip(newest_catalogued.as_ref())
            .is_some_and(|(reported, newest)| reported > newest)
        {
            CoreUncataloguedReason::FutureVersion
        } else {
            CoreUncataloguedReason::NotFound
        }
    } else {
        CoreUncataloguedReason::Ambiguous
    };
    (
        CoreVersionCoordinate::Uncatalogued {
            normalized_version: Some(normalized.into()),
            reason,
        },
        CoreCompatibilityBasis::BinaryReported,
    )
}

fn release_coordinate(version: &CoreCatalogVersion) -> CoreVersionCoordinate {
    CoreVersionCoordinate::Release {
        tag: version.tag.clone(),
        normalized_version: version.normalized_version.clone(),
        commit_sha: version.commit_sha.clone(),
    }
}

fn profile_hash(target: &CoreTargetIdentity, decisions: &[CoreFeatureDecision]) -> Result<String> {
    let bytes = serde_json::to_vec(&(target, decisions))?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn program_catalog_key(kind: ProgramKind) -> &'static str {
    match kind {
        ProgramKind::Generic => "generic",
        ProgramKind::SingBox => "singBox",
        ProgramKind::Xray => "xray",
        ProgramKind::Mihomo => "mihomo",
    }
}

fn expected_upstream(kind: ProgramKind) -> Result<(&'static str, &'static str)> {
    match kind {
        ProgramKind::Xray => Ok(("XTLS/Xray-core", "main")),
        ProgramKind::Mihomo => Ok(("MetaCubeX/mihomo", "Alpha")),
        ProgramKind::SingBox => Ok(("SagerNet/sing-box", "testing")),
        ProgramKind::Generic => Err(invalid_compatibility(
            "Generic programs do not have Core compatibility history",
        )),
    }
}

fn validate_timestamp(value: &str) -> Result<()> {
    if value.len() != 20
        || !value.ends_with('Z')
        || value.as_bytes().get(4) != Some(&b'-')
        || value.as_bytes().get(7) != Some(&b'-')
        || value.as_bytes().get(10) != Some(&b'T')
        || value.as_bytes().get(13) != Some(&b':')
        || value.as_bytes().get(16) != Some(&b':')
    {
        return Err(invalid_compatibility(
            "Core compatibility timestamps must use second-precision UTC RFC 3339",
        ));
    }
    Ok(())
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

fn short_revision(value: &str) -> &str {
    value.get(..12).unwrap_or(value)
}

fn invalid_compatibility(message: impl Into<String>) -> CamelliaNexusError {
    CamelliaNexusError::new(ErrorCode::InvalidSpec, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_catalog_indexes_complete_version_and_surface_history() {
        let catalog = embedded_core_compatibility_catalog().expect("valid catalog");
        let xray = catalog.program(ProgramKind::Xray).unwrap();
        let mihomo = catalog.program(ProgramKind::Mihomo).unwrap();
        let sing_box = catalog.program(ProgramKind::SingBox).unwrap();
        assert_eq!(xray.versions.len(), 131);
        assert_eq!(mihomo.versions.len(), 104);
        assert_eq!(sing_box.versions.len(), 611);
        assert!(xray.surface.len() >= 800);
        assert!(mihomo.surface.len() >= 1_000);
        assert!(sing_box.surface.len() >= 1_100);
    }

    #[test]
    fn reported_release_resolves_without_claiming_official_origin() {
        let catalog = embedded_core_compatibility_catalog().unwrap();
        let probe = CoreProbeReport::from_reported_version(Some("Mihomo Meta v1.19.29".into()));
        let target = catalog
            .resolve_target(
                ProgramKind::Mihomo,
                &probe,
                &CoreCompatibilityPreference::Automatic,
                Some("a".repeat(64)),
            )
            .unwrap();
        assert_eq!(target.basis, CoreCompatibilityBasis::BinaryReported);
        assert!(matches!(
            target.coordinate,
            CoreVersionCoordinate::Release { ref tag, .. } if tag == "v1.19.29"
        ));
    }

    #[test]
    fn compatibility_preferences_are_validated_before_persistence() {
        assert!(
            CoreCompatibilityPreference::Automatic
                .validate_for_program(ProgramKind::Generic)
                .is_ok()
        );
        assert!(
            CoreCompatibilityPreference::Unknown
                .validate_for_program(ProgramKind::Generic)
                .is_err()
        );
        assert!(
            CoreCompatibilityPreference::Release { tag: String::new() }
                .validate_for_program(ProgramKind::Xray)
                .is_err()
        );
        assert!(
            CoreCompatibilityPreference::Commit {
                commit_sha: "a".repeat(40),
            }
            .validate_for_program(ProgramKind::Xray)
            .is_err()
        );
        let catalog = embedded_core_compatibility_catalog().unwrap();
        let known = &catalog.program(ProgramKind::Xray).unwrap().versions[0];
        assert!(
            CoreCompatibilityPreference::Release {
                tag: known.tag.clone(),
            }
            .validate_for_program(ProgramKind::Xray)
            .is_ok()
        );
    }

    #[test]
    fn ambiguous_historical_alias_is_not_guessed() {
        let catalog = embedded_core_compatibility_catalog().unwrap();
        let probe = CoreProbeReport::from_reported_version(Some("sing-box version 1.1.0".into()));
        let target = catalog
            .resolve_target(
                ProgramKind::SingBox,
                &probe,
                &CoreCompatibilityPreference::Automatic,
                None,
            )
            .unwrap();
        assert!(matches!(
            target.coordinate,
            CoreVersionCoordinate::Uncatalogued {
                reason: CoreUncataloguedReason::Ambiguous,
                ..
            }
        ));
    }

    #[test]
    fn reported_version_newer_than_the_catalog_is_classified_as_future() {
        let catalog = embedded_core_compatibility_catalog().unwrap();
        let probe = CoreProbeReport::from_reported_version(Some("Xray 9999.1.0".into()));
        let target = catalog
            .resolve_target(
                ProgramKind::Xray,
                &probe,
                &CoreCompatibilityPreference::Automatic,
                None,
            )
            .unwrap();
        assert!(matches!(
            target.coordinate,
            CoreVersionCoordinate::Uncatalogued {
                reason: CoreUncataloguedReason::FutureVersion,
                ..
            }
        ));
    }

    #[test]
    fn unknown_feature_decisions_remain_attemptable() {
        let target = CoreTargetIdentity::unknown(ProgramKind::Xray, None);
        let profile = CoreCompatibilityProfile::resolve(&target).unwrap();
        assert!(!profile.decisions.is_empty());
        assert!(profile.decisions.iter().all(|decision| {
            decision.availability == CoreFeatureAvailability::Unknown && decision.attempt_allowed
        }));
    }

    #[test]
    fn known_release_commit_inherits_release_feature_decisions() {
        let catalog = embedded_core_compatibility_catalog().unwrap();
        let version = catalog
            .program(ProgramKind::Xray)
            .and_then(|program| program.versions.last())
            .expect("xray release");
        let release_target = CoreTargetIdentity {
            program: ProgramKind::Xray,
            coordinate: release_coordinate(version),
            basis: CoreCompatibilityBasis::UserDeclared,
            catalog_revision: CORE_COMPATIBILITY_EXTRACTOR_REVISION.into(),
            reported_version: None,
            fingerprint_sha256: None,
        };
        let commit_target = CoreTargetIdentity {
            coordinate: CoreVersionCoordinate::Commit {
                commit_sha: version.commit_sha.clone(),
                branch: None,
            },
            ..release_target.clone()
        };
        let release = CoreCompatibilityProfile::resolve(&release_target).unwrap();
        let commit = CoreCompatibilityProfile::resolve(&commit_target).unwrap();
        assert_eq!(release.decisions, commit.decisions);
    }

    #[test]
    fn catalogued_versions_change_feature_decisions_at_the_reviewed_boundary() {
        let catalog = embedded_core_compatibility_catalog().unwrap();
        let program = catalog.program(ProgramKind::SingBox).unwrap();
        let profile_for = |tag: &str| {
            let version = program
                .versions
                .iter()
                .find(|version| version.tag == tag)
                .unwrap_or_else(|| panic!("missing catalogued sing-box release {tag}"));
            CoreCompatibilityProfile::resolve(&CoreTargetIdentity {
                program: ProgramKind::SingBox,
                coordinate: release_coordinate(version),
                basis: CoreCompatibilityBasis::UserDeclared,
                catalog_revision: CORE_COMPATIBILITY_EXTRACTOR_REVISION.into(),
                reported_version: None,
                fingerprint_sha256: None,
            })
            .unwrap()
        };
        let schema_decision = |profile: &CoreCompatibilityProfile| {
            profile
                .decisions
                .iter()
                .find(|decision| decision.feature_id.as_str() == "core.cli.generatedSchema")
                .expect("generated-schema decision")
                .availability
        };

        assert_eq!(
            schema_decision(&profile_for("v1.13.18")),
            CoreFeatureAvailability::Unknown
        );
        assert_eq!(
            schema_decision(&profile_for("v1.14.0-beta.2")),
            CoreFeatureAvailability::Supported
        );
    }

    #[test]
    fn native_acceptance_is_bound_to_exact_binary_profile_and_candidate() {
        let evidence = CoreValidationEvidence {
            binary_sha256: "a".repeat(64),
            profile_hash: "b".repeat(64),
            config_hash: "c".repeat(64),
            validator_contract_revision: "validator-v1".into(),
            native_accepted: true,
            validated_unix_ms: 1,
        };
        assert!(evidence.validates(&"a".repeat(64), &"b".repeat(64), &"c".repeat(64)));
        assert!(!evidence.validates(&"d".repeat(64), &"b".repeat(64), &"c".repeat(64)));
        assert!(!evidence.validates(&"a".repeat(64), &"d".repeat(64), &"c".repeat(64)));
        assert!(!evidence.validates(&"a".repeat(64), &"b".repeat(64), &"d".repeat(64)));
        let mut rejected = evidence;
        rejected.native_accepted = false;
        assert!(!rejected.validates(&"a".repeat(64), &"b".repeat(64), &"c".repeat(64)));
    }
}
