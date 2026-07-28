mod generic;
mod mihomo;
mod sing_box;
mod xray;

use std::path::Path;

use crate::{
    ActionContext, ActionDescriptor, ActionPlan, ActionState, CommandOutput, CommandPlan,
    ConfigurationSchemaPlan, DetectedBinary, EditorDescriptor, LaunchPlan,
    PrivilegeAssessmentContext, PrivilegeConfigInput, PrivilegeReason, ProgramKind, ProgramSpec,
    ProgramState, Result,
};

pub use generic::GenericAdapter;
pub use mihomo::MihomoAdapter;
pub use sing_box::SingBoxAdapter;
pub use xray::XrayAdapter;

pub trait ProgramAdapter: Send + Sync {
    fn probe_plans(&self, executable: &Path, workspace: &Path) -> Vec<CommandPlan>;
    fn verify_probe(&self, outputs: &[CommandOutput]) -> Result<DetectedBinary>;
    fn launch_plan(&self, spec: &ProgramSpec, workspace: &Path) -> Result<LaunchPlan>;
    fn editor(&self, spec: &ProgramSpec) -> Option<EditorDescriptor>;
    fn configuration_schema_plan(
        &self,
        _spec: &ProgramSpec,
        _workspace: &Path,
    ) -> Option<ConfigurationSchemaPlan> {
        None
    }
    fn validate_plan(&self, context: &ActionContext) -> Option<CommandPlan>;
    fn actions(&self, state: &ProgramState) -> Vec<ActionDescriptor>;
    fn action_plan(&self, action_id: &str, context: &ActionContext) -> Result<ActionPlan>;
    fn privilege_inputs(&self, args: &[String], cwd: &Path) -> Vec<PrivilegeConfigInput>;
    fn assess_privilege_configuration(
        &self,
        content: &[u8],
        context: PrivilegeAssessmentContext,
    ) -> Result<Option<Vec<PrivilegeReason>>>;
}

#[derive(Clone, Default)]
pub struct AdapterRegistry {
    _private: (),
}

impl AdapterRegistry {
    pub fn get(&self, kind: ProgramKind) -> &'static dyn ProgramAdapter {
        static GENERIC: GenericAdapter = GenericAdapter;
        static SING_BOX: SingBoxAdapter = SingBoxAdapter;
        static XRAY: XrayAdapter = XrayAdapter;
        static MIHOMO: MihomoAdapter = MihomoAdapter;

        match kind {
            ProgramKind::Generic => &GENERIC,
            ProgramKind::SingBox => &SING_BOX,
            ProgramKind::Xray => &XRAY,
            ProgramKind::Mihomo => &MIHOMO,
        }
    }
}

pub(crate) fn base_launch_plan(
    spec: &ProgramSpec,
    workspace: &Path,
    args: Vec<String>,
    adapter: &dyn ProgramAdapter,
) -> LaunchPlan {
    let mut environment = spec.environment.clone();
    environment.insert(
        "CAMELLIA_NEXUS_WORKSPACE".into(),
        workspace.to_string_lossy().into_owned(),
    );
    environment.insert(
        "CAMELLIA_NEXUS_DATA_DIR".into(),
        spec.runtime_data_directory_path(workspace)
            .to_string_lossy()
            .into_owned(),
    );
    let cwd = spec.working_directory_path(workspace);
    let privilege_inputs = adapter.privilege_inputs(&args, &cwd);
    LaunchPlan {
        program_id: spec.id.clone(),
        workspace: workspace.to_path_buf(),
        managed_executable: spec.executable.is_managed(),
        executable: spec.executable_path(workspace),
        args,
        cwd,
        environment,
        stdout_log: spec.log_path(workspace, "stdout.log"),
        stderr_log: spec.log_path(workspace, "stderr.log"),
        program_kind: spec.program_type.kind(),
        privilege_policy: spec.privilege_policy,
        privilege_inputs,
        interactive: false,
    }
}

pub(crate) fn config_privilege_inputs(
    args: &[String],
    cwd: &Path,
    file_flags: &[&str],
    directory_flags: &[&str],
) -> Vec<PrivilegeConfigInput> {
    let mut inputs = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        let mut matched = false;
        for (flags, directory) in [(file_flags, false), (directory_flags, true)] {
            for flag in flags {
                let value = if argument == flag {
                    args.get(index + 1).map(String::as_str)
                } else {
                    argument
                        .strip_prefix(flag)
                        .and_then(|suffix| suffix.strip_prefix('='))
                };
                let Some(value) = value.filter(|value| !value.is_empty()) else {
                    continue;
                };
                let path = resolve_argument_path(cwd, value);
                inputs.push(if directory {
                    PrivilegeConfigInput::Directory { path }
                } else {
                    PrivilegeConfigInput::File { path }
                });
                if argument == flag {
                    index += 1;
                }
                matched = true;
                break;
            }
            if matched {
                break;
            }
        }
        index += 1;
    }
    inputs
}

fn resolve_argument_path(cwd: &Path, value: &str) -> std::path::PathBuf {
    let path = std::path::PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

pub(crate) fn tool_plan(spec: &ProgramSpec, workspace: &Path, args: Vec<String>) -> CommandPlan {
    let mut plan = CommandPlan::tool(
        spec.executable_path(workspace),
        args,
        spec.working_directory_path(workspace),
    );
    plan.environment = spec.environment.clone();
    plan.environment.insert(
        "CAMELLIA_NEXUS_WORKSPACE".into(),
        workspace.to_string_lossy().into_owned(),
    );
    plan.environment.insert(
        "CAMELLIA_NEXUS_DATA_DIR".into(),
        spec.runtime_data_directory_path(workspace)
            .to_string_lossy()
            .into_owned(),
    );
    plan
}

pub(crate) fn omit_leading_run(args: &[String]) -> &[String] {
    if args.first().is_some_and(|argument| argument == "run") {
        &args[1..]
    } else {
        args
    }
}

pub(crate) fn all_action_states() -> Vec<ActionState> {
    vec![
        ActionState::Stopped,
        ActionState::Starting,
        ActionState::Running,
        ActionState::Stopping,
        ActionState::Exited,
        ActionState::Backoff,
        ActionState::Error,
    ]
}

#[cfg(test)]
mod privilege_tests {
    use std::path::{Path, PathBuf};

    use crate::{PrivilegeConfigInput, ProgramKind};

    use super::config_privilege_inputs;

    #[test]
    fn every_program_kind_has_a_complete_adapter_registration() {
        let registry = super::AdapterRegistry::default();
        for kind in ProgramKind::ALL {
            let adapter = registry.get(kind);
            let result = adapter
                .assess_privilege_configuration(
                    b"{}",
                    crate::PrivilegeAssessmentContext {
                        privileged_ports_require_elevation: false,
                    },
                )
                .expect("adapter assessment contract");
            if kind == ProgramKind::Generic {
                assert!(result.is_none());
            } else {
                assert!(result.is_some());
            }
        }
    }

    #[test]
    fn actual_sing_box_config_arguments_become_privilege_inputs() {
        let inputs = config_privilege_inputs(
            &[
                "run".into(),
                "--config=external.json".into(),
                "--config-directory".into(),
                "parts".into(),
                "-c".into(),
                "/managed/config.json".into(),
            ],
            Path::new("/workspace/bin"),
            &["-c", "--config"],
            &["-C", "--config-directory"],
        );
        assert_eq!(
            inputs,
            vec![
                PrivilegeConfigInput::File {
                    path: PathBuf::from("/workspace/bin/external.json"),
                },
                PrivilegeConfigInput::Directory {
                    path: PathBuf::from("/workspace/bin/parts"),
                },
                PrivilegeConfigInput::File {
                    path: PathBuf::from("/managed/config.json"),
                },
            ]
        );
    }

    #[test]
    fn mihomo_attached_config_argument_is_resolved_from_the_working_directory() {
        assert_eq!(
            config_privilege_inputs(
                &["-f=profile.yaml".into()],
                Path::new("/workspace/data"),
                &["-f", "--f", "-config", "--config"],
                &[],
            ),
            vec![PrivilegeConfigInput::File {
                path: PathBuf::from("/workspace/data/profile.yaml"),
            }]
        );
    }
}
