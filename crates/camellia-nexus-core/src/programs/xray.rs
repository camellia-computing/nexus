use std::path::Path;

use crate::{
    ActionContext, ActionDescriptor, ActionPlan, CamelliaNexusError, CommandOutput, CommandPlan,
    DetectedBinary, EditorDescriptor, EditorLanguage, ErrorCode, LaunchPlan,
    PrivilegeAssessmentContext, PrivilegeConfigInput, PrivilegeReason, ProgramSpec, ProgramState,
    ProgramType, Result,
};

use super::{
    ProgramAdapter, all_action_states, base_launch_plan, config_privilege_inputs, omit_leading_run,
    tool_plan,
};

pub struct XrayAdapter;

impl ProgramAdapter for XrayAdapter {
    fn probe_plans(&self, executable: &Path, workspace: &Path) -> Vec<CommandPlan> {
        [vec!["version".into()], vec!["help".into(), "run".into()]]
            .into_iter()
            .map(|args| {
                let cwd = executable.parent().unwrap_or(workspace).to_path_buf();
                CommandPlan::tool(executable.to_path_buf(), args, cwd)
            })
            .collect()
    }

    fn verify_probe(&self, outputs: &[CommandOutput]) -> Result<DetectedBinary> {
        if outputs.len() != 2 || outputs.iter().any(|output| !output.success) {
            return Err(unsupported("Xray probe command failed"));
        }
        let version = combined(&outputs[0]);
        let help = combined(&outputs[1]);
        if !version.to_ascii_lowercase().contains("xray")
            || !["-c", "-format", "-test", "-dump"]
                .iter()
                .all(|flag| help.contains(flag))
        {
            return Err(unsupported("Xray CLI capabilities are unsupported"));
        }
        Ok(DetectedBinary {
            version: version
                .lines()
                .find(|line| !line.trim().is_empty())
                .map(|line| line.trim().to_owned()),
        })
    }

    fn launch_plan(&self, spec: &ProgramSpec, workspace: &Path) -> Result<LaunchPlan> {
        let ProgramType::Xray {
            main_config,
            extra_args,
        } = &spec.program_type
        else {
            return Err(CamelliaNexusError::invalid_spec(
                "Xray adapter received a different program kind",
            ));
        };
        let extra_args = omit_leading_run(extra_args);
        let mut args = vec!["run".into()];
        if !contains_option(extra_args, &["-format"]) {
            args.push("-format=json".into());
        }
        args.extend(extra_args.iter().cloned());
        if let Some(main_config) = main_config {
            args.extend([
                "-c".into(),
                workspace.join(main_config).to_string_lossy().into_owned(),
            ]);
        }
        Ok(base_launch_plan(spec, workspace, args, self))
    }

    fn editor(&self, spec: &ProgramSpec) -> Option<EditorDescriptor> {
        let ProgramType::Xray { main_config, .. } = &spec.program_type else {
            return None;
        };
        main_config.as_ref()?;
        Some(EditorDescriptor {
            language: EditorLanguage::Jsonc,
            documentation_url: "https://xtls.github.io/en/config/".into(),
            configuration_schema: None,
        })
    }

    fn validate_plan(&self, context: &ActionContext) -> Option<CommandPlan> {
        let ProgramType::Xray { extra_args, .. } = &context.spec.program_type else {
            return None;
        };
        let mut args = vec!["run".into(), "-test".into()];
        let selected = config_arguments(omit_leading_run(extra_args));
        if !contains_option(&selected, &["-format"]) {
            args.push("-format=json".into());
        }
        args.extend(selected);
        args.extend([
            "-c".into(),
            context.staged_config.to_string_lossy().into_owned(),
        ]);
        Some(tool_plan(&context.spec, &context.workspace, args))
    }

    fn actions(&self, _state: &ProgramState) -> Vec<ActionDescriptor> {
        vec![ActionDescriptor {
            id: "dump-config".into(),
            label: "Dump parsed configuration".into(),
            allowed_states: all_action_states(),
            confirmation: false,
        }]
    }

    fn action_plan(&self, action_id: &str, context: &ActionContext) -> Result<ActionPlan> {
        if action_id != "dump-config" {
            return Err(CamelliaNexusError::new(
                ErrorCode::NotFound,
                "Unknown Xray action",
            ));
        }
        let staged = &context.staged_config;
        let ProgramType::Xray { extra_args, .. } = &context.spec.program_type else {
            return Err(CamelliaNexusError::invalid_spec(
                "Xray action received a different program kind",
            ));
        };
        let selected = config_arguments(omit_leading_run(extra_args));
        let mut args = vec!["run".into(), "-test".into(), "-dump".into()];
        if !contains_option(&selected, &["-format"]) {
            args.push("-format=json".into());
        }
        args.extend(selected);
        args.extend(["-c".into(), staged.to_string_lossy().into_owned()]);
        Ok(ActionPlan::Run(tool_plan(
            &context.spec,
            &context.workspace,
            args,
        )))
    }

    fn privilege_inputs(&self, args: &[String], cwd: &Path) -> Vec<PrivilegeConfigInput> {
        config_privilege_inputs(args, cwd, &["-c", "-config"], &["-confdir"])
    }

    fn assess_privilege_configuration(
        &self,
        content: &[u8],
        context: PrivilegeAssessmentContext,
    ) -> Result<Option<Vec<PrivilegeReason>>> {
        crate::privileges::assess_json_proxy_configuration(content, context).map(Some)
    }
}

fn combined(output: &CommandOutput) -> String {
    format!("{}\n{}", output.stdout, output.stderr)
}

fn contains_option(args: &[String], flags: &[&str]) -> bool {
    args.iter().any(|argument| {
        flags
            .iter()
            .any(|flag| argument == flag || argument.starts_with(&format!("{flag}=")))
    })
}

fn config_arguments(args: &[String]) -> Vec<String> {
    let flags = ["-c", "-config", "-confdir", "-format"];
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

fn unsupported(details: &str) -> CamelliaNexusError {
    CamelliaNexusError::new(ErrorCode::UnsupportedBinary, "Unsupported Xray binary")
        .with_details(details)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn spec(extra_args: Vec<String>) -> ProgramSpec {
        ProgramSpec {
            schema_version: crate::SCHEMA_VERSION,
            id: crate::ProgramId::parse("xray-test").expect("id"),
            name: "Xray".into(),
            executable: crate::ExecutableSpec::Managed {
                path: "bin/xray".into(),
                metadata: None,
            },
            program_type: ProgramType::Xray {
                main_config: Some("config/managed.json".into()),
                extra_args,
            },
            managed_config: None,
            working_directory: "bin".into(),
            environment: BTreeMap::new(),
            auto_start: false,
            restart_policy: crate::RestartPolicy::Never,
            privilege_policy: Default::default(),
        }
    }

    #[test]
    fn verifies_expected_capabilities() {
        let output = |text: &str| CommandOutput {
            code: Some(0),
            success: true,
            stdout: text.into(),
            stderr: String::new(),
        };
        let result =
            XrayAdapter.verify_probe(&[output("Xray 26.6.1"), output("-c -format -test -dump")]);
        assert!(result.is_ok());
    }

    #[test]
    fn stored_config_is_merged_after_explicit_config() {
        let plan = XrayAdapter
            .launch_plan(&spec(vec!["-c=custom.json".into()]), Path::new("workspace"))
            .expect("plan");
        assert!(
            plan.args
                .iter()
                .any(|argument| argument == "-c=custom.json")
        );
        assert!(
            plan.args
                .iter()
                .any(|argument| Path::new(argument) == Path::new("workspace/config/managed.json"))
        );
        assert_eq!(
            plan.args.last().map(Path::new),
            Some(Path::new("workspace").join("config/managed.json").as_path())
        );
    }

    #[test]
    fn optional_run_subcommand_is_not_duplicated() {
        let mut external = spec(vec!["run".into(), "-c".into(), "xray.json".into()]);
        external.executable = crate::ExecutableSpec::External {
            path: "/tools/xray/xray".into(),
            metadata: None,
        };
        external.working_directory = "/tools/xray".into();

        let plan = XrayAdapter
            .launch_plan(&external, Path::new("workspace"))
            .expect("plan");

        assert_eq!(plan.cwd, Path::new("/tools/xray"));
        assert_eq!(
            &plan.args[..5],
            ["run", "-format=json", "-c", "xray.json", "-c"]
        );
        assert_eq!(
            plan.args.last().map(Path::new),
            Some(Path::new("workspace").join("config/managed.json").as_path())
        );
    }

    #[test]
    fn validation_uses_external_config_before_staged_override() {
        let context = ActionContext {
            spec: spec(vec!["-c".into(), "external.json".into()]),
            workspace: "workspace".into(),
            staged_config: "workspace/config/staged.json".into(),
        };
        let plan = XrayAdapter
            .validate_plan(&context)
            .expect("validation plan");
        let external = plan
            .args
            .iter()
            .position(|argument| argument == "external.json")
            .expect("external config");
        let staged = plan
            .args
            .iter()
            .position(|argument| Path::new(argument) == context.staged_config)
            .expect("staged config");
        assert!(external < staged);
    }

    #[test]
    fn user_format_and_config_arguments_are_preserved() {
        let plan = XrayAdapter
            .launch_plan(
                &spec(vec![
                    "-format=yaml".into(),
                    "-config".into(),
                    "input.yaml".into(),
                    "-dump".into(),
                ]),
                Path::new("workspace"),
            )
            .expect("plan");
        assert_eq!(
            &plan.args[..6],
            [
                "run",
                "-format=yaml",
                "-config",
                "input.yaml",
                "-dump",
                "-c"
            ]
        );
        assert_eq!(
            plan.args.last().map(Path::new),
            Some(Path::new("workspace").join("config/managed.json").as_path())
        );
    }
}
