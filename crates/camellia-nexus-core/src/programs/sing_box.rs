use std::path::Path;

use crate::{
    ActionContext, ActionDescriptor, ActionPlan, CamelliaNexusError, CommandOutput, CommandPlan,
    ConfigurationSchemaDescriptor, ConfigurationSchemaPlan, ConfigurationSchemaSource,
    CoreCompatibilityProfile, CoreFeatureAvailability, DetectedBinary, EditorDescriptor,
    EditorLanguage, ErrorCode, JsonSchemaDialect, LaunchPlan, MAX_CONFIGURATION_SCHEMA_BYTES,
    PrivilegeAssessmentContext, PrivilegeConfigInput, PrivilegeReason, ProgramSpec, ProgramState,
    ProgramType, Result,
};

use super::{
    ProgramAdapter, all_action_states, base_launch_plan, config_privilege_inputs, omit_leading_run,
    tool_plan,
};

pub struct SingBoxAdapter;

impl ProgramAdapter for SingBoxAdapter {
    fn probe_plans(&self, executable: &Path, workspace: &Path) -> Vec<CommandPlan> {
        [
            vec!["version".into()],
            vec!["check".into(), "--help".into()],
            vec!["format".into(), "--help".into()],
            vec!["schema".into(), "--help".into()],
        ]
        .into_iter()
        .map(|args| {
            let cwd = executable.parent().unwrap_or(workspace).to_path_buf();
            CommandPlan::tool(executable.to_path_buf(), args, cwd)
        })
        .collect()
    }

    fn verify_probe(&self, outputs: &[CommandOutput]) -> Result<DetectedBinary> {
        if outputs.len() != 4 || outputs[..3].iter().any(|output| !output.success) {
            return Err(unsupported("sing-box probe command failed"));
        }
        let version_text = combined(&outputs[0]).to_ascii_lowercase();
        let check_help = combined(&outputs[1]);
        let format_help = combined(&outputs[2]);
        if !version_text.contains("sing-box")
            || !contains_any(&check_help, &["-c", "--config"])
            || !contains_any(&check_help, &["-D", "--directory"])
            || !format_help.contains("-w")
        {
            return Err(unsupported("sing-box CLI capabilities are unsupported"));
        }
        let mut probe =
            crate::CoreProbeReport::from_reported_version(first_non_empty_line(&outputs[0]));
        probe.cli_observations = vec![
            crate::CoreCliObservation {
                id: "core.cli.nativeValidation".into(),
                available: true,
            },
            crate::CoreCliObservation {
                id: "core.cli.generatedSchema".into(),
                available: outputs[3].success,
            },
        ];
        let core_target = crate::embedded_core_compatibility_catalog()?.resolve_target(
            crate::ProgramKind::SingBox,
            &probe,
            &crate::CoreCompatibilityPreference::Automatic,
            None,
        )?;
        Ok(DetectedBinary {
            probe: Some(probe),
            core_target: Some(core_target),
        })
    }

    fn launch_plan(&self, spec: &ProgramSpec, workspace: &Path) -> Result<LaunchPlan> {
        let ProgramType::SingBox { extra_args } = &spec.program_type else {
            return Err(CamelliaNexusError::invalid_spec(
                "SingBox adapter received a different program kind",
            ));
        };
        let extra_args = omit_leading_run(extra_args);
        let mut args = vec!["run".into()];
        if !contains_option(extra_args, &["-D", "--directory"]) {
            args.extend([
                "-D".into(),
                spec.working_directory_path(workspace)
                    .to_string_lossy()
                    .into_owned(),
            ]);
        }
        args.extend(extra_args.iter().cloned());
        args.extend([
            "-c".into(),
            workspace
                .join(
                    spec.program_type
                        .main_config()
                        .expect("supported Core config"),
                )
                .to_string_lossy()
                .into_owned(),
        ]);
        Ok(base_launch_plan(spec, workspace, args, self))
    }

    fn editor(&self, spec: &ProgramSpec) -> Option<EditorDescriptor> {
        let ProgramType::SingBox { .. } = &spec.program_type else {
            return None;
        };
        Some(EditorDescriptor {
            language: EditorLanguage::Jsonc,
            documentation_url: "https://sing-box.sagernet.org/configuration/".into(),
            configuration_schema: sing_box_schema_descriptor(spec),
        })
    }

    fn configuration_schema_plan(
        &self,
        spec: &ProgramSpec,
        workspace: &Path,
    ) -> Option<ConfigurationSchemaPlan> {
        let descriptor = sing_box_schema_descriptor(spec)?;
        let mut command = tool_plan(spec, workspace, vec!["schema".into()]);
        command.max_output_bytes = MAX_CONFIGURATION_SCHEMA_BYTES;
        Some(ConfigurationSchemaPlan {
            descriptor,
            command,
        })
    }

    fn validate_plan(&self, context: &ActionContext) -> Option<CommandPlan> {
        let ProgramType::SingBox { extra_args, .. } = &context.spec.program_type else {
            return None;
        };
        let mut args = vec!["check".into()];
        args.extend(config_arguments(omit_leading_run(extra_args)));
        args.extend([
            "-c".into(),
            context.staged_config.to_string_lossy().into_owned(),
        ]);
        if !contains_option(&args, &["-D", "--directory"]) {
            args.extend([
                "-D".into(),
                context
                    .spec
                    .working_directory_path(&context.workspace)
                    .to_string_lossy()
                    .into_owned(),
            ]);
        }
        Some(tool_plan(&context.spec, &context.workspace, args))
    }

    fn actions(&self, _state: &ProgramState) -> Vec<ActionDescriptor> {
        vec![ActionDescriptor {
            id: "format-config".into(),
            label: "Format with sing-box".into(),
            allowed_states: all_action_states(),
            confirmation: false,
        }]
    }

    fn action_plan(&self, action_id: &str, context: &ActionContext) -> Result<ActionPlan> {
        if action_id != "format-config" {
            return Err(CamelliaNexusError::new(
                ErrorCode::NotFound,
                "Unknown sing-box action",
            ));
        }
        let staged = &context.staged_config;
        let ProgramType::SingBox { extra_args, .. } = &context.spec.program_type else {
            return Err(CamelliaNexusError::invalid_spec(
                "Sing-box action received a different program kind",
            ));
        };
        let mut args = vec!["format".into(), "-w".into()];
        let directory = directory_arguments(omit_leading_run(extra_args));
        if directory.is_empty() {
            args.extend([
                "-D".into(),
                context
                    .spec
                    .working_directory_path(&context.workspace)
                    .to_string_lossy()
                    .into_owned(),
            ]);
        } else {
            args.extend(directory);
        }
        args.extend(["-c".into(), staged.to_string_lossy().into_owned()]);
        let command = tool_plan(&context.spec, &context.workspace, args);
        let validate_after = self.validate_plan(context).ok_or_else(|| {
            CamelliaNexusError::new(
                ErrorCode::Internal,
                "Sing-box validation plan is unavailable",
            )
        })?;
        Ok(ActionPlan::Format {
            command,
            validate_after,
        })
    }

    fn privilege_inputs(&self, args: &[String], cwd: &Path) -> Vec<PrivilegeConfigInput> {
        config_privilege_inputs(
            args,
            cwd,
            &["-c", "--config"],
            &["-C", "--config-directory"],
        )
    }

    fn assess_privilege_configuration(
        &self,
        content: &[u8],
        context: PrivilegeAssessmentContext,
    ) -> Result<Option<Vec<PrivilegeReason>>> {
        crate::privileges::assess_json_proxy_configuration(content, context).map(Some)
    }
}

fn sing_box_schema_supported(spec: &ProgramSpec) -> bool {
    let Some(metadata) = spec.executable.metadata() else {
        return false;
    };
    if metadata
        .probe
        .as_ref()
        .and_then(|probe| probe.observes("core.cli.generatedSchema"))
        == Some(true)
    {
        return true;
    }
    metadata.core_target.as_ref().is_some_and(|target| {
        CoreCompatibilityProfile::resolve(target).is_ok_and(|profile| {
            profile
                .decision("core.cli.generatedSchema")
                .is_some_and(|decision| decision.availability == CoreFeatureAvailability::Supported)
        })
    })
}

fn sing_box_schema_descriptor(spec: &ProgramSpec) -> Option<ConfigurationSchemaDescriptor> {
    sing_box_schema_supported(spec).then_some(ConfigurationSchemaDescriptor {
        source: ConfigurationSchemaSource::ProgramBinary,
        dialect: JsonSchemaDialect::Draft202012,
    })
}

fn unsupported(details: &str) -> CamelliaNexusError {
    CamelliaNexusError::new(ErrorCode::UnsupportedBinary, "Unsupported sing-box binary")
        .with_details(details)
}

fn combined(output: &CommandOutput) -> String {
    format!("{}\n{}", output.stdout, output.stderr)
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn contains_option(args: &[String], flags: &[&str]) -> bool {
    args.iter().any(|argument| {
        flags
            .iter()
            .any(|flag| argument == flag || argument.starts_with(&format!("{flag}=")))
    })
}

fn config_arguments(args: &[String]) -> Vec<String> {
    select_arguments(
        args,
        &[
            "-c",
            "--config",
            "-C",
            "--config-directory",
            "-D",
            "--directory",
        ],
    )
}

fn directory_arguments(args: &[String]) -> Vec<String> {
    select_arguments(args, &["-D", "--directory"])
}

fn select_arguments(args: &[String], flags: &[&str]) -> Vec<String> {
    let mut selected = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if flags.contains(&argument.as_str()) {
            selected.push(argument.clone());
            if let Some(value) = args.get(index + 1) {
                selected.push(value.clone());
                index += 1;
            }
        } else if flags
            .iter()
            .any(|flag| argument.starts_with(&format!("{flag}=")))
        {
            selected.push(argument.clone());
        }
        index += 1;
    }
    selected
}

fn first_non_empty_line(output: &CommandOutput) -> Option<String> {
    combined(output)
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn spec(extra_args: Vec<String>) -> ProgramSpec {
        ProgramSpec {
            schema_version: crate::SCHEMA_VERSION,
            id: crate::ProgramId::parse("sing-box-test").expect("id"),
            name: "sing-box".into(),
            executable: crate::ExecutableSpec::Managed {
                path: "bin/sing-box".into(),
                compatibility: Default::default(),
                metadata: None,
            },
            program_type: ProgramType::SingBox { extra_args },
            managed_config: None,
            working_directory: "bin".into(),
            environment: BTreeMap::new(),
            auto_start: false,
            restart_policy: crate::RestartPolicy::Never,
            privilege_policy: Default::default(),
        }
    }

    fn spec_with_version(version: &str) -> ProgramSpec {
        let mut spec = spec(Vec::new());
        let probe = crate::CoreProbeReport::from_reported_version(Some(format!(
            "sing-box version {version}"
        )));
        let target = crate::embedded_core_compatibility_catalog()
            .unwrap()
            .resolve_target(
                crate::ProgramKind::SingBox,
                &probe,
                &crate::CoreCompatibilityPreference::Automatic,
                Some("a".repeat(64)),
            )
            .unwrap();
        spec.executable.set_metadata(crate::ExecutableMetadata {
            fingerprint: crate::CoreBinaryFingerprint {
                sha256: "a".repeat(64),
                size: 1,
                modified_unix_ms: 1,
            },
            probe: Some(probe),
            core_target: Some(target),
        });
        spec
    }

    #[test]
    fn verifies_expected_capabilities() {
        let output = |text: &str| CommandOutput {
            code: Some(0),
            success: true,
            stdout: text.into(),
            stderr: String::new(),
        };
        let result = SingBoxAdapter.verify_probe(&[
            output("sing-box version 1.12.0"),
            output("-c --config -D --directory"),
            output("-w -c --config"),
            CommandOutput {
                code: Some(1),
                success: false,
                stdout: String::new(),
                stderr: "unknown command schema".into(),
            },
        ]);
        let detected = result.expect("probe");
        assert_eq!(
            detected.probe.unwrap().normalized_version.as_deref(),
            Some("1.12.0")
        );
        assert_eq!(
            detected.core_target.unwrap().basis,
            crate::CoreCompatibilityBasis::BinaryReported
        );
    }

    #[test]
    fn schema_capability_starts_at_beta_two() {
        let workspace = Path::new("workspace");
        assert!(
            SingBoxAdapter
                .configuration_schema_plan(&spec_with_version("1.14.0-beta.1"), workspace)
                .is_none()
        );
        let beta = spec_with_version("1.14.0-beta.2");
        assert!(
            SingBoxAdapter
                .editor(&beta)
                .and_then(|editor| editor.configuration_schema)
                .is_some()
        );
        let plan = SingBoxAdapter
            .configuration_schema_plan(&beta, workspace)
            .expect("schema plan");
        assert_eq!(plan.command.args, ["schema"]);
        assert_eq!(
            plan.command.max_output_bytes,
            crate::MAX_CONFIGURATION_SCHEMA_BYTES
        );
        assert_eq!(
            plan.descriptor,
            ConfigurationSchemaDescriptor {
                source: ConfigurationSchemaSource::ProgramBinary,
                dialect: JsonSchemaDialect::Draft202012,
            }
        );
        assert!(
            SingBoxAdapter
                .configuration_schema_plan(&spec_with_version("1.14.0-beta.14"), workspace)
                .is_some()
        );
        assert!(
            SingBoxAdapter
                .configuration_schema_plan(&spec_with_version("1.14.0-rc.1"), workspace)
                .is_some()
        );
    }

    #[test]
    fn launch_uses_the_single_client_owned_active_config() {
        let plan = SingBoxAdapter
            .launch_plan(&spec(Vec::new()), Path::new("workspace"))
            .expect("plan");
        assert!(
            plan.args
                .iter()
                .any(|argument| Path::new(argument) == Path::new("workspace/config/active.json"))
        );
        assert_eq!(
            plan.args.last().map(Path::new),
            Some(Path::new("workspace").join("config/active.json").as_path())
        );
    }

    #[test]
    fn managed_config_is_absolute_when_package_is_the_working_directory() {
        let workspace = Path::new("workspace");
        let plan = SingBoxAdapter
            .launch_plan(&spec(Vec::new()), workspace)
            .expect("plan");
        assert_eq!(plan.cwd, workspace.join("bin"));
        assert!(
            plan.args
                .iter()
                .any(|argument| Path::new(argument) == workspace.join("config/active.json"))
        );
        let directory_index = plan
            .args
            .iter()
            .position(|argument| argument == "-D")
            .expect("directory option");
        assert_eq!(
            Path::new(&plan.args[directory_index + 1]),
            workspace.join("bin")
        );
    }

    #[test]
    fn validation_uses_only_the_staged_candidate() {
        let context = ActionContext {
            spec: spec(Vec::new()),
            workspace: "workspace".into(),
            staged_config: "workspace/config/staged.json".into(),
        };
        let plan = SingBoxAdapter
            .validate_plan(&context)
            .expect("validation plan");
        let staged = plan
            .args
            .iter()
            .position(|argument| Path::new(argument) == context.staged_config)
            .expect("staged config");
        assert_eq!(plan.args[staged.saturating_sub(1)], "-c");
    }

    #[test]
    fn format_respects_the_user_directory_without_touching_external_configs() {
        let context = ActionContext {
            spec: spec(vec![
                "--config".into(),
                "external.json".into(),
                "--directory".into(),
                "assets".into(),
            ]),
            workspace: "workspace".into(),
            staged_config: "workspace/config/staged.json".into(),
        };
        let ActionPlan::Format { command, .. } = SingBoxAdapter
            .action_plan("format-config", &context)
            .expect("format plan")
        else {
            panic!("expected format plan");
        };
        assert!(
            command
                .args
                .windows(2)
                .any(|pair| pair == ["--directory", "assets"])
        );
        assert!(
            !command
                .args
                .iter()
                .any(|argument| argument == "external.json")
        );
    }

    #[test]
    fn optional_run_subcommand_is_not_duplicated() {
        let mut external = spec(vec!["run".into()]);
        external.executable = crate::ExecutableSpec::External {
            path: "/tools/sing-box/sing-box".into(),
            compatibility: Default::default(),
            metadata: None,
        };
        external.working_directory = "/tools/sing-box".into();

        let plan = SingBoxAdapter
            .launch_plan(&external, Path::new("workspace"))
            .expect("plan");

        assert_eq!(plan.cwd, Path::new("/tools/sing-box"));
        assert_eq!(&plan.args[..4], ["run", "-D", "/tools/sing-box", "-c"]);
        assert_eq!(
            plan.args.last().map(Path::new),
            Some(Path::new("workspace/config/active.json"))
        );
    }
}
