//! Source-backed configuration knowledge and maintained binary admission.

use std::{
    collections::{BTreeSet, HashSet},
    sync::OnceLock,
};

use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{CamelliaNexusError, ErrorCode, ProgramKind, Result};

const EMBEDDED_KNOWLEDGE: &str = include_str!("../core-knowledge.json");
static KNOWLEDGE: OnceLock<std::result::Result<CoreKnowledgeCatalog, String>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StableFamilyPolicy {
    MajorMinor,
    ReleaseMonth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreStableRelease {
    pub tag: String,
    pub version: String,
    pub commit_sha: String,
    pub module_path: String,
    pub dependencies: Vec<KnowledgeDependencySource>,
    pub published_at: String,
    pub source_timestamp: String,
    pub source_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeDependencySource {
    pub module_path: String,
    pub version: String,
    pub repository: String,
    pub commit_sha: String,
    pub source_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeSourceReference {
    pub module_path: String,
    pub path: String,
    pub symbol: String,
}

impl CoreStableRelease {
    pub fn source_commit(&self, source: &KnowledgeSourceReference) -> Option<&str> {
        if source.module_path == self.module_path {
            Some(&self.commit_sha)
        } else {
            self.dependencies
                .iter()
                .find(|dependency| dependency.module_path == source.module_path)
                .map(|dependency| dependency.commit_sha.as_str())
        }
    }
}

fn valid_source_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && !value.contains('\\')
        && !value
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
        && !value.split('/').any(|part| matches!(part, "" | "." | ".."))
}

fn valid_commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_source_references<'a>(
    program: &ProgramKnowledgeDescriptor,
    tags: &[String],
    sources: impl IntoIterator<Item = &'a KnowledgeSourceReference>,
) -> Result<()> {
    for source in sources {
        if !valid_source_path(&source.path)
            || source.symbol.is_empty()
            || source.symbol.len() > 1024
            || source.symbol.chars().any(char::is_control)
            || tags.iter().any(|tag| {
                !program
                    .releases
                    .iter()
                    .any(|release| &release.tag == tag && release.source_commit(source).is_some())
            })
        {
            return Err(invalid_knowledge(
                "Source reference has no exact module identity",
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum KnowledgeTypeShape {
    Builtin {
        name: String,
    },
    Named {
        package: String,
        name: String,
    },
    Pointer {
        element: Box<KnowledgeTypeShape>,
    },
    Sequence {
        element: Box<KnowledgeTypeShape>,
    },
    Mapping {
        key: Box<KnowledgeTypeShape>,
        element: Box<KnowledgeTypeShape>,
    },
    Generic {
        target: Box<KnowledgeTypeShape>,
        arguments: Vec<KnowledgeTypeShape>,
    },
    Structure,
    Dynamic,
    Opaque {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeField {
    pub name: String,
    pub shape: KnowledgeTypeShape,
    #[serde(rename = "type")]
    pub go_type: String,
    pub embedded: bool,
    pub tags: std::collections::BTreeMap<String, String>,
    pub annotations: std::collections::BTreeMap<String, String>,
    pub source: KnowledgeSourceReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeDeclaration {
    pub id: String,
    pub name: String,
    pub package: String,
    #[serde(rename = "type")]
    pub go_type: String,
    pub shape: KnowledgeTypeShape,
    pub alias: bool,
    pub build_constraint: String,
    pub imports: std::collections::BTreeMap<String, String>,
    pub fields: Vec<KnowledgeField>,
    pub methods: Vec<String>,
    pub source: KnowledgeSourceReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeDeclarationVariant {
    pub declaration: KnowledgeDeclaration,
    pub releases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum KnowledgeValueConstraint {
    Enum {
        values: Vec<String>,
        allow_null: bool,
    },
    Platform {
        condition: String,
        activation: KnowledgeValueActivation,
    },
    Requires {
        when_value: serde_json::Value,
        sibling: String,
        value: serde_json::Value,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KnowledgeValueActivation {
    Enabled,
    NonEmpty,
    NonZero,
    Positive,
}

impl KnowledgeValueActivation {
    pub fn is_active(self, value: &serde_json::Value) -> bool {
        if value.is_null() {
            return false;
        }
        match self {
            Self::Enabled => value.as_bool().unwrap_or(true),
            Self::NonEmpty => value.as_str().is_none_or(|value| !value.is_empty()),
            Self::NonZero => value.as_f64().is_none_or(|value| value != 0.0),
            Self::Positive => value.as_f64().is_none_or(|value| value > 0.0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeBehaviorEvidence {
    pub source: KnowledgeSourceReference,
    pub body_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeSemanticRule {
    pub id: String,
    pub path: Vec<String>,
    pub declaration: String,
    pub field: String,
    pub field_hash: String,
    pub message_key: String,
    pub constraint: KnowledgeValueConstraint,
    pub default_value: serde_json::Value,
    pub default_when: Vec<KnowledgeDefaultCondition>,
    pub build_constraint: String,
    pub when: Vec<KnowledgeFieldPredicate>,
    pub evidence: Vec<KnowledgeBehaviorEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeFieldPredicate {
    pub sibling: String,
    pub values: Vec<serde_json::Value>,
    pub allow_missing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KnowledgeDefaultCondition {
    Missing,
    Null,
    EmptyString,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeSemanticRuleVariant {
    pub rule: KnowledgeSemanticRule,
    pub releases: Vec<String>,
}

/// An explicitly reviewed custom decoder; underlying Go fields alone are not a wire contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeValueDecoder {
    pub declaration: String,
    pub object_declaration: String,
    pub encoding: String,
    pub scalar_forms: Vec<KnowledgeScalarForm>,
    pub key_comparison: KnowledgeFieldComparison,
    pub build_constraint: String,
    pub evidence: Vec<KnowledgeBehaviorEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KnowledgeScalarForm {
    Boolean,
    Null,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeValueDecoderVariant {
    pub decoder: KnowledgeValueDecoder,
    pub releases: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KnowledgeConstructorStatus {
    Registered,
    Rejected,
}

/// Source dispatch evidence, separate from evidence for an installed build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KnowledgeDiscriminatorComparison {
    Exact,
    AsciiCaseInsensitive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KnowledgeFieldComparison {
    Exact,
    AsciiCaseInsensitive,
    AsciiCaseInsensitiveUnderscore,
}

impl KnowledgeFieldComparison {
    pub fn matches(self, declared: &str, key: &str) -> bool {
        match self {
            Self::Exact => declared == key,
            Self::AsciiCaseInsensitive => declared.eq_ignore_ascii_case(key),
            Self::AsciiCaseInsensitiveUnderscore => declared
                .replace('_', "-")
                .eq_ignore_ascii_case(&key.replace('_', "-")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeDecodedSharedOption {
    pub path: Vec<String>,
    pub declaration: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeDecodedObjectLayout {
    pub build_constraint: String,
    pub envelope_declarations: Vec<String>,
    pub envelope_key_comparison: KnowledgeFieldComparison,
    pub shared_options: Vec<KnowledgeDecodedSharedOption>,
    pub key_comparison: KnowledgeFieldComparison,
    pub root_field_names: bool,
    pub nested_embedded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeDecoderBinding {
    pub path: Vec<String>,
    pub discriminator: String,
    pub discriminator_comparison: KnowledgeDiscriminatorComparison,
    pub value: String,
    pub options_path: Vec<String>,
    pub declaration: String,
    pub encoding: String,
    pub object_layout: Option<KnowledgeDecodedObjectLayout>,
    pub build_constraint: String,
    pub constructor: String,
    pub constructor_status: KnowledgeConstructorStatus,
    pub evidence: Vec<KnowledgeBehaviorEvidence>,
}

impl KnowledgeDecoderBinding {
    pub fn matches_discriminator(&self, value: &str) -> bool {
        match self.discriminator_comparison {
            KnowledgeDiscriminatorComparison::Exact => self.value == value,
            KnowledgeDiscriminatorComparison::AsciiCaseInsensitive => {
                self.value.eq_ignore_ascii_case(value)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeDecoderBindingVariant {
    pub binding: KnowledgeDecoderBinding,
    pub releases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeReportedBuildTag {
    pub tag: String,
    pub evidence: Vec<KnowledgeBehaviorEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeReportedBuildTagVariant {
    pub report: KnowledgeReportedBuildTag,
    pub releases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProgramKnowledgeDescriptor {
    pub program: ProgramKind,
    pub repository: String,
    pub family_policy: StableFamilyPolicy,
    pub latest_stable: String,
    pub families: Vec<String>,
    pub configuration_roots: Vec<String>,
    pub outbound_collection: String,
    pub share_protocols: std::collections::BTreeMap<String, String>,
    pub releases: Vec<CoreStableRelease>,
    pub declarations: Vec<KnowledgeDeclarationVariant>,
    pub semantic_rules: Vec<KnowledgeSemanticRuleVariant>,
    pub decoder_bindings: Vec<KnowledgeDecoderBindingVariant>,
    pub value_decoders: Vec<KnowledgeValueDecoderVariant>,
    pub reported_build_tags: Vec<KnowledgeReportedBuildTagVariant>,
}

impl ProgramKnowledgeDescriptor {
    pub fn reported_build_tags_for(&self, release: &str) -> Vec<&str> {
        self.reported_build_tags
            .iter()
            .filter(|variant| variant.releases.iter().any(|tag| tag == release))
            .map(|variant| variant.report.tag.as_str())
            .collect()
    }

    pub fn decoder_bindings_for<'a>(
        &'a self,
        release: &'a str,
        collection: &'a str,
        value: &'a str,
    ) -> impl Iterator<Item = &'a KnowledgeDecoderBinding> {
        self.decoder_bindings.iter().filter_map(move |variant| {
            (variant.releases.iter().any(|tag| tag == release)
                && variant
                    .binding
                    .path
                    .first()
                    .is_some_and(|path| path == collection)
                && variant.binding.matches_discriminator(value))
            .then_some(&variant.binding)
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreKnowledgeCatalog {
    pub content_hash: String,
    pub producer_hash: String,
    pub reports_hash: String,
    pub programs: Vec<ProgramKnowledgeDescriptor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CoreAdmissionStatus {
    Admitted,
    TooOld,
    NotMaintained,
    Prerelease,
    Unrecognized,
    IdentityMismatch,
    ProbeRejected,
}

impl CoreAdmissionStatus {
    pub fn message_key(self) -> &'static str {
        match self {
            Self::Admitted => "CORE_ADMISSION_ACCEPTED",
            Self::TooOld => "CORE_VERSION_TOO_OLD",
            Self::NotMaintained => "CORE_VERSION_NOT_MAINTAINED",
            Self::Prerelease => "CORE_PRERELEASE_NOT_SUPPORTED",
            Self::Unrecognized => "CORE_VERSION_UNRECOGNIZED",
            Self::IdentityMismatch => "CORE_BINARY_IDENTITY_MISMATCH",
            Self::ProbeRejected => "CORE_PROGRAM_CHECK_UNAVAILABLE",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreAdmissionReport {
    pub program: ProgramKind,
    pub status: CoreAdmissionStatus,
    pub message_key: String,
    pub maintained_families: Vec<String>,
    pub baseline: Option<CoreStableRelease>,
    pub knowledge_hash: String,
}

impl CoreAdmissionReport {
    pub fn from_rejection(kind: ProgramKind, error: &CamelliaNexusError) -> Result<Option<Self>> {
        if error.code != ErrorCode::UnsupportedBinary {
            return Ok(None);
        }
        let status = [
            CoreAdmissionStatus::TooOld,
            CoreAdmissionStatus::NotMaintained,
            CoreAdmissionStatus::Prerelease,
            CoreAdmissionStatus::Unrecognized,
            CoreAdmissionStatus::IdentityMismatch,
            CoreAdmissionStatus::ProbeRejected,
        ]
        .into_iter()
        .find(|status| Some(status.message_key()) == error.message_key.as_deref())
        .or_else(|| {
            error
                .message_key
                .is_none()
                .then_some(CoreAdmissionStatus::ProbeRejected)
        });
        let Some(status) = status else {
            return Ok(None);
        };
        let mut report = embedded_core_knowledge()?.assess(kind, None, None)?;
        report.status = status;
        report.message_key = status.message_key().into();
        Ok(Some(report))
    }

    pub fn require_admitted(&self) -> Result<&CoreStableRelease> {
        if self.status == CoreAdmissionStatus::Admitted {
            return self
                .baseline
                .as_ref()
                .ok_or_else(|| invalid_knowledge("Admitted binary has no baseline"));
        }
        Err(CamelliaNexusError::new(
            ErrorCode::UnsupportedBinary,
            "Select a program from the maintained stable releases",
        )
        .with_message_key(&self.message_key)
        .with_details(format!(
            "maintained families: {}",
            self.maintained_families.join(", ")
        )))
    }
}

impl CoreKnowledgeCatalog {
    pub fn program(&self, kind: ProgramKind) -> Option<&ProgramKnowledgeDescriptor> {
        self.programs.iter().find(|program| program.program == kind)
    }

    pub fn assess(
        &self,
        kind: ProgramKind,
        version: Option<&Version>,
        reported_commit: Option<&str>,
    ) -> Result<CoreAdmissionReport> {
        let program = self
            .program(kind)
            .ok_or_else(|| invalid_knowledge("Program knowledge is unavailable"))?;
        let by_commit = reported_commit.and_then(|commit| {
            program
                .releases
                .iter()
                .find(|release| release.commit_sha == commit)
        });
        let mut report = CoreAdmissionReport {
            program: kind,
            status: CoreAdmissionStatus::Unrecognized,
            message_key: String::new(),
            maintained_families: program.families.clone(),
            baseline: None,
            knowledge_hash: self.content_hash.clone(),
        };
        if let Some(version) = version {
            let canonical = format!("{}.{}.{}", version.major, version.minor, version.patch);
            let by_version = program
                .releases
                .iter()
                .find(|release| release.version == canonical);
            report.status = if !version.pre.is_empty() {
                CoreAdmissionStatus::Prerelease
            } else if by_commit.is_some_and(|release| release.version != canonical) {
                CoreAdmissionStatus::IdentityMismatch
            } else if let Some(release) = by_version {
                report.baseline = Some(release.clone());
                CoreAdmissionStatus::Admitted
            } else {
                let minimum = program
                    .releases
                    .iter()
                    .filter_map(|release| Version::parse(&release.version).ok())
                    .min();
                if minimum.is_some_and(|minimum| version < &minimum) {
                    CoreAdmissionStatus::TooOld
                } else {
                    CoreAdmissionStatus::NotMaintained
                }
            };
        } else if let Some(release) = by_commit {
            report.baseline = Some(release.clone());
            report.status = CoreAdmissionStatus::Admitted;
        }
        report.message_key = report.status.message_key().into();
        Ok(report)
    }

    pub fn validate(&self) -> Result<()> {
        let mut kinds = HashSet::new();
        for program in &self.programs {
            if program.program == ProgramKind::Generic || !kinds.insert(program.program) {
                return Err(invalid_knowledge(
                    "Knowledge contains an invalid program registry",
                ));
            }
            if program.families.len() != 2
                || program.families[0] == program.families[1]
                || program.configuration_roots.is_empty()
                || program.outbound_collection.is_empty()
                || program.share_protocols.is_empty()
            {
                return Err(invalid_knowledge(
                    "Knowledge must define two stable families and configuration roots",
                ));
            }
            for (feature, registry_value) in &program.share_protocols {
                crate::CoreFeatureId::parse(feature)?;
                if registry_value.is_empty() || registry_value.len() > 128 {
                    return Err(invalid_knowledge(
                        "Share protocol registry value is invalid",
                    ));
                }
            }
            let mut versions = BTreeSet::new();
            let mut families = BTreeSet::new();
            let mut tags = BTreeSet::new();
            for release in &program.releases {
                let version = Version::parse(&release.version)
                    .map_err(|_| invalid_knowledge("Knowledge release version is invalid"))?;
                if !version.pre.is_empty()
                    || !version.build.is_empty()
                    || !versions.insert(version.clone())
                    || !tags.insert(release.tag.as_str())
                    || release.tag.trim_start_matches('v') != release.version
                {
                    return Err(invalid_knowledge(
                        "Knowledge contains an unstable or duplicate release",
                    ));
                }
                if program.family_policy == StableFamilyPolicy::ReleaseMonth
                    && !(1..=12).contains(&version.minor)
                {
                    return Err(invalid_knowledge(
                        "Release family must name a calendar month",
                    ));
                }
                if !valid_commit(&release.commit_sha)
                    || release.source_url
                        != format!(
                            "https://github.com/{}/tree/{}",
                            program.repository, release.commit_sha
                        )
                {
                    return Err(invalid_knowledge(
                        "Knowledge source must bind an exact commit",
                    ));
                }
                if !valid_source_path(&release.module_path) {
                    return Err(invalid_knowledge("Knowledge module identity is invalid"));
                }
                let mut modules = HashSet::from([release.module_path.as_str()]);
                if release.dependencies.len() > 64
                    || release.dependencies.iter().any(|dependency| {
                        !valid_source_path(&dependency.module_path)
                            || !modules.insert(dependency.module_path.as_str())
                            || !valid_source_path(&dependency.repository)
                            || dependency.repository.split('/').count() != 2
                            || !valid_commit(&dependency.commit_sha)
                            || !dependency.version.starts_with('v')
                            || Version::parse(&dependency.version[1..]).is_err()
                            || dependency.source_url
                                != format!(
                                    "https://github.com/{}/tree/{}",
                                    dependency.repository, dependency.commit_sha
                                )
                    })
                {
                    return Err(invalid_knowledge("Dependency source identity is invalid"));
                }
                families.insert(format!("{}.{}", version.major, version.minor));
            }
            if families != program.families.iter().cloned().collect()
                || !tags.contains(program.latest_stable.as_str())
            {
                return Err(invalid_knowledge("Knowledge release window is incomplete"));
            }
            for release in &program.releases {
                if !program.decoder_bindings.iter().any(|variant| {
                    variant.releases.contains(&release.tag)
                        && variant.binding.path.first() == Some(&program.outbound_collection)
                }) {
                    return Err(invalid_knowledge(
                        "Outbound registry has no exact release evidence",
                    ));
                }
            }
            for variant in &program.declarations {
                validate_source_references(
                    program,
                    &variant.releases,
                    std::iter::once(&variant.declaration.source)
                        .chain(variant.declaration.fields.iter().map(|field| &field.source)),
                )?;
                let declaration = &variant.declaration;
                if declaration.id != format!("{}#{}", declaration.package, declaration.name)
                    || variant.releases.iter().any(|tag| {
                        program
                            .releases
                            .iter()
                            .find(|release| &release.tag == tag)
                            .is_some_and(|release| {
                                let directory = declaration
                                    .source
                                    .path
                                    .rsplit_once('/')
                                    .map_or(".", |(directory, _)| directory);
                                let expected =
                                    if declaration.source.module_path == release.module_path {
                                        directory.to_owned()
                                    } else if directory == "." {
                                        declaration.source.module_path.clone()
                                    } else {
                                        format!("{}/{directory}", declaration.source.module_path)
                                    };
                                declaration.package != expected
                            })
                    })
                    || declaration.fields.iter().any(|field| {
                        field.source.module_path != declaration.source.module_path
                            || field.source.path != declaration.source.path
                    })
                {
                    return Err(invalid_knowledge(
                        "Declaration namespace does not match its source module",
                    ));
                }
                let mut shape_visits = 0;
                validate_type_shape(&variant.declaration.shape, &mut shape_visits, 0)?;
                for field in &variant.declaration.fields {
                    validate_type_shape(&field.shape, &mut shape_visits, 0)?;
                }
                if variant.releases.is_empty()
                    || variant
                        .releases
                        .iter()
                        .any(|tag| !tags.contains(tag.as_str()))
                {
                    return Err(invalid_knowledge(
                        "Configuration declaration has no maintained source",
                    ));
                }
            }
            let mut rule_bindings = HashSet::new();
            for variant in &program.semantic_rules {
                let rule = &variant.rule;
                validate_source_references(
                    program,
                    &variant.releases,
                    rule.evidence.iter().map(|evidence| &evidence.source),
                )?;
                let valid_constraint = match &rule.constraint {
                    KnowledgeValueConstraint::Enum { values, .. } => !values.is_empty(),
                    KnowledgeValueConstraint::Platform { condition, .. } => {
                        !condition.is_empty() && condition.len() <= 1024
                    }
                    KnowledgeValueConstraint::Requires { sibling, .. } => {
                        !sibling.is_empty() && sibling.len() <= 256
                    }
                };
                if rule.id.is_empty()
                    || rule.message_key.is_empty()
                    || rule.path.is_empty()
                    || rule.path.len() > 64
                    || !valid_constraint
                    || variant.releases.is_empty()
                    || rule.when.len() > 16
                    || rule.when.iter().any(|predicate| {
                        predicate.sibling.is_empty()
                            || predicate.sibling.len() > 256
                            || predicate.values.is_empty()
                            || predicate.values.len() > 32
                    })
                    || rule.evidence.is_empty()
                    || rule.evidence.len() > 32
                    || rule.evidence.iter().any(|evidence| {
                        evidence.body_hash.len() != 64
                            || !evidence
                                .body_hash
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    })
                {
                    return Err(invalid_knowledge(
                        "Semantic rule has invalid evidence or constraints",
                    ));
                }
                for tag in &variant.releases {
                    if !tags.contains(tag.as_str())
                        || !rule_bindings.insert((tag, &rule.id))
                        || !program.declarations.iter().any(|entry| {
                            entry.releases.contains(tag)
                                && entry.declaration.id == rule.declaration
                                && entry.declaration.fields.iter().any(|field| {
                                    field.name == rule.field
                                        && serde_json::to_vec(&serde_json::json!({
                                            "type": field.go_type,
                                            "embedded": field.embedded,
                                            "tags": field.tags,
                                            "annotations": field.annotations,
                                        }))
                                        .is_ok_and(
                                            |bytes| {
                                                crate::config_service::hash_bytes(&bytes)
                                                    == rule.field_hash
                                            },
                                        )
                                })
                        })
                    {
                        return Err(invalid_knowledge(
                            "Semantic rule must bind a unique maintained field",
                        ));
                    }
                }
            }
            let mut build_reports = HashSet::new();
            for variant in &program.reported_build_tags {
                let report = &variant.report;
                validate_source_references(
                    program,
                    &variant.releases,
                    report.evidence.iter().map(|entry| &entry.source),
                )?;
                if variant.releases.is_empty()
                    || report.tag.is_empty()
                    || report.tag.len() > 64
                    || !report
                        .tag
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'))
                    || report.evidence.len() < 4
                    || report.evidence.len() > 8
                    || report.evidence.iter().any(|evidence| {
                        evidence.body_hash.len() != 64
                            || !evidence
                                .body_hash
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                            || evidence.source.path.is_empty()
                            || evidence.source.symbol.is_empty()
                    })
                {
                    return Err(invalid_knowledge(
                        "Build report lacks bounded source evidence",
                    ));
                }
                for tag in &variant.releases {
                    if !tags.contains(tag.as_str()) || !build_reports.insert((tag, &report.tag)) {
                        return Err(invalid_knowledge(
                            "Build report must bind a unique maintained release",
                        ));
                    }
                }
            }
            if program
                .releases
                .iter()
                .any(|release| program.reported_build_tags_for(&release.tag).len() > 128)
            {
                return Err(invalid_knowledge(
                    "Build report exceeds the observation limit",
                ));
            }
            let mut value_decoders = HashSet::new();
            for variant in &program.value_decoders {
                let decoder = &variant.decoder;
                validate_source_references(
                    program,
                    &variant.releases,
                    decoder.evidence.iter().map(|entry| &entry.source),
                )?;
                let forms: HashSet<_> = decoder.scalar_forms.iter().collect();
                if variant.releases.is_empty()
                    || decoder.encoding != "json"
                    || forms.len() != decoder.scalar_forms.len()
                    || decoder.declaration == decoder.object_declaration
                    || decoder.build_constraint.len() > 4096
                    || decoder.evidence.is_empty()
                    || decoder.evidence.len() > 64
                    || decoder.evidence.iter().any(|entry| {
                        entry.body_hash.len() != 64
                            || !entry
                                .body_hash
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    })
                {
                    return Err(invalid_knowledge(
                        "Custom decoder lacks bounded source evidence",
                    ));
                }
                for tag in &variant.releases {
                    let declaration = |id: &str| {
                        let mut matches = program.declarations.iter().filter(|entry| {
                            entry.releases.contains(tag) && entry.declaration.id == id
                        });
                        let first = matches.next().map(|entry| &entry.declaration);
                        if matches.next().is_some() {
                            None
                        } else {
                            first
                        }
                    };
                    if !tags.contains(tag.as_str())
                        || !value_decoders.insert((tag, &decoder.declaration))
                        || declaration(&decoder.declaration).is_none_or(|entry| {
                            !entry.methods.iter().any(|method| method == "UnmarshalJSON")
                        })
                        || declaration(&decoder.object_declaration).is_none_or(|entry| {
                            entry.shape != KnowledgeTypeShape::Structure
                                || entry
                                    .methods
                                    .iter()
                                    .any(|method| method.starts_with("Unmarshal"))
                                || !entry.build_constraint.is_empty()
                        })
                    {
                        return Err(invalid_knowledge(
                            "Custom decoder must identify unique maintained declarations",
                        ));
                    }
                }
            }
            let mut decoder_bindings = HashSet::new();
            for variant in &program.decoder_bindings {
                let binding = &variant.binding;
                validate_source_references(
                    program,
                    &variant.releases,
                    binding.evidence.iter().map(|entry| &entry.source),
                )?;
                if let Some(layout) = &binding.object_layout {
                    let mut paths = HashSet::new();
                    let mut envelopes = HashSet::new();
                    if layout.envelope_declarations.len() > 32
                        || layout
                            .envelope_declarations
                            .iter()
                            .any(|id| !envelopes.insert(id))
                        || layout.shared_options.len() > 64
                        || layout.shared_options.iter().any(|entry| {
                            entry.path.is_empty()
                                || entry.path.len() > 64
                                || entry
                                    .path
                                    .iter()
                                    .any(|key| key.is_empty() || key.len() > 1024 || key == "*")
                                || entry.path.first() == Some(&binding.discriminator)
                                || !paths.insert(entry.path.first())
                        })
                    {
                        return Err(invalid_knowledge("Decoder object layout is invalid"));
                    }
                    for tag in &variant.releases {
                        for id in layout
                            .envelope_declarations
                            .iter()
                            .chain(layout.shared_options.iter().map(|entry| &entry.declaration))
                        {
                            if !program.declarations.iter().any(|entry| {
                                entry.releases.contains(tag) && &entry.declaration.id == id
                            }) {
                                return Err(invalid_knowledge(
                                    "Decoder object layout lacks exact source declarations",
                                ));
                            }
                        }
                    }
                }
                if variant.releases.is_empty()
                    || binding.path.is_empty()
                    || binding.path.len() > 64
                    || binding.options_path.len() > 64
                    || binding.discriminator.is_empty()
                    || binding.value.is_empty()
                    || !["json", "yaml", "proxy", "mapstructure"]
                        .contains(&binding.encoding.as_str())
                    || binding.evidence.is_empty()
                    || (binding.constructor_status == KnowledgeConstructorStatus::Registered
                        && binding.constructor.is_empty())
                    || binding.evidence.iter().any(|evidence| {
                        evidence.body_hash.len() != 64
                            || !evidence
                                .body_hash
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                            || evidence.source.path.is_empty()
                            || evidence.source.symbol.is_empty()
                    })
                {
                    return Err(invalid_knowledge(
                        "Decoder binding lacks bounded source evidence",
                    ));
                }
                for tag in &variant.releases {
                    if !tags.contains(tag.as_str())
                        || !decoder_bindings.insert((
                            tag,
                            &binding.path,
                            &binding.discriminator,
                            &binding.value,
                            &binding.build_constraint,
                        ))
                        || !program.declarations.iter().any(|entry| {
                            entry.releases.contains(tag)
                                && entry.declaration.id == binding.declaration
                        })
                    {
                        return Err(invalid_knowledge(
                            "Decoder binding must identify a unique maintained declaration",
                        ));
                    }
                }
            }
            for release in &program.releases {
                for root in &program.configuration_roots {
                    if !program.declarations.iter().any(|variant| {
                        variant.declaration.id == *root && variant.releases.contains(&release.tag)
                    }) {
                        return Err(invalid_knowledge(
                            "Configuration root is missing from a stable release",
                        ));
                    }
                }
            }
        }
        let expected: HashSet<_> = ProgramKind::ALL
            .into_iter()
            .filter(|kind| *kind != ProgramKind::Generic)
            .collect();
        if kinds != expected {
            return Err(invalid_knowledge(
                "Knowledge does not cover the registered Core programs",
            ));
        }
        Ok(())
    }
}

pub fn embedded_core_knowledge() -> Result<&'static CoreKnowledgeCatalog> {
    KNOWLEDGE
        .get_or_init(|| {
            let value: serde_json::Value = serde_json::from_str(EMBEDDED_KNOWLEDGE)
                .map_err(|_| "Embedded Core knowledge is not valid JSON".to_owned())?;
            let body = serde_json::json!({ "programs": value["programs"], "producerHash": value["producerHash"], "reportsHash": value["reportsHash"] });
            let bytes = serde_json::to_vec(&body)
                .map_err(|_| "Core knowledge cannot be canonicalized".to_owned())?;
            let hash: String = Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            if value["contentHash"].as_str() != Some(hash.as_str()) {
                return Err("Core knowledge content hash does not match".into());
            }
            let knowledge: CoreKnowledgeCatalog = serde_json::from_value(value)
                .map_err(|_| "Embedded Core knowledge contract is invalid".to_owned())?;
            knowledge.validate().map_err(|error| error.message)?;
            Ok(knowledge)
        })
        .as_ref()
        .map_err(|message| invalid_knowledge(message.clone()))
}

/// Read the program's version field, never an incidental dependency version.
pub fn parse_program_version(kind: ProgramKind, output: &str) -> Option<Version> {
    let prefix = crate::core_probe::version_prefix(kind)?;
    let mut matched = output
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with(prefix));
    let value = matched
        .next()?
        .strip_prefix(prefix)?
        .split_whitespace()
        .next()?;
    if matched.next().is_some() {
        return None;
    }
    Version::parse(value.strip_prefix('v').unwrap_or(value)).ok()
}

pub fn assess_core_probe(
    kind: ProgramKind,
    probe: &crate::CoreProbeReport,
) -> Result<CoreAdmissionReport> {
    probe.validate()?;
    let version = probe
        .normalized_version
        .as_deref()
        .and_then(|version| Version::parse(version).ok());
    let reported = probe
        .reported_version
        .as_deref()
        .and_then(|output| parse_program_version(kind, output));
    if version != reported
        || probe.revision != crate::CORE_BINARY_PROBE_REVISION
        || probe.identity_issue.is_some()
    {
        let mut report = embedded_core_knowledge()?.assess(kind, None, None)?;
        report.status = CoreAdmissionStatus::IdentityMismatch;
        report.message_key = report.status.message_key().into();
        return Ok(report);
    }
    if probe.prerelease {
        let mut report = embedded_core_knowledge()?.assess(kind, None, None)?;
        report.status = CoreAdmissionStatus::Prerelease;
        report.message_key = report.status.message_key().into();
        return Ok(report);
    }
    embedded_core_knowledge()?.assess(kind, version.as_ref(), probe.reported_commit.as_deref())
}

pub fn require_program_admission(spec: &crate::ProgramSpec) -> Result<()> {
    if spec.program_type.kind() == ProgramKind::Generic {
        return Ok(());
    }
    let probe = spec
        .executable
        .metadata()
        .and_then(|metadata| metadata.probe.as_ref());
    match probe {
        Some(probe) => {
            assess_core_probe(spec.program_type.kind(), probe)?.require_admitted()?;
        }
        None => {
            embedded_core_knowledge()?
                .assess(spec.program_type.kind(), None, None)?
                .require_admitted()?;
        }
    }
    Ok(())
}

fn validate_type_shape(shape: &KnowledgeTypeShape, visits: &mut usize, depth: usize) -> Result<()> {
    *visits += 1;
    if depth > 64 || *visits > 32_768 {
        return Err(invalid_knowledge(
            "Configuration type structure exceeds its limit",
        ));
    }
    let valid_name = |name: &str| {
        !name.is_empty()
            && name.len() <= 1024
            && !name.chars().any(|ch| ch.is_whitespace() || ch.is_control())
    };
    match shape {
        KnowledgeTypeShape::Builtin { name } | KnowledgeTypeShape::Opaque { name } => {
            if !valid_name(name) {
                return Err(invalid_knowledge("Configuration type name is invalid"));
            }
        }
        KnowledgeTypeShape::Named { package, name } => {
            if !valid_name(package) || !valid_name(name) {
                return Err(invalid_knowledge("Configuration type reference is invalid"));
            }
        }
        KnowledgeTypeShape::Pointer { element } | KnowledgeTypeShape::Sequence { element } => {
            validate_type_shape(element, visits, depth + 1)?;
        }
        KnowledgeTypeShape::Mapping { key, element } => {
            validate_type_shape(key, visits, depth + 1)?;
            validate_type_shape(element, visits, depth + 1)?;
        }
        KnowledgeTypeShape::Generic { target, arguments } => {
            if arguments.is_empty() || arguments.len() > 128 {
                return Err(invalid_knowledge(
                    "Configuration type arguments are invalid",
                ));
            }
            validate_type_shape(target, visits, depth + 1)?;
            for argument in arguments {
                validate_type_shape(argument, visits, depth + 1)?;
            }
        }
        KnowledgeTypeShape::Structure | KnowledgeTypeShape::Dynamic => {}
    }
    Ok(())
}

fn invalid_knowledge(message: impl Into<String>) -> CamelliaNexusError {
    CamelliaNexusError::new(ErrorCode::InvalidSpec, message)
        .with_message_key("CORE_KNOWLEDGE_INVALID")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_stable_boundaries_are_admitted_and_other_versions_are_rejected() {
        let knowledge = embedded_core_knowledge().unwrap();
        for program in &knowledge.programs {
            for release in &program.releases {
                let version = Version::parse(&release.version).unwrap();
                let report = knowledge
                    .assess(program.program, Some(&version), None)
                    .unwrap();
                assert_eq!(report.require_admitted().unwrap(), release);
            }
            for (version, expected) in [
                ("0.0.1", CoreAdmissionStatus::TooOld),
                ("999.0.0", CoreAdmissionStatus::NotMaintained),
                ("999.0.0-beta.1", CoreAdmissionStatus::Prerelease),
            ] {
                let report = knowledge
                    .assess(
                        program.program,
                        Some(&Version::parse(version).unwrap()),
                        None,
                    )
                    .unwrap();
                assert_eq!(report.status, expected);
                assert!(report.require_admitted().is_err());
            }
            assert_eq!(
                knowledge
                    .assess(program.program, None, None)
                    .unwrap()
                    .status,
                CoreAdmissionStatus::Unrecognized
            );
        }
    }

    #[test]
    fn custom_build_metadata_is_not_a_prerelease_or_an_origin_claim() {
        let knowledge = embedded_core_knowledge().unwrap();
        let program = knowledge.program(ProgramKind::SingBox).unwrap();
        let release = &program.releases[0];
        let version = Version::parse(&format!("{}+local.build", release.version)).unwrap();
        assert_eq!(
            knowledge
                .assess(program.program, Some(&version), None)
                .unwrap()
                .require_admitted()
                .unwrap(),
            release
        );
        assert_eq!(
            knowledge
                .assess(program.program, None, Some(&release.commit_sha))
                .unwrap()
                .require_admitted()
                .unwrap(),
            release
        );
        assert_eq!(
            knowledge
                .assess(program.program, None, Some(&release.commit_sha[..7]))
                .unwrap()
                .status,
            CoreAdmissionStatus::Unrecognized
        );
        assert_eq!(
            knowledge
                .assess(
                    program.program,
                    Some(&version),
                    Some(&program.releases.last().unwrap().commit_sha)
                )
                .unwrap()
                .status,
            CoreAdmissionStatus::IdentityMismatch
        );
    }

    #[test]
    fn version_detection_does_not_guess_from_go_versions_or_arbitrary_text() {
        assert_eq!(
            parse_program_version(
                ProgramKind::SingBox,
                "sing-box version 1.14.0\nEnvironment: go1.26.0 windows/amd64"
            ),
            Some(Version::new(1, 14, 0))
        );
        assert_eq!(
            parse_program_version(
                ProgramKind::Xray,
                "Xray 26.3.27 (Xray, Penetrates Everything.) Custom (go1.26.0 windows/amd64)"
            ),
            Some(Version::new(26, 3, 27))
        );
        assert_eq!(
            parse_program_version(
                ProgramKind::Mihomo,
                "Mihomo Meta v1.19.30 windows amd64 with go1.26.0"
            ),
            Some(Version::new(1, 19, 30))
        );
        for output in [
            "sing-box custom 1.14.0",
            "sing-box version unknown\nEnvironment: 1.14.0",
            "warning: sing-box version 1.14.0",
            "sing-box version 1.14.0\nsing-box version 1.13.0",
        ] {
            assert!(parse_program_version(ProgramKind::SingBox, output).is_none());
        }
    }

    #[test]
    fn proxy_fields_and_custom_decoders_are_preserved_as_structure_not_semantic_proof() {
        let program = embedded_core_knowledge()
            .unwrap()
            .program(ProgramKind::Mihomo)
            .unwrap();
        assert!(
            program
                .declarations
                .iter()
                .any(|entry| entry.declaration.fields.iter().any(|field| field
                    .tags
                    .get("proxy")
                    .is_some_and(|tag| tag.split(',').next() == Some("handshake-timeout"))))
        );
        let program = embedded_core_knowledge()
            .unwrap()
            .program(ProgramKind::SingBox)
            .unwrap();
        assert!(program.declarations.iter().any(|entry| {
            entry
                .declaration
                .methods
                .iter()
                .any(|method| method.starts_with("Unmarshal"))
        }));
    }

    #[test]
    fn type_relationships_are_bounded_and_require_explicit_module_identity() {
        for shape in [
            KnowledgeTypeShape::Named {
                package: String::new(),
                name: "Options".into(),
            },
            KnowledgeTypeShape::Generic {
                target: Box::new(KnowledgeTypeShape::Dynamic),
                arguments: vec![],
            },
        ] {
            assert!(validate_type_shape(&shape, &mut 0, 0).is_err());
        }
        let mut shape = KnowledgeTypeShape::Dynamic;
        for _ in 0..65 {
            shape = KnowledgeTypeShape::Pointer {
                element: Box::new(shape),
            };
        }
        assert!(validate_type_shape(&shape, &mut 0, 0).is_err());
        let mut knowledge = embedded_core_knowledge().unwrap().clone();
        knowledge.programs[0].releases[0].module_path = "example.test/../other".into();
        assert!(knowledge.validate().is_err());
    }

    #[test]
    fn value_decoders_bind_unique_declarations_and_review_evidence() {
        let knowledge = embedded_core_knowledge().unwrap();
        let program = knowledge.program(ProgramKind::SingBox).unwrap();
        for release in &program.releases {
            let decoders: Vec<_> = program
                .value_decoders
                .iter()
                .filter(|entry| entry.releases.contains(&release.tag))
                .collect();
            assert_eq!(decoders.len(), 1);
            let decoder = &decoders[0].decoder;
            assert_eq!(decoder.declaration, "option#UDPOverTCPOptions");
            assert_eq!(
                decoder.scalar_forms,
                [KnowledgeScalarForm::Boolean, KnowledgeScalarForm::Null]
            );
            assert!(
                decoder
                    .evidence
                    .iter()
                    .all(|entry| release.source_commit(&entry.source).is_some())
            );
        }
        for mutation in 0..8 {
            let mut invalid = knowledge.clone();
            let program = invalid
                .programs
                .iter_mut()
                .find(|entry| entry.program == ProgramKind::SingBox)
                .unwrap();
            let variant = &mut program.value_decoders[0];
            match mutation {
                0 => variant.decoder.object_declaration = variant.decoder.declaration.clone(),
                1 => variant.decoder.evidence.clear(),
                2 => variant.decoder.evidence[0].body_hash = "not-a-hash".into(),
                3 => variant
                    .decoder
                    .scalar_forms
                    .push(KnowledgeScalarForm::Boolean),
                4 => variant.decoder.object_declaration = "missing#Options".into(),
                5 => variant.releases.clear(),
                6 => {
                    let duplicate = variant.clone();
                    program.value_decoders.push(duplicate);
                }
                _ => {
                    let duplicate = program
                        .declarations
                        .iter()
                        .find(|entry| entry.declaration.id == variant.decoder.object_declaration)
                        .unwrap()
                        .clone();
                    program.declarations.push(duplicate);
                }
            }
            assert!(invalid.validate().is_err(), "mutation {mutation}");
        }
    }

    #[test]
    fn dependency_declarations_bind_their_own_exact_commit_for_every_release() {
        let knowledge = embedded_core_knowledge().unwrap();
        let program = knowledge.program(ProgramKind::SingBox).unwrap();
        for release in &program.releases {
            let dependency = release
                .dependencies
                .iter()
                .find(|source| source.module_path == "github.com/sagernet/sing")
                .unwrap();
            let declaration = program
                .declarations
                .iter()
                .find(|entry| {
                    entry.releases.contains(&release.tag)
                        && entry.declaration.id
                            == "github.com/sagernet/sing/common/json/badoption#Duration"
                })
                .unwrap();
            assert_eq!(
                release.source_commit(&declaration.declaration.source),
                Some(dependency.commit_sha.as_str())
            );
            assert_ne!(dependency.commit_sha, release.commit_sha);
            assert_eq!(
                declaration.declaration.source.path,
                "common/json/badoption/duration.go"
            );
        }
        for mutation in 0..5 {
            let mut invalid = knowledge.clone();
            let program = invalid
                .programs
                .iter_mut()
                .find(|entry| entry.program == ProgramKind::SingBox)
                .unwrap();
            match mutation {
                0 => program.releases[0].dependencies.clear(),
                1 => program.releases[0].dependencies[0].commit_sha = "a".repeat(12),
                2 => {
                    let duplicate = program.releases[0].dependencies[0].clone();
                    program.releases[0].dependencies.push(duplicate);
                }
                3 => {
                    let declaration = program
                        .declarations
                        .iter_mut()
                        .find(|entry| {
                            entry.declaration.source.module_path == "github.com/sagernet/sing"
                        })
                        .unwrap();
                    declaration.declaration.source.module_path = "example.test/unrecorded".into();
                }
                _ => {
                    program.releases[0].dependencies[0].source_url =
                        program.releases[0].source_url.clone()
                }
            }
            assert!(invalid.validate().is_err(), "mutation {mutation}");
        }
    }

    #[test]
    fn decoder_bindings_follow_exact_releases_and_build_branches() {
        let knowledge = embedded_core_knowledge().unwrap();
        let sing_box = knowledge.program(ProgramKind::SingBox).unwrap();
        for release in &sing_box.releases {
            let bindings: Vec<_> = sing_box
                .decoder_bindings_for(&release.tag, "outbounds", "tuic")
                .collect();
            assert!(bindings.iter().any(|binding| {
                binding.constructor_status == KnowledgeConstructorStatus::Registered
                    && binding.build_constraint.contains("with_quic")
                    && !binding.build_constraint.contains("!with_quic")
                    && binding.declaration == "option#TUICOutboundOptions"
                    && binding.evidence.len() >= 3
            }));
            assert!(bindings.iter().any(|binding| {
                binding.constructor_status == KnowledgeConstructorStatus::Rejected
                    && binding.build_constraint.contains("!with_quic")
            }));
        }
        let first = &sing_box.releases.first().unwrap().tag;
        let latest = &sing_box.latest_stable;
        assert!(sing_box.decoder_bindings_for(first, "outbounds", "wireguard")
            .any(|binding| binding.constructor_status == KnowledgeConstructorStatus::Registered));
        let current: Vec<_> = sing_box
            .decoder_bindings_for(latest, "outbounds", "wireguard")
            .collect();
        assert!(!current.is_empty());
        assert!(
            current
                .iter()
                .all(|binding| binding.constructor_status == KnowledgeConstructorStatus::Rejected)
        );
        assert_eq!(
            sing_box
                .decoder_bindings_for("unrecognized", "outbounds", "tuic")
                .count(),
            0
        );
        let mihomo = knowledge.program(ProgramKind::Mihomo).unwrap();
        for release in &mihomo.releases {
            let binding = mihomo
                .decoder_bindings_for(&release.tag, "proxies", "hysteria2")
                .next()
                .unwrap();
            assert_eq!(binding.encoding, "proxy");
            assert_eq!(binding.declaration, "adapter/outbound#Hysteria2Option");
            assert!(binding.options_path.is_empty());
            let layout = binding.object_layout.as_ref().unwrap();
            assert!(layout.envelope_declarations.is_empty());
            assert_eq!(
                layout.key_comparison,
                KnowledgeFieldComparison::AsciiCaseInsensitiveUnderscore
            );
            assert!(layout.shared_options.iter().any(|option| {
                option.path == ["smux"] && option.declaration == "adapter/outbound#SingMuxOption"
            }));
            for symbol in [
                "Decoder.Decode",
                "Decoder.decodeStructFromMap",
                "DefaultKeyReplacer",
            ] {
                assert!(binding.evidence.iter().any(|evidence| {
                    evidence.source.path == "common/structure/structure.go"
                        && evidence.source.symbol == symbol
                }));
            }
        }
        let xray = knowledge.program(ProgramKind::Xray).unwrap();
        for release in &xray.releases {
            for value in ["direct", "freedom"] {
                let binding = xray
                    .decoder_bindings_for(&release.tag, "outbounds", value)
                    .next()
                    .unwrap();
                assert_eq!(binding.discriminator, "protocol");
                assert_eq!(binding.options_path, ["settings"]);
                assert_eq!(binding.declaration, "infra/conf#FreedomConfig");
                assert!(binding.matches_discriminator(&value.to_ascii_uppercase()));
                assert!(
                    binding
                        .evidence
                        .iter()
                        .any(|evidence| evidence.source.symbol == "JSONConfigLoader.LoadWithID")
                );
            }
        }
    }

    #[test]
    fn reported_tags_bind_patch_level_output_and_both_compiler_branches() {
        let knowledge = embedded_core_knowledge().unwrap();
        let program = knowledge.program(ProgramKind::Mihomo).unwrap();
        let earliest = &program.releases.first().unwrap().tag;
        let latest = &program.latest_stable;
        assert!(
            !program
                .reported_build_tags_for(earliest)
                .contains(&"no_tailscale")
        );
        assert!(
            program
                .reported_build_tags_for(latest)
                .contains(&"no_tailscale")
        );
        assert!(program.reported_build_tags_for("not-a-release").is_empty());
        for variant in &program.reported_build_tags {
            assert_eq!(variant.report.evidence.len(), 4);
            assert!(
                variant
                    .report
                    .evidence
                    .iter()
                    .any(|evidence| evidence.source.path == "main.go")
            );
            assert!(
                variant
                    .report
                    .evidence
                    .iter()
                    .any(|evidence| evidence.source.symbol == "Tags")
            );
        }
        let mut invalid = knowledge.clone();
        let program = invalid
            .programs
            .iter_mut()
            .find(|program| program.program == ProgramKind::Mihomo)
            .unwrap();
        program
            .reported_build_tags
            .push(program.reported_build_tags[0].clone());
        assert!(
            invalid
                .validate()
                .unwrap_err()
                .message
                .contains("unique maintained release")
        );
        let mut invalid = knowledge.clone();
        let program = invalid
            .programs
            .iter_mut()
            .find(|program| program.program == ProgramKind::Mihomo)
            .unwrap();
        program.reported_build_tags[0].report.evidence.pop();
        assert!(
            invalid
                .validate()
                .unwrap_err()
                .message
                .contains("source evidence")
        );
    }

    #[test]
    fn invalid_decoder_evidence_or_ambiguous_dispatch_is_rejected() {
        let knowledge = embedded_core_knowledge().unwrap();
        let mut invalid = knowledge.clone();
        invalid.programs[0].decoder_bindings[0]
            .binding
            .evidence
            .clear();
        assert!(invalid.validate().is_err());
        let mut invalid = knowledge.clone();
        let duplicate = invalid.programs[0].decoder_bindings[0].clone();
        invalid.programs[0].decoder_bindings.push(duplicate);
        assert!(invalid.validate().is_err());
        let mut invalid = knowledge.clone();
        invalid.programs[0].decoder_bindings[0].binding.declaration = "unresolved#Options".into();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn decoded_layout_rejects_missing_declarations_and_ambiguous_shared_paths() {
        let knowledge = embedded_core_knowledge().unwrap();
        for mutation in 0..5 {
            let mut invalid = knowledge.clone();
            let program = invalid
                .programs
                .iter_mut()
                .find(|program| program.program == ProgramKind::Mihomo)
                .unwrap();
            let binding = &mut program.decoder_bindings[0].binding;
            let layout = binding.object_layout.as_mut().unwrap();
            match mutation {
                0 => layout.shared_options[0].declaration = "missing#Options".into(),
                1 => layout.shared_options[0].path = vec!["*".into()],
                2 => layout.shared_options[0].path = vec![binding.discriminator.clone()],
                3 => {
                    let mut overlapping = layout.shared_options[0].clone();
                    overlapping.path.push("child".into());
                    layout.shared_options.push(overlapping);
                }
                4 => layout.envelope_declarations.push("missing#Envelope".into()),
                _ => unreachable!(),
            }
            assert!(invalid.validate().is_err(), "mutation {mutation}");
        }
    }

    #[test]
    fn normalized_identity_and_probe_implementation_must_match_observations() {
        let baseline = embedded_core_knowledge()
            .unwrap()
            .program(ProgramKind::SingBox)
            .unwrap()
            .releases
            .last()
            .unwrap();
        let probe = crate::CoreProbeReport::from_program_output(
            ProgramKind::SingBox,
            &format!("sing-box version {}", baseline.version),
        );
        assess_core_probe(ProgramKind::SingBox, &probe)
            .unwrap()
            .require_admitted()
            .unwrap();
        let mut changed = probe.clone();
        changed.normalized_version = Some("999.0.0".into());
        assert_eq!(
            assess_core_probe(ProgramKind::SingBox, &changed)
                .unwrap()
                .status,
            CoreAdmissionStatus::IdentityMismatch
        );
        changed = probe;
        changed.revision = "0".repeat(64);
        assert_eq!(
            assess_core_probe(ProgramKind::SingBox, &changed)
                .unwrap()
                .status,
            CoreAdmissionStatus::IdentityMismatch
        );
        assert_eq!(crate::CORE_IMPLEMENTATION_REVISION.len(), 64);
        let ambiguous = crate::CoreProbeReport::from_program_output(
            ProgramKind::SingBox,
            &format!(
                "sing-box version {}\nRevision: {}\nRevision: {}",
                baseline.version,
                "a".repeat(40),
                "b".repeat(40)
            ),
        );
        assert_eq!(
            assess_core_probe(ProgramKind::SingBox, &ambiguous)
                .unwrap()
                .status,
            CoreAdmissionStatus::IdentityMismatch
        );
    }
}
