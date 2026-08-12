//! Exact current upstream selectors, separate from compatibility history.
//!
//! `main`, `Alpha`, `testing`, and GitHub `latest` are moving maintenance
//! policies.  The manifest pins their current exact source identities.  It
//! never embeds feature booleans: release/configuration history and feature
//! decisions are owned by `core_compatibility`.

use std::collections::HashMap;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::{
    CORE_COMPATIBILITY_EXTRACTOR_REVISION, CamelliaNexusError, CoreCompatibilityBasis,
    CoreTargetIdentity, CoreVersionCoordinate, ErrorCode, ProgramKind, Result,
    embedded_core_compatibility_catalog,
};

pub const CORE_UPSTREAM_MANIFEST_SCHEMA_VERSION: u32 = 2;
const EMBEDDED_CORE_UPSTREAM_MANIFEST: &str = include_str!("../core-upstream-versions.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CoreTrackedChannel {
    Development,
    Stable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CoreTrackingPolicy {
    BranchHead,
    LatestStableRelease,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreTrackedVersion {
    pub policy: CoreTrackingPolicy,
    pub tracked_ref: String,
    pub resolved_ref: String,
    pub commit_sha: String,
    pub source_timestamp: String,
    pub source_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreProgramVersionTracks {
    pub program: ProgramKind,
    pub repository: String,
    pub development: CoreTrackedVersion,
    pub stable: CoreTrackedVersion,
}

impl CoreProgramVersionTracks {
    pub fn version(&self, channel: CoreTrackedChannel) -> &CoreTrackedVersion {
        match channel {
            CoreTrackedChannel::Development => &self.development,
            CoreTrackedChannel::Stable => &self.stable,
        }
    }

    pub fn target(&self, channel: CoreTrackedChannel) -> CoreTargetIdentity {
        let version = self.version(channel);
        let coordinate = match channel {
            CoreTrackedChannel::Development => CoreVersionCoordinate::Commit {
                commit_sha: version.commit_sha.clone(),
                branch: Some(version.tracked_ref.clone()),
            },
            CoreTrackedChannel::Stable => CoreVersionCoordinate::Release {
                tag: version.resolved_ref.clone(),
                normalized_version: parse_reported_core_version(&version.resolved_ref)
                    .expect("validated stable release")
                    .to_string(),
                commit_sha: version.commit_sha.clone(),
            },
        };
        CoreTargetIdentity {
            program: self.program,
            coordinate,
            basis: CoreCompatibilityBasis::TrackedSource,
            catalog_revision: CORE_COMPATIBILITY_EXTRACTOR_REVISION.into(),
            reported_version: (channel == CoreTrackedChannel::Stable)
                .then(|| version.resolved_ref.clone()),
            fingerprint_sha256: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreUpstreamManifest {
    pub schema_version: u32,
    pub observed_at: String,
    pub compatibility_catalog_revision: String,
    pub programs: Vec<CoreProgramVersionTracks>,
}

impl CoreUpstreamManifest {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CORE_UPSTREAM_MANIFEST_SCHEMA_VERSION {
            return Err(invalid_tracking(format!(
                "Unsupported Core upstream manifest schema {}",
                self.schema_version
            )));
        }
        if self.compatibility_catalog_revision != CORE_COMPATIBILITY_EXTRACTOR_REVISION {
            return Err(invalid_tracking(
                "Core tracking manifest references a different compatibility catalog revision",
            ));
        }
        validate_timestamp(&self.observed_at, "manifest observedAt")?;
        let catalog = embedded_core_compatibility_catalog()?;
        let mut programs = HashMap::new();
        for tracks in &self.programs {
            if programs.insert(tracks.program, tracks).is_some() {
                return Err(invalid_tracking(
                    "Core upstream manifest contains a duplicate program",
                ));
            }
            let (repository, development_ref) = expected_upstream(tracks.program)?;
            if tracks.repository != repository {
                return Err(invalid_tracking(format!(
                    "{} must track repository {repository}",
                    program_label(tracks.program)
                )));
            }
            if tracks.development.policy != CoreTrackingPolicy::BranchHead
                || tracks.development.tracked_ref != development_ref
            {
                return Err(invalid_tracking(format!(
                    "{} development channel must track the {development_ref} branch head",
                    program_label(tracks.program)
                )));
            }
            if tracks.stable.policy != CoreTrackingPolicy::LatestStableRelease
                || tracks.stable.tracked_ref != "latest"
            {
                return Err(invalid_tracking(format!(
                    "{} stable channel must track GitHub latest stable release",
                    program_label(tracks.program)
                )));
            }
            validate_tracked_version(
                tracks.program,
                CoreTrackedChannel::Development,
                &tracks.repository,
                &tracks.development,
                &self.observed_at,
            )?;
            validate_tracked_version(
                tracks.program,
                CoreTrackedChannel::Stable,
                &tracks.repository,
                &tracks.stable,
                &self.observed_at,
            )?;
            let catalog_program = catalog.program(tracks.program).ok_or_else(|| {
                invalid_tracking("Tracked Core is missing from compatibility history")
            })?;
            if !catalog_program.versions.iter().any(|version| {
                version.tag == tracks.stable.resolved_ref
                    && version.commit_sha == tracks.stable.commit_sha
            }) {
                return Err(invalid_tracking(
                    "Tracked stable Core does not match an exact compatibility catalog release",
                ));
            }
        }
        for kind in [ProgramKind::Xray, ProgramKind::Mihomo, ProgramKind::SingBox] {
            if !programs.contains_key(&kind) {
                return Err(invalid_tracking(format!(
                    "Core upstream manifest is missing {}",
                    program_label(kind)
                )));
            }
        }
        if programs.len() != 3 {
            return Err(invalid_tracking(
                "Core upstream manifest must contain only Xray, Mihomo, and sing-box",
            ));
        }
        Ok(())
    }

    pub fn program(&self, kind: ProgramKind) -> Option<&CoreProgramVersionTracks> {
        self.programs.iter().find(|program| program.program == kind)
    }

    pub fn target(
        &self,
        kind: ProgramKind,
        channel: CoreTrackedChannel,
    ) -> Result<CoreTargetIdentity> {
        let tracks = self.program(kind).ok_or_else(|| {
            invalid_tracking(format!(
                "No tracked upstream versions exist for {}",
                program_label(kind)
            ))
        })?;
        let target = tracks.target(channel);
        target.validate()?;
        Ok(target)
    }
}

pub fn embedded_core_upstream_manifest() -> Result<CoreUpstreamManifest> {
    let manifest = serde_json::from_str::<CoreUpstreamManifest>(EMBEDDED_CORE_UPSTREAM_MANIFEST)
        .map_err(|error| {
            invalid_tracking("Embedded Core upstream manifest is invalid")
                .with_details(error.to_string())
        })?;
    manifest.validate()?;
    Ok(manifest)
}

pub fn tracked_core_target(
    kind: ProgramKind,
    channel: CoreTrackedChannel,
) -> Result<CoreTargetIdentity> {
    embedded_core_upstream_manifest()?.target(kind, channel)
}

pub fn parse_reported_core_version(value: &str) -> Option<Version> {
    value
        .split(|character: char| {
            character.is_whitespace()
                || matches!(character, ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}')
        })
        .filter(|candidate| !candidate.is_empty())
        .find_map(|candidate| {
            let candidate = candidate
                .trim_matches(|character: char| matches!(character, ':' | '=' | '"' | '\''))
                .trim_start_matches(['v', 'V']);
            Version::parse(candidate).ok()
        })
}

fn validate_tracked_version(
    program: ProgramKind,
    channel: CoreTrackedChannel,
    repository: &str,
    version: &CoreTrackedVersion,
    observed_at: &str,
) -> Result<()> {
    let (expected_repository, expected_development_ref) = expected_upstream(program)?;
    if repository != expected_repository {
        return Err(invalid_tracking("Tracked Core repository is not canonical"));
    }
    match channel {
        CoreTrackedChannel::Development => {
            if version.policy != CoreTrackingPolicy::BranchHead
                || version.tracked_ref != expected_development_ref
            {
                return Err(invalid_tracking(
                    "Development Core identity has the wrong branch policy",
                ));
            }
            if version.resolved_ref != version.commit_sha {
                return Err(invalid_tracking(
                    "Development Core resolvedRef must be its exact commit SHA",
                ));
            }
        }
        CoreTrackedChannel::Stable => {
            if version.policy != CoreTrackingPolicy::LatestStableRelease
                || version.tracked_ref != "latest"
            {
                return Err(invalid_tracking(
                    "Stable Core identity has the wrong release policy",
                ));
            }
            let semver = parse_reported_core_version(&version.resolved_ref).ok_or_else(|| {
                invalid_tracking("Stable Core resolvedRef must contain a SemVer release tag")
            })?;
            if !semver.pre.is_empty() {
                return Err(invalid_tracking(
                    "Stable Core identity cannot resolve to a prerelease",
                ));
            }
        }
    }
    if !is_commit_sha(&version.commit_sha) {
        return Err(invalid_tracking(
            "Tracked Core commit SHA must contain 40 lowercase hexadecimal characters",
        ));
    }
    if version.resolved_ref.trim().is_empty() || version.tracked_ref.trim().is_empty() {
        return Err(invalid_tracking("Tracked Core refs cannot be empty"));
    }
    validate_timestamp(&version.source_timestamp, "sourceTimestamp")?;
    validate_timestamp(observed_at, "observedAt")?;
    if !version
        .source_url
        .starts_with(&format!("https://github.com/{repository}/"))
    {
        return Err(invalid_tracking(
            "Tracked Core source URL must belong to its canonical GitHub repository",
        ));
    }
    Ok(())
}

fn expected_upstream(kind: ProgramKind) -> Result<(&'static str, &'static str)> {
    match kind {
        ProgramKind::Xray => Ok(("XTLS/Xray-core", "main")),
        ProgramKind::Mihomo => Ok(("MetaCubeX/mihomo", "Alpha")),
        ProgramKind::SingBox => Ok(("SagerNet/sing-box", "testing")),
        ProgramKind::Generic => Err(invalid_tracking(
            "Generic programs do not have a tracked Core upstream",
        )),
    }
}

fn is_commit_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn validate_timestamp(value: &str, label: &str) -> Result<()> {
    if value.len() != 20
        || !value.ends_with('Z')
        || value.as_bytes().get(4) != Some(&b'-')
        || value.as_bytes().get(7) != Some(&b'-')
        || value.as_bytes().get(10) != Some(&b'T')
        || value.as_bytes().get(13) != Some(&b':')
        || value.as_bytes().get(16) != Some(&b':')
    {
        return Err(invalid_tracking(format!(
            "Core upstream {label} must use second-precision UTC RFC 3339"
        )));
    }
    Ok(())
}

fn program_label(kind: ProgramKind) -> &'static str {
    match kind {
        ProgramKind::Generic => "Generic",
        ProgramKind::SingBox => "sing-box",
        ProgramKind::Xray => "Xray",
        ProgramKind::Mihomo => "Mihomo",
    }
}

fn invalid_tracking(message: impl Into<String>) -> CamelliaNexusError {
    CamelliaNexusError::new(ErrorCode::InvalidSpec, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_manifest_exposes_two_exact_targets_per_core() {
        let manifest = embedded_core_upstream_manifest().expect("valid embedded manifest");
        assert_eq!(manifest.programs.len(), 3);
        for kind in [ProgramKind::Xray, ProgramKind::Mihomo, ProgramKind::SingBox] {
            for channel in [CoreTrackedChannel::Development, CoreTrackedChannel::Stable] {
                let target = manifest.target(kind, channel).expect("tracked target");
                assert_eq!(target.program, kind);
                assert_eq!(target.basis, CoreCompatibilityBasis::TrackedSource);
                match (channel, target.coordinate) {
                    (CoreTrackedChannel::Development, CoreVersionCoordinate::Commit { .. }) => {}
                    (CoreTrackedChannel::Stable, CoreVersionCoordinate::Release { .. }) => {}
                    _ => panic!("tracked channel resolved to wrong coordinate"),
                }
            }
        }
    }

    #[test]
    fn development_refs_remain_case_exact() {
        let manifest = embedded_core_upstream_manifest().unwrap();
        assert_eq!(
            manifest
                .program(ProgramKind::Xray)
                .unwrap()
                .development
                .tracked_ref,
            "main"
        );
        assert_eq!(
            manifest
                .program(ProgramKind::Mihomo)
                .unwrap()
                .development
                .tracked_ref,
            "Alpha"
        );
        assert_eq!(
            manifest
                .program(ProgramKind::SingBox)
                .unwrap()
                .development
                .tracked_ref,
            "testing"
        );
    }

    #[test]
    fn stable_targets_are_bound_to_catalog_releases() {
        for kind in [ProgramKind::Xray, ProgramKind::Mihomo, ProgramKind::SingBox] {
            let target = tracked_core_target(kind, CoreTrackedChannel::Stable).unwrap();
            let CoreVersionCoordinate::Release {
                tag, commit_sha, ..
            } = target.coordinate
            else {
                panic!("stable release coordinate");
            };
            let program = embedded_core_compatibility_catalog()
                .unwrap()
                .program(kind)
                .unwrap();
            assert!(
                program
                    .versions
                    .iter()
                    .any(|version| { version.tag == tag && version.commit_sha == commit_sha })
            );
        }
    }

    #[test]
    fn reported_prerelease_is_a_version_claim_not_source_provenance() {
        assert_eq!(
            parse_reported_core_version("sing-box version 1.14.0-beta.2"),
            Some(Version::parse("1.14.0-beta.2").unwrap())
        );
        assert_eq!(
            parse_reported_core_version("Mihomo Meta alpha-e183c58"),
            None
        );
    }
}
