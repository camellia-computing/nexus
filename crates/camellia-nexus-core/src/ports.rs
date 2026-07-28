use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;

use crate::{
    CommandOutput, CommandPlan, CreateAssets, ExecutableMetadata, LaunchPlan, LoadReport, LogChunk,
    LogStream, ProcessExit, ProgramConfigTransaction, ProgramId, ProgramSpec, RawConfig, Result,
    StagedConfig, StagedPackage,
};

#[async_trait]
pub trait ManagedProcess: Send {
    fn pid(&self) -> u32;
    async fn wait(&mut self) -> Result<ProcessExit>;
    async fn stop(&mut self) -> Result<ProcessExit>;
}

#[async_trait]
pub trait ProcessDriver: Send + Sync {
    async fn spawn(&self, plan: LaunchPlan) -> Result<Box<dyn ManagedProcess>>;
}

#[async_trait]
pub trait ToolRunner: Send + Sync {
    async fn run(&self, plan: CommandPlan) -> Result<CommandOutput>;
}

#[async_trait]
pub trait ProgramStore: Send + Sync {
    async fn load_all(&self) -> Result<LoadReport>;
    async fn create_pending(&self, spec: &ProgramSpec, assets: CreateAssets) -> Result<PathBuf>;
    async fn commit_create(&self, id: &ProgramId) -> Result<()>;
    async fn discard_pending(&self, id: &ProgramId) -> Result<()>;
    async fn save(&self, spec: &ProgramSpec) -> Result<()>;
    async fn workspace(&self, id: &ProgramId) -> Result<PathBuf>;
    async fn executable_metadata(&self, spec: &ProgramSpec) -> Result<ExecutableMetadata>;
    async fn stage_package(&self, spec: &ProgramSpec, source: &Path) -> Result<StagedPackage>;
    async fn commit_package(
        &self,
        staged: StagedPackage,
        expected_spec: &ProgramSpec,
        next_spec: &ProgramSpec,
    ) -> Result<()>;
    async fn discard_package(&self, staged: StagedPackage) -> Result<()>;
    async fn begin_program_config_update(
        &self,
        expected_spec: &ProgramSpec,
        next_spec: &ProgramSpec,
        staged_config: StagedConfig,
        expected_config_hash: &str,
    ) -> Result<ProgramConfigTransaction>;
    async fn finalize_program_config_update(
        &self,
        transaction: ProgramConfigTransaction,
    ) -> Result<()>;
    async fn rollback_program_config_update(
        &self,
        transaction: ProgramConfigTransaction,
    ) -> Result<()>;
    async fn read_log(
        &self,
        spec: &ProgramSpec,
        stream: LogStream,
        max_bytes: usize,
    ) -> Result<LogChunk>;
    async fn clear_logs(&self, spec: &ProgramSpec) -> Result<()>;
    async fn recover_workspace(&self, spec: &ProgramSpec) -> Result<()>;
    async fn remove_workspace(&self, id: &ProgramId) -> Result<()>;
}

#[async_trait]
pub trait ConfigStore: Send + Sync {
    async fn load(&self, spec: &ProgramSpec) -> Result<RawConfig>;
    async fn stage(&self, spec: &ProgramSpec, content: &[u8]) -> Result<StagedConfig>;
    async fn read_staged(&self, staged: &StagedConfig) -> Result<String>;
    async fn current_hash(&self, spec: &ProgramSpec) -> Result<String>;
    async fn atomic_replace_with_backup(
        &self,
        staged: StagedConfig,
        expected_hash: &str,
    ) -> Result<()>;
    async fn finalize_replace(&self, spec: &ProgramSpec) -> Result<()>;
    async fn restore_backup(&self, spec: &ProgramSpec) -> Result<()>;
    async fn discard_staged(&self, staged: StagedConfig) -> Result<()>;
    async fn recover(&self, spec: &ProgramSpec) -> Result<()>;
}

pub type DynProcessDriver = Arc<dyn ProcessDriver>;
pub type DynToolRunner = Arc<dyn ToolRunner>;
pub type DynProgramStore = Arc<dyn ProgramStore>;
pub type DynConfigStore = Arc<dyn ConfigStore>;
