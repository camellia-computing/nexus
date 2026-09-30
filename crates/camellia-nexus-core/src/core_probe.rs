//! Program-specific build observations; absence is not proof of a disabled feature.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::ProgramKind;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreBuildObservation {
    pub go_version: Option<CoreGoVersion>,
    pub operating_system: Option<String>,
    pub architecture: Option<String>,
    pub tags: Option<Vec<String>>,
    pub cgo: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreGoVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl CoreGoVersion {
    fn parse(value: &str) -> Option<Self> {
        let version = value.strip_prefix("go")?;
        let parts: Vec<_> = version.split('.').collect();
        if !(2..=3).contains(&parts.len()) {
            return None;
        }
        let component = |part: &str| {
            (!part.is_empty()
                && (part.len() == 1 || !part.starts_with('0'))
                && part.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| part.parse::<u16>().ok())
            .flatten()
        };
        Some(Self {
            major: component(parts[0])?,
            minor: component(parts[1])?,
            patch: parts
                .get(2)
                .map(|part| component(part))
                .unwrap_or(Some(0))?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CoreProbeIdentityIssue {
    AmbiguousOutput,
    MalformedBuildInfo,
}

pub(crate) fn version_prefix(kind: ProgramKind) -> Option<&'static str> {
    match kind {
        ProgramKind::SingBox => Some("sing-box version "),
        ProgramKind::Xray => Some("Xray "),
        ProgramKind::Mihomo => Some("Mihomo Meta "),
        ProgramKind::Generic => None,
    }
}

fn unique_line<'a>(
    output: &'a str,
    prefix: &str,
) -> Result<Option<&'a str>, CoreProbeIdentityIssue> {
    let prefix = if prefix.ends_with(": ") {
        prefix.trim_end()
    } else {
        prefix
    };
    let mut matches = output
        .lines()
        .filter_map(|line| line.trim().strip_prefix(prefix).map(str::trim));
    let value = matches.next();
    if matches.next().is_some() {
        return Err(CoreProbeIdentityIssue::AmbiguousOutput);
    }
    Ok(value)
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'))
}

impl CoreBuildObservation {
    pub(crate) fn is_bounded(&self) -> bool {
        self.operating_system.as_deref().is_none_or(identifier)
            && self.architecture.as_deref().is_none_or(identifier)
            && self
                .tags
                .as_ref()
                .is_none_or(|tags| tags.len() <= 128 && tags.iter().all(|tag| identifier(tag)))
    }
}

fn platform(
    value: &str,
    observation: &mut CoreBuildObservation,
) -> Result<(), CoreProbeIdentityIssue> {
    let (operating_system, architecture) = value
        .split_once('/')
        .ok_or(CoreProbeIdentityIssue::MalformedBuildInfo)?;
    if !identifier(operating_system) || !identifier(architecture) {
        return Err(CoreProbeIdentityIssue::MalformedBuildInfo);
    }
    observation.operating_system = Some(operating_system.into());
    observation.architecture = Some(architecture.into());
    Ok(())
}

pub(crate) fn parse_build_observation(
    kind: ProgramKind,
    output: &str,
) -> Result<(CoreBuildObservation, Option<String>), CoreProbeIdentityIssue> {
    let prefix = version_prefix(kind).ok_or(CoreProbeIdentityIssue::MalformedBuildInfo)?;
    let header = unique_line(output, prefix)?.ok_or(CoreProbeIdentityIssue::MalformedBuildInfo)?;
    if output.lines().any(|line| {
        [ProgramKind::SingBox, ProgramKind::Xray, ProgramKind::Mihomo]
            .into_iter()
            .filter(|candidate| *candidate != kind)
            .any(|candidate| {
                line.trim()
                    .starts_with(version_prefix(candidate).expect("Core prefix"))
            })
    }) {
        return Err(CoreProbeIdentityIssue::AmbiguousOutput);
    }
    let mut observation = CoreBuildObservation::default();
    let mut revision = None;
    let tags = match kind {
        ProgramKind::SingBox => {
            if let Some(environment) = unique_line(output, "Environment: ")? {
                let tokens: Vec<_> = environment.split_whitespace().collect();
                if tokens.len() != 2 || !tokens[0].starts_with("go") {
                    return Err(CoreProbeIdentityIssue::MalformedBuildInfo);
                }
                platform(tokens[1], &mut observation)?;
                observation.go_version = CoreGoVersion::parse(tokens[0]);
            }
            if let Some(cgo) = unique_line(output, "CGO: ")? {
                observation.cgo = Some(match cgo {
                    "enabled" => true,
                    "disabled" => false,
                    _ => return Err(CoreProbeIdentityIssue::MalformedBuildInfo),
                });
            }
            if let Some(commit) = unique_line(output, "Revision: ")? {
                if commit.len() != 40
                    || !commit
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                {
                    return Err(CoreProbeIdentityIssue::MalformedBuildInfo);
                }
                revision = Some(commit.into());
            }
            unique_line(output, "Tags: ")?
        }
        ProgramKind::Mihomo => {
            let tokens: Vec<_> = header.split_whitespace().collect();
            if tokens.len() >= 5 && tokens[3] == "with" && tokens[4].starts_with("go") {
                platform(&format!("{}/{}", tokens[1], tokens[2]), &mut observation)?;
                observation.go_version = CoreGoVersion::parse(tokens[4]);
            }
            unique_line(output, "Use tags: ")?
        }
        ProgramKind::Xray => {
            if let Some((_, suffix)) = header.rsplit_once(" (")
                && let Some(environment) = suffix.strip_suffix(')')
            {
                let tokens: Vec<_> = environment.split_whitespace().collect();
                if tokens.len() == 2 && tokens[0].starts_with("go") {
                    platform(tokens[1], &mut observation)?;
                    observation.go_version = CoreGoVersion::parse(tokens[0]);
                }
            }
            None
        }
        ProgramKind::Generic => unreachable!(),
    };
    if let Some(tags) = tags {
        let tags: BTreeSet<_> = tags
            .split(|character: char| character == ',' || character.is_whitespace())
            .filter(|tag| !tag.is_empty())
            .collect();
        if tags.len() > 128 || tags.iter().any(|tag| !identifier(tag)) {
            return Err(CoreProbeIdentityIssue::MalformedBuildInfo);
        }
        observation.tags = Some(tags.into_iter().map(str::to_owned).collect());
    }
    Ok((observation, revision))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiler_version_is_numeric_and_only_read_from_program_build_fields() {
        let expected = Some(CoreGoVersion {
            major: 1,
            minor: 26,
            patch: 0,
        });
        for (program, output) in [
            (
                ProgramKind::SingBox,
                "sing-box version 1.14.1\nEnvironment: go1.26.0 linux/amd64",
            ),
            (
                ProgramKind::Mihomo,
                "Mihomo Meta v1.19.31 linux amd64 with go1.26.0",
            ),
            (
                ProgramKind::Xray,
                "Xray 26.3.27 (Xray, Penetrates Everything.) Custom (go1.26.0 linux/amd64)",
            ),
        ] {
            assert_eq!(
                parse_build_observation(program, output)
                    .unwrap()
                    .0
                    .go_version,
                expected
            );
        }
        assert_eq!(CoreGoVersion::parse("go1.26"), expected);
        for version in [
            "go1.26rc1",
            "go1.26.0-custom",
            "go01.26.0",
            "go1.026.0",
            "go1.999999.0",
            "go1.26.0.1",
            "1.26.0",
        ] {
            assert!(CoreGoVersion::parse(version).is_none(), "{version}");
        }
        let observation = parse_build_observation(
            ProgramKind::SingBox,
            "sing-box version 1.14.1\nExtra: go1.26.0\nTags: go1.26",
        )
        .unwrap()
        .0;
        assert!(observation.go_version.is_none());
    }

    #[test]
    fn program_formats_keep_build_conditions_separate_from_the_baseline() {
        let (sing_box, revision) = parse_build_observation(ProgramKind::SingBox,
            &format!("sing-box version 1.14.0\nEnvironment: go1.26.0 windows/amd64\nTags: with_quic,with_gvisor,with_quic\nRevision: {}\nCGO: disabled", "a".repeat(40))).unwrap();
        assert_eq!(sing_box.operating_system.as_deref(), Some("windows"));
        assert_eq!(sing_box.architecture.as_deref(), Some("amd64"));
        assert_eq!(
            sing_box.tags,
            Some(vec!["with_gvisor".into(), "with_quic".into()])
        );
        assert_eq!(sing_box.cgo, Some(false));
        assert_eq!(revision, Some("a".repeat(40)));
        let (mihomo, _) = parse_build_observation(
            ProgramKind::Mihomo,
            "Mihomo Meta v1.19.30 linux arm64 with go1.26.0\nUse tags: with_gvisor, with_quic",
        )
        .unwrap();
        assert_eq!(mihomo.architecture.as_deref(), Some("arm64"));
        assert_eq!(mihomo.tags.unwrap().len(), 2);
        let (xray, commit) = parse_build_observation(
            ProgramKind::Xray,
            "Xray 26.3.27 (Xray, Penetrates Everything.) abc1234 (go1.26.0 windows/amd64)",
        )
        .unwrap();
        assert_eq!(xray.operating_system.as_deref(), Some("windows"));
        assert!(xray.tags.is_none());
        assert!(commit.is_none());
    }

    #[test]
    fn ambiguous_or_malformed_evidence_is_not_silently_ignored() {
        for suffix in [
            "Tags: a\nTags: b",
            "Environment: go1.26.0 linux/amd64\nEnvironment: go1.26.0 windows/amd64",
            "Revision: 1234567",
            "CGO: perhaps",
            "Tags: secret/invalid",
            "Mihomo Meta v1.19.30",
            "sing-box version 1.13.0",
        ] {
            assert!(
                parse_build_observation(
                    ProgramKind::SingBox,
                    &format!("sing-box version 1.14.0\n{suffix}")
                )
                .is_err()
            );
        }
        let (observation, _) =
            parse_build_observation(ProgramKind::SingBox, "sing-box version 1.14.0").unwrap();
        assert!(observation.tags.is_none());
        let (observation, _) =
            parse_build_observation(ProgramKind::SingBox, "sing-box version 1.14.0\nTags: ")
                .unwrap();
        assert_eq!(observation.tags, Some(vec![]));
    }
}
