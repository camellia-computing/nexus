//! Bounded evaluation of source build expressions against observed binary metadata.

use crate::{CoreBuildObservation, ProgramKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuildCondition {
    Satisfied,
    Excluded,
    Unconfirmed,
}

impl BuildCondition {
    fn from_observation(value: Option<bool>) -> Self {
        match value {
            Some(true) => Self::Satisfied,
            Some(false) => Self::Excluded,
            None => Self::Unconfirmed,
        }
    }

    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::Excluded, _) | (_, Self::Excluded) => Self::Excluded,
            (Self::Satisfied, Self::Satisfied) => Self::Satisfied,
            _ => Self::Unconfirmed,
        }
    }

    fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::Satisfied, _) | (_, Self::Satisfied) => Self::Satisfied,
            (Self::Excluded, Self::Excluded) => Self::Excluded,
            _ => Self::Unconfirmed,
        }
    }

    fn not(self) -> Self {
        match self {
            Self::Satisfied => Self::Excluded,
            Self::Excluded => Self::Satisfied,
            Self::Unconfirmed => Self::Unconfirmed,
        }
    }
}

const OPERATING_SYSTEMS: &[&str] = &[
    "aix",
    "android",
    "darwin",
    "dragonfly",
    "freebsd",
    "hurd",
    "illumos",
    "ios",
    "js",
    "linux",
    "nacl",
    "netbsd",
    "openbsd",
    "plan9",
    "solaris",
    "wasip1",
    "windows",
    "zos",
];
const ARCHITECTURES: &[&str] = &[
    "386",
    "amd64",
    "amd64p32",
    "arm",
    "armbe",
    "arm64",
    "arm64be",
    "loong64",
    "mips",
    "mipsle",
    "mips64",
    "mips64le",
    "mips64p32",
    "mips64p32le",
    "ppc",
    "ppc64",
    "ppc64le",
    "riscv",
    "riscv64",
    "s390",
    "s390x",
    "sparc",
    "sparc64",
    "wasm",
];

fn tag_condition(
    program: ProgramKind,
    build: &CoreBuildObservation,
    tag: &str,
    reported_tags: &[&str],
) -> BuildCondition {
    // Go's implicit platform aliases apply independently of explicit -tags.
    let observation = if tag == "cgo" {
        build.cgo
    } else if let Some(minor) = tag.strip_prefix("go1.") {
        let minor = minor
            .parse::<u16>()
            .ok()
            .filter(|value| value.to_string() == minor);
        minor
            .zip(build.go_version)
            .map(|(minor, version)| (version.major, version.minor) >= (1, minor))
    } else if tag == "unix" || OPERATING_SYSTEMS.contains(&tag) {
        build
            .operating_system
            .as_deref()
            .filter(|os| OPERATING_SYSTEMS.contains(os))
            .map(|os| {
                if tag == "unix" {
                    matches!(
                        os,
                        "aix"
                            | "android"
                            | "darwin"
                            | "dragonfly"
                            | "freebsd"
                            | "hurd"
                            | "illumos"
                            | "ios"
                            | "linux"
                            | "netbsd"
                            | "openbsd"
                            | "solaris"
                    )
                } else {
                    tag == os
                        || matches!(
                            (os, tag),
                            ("android", "linux") | ("ios", "darwin") | ("illumos", "solaris")
                        )
                }
            })
    } else if ARCHITECTURES.contains(&tag) {
        build
            .architecture
            .as_deref()
            .filter(|arch| ARCHITECTURES.contains(arch))
            .map(|arch| tag == arch)
    } else {
        // Compiler, toolchain and architecture feature tags are not described by a -tags list.
        let implicit = matches!(tag, "gc" | "gccgo")
            || tag.starts_with("go1.")
            || ARCHITECTURES.iter().any(|arch| {
                tag.strip_prefix(arch)
                    .is_some_and(|suffix| suffix.starts_with('.'))
            });
        match build.tags.as_ref() {
            Some(tags) if tags.iter().any(|observed| observed == tag) => Some(true),
            Some(_)
                if !implicit
                    && (program == ProgramKind::SingBox || reported_tags.contains(&tag)) =>
            {
                Some(false)
            }
            // Absence only has meaning for source-proven reported flags.
            _ => None,
        }
    };
    BuildCondition::from_observation(observation)
}

pub(crate) fn evaluate_build_condition(
    program: ProgramKind,
    build: Option<&CoreBuildObservation>,
    expression: &str,
    reported_tags: &[&str],
) -> BuildCondition {
    if expression.trim().is_empty() {
        return BuildCondition::Satisfied;
    }
    if expression.len() > 4096 || build.is_some_and(|value| !value.is_bounded()) {
        return BuildCondition::Unconfirmed;
    }
    let empty = CoreBuildObservation::default();
    let mut parser = Parser {
        source: expression.as_bytes(),
        offset: 0,
        program,
        build: build.unwrap_or(&empty),
        reported_tags,
    };
    match parser.expression(0) {
        Some(value)
            if {
                parser.whitespace();
                parser.offset == parser.source.len()
            } =>
        {
            value
        }
        _ => BuildCondition::Unconfirmed,
    }
}

struct Parser<'a> {
    source: &'a [u8],
    offset: usize,
    program: ProgramKind,
    build: &'a CoreBuildObservation,
    reported_tags: &'a [&'a str],
}

impl Parser<'_> {
    fn whitespace(&mut self) {
        while self
            .source
            .get(self.offset)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.offset += 1;
        }
    }

    fn take(&mut self, value: &[u8]) -> bool {
        self.whitespace();
        if self.source[self.offset..].starts_with(value) {
            self.offset += value.len();
            true
        } else {
            false
        }
    }

    fn expression(&mut self, depth: usize) -> Option<BuildCondition> {
        let mut result = self.conjunction(depth)?;
        while self.take(b"||") {
            result = result.or(self.conjunction(depth)?);
        }
        Some(result)
    }

    fn conjunction(&mut self, depth: usize) -> Option<BuildCondition> {
        let mut result = self.term(depth)?;
        while self.take(b"&&") {
            result = result.and(self.term(depth)?);
        }
        Some(result)
    }

    fn term(&mut self, depth: usize) -> Option<BuildCondition> {
        if depth > 64 {
            return None;
        }
        if self.take(b"!") {
            return self.term(depth + 1).map(BuildCondition::not);
        }
        if self.take(b"(") {
            let result = self.expression(depth + 1)?;
            return self.take(b")").then_some(result);
        }
        self.whitespace();
        let start = self.offset;
        while self
            .source
            .get(self.offset)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'))
        {
            self.offset += 1;
        }
        let token = std::str::from_utf8(&self.source[start..self.offset]).ok()?;
        (!token.is_empty())
            .then(|| tag_condition(self.program, self.build, token, self.reported_tags))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use BuildCondition::*;

    fn evaluate_build_condition(
        program: ProgramKind,
        build: Option<&CoreBuildObservation>,
        expression: &str,
    ) -> BuildCondition {
        super::evaluate_build_condition(program, build, expression, &[])
    }

    #[test]
    fn compiler_release_conditions_require_explicit_toolchain_observation() {
        let build = CoreBuildObservation {
            go_version: Some(crate::CoreGoVersion {
                major: 1,
                minor: 26,
                patch: 3,
            }),
            tags: Some(vec![]),
            ..Default::default()
        };
        for (expression, expected) in [
            ("go1.20 && !without_contextjson", Satisfied),
            ("go1.26", Satisfied),
            ("go1.27", Excluded),
            ("!go1.27", Satisfied),
            ("go1.026", Unconfirmed),
            ("go1.26.3", Unconfirmed),
        ] {
            assert_eq!(
                evaluate_build_condition(ProgramKind::SingBox, Some(&build), expression),
                expected,
                "{expression}"
            );
        }
        let missing = CoreBuildObservation {
            go_version: None,
            tags: Some(vec!["go1.26".into()]),
            ..build
        };
        assert_eq!(
            evaluate_build_condition(ProgramKind::SingBox, Some(&missing), "go1.26"),
            Unconfirmed
        );
    }

    #[test]
    fn only_source_proven_reported_flags_can_be_negated_from_an_explicit_report() {
        let build = CoreBuildObservation {
            tags: Some(vec!["with_gvisor".into()]),
            ..Default::default()
        };
        assert_eq!(
            super::evaluate_build_condition(
                ProgramKind::Mihomo,
                Some(&build),
                "with_gvisor && !no_tailscale",
                &["with_gvisor", "no_tailscale"]
            ),
            Satisfied
        );
        assert_eq!(
            super::evaluate_build_condition(
                ProgramKind::Mihomo,
                Some(&build),
                "!unreported_flag",
                &["no_tailscale"]
            ),
            Unconfirmed
        );
        assert_eq!(
            super::evaluate_build_condition(
                ProgramKind::Mihomo,
                None,
                "!no_tailscale",
                &["no_tailscale"]
            ),
            Unconfirmed
        );
    }

    #[test]
    fn expressions_preserve_unknown_evidence_and_operator_precedence() {
        let build = CoreBuildObservation {
            go_version: None,
            operating_system: Some("windows".into()),
            architecture: Some("amd64".into()),
            tags: Some(vec!["with_quic".into()]),
            cgo: Some(false),
        };
        for (expression, expected) in [
            ("with_quic", Satisfied),
            ("!with_quic", Excluded),
            ("(windows && amd64) || linux", Satisfied),
            ("linux || windows && cgo", Excluded),
            ("windows && !cgo && !with_wireguard", Satisfied),
            ("go1.26", Unconfirmed),
            ("!go1.26", Unconfirmed),
            ("amd64.v2", Unconfirmed),
            ("!gc", Unconfirmed),
            ("linux && gc", Excluded),
            ("windows || gc", Satisfied),
            ("windows ||", Unconfirmed),
            ("with_quic garbage", Unconfirmed),
            ("with_quic & windows", Unconfirmed),
            ("(with_quic", Unconfirmed),
        ] {
            assert_eq!(
                evaluate_build_condition(ProgramKind::SingBox, Some(&build), expression),
                expected,
                "{expression}"
            );
        }
        assert_eq!(
            evaluate_build_condition(ProgramKind::Mihomo, Some(&build), "with_quic"),
            Satisfied
        );
        assert_eq!(
            evaluate_build_condition(ProgramKind::Mihomo, Some(&build), "!no_tailscale"),
            Unconfirmed
        );
        assert_eq!(
            evaluate_build_condition(ProgramKind::SingBox, None, "!with_quic"),
            Unconfirmed
        );
        assert_eq!(
            evaluate_build_condition(ProgramKind::SingBox, None, ""),
            Satisfied
        );
    }

    #[test]
    fn implicit_platform_aliases_do_not_infer_unobserved_architecture_features() {
        for (os, alias) in [
            ("android", "linux"),
            ("ios", "darwin"),
            ("illumos", "solaris"),
        ] {
            let build = CoreBuildObservation {
                operating_system: Some(os.into()),
                ..Default::default()
            };
            assert_eq!(
                evaluate_build_condition(
                    ProgramKind::SingBox,
                    Some(&build),
                    &format!("{os} && {alias} && unix")
                ),
                Satisfied
            );
            assert_eq!(
                evaluate_build_condition(ProgramKind::SingBox, Some(&build), "amd64"),
                Unconfirmed
            );
        }
    }

    #[test]
    fn malformed_or_excessive_expressions_do_not_become_positive_evidence() {
        for expression in [
            "!".repeat(256),
            "x".repeat(4097),
            "(".repeat(256),
            "☃".into(),
        ] {
            assert_eq!(
                evaluate_build_condition(ProgramKind::SingBox, None, &expression),
                Unconfirmed
            );
        }
    }
}
