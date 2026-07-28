use std::path::Path;

use crate::{
    ActionContext, ActionDescriptor, ActionPlan, CamelliaNexusError, CommandOutput, CommandPlan,
    DetectedBinary, EditorDescriptor, ErrorCode, LaunchPlan, PrivilegeAssessmentContext,
    PrivilegeConfigInput, PrivilegeReason, ProgramSpec, ProgramState, ProgramType, Result,
};

use super::{ProgramAdapter, base_launch_plan};

pub struct GenericAdapter;

impl ProgramAdapter for GenericAdapter {
    fn probe_plans(&self, _executable: &Path, _workspace: &Path) -> Vec<CommandPlan> {
        Vec::new()
    }

    fn verify_probe(&self, _outputs: &[CommandOutput]) -> Result<DetectedBinary> {
        Ok(DetectedBinary { version: None })
    }

    fn launch_plan(&self, spec: &ProgramSpec, workspace: &Path) -> Result<LaunchPlan> {
        let ProgramType::Generic { args } = &spec.program_type else {
            return Err(CamelliaNexusError::new(
                ErrorCode::InvalidSpec,
                "Generic adapter received non-generic spec",
            ));
        };
        Ok(base_launch_plan(spec, workspace, args.clone(), self))
    }

    fn editor(&self, _spec: &ProgramSpec) -> Option<EditorDescriptor> {
        None
    }

    fn validate_plan(&self, _context: &ActionContext) -> Option<CommandPlan> {
        None
    }

    fn actions(&self, _state: &ProgramState) -> Vec<ActionDescriptor> {
        Vec::new()
    }

    fn action_plan(&self, _action_id: &str, _context: &ActionContext) -> Result<ActionPlan> {
        Err(CamelliaNexusError::new(
            ErrorCode::NotFound,
            "Generic programs have no program-specific actions",
        ))
    }

    fn privilege_inputs(&self, _args: &[String], _cwd: &Path) -> Vec<PrivilegeConfigInput> {
        Vec::new()
    }

    fn assess_privilege_configuration(
        &self,
        _content: &[u8],
        _context: PrivilegeAssessmentContext,
    ) -> Result<Option<Vec<PrivilegeReason>>> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn preserves_every_user_argument_for_external_programs() {
        let spec = ProgramSpec {
            schema_version: crate::SCHEMA_VERSION,
            id: crate::ProgramId::parse("generic-test").expect("id"),
            name: "Generic".into(),
            executable: crate::ExecutableSpec::External {
                path: "/tools/program/program".into(),
                metadata: None,
            },
            program_type: ProgramType::Generic {
                args: vec![
                    "run".into(),
                    "-c".into(),
                    "input.json".into(),
                    "--output".into(),
                    "result.txt".into(),
                ],
            },
            managed_config: None,
            working_directory: "/tools/program".into(),
            environment: BTreeMap::new(),
            auto_start: false,
            restart_policy: crate::RestartPolicy::Never,
            privilege_policy: Default::default(),
        };

        let plan = GenericAdapter
            .launch_plan(&spec, Path::new("workspace"))
            .expect("plan");

        assert_eq!(plan.cwd, Path::new("/tools/program"));
        assert_eq!(
            plan.args,
            ["run", "-c", "input.json", "--output", "result.txt"]
        );
    }
}
