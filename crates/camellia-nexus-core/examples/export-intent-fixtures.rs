//! Generates read-only native capability fixtures for the browser regression backend.
use camellia_nexus_core::*;
use std::collections::{BTreeMap, BTreeSet};

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let destination = arguments.next().ok_or("fixture destination required")?;
    let settings_destination = arguments.next().ok_or("settings destination required")?;
    let baselines_destination = arguments.next().ok_or("baseline destination required")?;
    let knowledge = embedded_core_knowledge()?;
    let mut catalogs = BTreeMap::new();
    let mut settings = BTreeMap::new();
    let mut baselines = BTreeMap::new();
    for program in [ProgramKind::SingBox, ProgramKind::Xray, ProgramKind::Mihomo] {
        for platform in ["windows", "linux"] {
            let release = knowledge
                .program(program)
                .ok_or("program missing")?
                .releases
                .last()
                .ok_or("release missing")?;
            let output = match program {
                ProgramKind::SingBox => format!(
                    "sing-box version {}\nEnvironment: go1.26.0 {platform}/amd64\nTags: with_gvisor,with_quic",
                    release.version
                ),
                ProgramKind::Xray => format!(
                    "Xray {} (Xray, Penetrates Everything.) Custom (go1.26.0 {platform}/amd64)",
                    release.version
                ),
                ProgramKind::Mihomo => format!(
                    "Mihomo Meta v{} {platform} amd64 with go1.26.0\nUse tags: with_gvisor",
                    release.version
                ),
                _ => unreachable!(),
            };
            let profile = CoreCapabilityProfile::resolve(
                program,
                &CoreProbeReport::from_program_output(program, &output),
                &CoreBinaryFingerprint {
                    sha256: "a".repeat(64),
                    size: 1,
                    modified_unix_ms: 1,
                },
            )?;
            let catalog = configuration_intent_catalog(&profile)?;
            let target = CoreTargetIdentity::unknown(program, None).bind_fingerprint(
                &CoreBinaryFingerprint {
                    sha256: profile.binary_sha256.clone(),
                    size: 1,
                    modified_unix_ms: 1,
                },
            );
            let state = ConfigurationState::from_merge(
                program,
                0,
                0,
                merge_configuration_sources(
                    program,
                    &[SourceSnapshot::parse(
                        "fixture",
                        "fixture",
                        ConfigurationFormat::for_kind(program)
                            .ok_or("configuration format missing")?,
                        b"{}",
                        0,
                        false,
                    )?],
                )?,
                CoreCompatibilityProfile::resolve(&target)?,
            )?;
            let mut setting_nodes = Vec::new();
            for node in catalog
                .nodes
                .iter()
                .filter(|node| node.setting_id.is_some())
            {
                if let Some(setting) =
                    configuration_intent_context(&profile, &state, Some(&node.node_id))?.setting
                {
                    setting_nodes
                        .push(serde_json::json!({"descriptor":setting, "pattern":node.pattern}));
                }
            }
            let node_index = catalog
                .nodes
                .iter()
                .map(|node| (&node.node_id, node))
                .collect::<BTreeMap<_, _>>();
            let mut relation_nodes = BTreeSet::new();
            for mut node in catalog
                .nodes
                .iter()
                .filter(|node| !node.semantic_rule_ids.is_empty())
            {
                while relation_nodes.insert(&node.node_id) {
                    let Some(parent) = &node.parent_id else { break };
                    node = node_index
                        .get(parent)
                        .ok_or("semantic field ancestor missing")?;
                }
            }
            let nodes = catalog
                .nodes
                .iter()
                .filter(|node| node.pattern.len() <= 4 || relation_nodes.contains(&node.node_id))
                .map(|node| {
                    let mut value = serde_json::to_value(node)?;
                    let fields = value.as_object_mut().ok_or("node object required")?;
                    for key in ["shape", "source", "sourceCommit", "semanticRuleIds"] {
                        fields.remove(key);
                    }
                    Ok(value)
                })
                .collect::<std::result::Result<Vec<_>, Box<dyn std::error::Error>>>()?;
            let program_name = serde_json::to_value(program)?
                .as_str()
                .ok_or("program name missing")?
                .to_owned();
            if platform == "windows" {
                let descriptor = knowledge.program(program).ok_or("program missing")?;
                let previous_prefix = format!(
                    "{}.",
                    descriptor
                        .families
                        .get(1)
                        .ok_or("previous family missing")?
                );
                let previous = descriptor
                    .releases
                    .iter()
                    .rev()
                    .find(|release| release.version.starts_with(&previous_prefix))
                    .ok_or("previous release missing")?;
                baselines.insert(
                    program_name.clone(),
                    serde_json::json!({
                        "families": descriptor.families, "latest": release, "previous": previous,
                        "knowledgeHash": knowledge.content_hash,
                    }),
                );
            }
            let key = if platform == "windows" {
                program_name
            } else {
                format!("{program_name}-{platform}")
            };
            settings.insert(key.clone(), setting_nodes);
            catalogs.insert(
                key,
                serde_json::json!({ "nodes": nodes, "relations": catalog.relations }),
            );
        }
    }
    std::fs::write(destination, serde_json::to_vec(&catalogs)?)?;
    std::fs::write(settings_destination, serde_json::to_vec(&settings)?)?;
    std::fs::write(baselines_destination, serde_json::to_vec(&baselines)?)?;
    Ok(())
}
