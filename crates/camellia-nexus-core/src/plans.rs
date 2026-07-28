use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use serde::{Deserialize, Serialize};

use crate::{
    ConfigurationSchemaDescriptor, ExecutableMetadata, PrivilegePolicy, ProgramId, ProgramKind,
    ProgramSpec,
};

pub const TOOL_TIMEOUT: Duration = Duration::from_secs(10);
pub const TOOL_OUTPUT_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct CommandPlan {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub environment: BTreeMap<String, String>,
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

impl CommandPlan {
    pub fn tool(executable: PathBuf, args: Vec<String>, cwd: PathBuf) -> Self {
        Self {
            executable,
            args,
            cwd,
            environment: BTreeMap::new(),
            timeout: TOOL_TIMEOUT,
            max_output_bytes: TOOL_OUTPUT_LIMIT,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfigurationSchemaPlan {
    pub descriptor: ConfigurationSchemaDescriptor,
    pub command: CommandPlan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchPlan {
    pub program_id: ProgramId,
    pub workspace: PathBuf,
    pub managed_executable: bool,
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub environment: BTreeMap<String, String>,
    pub stdout_log: PathBuf,
    pub stderr_log: PathBuf,
    pub program_kind: ProgramKind,
    pub privilege_policy: PrivilegePolicy,
    pub privilege_inputs: Vec<PrivilegeConfigInput>,
    pub interactive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PrivilegeConfigInput {
    File { path: PathBuf },
    Directory { path: PathBuf },
}

pub const PRIVILEGE_BROKER_PROTOCOL_VERSION: u32 = 2;

#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PrivilegeBrokerRequest {
    Launch {
        protocol_version: u32,
        request_id: String,
        plan: Box<LaunchPlan>,
    },
    Stop {
        request_id: String,
        program_id: ProgramId,
    },
    Shutdown {
        request_id: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PrivilegeBrokerEvent {
    Hello {
        protocol_version: u32,
        nonce: String,
        broker_pid: u32,
    },
    Started {
        request_id: String,
        program_id: ProgramId,
        pid: u32,
    },
    Exited {
        program_id: ProgramId,
        exit: ProcessExit,
    },
    Failed {
        request_id: String,
        program_id: Option<ProgramId>,
        error: crate::CamelliaNexusError,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandOutput {
    pub code: Option<i32>,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone)]
pub struct DetectedBinary {
    pub version: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ActionContext {
    pub spec: ProgramSpec,
    pub workspace: PathBuf,
    pub staged_config: PathBuf,
}

#[derive(Debug, Clone)]
pub enum ActionPlan {
    Run(CommandPlan),
    Format {
        command: CommandPlan,
        validate_after: CommandPlan,
    },
}

#[derive(Debug, Clone)]
pub struct CreateAssets {
    pub package_source: Option<PathBuf>,
    pub initial_config: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct StagedConfig {
    pub path: PathBuf,
    pub target: PathBuf,
    pub backup: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ProgramConfigTransaction {
    pub program_id: ProgramId,
    pub config_target: PathBuf,
}

#[derive(Debug, Clone)]
pub struct StagedPackage {
    pub program_id: ProgramId,
    pub staged_directory: PathBuf,
    pub executable: PathBuf,
    pub metadata: ExecutableMetadata,
}

#[derive(Debug, Clone)]
pub struct StoredProgram {
    pub spec: ProgramSpec,
    pub workspace: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct LoadReport {
    pub valid: Vec<StoredProgram>,
    pub invalid: Vec<InvalidProgram>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InvalidProgram {
    pub path: PathBuf,
    pub error: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessExit {
    pub code: Option<i32>,
    pub success: bool,
}
