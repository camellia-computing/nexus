use std::path::Path;

use crate::{
    ActionContext, ActionDescriptor, ActionPlan, CamelliaNexusError, CommandOutput, CommandPlan,
    DetectedBinary, EditorDescriptor, EditorLanguage, ErrorCode, LaunchPlan,
    PrivilegeAssessmentContext, PrivilegeConfigInput, PrivilegeReason, ProgramSpec, ProgramState,
    ProgramType, Result,
};

use super::{ProgramAdapter, base_launch_plan, config_privilege_inputs, tool_plan};

pub struct MihomoAdapter;

impl ProgramAdapter for MihomoAdapter {
    fn probe_plans(&self, executable: &Path, workspace: &Path) -> Vec<CommandPlan> {
        [vec!["-v".into()], vec!["-h".into()]]
            .into_iter()
            .map(|args| {
                let cwd = executable.parent().unwrap_or(workspace).to_path_buf();
                CommandPlan::tool(executable.to_path_buf(), args, cwd)
            })
            .collect()
    }

    fn verify_probe(&self, outputs: &[CommandOutput]) -> Result<DetectedBinary> {
        if outputs.len() != 2 || outputs.iter().any(|output| !output.success) {
            return Err(unsupported("Mihomo probe command failed"));
        }
        let version = combined(&outputs[0]);
        let help = combined(&outputs[1]);
        if !version.contains("Mihomo Meta")
            || !["-d", "-f", "-t"].iter().all(|flag| help.contains(flag))
        {
            return Err(unsupported("Mihomo CLI capabilities are unsupported"));
        }
        Ok(DetectedBinary {
            version: version
                .lines()
                .find(|line| !line.trim().is_empty())
                .map(|line| line.trim().to_owned()),
        })
    }

    fn launch_plan(&self, spec: &ProgramSpec, workspace: &Path) -> Result<LaunchPlan> {
        let ProgramType::Mihomo {
            main_config,
            extra_args,
        } = &spec.program_type
        else {
            return Err(CamelliaNexusError::invalid_spec(
                "Mihomo adapter received a different program kind",
            ));
        };
        let mut args = Vec::new();
        if !contains_option(extra_args, &["-d", "--d"]) {
            args.push(format!(
                "-d={}",
                spec.runtime_data_directory_path(workspace)
                    .to_string_lossy()
            ));
        }
        if let Some(main_config) = main_config {
            args.push(format!(
                "-f={}",
                workspace.join(main_config).to_string_lossy()
            ));
        }
        args.extend(extra_args.iter().cloned());
        Ok(base_launch_plan(spec, workspace, args, self))
    }

    fn editor(&self, spec: &ProgramSpec) -> Option<EditorDescriptor> {
        let ProgramType::Mihomo { main_config, .. } = &spec.program_type else {
            return None;
        };
        main_config.as_ref()?;
        Some(EditorDescriptor {
            language: EditorLanguage::Yaml,
            documentation_url: "https://wiki.metacubex.one/config/".into(),
            configuration_schema: None,
        })
    }

    fn validate_plan(&self, context: &ActionContext) -> Option<CommandPlan> {
        let ProgramType::Mihomo { extra_args, .. } = &context.spec.program_type else {
            return None;
        };
        let mut args = vec!["-t".into()];
        let selected = validation_arguments(extra_args);
        if !contains_option(&selected, &["-d", "--d"]) {
            args.push(format!(
                "-d={}",
                context
                    .spec
                    .runtime_data_directory_path(&context.workspace)
                    .to_string_lossy()
            ));
        }
        args.extend(selected);
        args.push(format!("-f={}", context.staged_config.to_string_lossy()));
        Some(tool_plan(&context.spec, &context.workspace, args))
    }

    fn actions(&self, _state: &ProgramState) -> Vec<ActionDescriptor> {
        Vec::new()
    }

    fn action_plan(&self, _action_id: &str, _context: &ActionContext) -> Result<ActionPlan> {
        Err(CamelliaNexusError::new(
            ErrorCode::NotFound,
            "Mihomo has no program-specific actions",
        ))
    }

    fn privilege_inputs(&self, args: &[String], cwd: &Path) -> Vec<PrivilegeConfigInput> {
        config_privilege_inputs(args, cwd, &["-f", "--f", "-config", "--config"], &[])
    }

    fn assess_privilege_configuration(
        &self,
        content: &[u8],
        context: PrivilegeAssessmentContext,
    ) -> Result<Option<Vec<PrivilegeReason>>> {
        crate::privileges::assess_mihomo_configuration(content, context).map(Some)
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

fn validation_arguments(args: &[String]) -> Vec<String> {
    let value_flags = ["-d", "--d"];
    let bool_flags = ["-m", "--m"];
    let mut selected = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if value_flags.contains(&argument.as_str()) {
            selected.push(argument.clone());
            if let Some(value) = args.get(index + 1) {
                selected.push(value.clone());
                index += 1;
            }
        } else if bool_flags.contains(&argument.as_str())
            || value_flags
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
    CamelliaNexusError::new(ErrorCode::UnsupportedBinary, "Unsupported Mihomo binary")
        .with_details(details)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn spec(extra_args: Vec<String>) -> ProgramSpec {
        ProgramSpec {
            schema_version: crate::SCHEMA_VERSION,
            id: crate::ProgramId::parse("mihomo-test").expect("id"),
            name: "Mihomo".into(),
            executable: crate::ExecutableSpec::Managed {
                path: "bin/mihomo".into(),
                metadata: None,
            },
            program_type: ProgramType::Mihomo {
                main_config: Some("config/managed.yaml".into()),
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

    fn assert_attached_path(argument: &str, option: &str, expected: &Path) {
        let prefix = format!("{option}=");
        let actual = argument
            .strip_prefix(&prefix)
            .expect("attached path option");
        assert_eq!(Path::new(actual), expected);
    }

    #[test]
    fn verifies_expected_capabilities() {
        let output = |text: &str| CommandOutput {
            code: Some(0),
            success: true,
            stdout: text.into(),
            stderr: String::new(),
        };
        let result = MihomoAdapter.verify_probe(&[
            output("Mihomo Meta 1.10.0 linux amd64"),
            output("-d string -f string -t"),
        ]);
        assert!(result.is_ok());
    }

    #[test]
    fn launch_uses_yaml_config_and_isolated_data_directory_without_run() {
        let plan = MihomoAdapter
            .launch_plan(&spec(Vec::new()), Path::new("workspace"))
            .expect("plan");
        assert_eq!(plan.args.len(), 2);
        assert_attached_path(&plan.args[0], "-d", &Path::new("workspace").join("data"));
        assert_attached_path(
            &plan.args[1],
            "-f",
            Path::new("workspace/config/managed.yaml"),
        );
    }

    #[test]
    fn user_home_directory_is_preserved() {
        let plan = MihomoAdapter
            .launch_plan(
                &spec(vec!["-d".into(), "/var/lib/mihomo".into(), "-m".into()]),
                Path::new("workspace"),
            )
            .expect("plan");
        assert_eq!(plan.args.len(), 4);
        assert_attached_path(
            &plan.args[0],
            "-f",
            Path::new("workspace/config/managed.yaml"),
        );
        assert_eq!(plan.args[1..], ["-d", "/var/lib/mihomo", "-m"]);
    }

    #[test]
    fn validation_keeps_runtime_options_and_targets_staged_yaml() {
        let context = ActionContext {
            spec: spec(vec!["-d=/var/lib/mihomo".into(), "-m".into(), "-v".into()]),
            workspace: "workspace".into(),
            staged_config: "workspace/config/staged.yaml".into(),
        };
        let plan = MihomoAdapter
            .validate_plan(&context)
            .expect("validation plan");
        assert_eq!(&plan.args[..3], ["-t", "-d=/var/lib/mihomo", "-m"]);
        assert_attached_path(
            &plan.args[3],
            "-f",
            Path::new("workspace/config/staged.yaml"),
        );
    }

    #[test]
    fn editor_uses_yaml_documentation() {
        let editor = MihomoAdapter.editor(&spec(Vec::new())).expect("editor");
        assert_eq!(editor.language, EditorLanguage::Yaml);
        assert_eq!(
            editor.documentation_url,
            "https://wiki.metacubex.one/config/"
        );
    }
}
