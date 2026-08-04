use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock, broadcast, watch};

use crate::{
    ActionDescriptor, ActionResult, AdapterRegistry, CamelliaNexusError, ConfigDocument,
    ConfigService, ConfigurationSchemaDocument, CreateAssets, DynConfigStore, DynProcessDriver,
    DynProgramStore, DynToolRunner, ErrorCode, LoadReport, LogChunk, LogStream, ManagerEvent,
    Mutation, PreparedConfigGuard, ProgramId, ProgramSpec, ProgramState, ProgramSummary, Result,
    ValidationResult,
};

const EVENT_CAPACITY: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProgramRequest {
    pub spec: ProgramSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_source: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_config: Option<String>,
}

pub struct ProgramManager {
    controllers: RwLock<HashMap<ProgramId, crate::ControllerHandle>>,
    driver: DynProcessDriver,
    program_store: DynProgramStore,
    config_store: DynConfigStore,
    config_service: Arc<ConfigService>,
    adapters: AdapterRegistry,
    events: broadcast::Sender<ManagerEvent>,
    registry_mutations: Mutex<()>,
    automatic_restarts_enabled: Arc<AtomicBool>,
    automatic_lifecycle_gate: Arc<RwLock<()>>,
    auto_start_reconciliation: Mutex<()>,
    pending_auto_starts: Mutex<HashSet<ProgramId>>,
    package_preparations: Mutex<HashMap<ProgramId, Arc<Mutex<()>>>>,
    lifecycle: ManagerLifecycle,
}

struct ManagerLifecycle {
    gate: Arc<RwLock<()>>,
    shutdown_started: AtomicBool,
    shutdown_signal: watch::Sender<bool>,
}

impl Default for ManagerLifecycle {
    fn default() -> Self {
        let (shutdown_signal, _) = watch::channel(false);
        Self {
            gate: Arc::new(RwLock::new(())),
            shutdown_started: AtomicBool::new(false),
            shutdown_signal,
        }
    }
}

impl ManagerLifecycle {
    async fn mutation_permit(&self) -> Result<tokio::sync::OwnedRwLockReadGuard<()>> {
        let permit = self.gate.clone().read_owned().await;
        if self.shutdown_started.load(Ordering::Acquire) {
            return Err(manager_shutting_down_error());
        }
        Ok(permit)
    }

    async fn begin_shutdown(&self) -> Option<tokio::sync::OwnedRwLockWriteGuard<()>> {
        if self.shutdown_started.swap(true, Ordering::AcqRel) {
            return None;
        }
        // Publish closing before waiting for existing readers so long-lived mutations can
        // quiesce themselves instead of preventing the shutdown writer from ever arriving.
        self.shutdown_signal.send_replace(true);
        Some(self.gate.clone().write_owned().await)
    }

    fn is_shutdown_started(&self) -> bool {
        self.shutdown_started.load(Ordering::Acquire)
    }

    async fn shutdown_requested(&self) {
        let mut shutdown = self.shutdown_signal.subscribe();
        loop {
            if *shutdown.borrow_and_update() {
                return;
            }
            if shutdown.changed().await.is_err() {
                return;
            }
        }
    }
}

fn manager_shutting_down_error() -> CamelliaNexusError {
    CamelliaNexusError::new(ErrorCode::InvalidState, "Program manager is shutting down")
}

#[derive(Debug)]
pub struct PreparedProgramUpdate {
    expected_spec: ProgramSpec,
    next_spec: ProgramSpec,
}

#[derive(Debug)]
pub struct PreparedProgramCreate {
    spec: ProgramSpec,
    workspace: PathBuf,
}

pub struct PreparedPackageGuard {
    expected_spec: ProgramSpec,
    next_spec: ProgramSpec,
    staged: crate::StagedPackage,
    _preparation: tokio::sync::OwnedMutexGuard<()>,
    _operation: tokio::sync::OwnedRwLockReadGuard<()>,
}

impl PreparedProgramCreate {
    pub fn program_id(&self) -> &ProgramId {
        &self.spec.id
    }
}

impl PreparedProgramUpdate {
    pub fn expected_spec(&self) -> &ProgramSpec {
        &self.expected_spec
    }

    pub fn next_spec(&self) -> &ProgramSpec {
        &self.next_spec
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StopActiveReport {
    pub attempted: usize,
    pub stopped: usize,
    pub already_stopped: usize,
    pub failed: usize,
    pub failed_program_ids: Vec<ProgramId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AutoStartReport {
    pub eligible: usize,
    pub started: usize,
    pub already_active: usize,
    pub skipped: usize,
    pub failed: usize,
    pub failed_program_ids: Vec<ProgramId>,
}

impl ProgramManager {
    pub fn new(
        driver: DynProcessDriver,
        program_store: DynProgramStore,
        config_store: DynConfigStore,
        tool_runner: DynToolRunner,
    ) -> Arc<Self> {
        let adapters = AdapterRegistry::default();
        let config_service = Arc::new(ConfigService::new(
            config_store.clone(),
            program_store.clone(),
            tool_runner.clone(),
            adapters.clone(),
        ));
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        Arc::new(Self {
            controllers: RwLock::new(HashMap::new()),
            driver,
            program_store,
            config_store,
            config_service,
            adapters,
            events,
            registry_mutations: Mutex::new(()),
            automatic_restarts_enabled: Arc::new(AtomicBool::new(true)),
            automatic_lifecycle_gate: Arc::new(RwLock::new(())),
            auto_start_reconciliation: Mutex::new(()),
            pending_auto_starts: Mutex::new(HashSet::new()),
            package_preparations: Mutex::new(HashMap::new()),
            lifecycle: ManagerLifecycle::default(),
        })
    }

    pub async fn set_automatic_restarts_enabled(&self, enabled: bool) {
        let Ok(_mutation) = self.lifecycle.mutation_permit().await else {
            return;
        };
        let _exclusive = self.automatic_lifecycle_gate.write().await;
        self.automatic_restarts_enabled
            .store(enabled, Ordering::Release);
        if !enabled {
            self.pending_auto_starts.lock().await.clear();
        }
    }

    /// Linearizes license fail-closed with automatic starts/restarts before stopping processes.
    /// Once this returns, no automatic lifecycle task that observed the old policy can remain.
    pub async fn disable_automatic_restarts_and_stop_active(&self) -> StopActiveReport {
        let Ok(_mutation) = self.lifecycle.mutation_permit().await else {
            return StopActiveReport::default();
        };
        {
            // Taking the writer drains every automatic start/restart that entered
            // under the previous policy. Release it before sending Stop commands:
            // a controller waiting on this gate must remain able to receive them.
            let _exclusive = self.automatic_lifecycle_gate.write().await;
            self.automatic_restarts_enabled
                .store(false, Ordering::Release);
            self.pending_auto_starts.lock().await.clear();
        }
        self.stop_active_inner().await
    }

    pub async fn initialize(self: &Arc<Self>) -> Result<LoadReport> {
        self.initialize_with_startup_delay(Duration::ZERO).await
    }

    pub async fn initialize_with_startup_delay(
        self: &Arc<Self>,
        startup_delay: Duration,
    ) -> Result<LoadReport> {
        let report = self.initialize_without_auto_start().await?;
        let _ = self.reconcile_auto_start_programs(startup_delay).await;
        Ok(report)
    }

    pub async fn initialize_without_auto_start(self: &Arc<Self>) -> Result<LoadReport> {
        let _mutation = self.lifecycle.mutation_permit().await?;
        let mut report = self.program_store.load_all().await?;
        report
            .valid
            .sort_by(|left, right| left.spec.id.cmp(&right.spec.id));
        let mut recovered = Vec::with_capacity(report.valid.len());
        for stored in std::mem::take(&mut report.valid) {
            if let Err(error) = self.ensure_external_profile_compatible(&stored.spec).await {
                report.invalid.push(crate::InvalidProgram {
                    path: stored.workspace,
                    error: error.to_string(),
                });
                continue;
            }
            if let Err(error) = self.ensure_dashboard_port_available(&stored.spec).await {
                report.invalid.push(crate::InvalidProgram {
                    path: stored.workspace,
                    error: error.to_string(),
                });
                continue;
            }
            let recovery = async {
                self.program_store.recover_workspace(&stored.spec).await?;
                self.config_store.recover(&stored.spec).await
            }
            .await;
            if let Err(error) = recovery {
                report.invalid.push(crate::InvalidProgram {
                    path: stored.workspace,
                    error: error.to_string(),
                });
                continue;
            }
            self.attach_controller(stored.spec.clone(), stored.workspace.clone())
                .await;
            recovered.push(stored);
        }
        report.valid = recovered;
        Ok(report)
    }

    pub async fn list(&self) -> Vec<ProgramSummary> {
        let handles: Vec<_> = self.controllers.read().await.values().cloned().collect();
        let mut summaries = Vec::with_capacity(handles.len());
        for handle in handles {
            let spec = handle.spec().await;
            summaries.push(ProgramSummary {
                id: spec.id,
                name: spec.name,
                kind: spec.program_type.kind(),
                auto_start: spec.auto_start,
                state: handle.state(),
            });
        }
        summaries.sort_by(|a, b| a.id.cmp(&b.id));
        summaries
    }

    pub async fn get(&self, id: &ProgramId) -> Result<(ProgramSpec, ProgramState)> {
        let handle = self.handle(id).await?;
        let _lease = handle.operation_lease().await?;
        Ok((handle.spec().await, handle.state()))
    }

    pub async fn create(self: &Arc<Self>, request: CreateProgramRequest) -> Result<()> {
        let prepared = self.prepare_create(request).await?;
        self.commit_create(prepared).await
    }

    pub async fn prepare_create(
        self: &Arc<Self>,
        request: CreateProgramRequest,
    ) -> Result<PreparedProgramCreate> {
        let CreateProgramRequest {
            mut spec,
            package_source,
            initial_config,
        } = request;
        spec.normalize_runtime_directory()?;
        spec.validate()?;
        self.ensure_external_profile_compatible(&spec).await?;
        self.ensure_dashboard_port_available(&spec).await?;
        match (&spec.executable, &package_source) {
            (crate::ExecutableSpec::Managed { .. }, Some(source)) if !source.is_absolute() => {
                return Err(CamelliaNexusError::new(
                    ErrorCode::InvalidPath,
                    "Program source directory must be an absolute path",
                ));
            }
            (crate::ExecutableSpec::Managed { .. }, None) => {
                return Err(CamelliaNexusError::invalid_spec(
                    "Managed executable requires a program source directory",
                ));
            }
            (crate::ExecutableSpec::External { .. }, Some(_)) => {
                return Err(CamelliaNexusError::invalid_spec(
                    "External executable cannot use a managed program source",
                ));
            }
            _ => {}
        }
        if spec.program_type.main_config().is_some() != initial_config.is_some() {
            return Err(CamelliaNexusError::invalid_spec(
                "Stored configuration path and initial configuration must be provided together",
            ));
        }
        if initial_config
            .as_ref()
            .is_some_and(|content| content.len() > crate::MAX_CONFIG_BYTES)
        {
            return Err(CamelliaNexusError::invalid_spec(
                "Initial configuration exceeds the 4 MiB limit",
            ));
        }
        if self.controllers.read().await.contains_key(&spec.id) {
            return Err(CamelliaNexusError::new(
                ErrorCode::AlreadyExists,
                "Program already exists",
            ));
        }
        let assets = CreateAssets {
            package_source,
            initial_config: initial_config.map(String::into_bytes),
        };
        let workspace = self
            .program_store
            .create_pending(&spec, assets)
            .await
            .map_err(|error| creation_context(error, "Failed to prepare the program workspace"))?;
        let result = async {
            let detected_version = self.config_service.probe_binary(&spec).await?;
            let mut metadata = self.program_store.executable_metadata(&spec).await?;
            metadata.detected_version = detected_version;
            spec.executable.set_metadata(metadata);
            if spec.program_type.main_config().is_some() {
                let document = self.config_service.load(&spec).await?;
                let validation = self
                    .config_service
                    .validate(&spec, document.content, document.base_hash)
                    .await?;
                if !validation.valid {
                    return Err(CamelliaNexusError::new(
                        ErrorCode::ConfigInvalid,
                        "Initial configuration is invalid",
                    )
                    .with_details(format!("{}\n{}", validation.stdout, validation.stderr)));
                }
            }
            self.program_store
                .save(&spec)
                .await
                .map_err(|error| creation_context(error, "Failed to save program metadata"))?;
            Ok::<_, CamelliaNexusError>(())
        }
        .await;
        if let Err(error) = result {
            let _ = self.program_store.discard_pending(&spec.id).await;
            return Err(error);
        }
        Ok(PreparedProgramCreate { spec, workspace })
    }

    pub async fn discard_prepared_create(&self, prepared: PreparedProgramCreate) -> Result<()> {
        self.program_store.discard_pending(&prepared.spec.id).await
    }

    pub async fn commit_create(self: &Arc<Self>, prepared: PreparedProgramCreate) -> Result<()> {
        let _mutation = match self.lifecycle.mutation_permit().await {
            Ok(permit) => permit,
            Err(error) => {
                let _ = self.program_store.discard_pending(&prepared.spec.id).await;
                return Err(error);
            }
        };
        let _registry = self.registry_mutations.lock().await;
        if let Err(error) = self
            .ensure_external_profile_compatible(&prepared.spec)
            .await
        {
            let _ = self.program_store.discard_pending(&prepared.spec.id).await;
            return Err(error);
        }
        if let Err(error) = self.ensure_dashboard_port_available(&prepared.spec).await {
            let _ = self.program_store.discard_pending(&prepared.spec.id).await;
            return Err(error);
        }
        if self
            .controllers
            .read()
            .await
            .contains_key(&prepared.spec.id)
        {
            let _ = self.program_store.discard_pending(&prepared.spec.id).await;
            return Err(CamelliaNexusError::new(
                ErrorCode::AlreadyExists,
                "Program already exists",
            ));
        }
        if let Err(error) = self.program_store.commit_create(&prepared.spec.id).await {
            let _ = self.program_store.discard_pending(&prepared.spec.id).await;
            return Err(error);
        }
        self.attach_controller(prepared.spec, prepared.workspace)
            .await;
        let _ = self.events.send(ManagerEvent::ProgramListChanged);
        Ok(())
    }

    pub async fn update(&self, spec: ProgramSpec) -> Result<()> {
        let prepared = self.prepare_update(spec).await?;
        self.commit_update(prepared, false).await
    }

    pub async fn prepare_update(&self, mut spec: ProgramSpec) -> Result<PreparedProgramUpdate> {
        spec.normalize_runtime_directory()?;
        spec.validate()?;
        self.ensure_external_profile_compatible(&spec).await?;
        self.ensure_dashboard_port_available(&spec).await?;
        let (current, _) = self.get(&spec.id).await?;
        let identity_changed = current.executable != spec.executable
            || current.program_type.kind() != spec.program_type.kind();
        if identity_changed {
            let detected_version = self.config_service.probe_binary(&spec).await?;
            let mut metadata = self.program_store.executable_metadata(&spec).await?;
            metadata.detected_version = detected_version;
            spec.executable.set_metadata(metadata);
        }
        if current.program_type != spec.program_type && spec.program_type.main_config().is_some() {
            let document = self.config_service.load(&spec).await?;
            let validation = self
                .config_service
                .validate(&spec, document.content, document.base_hash)
                .await?;
            if !validation.valid {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    "Configuration is invalid for the selected program type",
                )
                .with_details(format!("{}\n{}", validation.stdout, validation.stderr)));
            }
        }
        Ok(PreparedProgramUpdate {
            expected_spec: current,
            next_spec: spec,
        })
    }

    pub async fn commit_update(
        &self,
        prepared: PreparedProgramUpdate,
        restart: bool,
    ) -> Result<()> {
        let _mutation = self.lifecycle.mutation_permit().await?;
        let _registry = self.registry_mutations.lock().await;
        self.ensure_external_profile_compatible(&prepared.next_spec)
            .await?;
        self.ensure_dashboard_port_available(&prepared.next_spec)
            .await?;
        let handle = self.handle(&prepared.next_spec.id).await?;
        let _lease = handle.operation_lease().await?;
        let mutation = if restart {
            Mutation::UpdateSpecAndRestart {
                expected_spec: Box::new(prepared.expected_spec),
                next_spec: Box::new(prepared.next_spec),
            }
        } else {
            Mutation::UpdateSpec {
                expected_spec: Box::new(prepared.expected_spec),
                next_spec: Box::new(prepared.next_spec),
            }
        };
        handle.mutate(mutation).await?;
        let _ = self.events.send(ManagerEvent::ProgramListChanged);
        Ok(())
    }

    /// Saves settings and performs the required lifecycle transition as one controller mutation.
    /// No start, stop, removal, or second settings update can interleave between these steps.
    pub async fn update_and_restart(&self, spec: ProgramSpec) -> Result<()> {
        let prepared = self.prepare_update(spec).await?;
        self.commit_update(prepared, true).await
    }

    pub async fn remove(&self, id: &ProgramId) -> Result<()> {
        let _mutation = self.lifecycle.mutation_permit().await?;
        self.pending_auto_starts.lock().await.remove(id);
        let _registry = self.registry_mutations.lock().await;
        let handle = self.handle_unchecked(id).await?;
        let removal = handle.begin_removal().await?;
        handle.mutate(Mutation::Stop).await?;
        self.program_store.remove_workspace(id).await?;
        self.controllers.write().await.remove(id);
        self.config_service.forget_program(id).await;
        self.package_preparations.lock().await.remove(id);
        if let Err(error) = handle.mutate(Mutation::Shutdown).await {
            tracing::warn!(program = %id, %error, "removed stopped program but controller shutdown did not acknowledge");
        }
        removal.commit();
        let _ = self.events.send(ManagerEvent::ProgramListChanged);
        Ok(())
    }

    pub async fn start(&self, id: &ProgramId) -> Result<()> {
        let _mutation = self.lifecycle.mutation_permit().await?;
        let handle = self.handle(id).await?;
        let _lease = handle.operation_lease().await?;
        handle.mutate(Mutation::Start { interactive: true }).await?;
        Ok(())
    }

    pub async fn stop(&self, id: &ProgramId) -> Result<()> {
        let _mutation = self.lifecycle.mutation_permit().await?;
        self.pending_auto_starts.lock().await.remove(id);
        let handle = self.handle(id).await?;
        let _lease = handle.operation_lease().await?;
        handle.mutate(Mutation::Stop).await?;
        Ok(())
    }

    pub async fn stop_active(&self) -> StopActiveReport {
        let Ok(_mutation) = self.lifecycle.mutation_permit().await else {
            return StopActiveReport::default();
        };
        self.stop_active_inner().await
    }

    async fn stop_active_inner(&self) -> StopActiveReport {
        let handles: Vec<_> = self.controllers.read().await.values().cloned().collect();
        let mut tasks = tokio::task::JoinSet::new();
        let mut report = StopActiveReport::default();
        let mut active_handles = Vec::new();
        for handle in handles {
            if !program_state_is_active(&handle.state()) {
                report.already_stopped += 1;
                continue;
            }
            report.attempted += 1;
            let program_id = handle.spec().await.id;
            active_handles.push((program_id.clone(), handle.clone()));
            tasks.spawn(async move { (program_id, handle.mutate(Mutation::Stop).await) });
        }
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok((_program_id, Ok(_))) => {}
                Ok((program_id, Err(error))) => {
                    // A failed command is not necessarily a failed stop: the process may have
                    // exited concurrently or another shutdown may have completed first.
                    tracing::warn!(%program_id, %error, "program stop command did not complete cleanly after license became inactive");
                }
                Err(error) => {
                    tracing::error!(%error, "program stop task failed after license became inactive");
                }
            }
        }

        // Commands are only attempts. Reconcile once, from controller state, so transient
        // command errors and concurrent removals cannot inflate or double-count failures.
        tokio::task::yield_now().await;
        for (program_id, handle) in active_handles {
            if program_state_is_active(&handle.state()) {
                report.failed_program_ids.push(program_id);
            } else {
                report.stopped += 1;
            }
        }
        report.failed_program_ids.sort();
        report.failed_program_ids.dedup();
        report.failed = report.failed_program_ids.len();
        report
    }

    pub async fn restart(&self, id: &ProgramId) -> Result<()> {
        let _mutation = self.lifecycle.mutation_permit().await?;
        let handle = self.handle(id).await?;
        let _lease = handle.operation_lease().await?;
        handle
            .mutate(Mutation::Restart { interactive: true })
            .await?;
        Ok(())
    }

    pub async fn replace_package(&self, id: &ProgramId, source: PathBuf) -> Result<()> {
        let prepared = self.prepare_package(id, source).await?;
        self.commit_package(prepared).await
    }

    pub async fn prepare_package(
        &self,
        id: &ProgramId,
        source: PathBuf,
    ) -> Result<PreparedPackageGuard> {
        if !source.is_absolute() {
            return Err(CamelliaNexusError::new(
                ErrorCode::InvalidPath,
                "Program source directory must be an absolute path",
            ));
        }
        let preparation_lock = {
            let mut preparations = self.package_preparations.lock().await;
            preparations
                .entry(id.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let preparation = preparation_lock.lock_owned().await;
        let handle = self.handle(id).await?;
        let operation = handle.operation_lease().await?;
        let state = handle.state();
        if !matches!(
            state,
            ProgramState::Stopped | ProgramState::Exited { .. } | ProgramState::Error { .. }
        ) {
            return Err(CamelliaNexusError::new(
                ErrorCode::InvalidState,
                "Stop the active program or pending retry before replacing its files",
            ));
        }
        let expected_spec = handle.spec().await;
        if !expected_spec.executable.is_managed() {
            return Err(CamelliaNexusError::new(
                ErrorCode::InvalidSpec,
                "External executables do not have a managed package",
            ));
        }
        let workspace = self.program_store.workspace(id).await?;
        let staged = self
            .program_store
            .stage_package(&expected_spec, &source)
            .await?;
        let probe = self
            .config_service
            .probe_executable(&expected_spec, staged.executable.clone(), workspace)
            .await;
        let detected_version = match probe {
            Ok(version) => version,
            Err(error) => {
                let _ = self.program_store.discard_package(staged).await;
                return Err(error);
            }
        };
        let mut next_spec = expected_spec.clone();
        let mut metadata = staged.metadata.clone();
        metadata.detected_version = detected_version;
        next_spec.executable.set_metadata(metadata);
        if let Err(error) = next_spec.validate() {
            let _ = self.program_store.discard_package(staged).await;
            return Err(error);
        }
        Ok(PreparedPackageGuard {
            expected_spec,
            next_spec,
            staged,
            _preparation: preparation,
            _operation: operation,
        })
    }

    pub async fn discard_prepared_package(&self, prepared: PreparedPackageGuard) -> Result<()> {
        let PreparedPackageGuard { staged, .. } = prepared;
        self.program_store.discard_package(staged).await
    }

    pub async fn commit_package(&self, prepared: PreparedPackageGuard) -> Result<()> {
        let _mutation = match self.lifecycle.mutation_permit().await {
            Ok(permit) => permit,
            Err(error) => {
                let _ = self.discard_prepared_package(prepared).await;
                return Err(error);
            }
        };
        let program_id = prepared.expected_spec.id.clone();
        let handle = match self.handle(&program_id).await {
            Ok(handle) => handle,
            Err(error) => {
                let _ = self.discard_prepared_package(prepared).await;
                return Err(error);
            }
        };
        let mutation_guard = match handle.try_reserve_mutation() {
            Ok(guard) => guard,
            Err(error) => {
                let _ = self.discard_prepared_package(prepared).await;
                return Err(error);
            }
        };
        let PreparedPackageGuard {
            expected_spec,
            next_spec,
            staged,
            ..
        } = prepared;
        handle
            .mutate_reserved(
                Mutation::CommitPreparedPackage {
                    expected_spec: Box::new(expected_spec),
                    next_spec: Box::new(next_spec),
                    staged,
                },
                mutation_guard,
            )
            .await?;
        let _ = self.events.send(ManagerEvent::ProgramListChanged);
        Ok(())
    }

    pub async fn list_actions(&self, id: &ProgramId) -> Result<Vec<ActionDescriptor>> {
        let handle = self.handle(id).await?;
        let _lease = handle.operation_lease().await?;
        let spec = handle.spec().await;
        if spec.program_type.main_config().is_none() {
            return Ok(Vec::new());
        }
        Ok(self
            .adapters
            .get(spec.program_type.kind())
            .actions(&handle.state()))
    }

    pub async fn load_config(&self, id: &ProgramId) -> Result<ConfigDocument> {
        let handle = self.handle(id).await?;
        let _lease = handle.operation_lease().await?;
        let spec = handle.spec().await;
        self.config_service.load(&spec).await
    }

    pub async fn load_configuration_schema(
        &self,
        id: &ProgramId,
    ) -> Result<Option<ConfigurationSchemaDocument>> {
        let handle = self.handle(id).await?;
        let _lease = handle.operation_lease().await?;
        let spec = handle.spec().await;
        let document = self.config_service.load_configuration_schema(&spec).await?;
        if handle.spec().await != spec {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Program changed while its configuration schema was loaded",
            ));
        }
        Ok(document)
    }

    pub async fn load_config_for_spec(
        &self,
        id: &ProgramId,
        expected_current_spec: &ProgramSpec,
        target_spec: &ProgramSpec,
    ) -> Result<ConfigDocument> {
        if &expected_current_spec.id != id || &target_spec.id != id {
            return Err(CamelliaNexusError::invalid_spec(
                "Configuration Program ids do not match",
            ));
        }
        let handle = self.handle(id).await?;
        if handle.spec().await != *expected_current_spec {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Program settings changed before the configuration was loaded",
            ));
        }
        self.config_service.load(target_spec).await
    }

    pub async fn validate_config(
        &self,
        id: &ProgramId,
        content: String,
        base_hash: String,
    ) -> Result<ValidationResult> {
        let handle = self.handle(id).await?;
        let _lease = handle.operation_lease().await?;
        let spec = handle.spec().await;
        self.config_service
            .validate(&spec, content, base_hash)
            .await
    }

    pub async fn apply_config(
        &self,
        id: &ProgramId,
        expected_spec: &ProgramSpec,
        content: String,
        base_hash: String,
        interactive: bool,
    ) -> Result<String> {
        let prepared = self
            .prepare_config(id, expected_spec, expected_spec, content, base_hash)
            .await?;
        self.apply_prepared_config(id, expected_spec, prepared, interactive)
            .await
    }

    pub async fn prepare_config(
        &self,
        id: &ProgramId,
        expected_current_spec: &ProgramSpec,
        target_spec: &ProgramSpec,
        content: String,
        base_hash: String,
    ) -> Result<PreparedConfigGuard> {
        if &expected_current_spec.id != id || &target_spec.id != id {
            return Err(CamelliaNexusError::invalid_spec(
                "Prepared configuration Program ids do not match",
            ));
        }
        target_spec.validate()?;
        let handle = self.handle(id).await?;
        let current = handle.spec().await;
        if current != *expected_current_spec {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Program settings changed before configuration preparation began",
            ));
        }
        self.config_service
            .prepare_apply(target_spec, content, base_hash)
            .await
    }

    pub async fn discard_prepared_config(&self, prepared: PreparedConfigGuard) -> Result<()> {
        self.config_service.discard(prepared).await
    }

    pub async fn apply_prepared_config(
        &self,
        id: &ProgramId,
        expected_spec: &ProgramSpec,
        prepared: PreparedConfigGuard,
        interactive: bool,
    ) -> Result<String> {
        let _mutation = match self.lifecycle.mutation_permit().await {
            Ok(permit) => permit,
            Err(error) => {
                let _ = self.config_service.discard(prepared).await;
                return Err(error);
            }
        };
        let handle = match self.handle(id).await {
            Ok(handle) => handle,
            Err(error) => {
                let _ = self.config_service.discard(prepared).await;
                return Err(error);
            }
        };
        let _lease = match handle.operation_lease().await {
            Ok(lease) => lease,
            Err(error) => {
                let _ = self.config_service.discard(prepared).await;
                return Err(error);
            }
        };
        let mutation_guard = match handle.try_reserve_mutation() {
            Ok(guard) => guard,
            Err(error) => {
                let _ = self.config_service.discard(prepared).await;
                return Err(error);
            }
        };
        handle
            .mutate_reserved(
                Mutation::ApplyPreparedConfig {
                    expected_spec: Box::new(expected_spec.clone()),
                    prepared,
                    interactive,
                },
                mutation_guard,
            )
            .await?
            .ok_or_else(|| CamelliaNexusError::new(ErrorCode::Internal, "Missing config hash"))
    }

    pub async fn commit_update_and_apply_config(
        &self,
        update: PreparedProgramUpdate,
        prepared: PreparedConfigGuard,
        interactive: bool,
    ) -> Result<String> {
        let _mutation = match self.lifecycle.mutation_permit().await {
            Ok(permit) => permit,
            Err(error) => {
                let _ = self.config_service.discard(prepared).await;
                return Err(error);
            }
        };
        let _registry = self.registry_mutations.lock().await;
        if let Err(error) = self
            .ensure_external_profile_compatible(&update.next_spec)
            .await
        {
            let _ = self.config_service.discard(prepared).await;
            return Err(error);
        }
        if let Err(error) = self
            .ensure_dashboard_port_available(&update.next_spec)
            .await
        {
            let _ = self.config_service.discard(prepared).await;
            return Err(error);
        }
        let handle = match self.handle(&update.next_spec.id).await {
            Ok(handle) => handle,
            Err(error) => {
                let _ = self.config_service.discard(prepared).await;
                return Err(error);
            }
        };
        let _lease = match handle.operation_lease().await {
            Ok(lease) => lease,
            Err(error) => {
                let _ = self.config_service.discard(prepared).await;
                return Err(error);
            }
        };
        let mutation_guard = match handle.try_reserve_mutation() {
            Ok(guard) => guard,
            Err(error) => {
                let _ = self.config_service.discard(prepared).await;
                return Err(error);
            }
        };
        let result = handle
            .mutate_reserved(
                Mutation::UpdateSpecAndApplyPreparedConfig {
                    expected_spec: Box::new(update.expected_spec),
                    next_spec: Box::new(update.next_spec),
                    prepared,
                    interactive,
                },
                mutation_guard,
            )
            .await?
            .ok_or_else(|| CamelliaNexusError::new(ErrorCode::Internal, "Missing config hash"))?;
        let _ = self.events.send(ManagerEvent::ProgramListChanged);
        Ok(result)
    }

    pub async fn run_action(
        &self,
        id: &ProgramId,
        action_id: String,
        content: String,
        base_hash: String,
    ) -> Result<ActionResult> {
        let _mutation = self.lifecycle.mutation_permit().await?;
        let handle = self.handle(id).await?;
        let _lease = handle.operation_lease().await?;
        let spec = handle.spec().await;
        let adapter = self.adapters.get(spec.program_type.kind());
        let state = handle.state();
        let descriptor = adapter
            .actions(&state)
            .into_iter()
            .find(|action| action.id == action_id)
            .ok_or_else(|| {
                CamelliaNexusError::new(ErrorCode::NotFound, "Program action not found")
            })?;
        if !descriptor
            .allowed_states
            .contains(&crate::ActionState::from(&state))
        {
            return Err(CamelliaNexusError::new(
                ErrorCode::InvalidState,
                "Program action is not allowed in the current state",
            ));
        }
        self.config_service
            .run_action(&spec, action_id, content, base_hash)
            .await
    }

    pub async fn read_log(
        &self,
        id: &ProgramId,
        stream: LogStream,
        max_bytes: usize,
    ) -> Result<LogChunk> {
        let handle = self.handle(id).await?;
        let _lease = handle.operation_lease().await?;
        let spec = handle.spec().await;
        self.program_store
            .read_log(&spec, stream, max_bytes.min(1024 * 1024))
            .await
    }

    pub async fn clear_logs(&self, id: &ProgramId) -> Result<()> {
        let _mutation = self.lifecycle.mutation_permit().await?;
        let handle = self.handle(id).await?;
        let _lease = handle.operation_lease().await?;
        let spec = handle.spec().await;
        self.program_store.clear_logs(&spec).await
    }

    pub async fn workspace(&self, id: &ProgramId) -> Result<PathBuf> {
        let handle = self.handle(id).await?;
        let _lease = handle.operation_lease().await?;
        self.program_store.workspace(id).await
    }

    pub async fn working_directory(&self, id: &ProgramId) -> Result<PathBuf> {
        let handle = self.handle(id).await?;
        let _lease = handle.operation_lease().await?;
        let spec = handle.spec().await;
        let workspace = self.program_store.workspace(id).await?;
        let directory = spec.working_directory_path(&workspace);
        if directory.is_dir() {
            Ok(directory)
        } else {
            Err(CamelliaNexusError::new(
                ErrorCode::NotFound,
                "Program working directory does not exist",
            ))
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ManagerEvent> {
        self.events.subscribe()
    }

    pub async fn shutdown(&self) -> ShutdownReport {
        let Some(_shutdown) = self.lifecycle.begin_shutdown().await else {
            return ShutdownReport::default();
        };
        {
            let _automatic = self.automatic_lifecycle_gate.write().await;
            self.automatic_restarts_enabled
                .store(false, Ordering::Release);
            self.pending_auto_starts.lock().await.clear();
        }
        let handles: Vec<_> = self
            .controllers
            .read()
            .await
            .iter()
            .map(|(id, handle)| (id.clone(), handle.clone()))
            .collect();
        let mut pending: std::collections::BTreeSet<_> =
            handles.iter().map(|(id, _)| id.clone()).collect();
        let mut tasks = tokio::task::JoinSet::new();
        for (id, handle) in handles {
            tasks.spawn(async move { (id, handle.mutate(Mutation::Shutdown).await) });
        }
        let mut failures = Vec::new();
        let completed = tokio::time::timeout(std::time::Duration::from_secs(25), async {
            while let Some(result) = tasks.join_next().await {
                match result {
                    Ok((id, Ok(_))) => {
                        pending.remove(&id);
                    }
                    Ok((id, Err(error))) => {
                        pending.remove(&id);
                        failures.push((id, error.to_string()));
                    }
                    Err(_) => {}
                }
            }
        })
        .await;
        let timed_out = completed.is_err();
        if timed_out {
            tasks.abort_all();
        }
        let unfinished_reason = if timed_out {
            "shutdown timed out"
        } else {
            "shutdown task terminated unexpectedly"
        };
        failures.extend(
            pending
                .into_iter()
                .map(|id| (id, unfinished_reason.to_owned())),
        );
        failures.sort_by(|left, right| left.0.cmp(&right.0));
        ShutdownReport {
            timed_out,
            failures,
        }
    }

    async fn attach_controller(self: &Arc<Self>, spec: ProgramSpec, workspace: PathBuf) {
        let id = spec.id.clone();
        let handle = crate::ControllerHandle::spawn(
            spec,
            workspace,
            crate::controller::ControllerDependencies {
                driver: self.driver.clone(),
                store: self.program_store.clone(),
                config_service: self.config_service.clone(),
                adapters: self.adapters.clone(),
                automatic_restarts_enabled: self.automatic_restarts_enabled.clone(),
                automatic_lifecycle_gate: self.automatic_lifecycle_gate.clone(),
            },
        );
        let mut state = handle.subscribe();
        let events = self.events.clone();
        let event_id = id.clone();
        tokio::spawn(async move {
            while state.changed().await.is_ok() {
                let _ = events.send(ManagerEvent::ProgramStateChanged {
                    id: event_id.clone(),
                    state: state.borrow().clone(),
                });
            }
        });
        self.controllers.write().await.insert(id, handle);
    }

    pub async fn reconcile_auto_start_programs(&self, startup_delay: Duration) -> AutoStartReport {
        // Do not hold a mutation permit while another reconciliation owns the queue. Otherwise
        // queued background work would itself delay the shutdown writer.
        let _reconciliation = tokio::select! {
            reconciliation = self.auto_start_reconciliation.lock() => reconciliation,
            () = self.lifecycle.shutdown_requested() => return AutoStartReport::default(),
        };
        let Ok(_mutation) = self.lifecycle.mutation_permit().await else {
            return AutoStartReport::default();
        };
        let handles: Vec<_> = self.controllers.read().await.values().cloned().collect();
        let mut scheduled = Vec::new();
        for handle in handles {
            let spec = handle.spec().await;
            if spec.auto_start {
                scheduled.push((spec.id, handle));
            }
        }
        scheduled.sort_by(|left, right| left.0.cmp(&right.0));
        let mut report = AutoStartReport {
            eligible: scheduled.len(),
            ..AutoStartReport::default()
        };
        let mut privilege_required_ids = Vec::new();
        {
            let mut pending = self.pending_auto_starts.lock().await;
            pending.clear();
            pending.extend(scheduled.iter().map(|(id, _)| id.clone()));
        }
        for (id, handle) in scheduled {
            if !startup_delay.is_zero() {
                tokio::select! {
                    () = tokio::time::sleep(startup_delay) => {}
                    () = self.lifecycle.shutdown_requested() => break,
                }
            }
            if self.lifecycle.is_shutdown_started() {
                break;
            }
            let _automatic_start = self.automatic_lifecycle_gate.read().await;
            if !self.automatic_restarts_enabled.load(Ordering::Acquire) {
                break;
            }
            if !self.pending_auto_starts.lock().await.remove(&id) {
                report.skipped += 1;
                continue;
            }
            if !handle.spec().await.auto_start {
                report.skipped += 1;
                continue;
            }
            if program_state_is_active(&handle.state()) {
                report.already_active += 1;
                continue;
            }
            let result = match handle.operation_lease().await {
                Ok(_lease) => handle.mutate(Mutation::Start { interactive: false }).await,
                Err(error) => Err(error),
            };
            match result {
                Ok(_) => report.started += 1,
                Err(error) => {
                    if error.code == ErrorCode::PrivilegeRequired {
                        privilege_required_ids.push(id.clone());
                    }
                    report.failed += 1;
                    report.failed_program_ids.push(id);
                }
            }
        }
        let mut pending = self.pending_auto_starts.lock().await;
        report.skipped += pending.len();
        pending.clear();
        if !privilege_required_ids.is_empty() {
            let _ = self
                .events
                .send(ManagerEvent::ProgramAutoStartPrivilegeRequired {
                    ids: privilege_required_ids,
                });
        }
        report
    }

    async fn handle(&self, id: &ProgramId) -> Result<crate::ControllerHandle> {
        let handle = self.handle_unchecked(id).await?;
        if handle.is_removing() {
            return Err(CamelliaNexusError::new(
                ErrorCode::ProgramBusy,
                "Program is being removed",
            ));
        }
        Ok(handle)
    }

    async fn handle_unchecked(&self, id: &ProgramId) -> Result<crate::ControllerHandle> {
        self.controllers
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| CamelliaNexusError::new(ErrorCode::NotFound, "Program not found"))
    }

    async fn ensure_external_profile_compatible(&self, candidate: &ProgramSpec) -> Result<()> {
        let crate::ExecutableSpec::External {
            path: candidate_path,
            ..
        } = &candidate.executable
        else {
            return Ok(());
        };
        let handles: Vec<_> = self.controllers.read().await.values().cloned().collect();
        for handle in handles {
            let existing = handle.spec().await;
            if existing.id == candidate.id {
                continue;
            }
            let crate::ExecutableSpec::External {
                path: existing_path,
                ..
            } = &existing.executable
            else {
                continue;
            };
            if same_executable(existing_path, candidate_path) {
                return Err(CamelliaNexusError::new(
                    ErrorCode::InvalidSpec,
                    "This executable is already assigned to a different program profile",
                )
                .with_details(format!(
                    "{} uses the {:?} profile",
                    existing.name,
                    existing.program_type.kind()
                )));
            }
        }
        Ok(())
    }

    async fn ensure_dashboard_port_available(&self, candidate: &ProgramSpec) -> Result<()> {
        let candidate_ports = dashboard_ports(candidate);
        if candidate_ports.is_empty() {
            return Ok(());
        }
        let handles: Vec<_> = self.controllers.read().await.values().cloned().collect();
        for handle in handles {
            let existing = handle.spec().await;
            if existing.id == candidate.id {
                continue;
            }
            let existing_ports = dashboard_ports(&existing);
            if let Some(port) = candidate_ports
                .iter()
                .find(|port| existing_ports.contains(port))
            {
                return Err(CamelliaNexusError::invalid_spec(format!(
                    "Dashboard port {port} is already used by {}",
                    existing.name
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct ShutdownReport {
    pub timed_out: bool,
    pub failures: Vec<(ProgramId, String)>,
}

impl ShutdownReport {
    pub fn succeeded(&self) -> bool {
        !self.timed_out && self.failures.is_empty()
    }
}

fn program_state_is_active(state: &ProgramState) -> bool {
    matches!(
        state,
        ProgramState::Starting
            | ProgramState::Running { .. }
            | ProgramState::Stopping
            | ProgramState::Backoff { .. }
            | ProgramState::StopFailed { .. }
    )
}

fn dashboard_ports(spec: &ProgramSpec) -> Vec<u16> {
    spec.managed_config
        .as_ref()
        .map(|managed| {
            [
                managed
                    .sing_box_dashboard
                    .as_ref()
                    .map(|dashboard| dashboard.listen_port),
                managed
                    .sing_box_clash_dashboard
                    .as_ref()
                    .map(|dashboard| dashboard.listen_port),
                managed
                    .xray_dashboard
                    .as_ref()
                    .map(|dashboard| dashboard.api_port),
                managed
                    .xray_dashboard
                    .as_ref()
                    .map(|dashboard| dashboard.metrics_port),
                managed
                    .mihomo_dashboard
                    .as_ref()
                    .map(|dashboard| dashboard.listen_port),
            ]
            .into_iter()
            .flatten()
            .collect()
        })
        .unwrap_or_default()
}

fn same_executable(left: &std::path::Path, right: &std::path::Path) -> bool {
    let left = std::fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf());
    let right = std::fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf());
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .replace('\\', "/")
            .eq_ignore_ascii_case(&right.to_string_lossy().replace('\\', "/"))
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn creation_context(error: CamelliaNexusError, message: &str) -> CamelliaNexusError {
    if error.code != ErrorCode::Storage {
        return error;
    }
    let details = error.details.unwrap_or(error.message);
    CamelliaNexusError::new(ErrorCode::Storage, message).with_details(details)
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc, time::Duration};

    use super::*;
    use crate::{
        ExecutableSpec, ManagedConfigSpec, MihomoDashboardSpec, ProgramId, ProgramType,
        RestartPolicy, SCHEMA_VERSION,
    };

    #[tokio::test]
    async fn shutdown_gate_drains_mutations_and_rejects_late_work() {
        let lifecycle = Arc::new(ManagerLifecycle::default());
        let active = lifecycle.mutation_permit().await.expect("active mutation");
        let requested = lifecycle.shutdown_requested();
        tokio::pin!(requested);
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut requested)
                .await
                .is_err(),
            "the lifecycle must not report shutdown before it is requested"
        );
        let closing = lifecycle.begin_shutdown();
        tokio::pin!(closing);

        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut closing)
                .await
                .is_err(),
            "shutdown must wait for an active mutation"
        );
        assert!(
            lifecycle.shutdown_started.load(Ordering::Acquire),
            "shutdown must be observable while active mutations drain"
        );
        tokio::time::timeout(Duration::from_millis(100), &mut requested)
            .await
            .expect("shutdown notification deadline");
        drop(active);
        let shutdown = tokio::time::timeout(Duration::from_millis(100), &mut closing)
            .await
            .expect("shutdown gate deadline")
            .expect("first shutdown");

        let late = lifecycle.mutation_permit();
        tokio::pin!(late);
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut late)
                .await
                .is_err(),
            "the shutdown writer must exclude new mutations"
        );
        drop(shutdown);
        let error = tokio::time::timeout(Duration::from_millis(100), &mut late)
            .await
            .expect("late mutation deadline")
            .expect_err("late mutation must be rejected");
        assert_eq!(error.code, ErrorCode::InvalidState);
    }

    #[test]
    fn mihomo_dashboard_participates_in_shared_port_ownership() {
        let spec = ProgramSpec {
            schema_version: SCHEMA_VERSION,
            id: ProgramId::parse("mihomo-port-test").expect("id"),
            name: "Mihomo port test".into(),
            executable: ExecutableSpec::Managed {
                path: "bin/mihomo".into(),
                metadata: None,
            },
            program_type: ProgramType::Mihomo {
                main_config: Some("config/managed.yaml".into()),
                extra_args: Vec::new(),
            },
            managed_config: Some(ManagedConfigSpec {
                mihomo_dashboard: Some(MihomoDashboardSpec {
                    listen_port: 9092,
                    download_url: None,
                }),
                ..ManagedConfigSpec::default()
            }),
            working_directory: "bin".into(),
            environment: BTreeMap::new(),
            auto_start: false,
            restart_policy: RestartPolicy::Never,
            privilege_policy: Default::default(),
        };

        assert_eq!(dashboard_ports(&spec), vec![9092]);
    }
}
