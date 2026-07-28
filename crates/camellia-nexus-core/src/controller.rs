use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock, mpsc, oneshot, watch};

use crate::{
    AdapterRegistry, CamelliaNexusError, ConfigService, DynProcessDriver, DynProgramStore,
    ErrorCode, ManagedProcess, PreparedConfigGuard, ProcessExit, ProgramSpec, ProgramState,
    RestartPolicy, Result, StagedPackage,
};

const COMMAND_CAPACITY: usize = 16;
const STABLE_RESET: Duration = Duration::from_secs(5 * 60);
const FAILURE_WINDOW: Duration = Duration::from_secs(10 * 60);
const MAX_FAILURES: usize = 10;
const PROCESS_STOP_TIMEOUT: Duration = Duration::from_secs(20);
const BACKOFF_SECONDS: [u64; 5] = [2, 4, 8, 16, 30];

#[derive(Debug)]
pub enum Mutation {
    Start {
        interactive: bool,
    },
    Stop,
    Restart {
        interactive: bool,
    },
    UpdateSpec {
        expected_spec: Box<ProgramSpec>,
        next_spec: Box<ProgramSpec>,
    },
    UpdateSpecAndRestart {
        expected_spec: Box<ProgramSpec>,
        next_spec: Box<ProgramSpec>,
    },
    CommitPreparedPackage {
        expected_spec: Box<ProgramSpec>,
        next_spec: Box<ProgramSpec>,
        staged: StagedPackage,
    },
    ApplyPreparedConfig {
        expected_spec: Box<ProgramSpec>,
        prepared: PreparedConfigGuard,
        interactive: bool,
    },
    UpdateSpecAndApplyPreparedConfig {
        expected_spec: Box<ProgramSpec>,
        next_spec: Box<ProgramSpec>,
        prepared: PreparedConfigGuard,
        interactive: bool,
    },
    Shutdown,
}

type MutationResponse = Result<Option<String>>;

struct Command {
    mutation: Mutation,
    response: oneshot::Sender<MutationResponse>,
    _guard: Option<MutationGuard>,
}

#[derive(Clone)]
pub struct ControllerHandle {
    tx: mpsc::Sender<Command>,
    spec: Arc<RwLock<ProgramSpec>>,
    state: watch::Receiver<ProgramState>,
    mutation_pending: Arc<AtomicBool>,
    operations: Arc<RwLock<()>>,
    removing: Arc<AtomicBool>,
}

pub(crate) struct ControllerDependencies {
    pub driver: DynProcessDriver,
    pub store: DynProgramStore,
    pub config_service: Arc<ConfigService>,
    pub adapters: AdapterRegistry,
    pub automatic_restarts_enabled: Arc<AtomicBool>,
    pub automatic_lifecycle_gate: Arc<RwLock<()>>,
}

impl ControllerHandle {
    pub(crate) fn spawn(
        spec: ProgramSpec,
        workspace: std::path::PathBuf,
        dependencies: ControllerDependencies,
    ) -> Self {
        let (tx, rx) = mpsc::channel(COMMAND_CAPACITY);
        let (state_tx, state) = watch::channel(ProgramState::Stopped);
        let spec = Arc::new(RwLock::new(spec));
        let controller = ProgramController {
            spec: spec.clone(),
            workspace,
            driver: dependencies.driver,
            store: dependencies.store,
            config_service: dependencies.config_service,
            adapters: dependencies.adapters,
            state_tx,
            rx,
            process: None,
            restart_deadline: None,
            restart_attempt: 0,
            started_at: None,
            recent_failures: VecDeque::new(),
            desired_running: false,
            automatic_restarts_enabled: dependencies.automatic_restarts_enabled,
            automatic_lifecycle_gate: dependencies.automatic_lifecycle_gate,
        };
        tokio::spawn(controller.run());
        Self {
            tx,
            spec,
            state,
            mutation_pending: Arc::new(AtomicBool::new(false)),
            operations: Arc::new(RwLock::new(())),
            removing: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn mutate(&self, mutation: Mutation) -> MutationResponse {
        let guard = if matches!(&mutation, Mutation::Stop | Mutation::Shutdown) {
            None
        } else {
            Some(MutationGuard::acquire(self.mutation_pending.clone())?)
        };
        self.send_mutation(mutation, guard).await
    }

    pub(crate) fn try_reserve_mutation(&self) -> Result<MutationGuard> {
        MutationGuard::acquire(self.mutation_pending.clone())
    }

    pub(crate) async fn mutate_reserved(
        &self,
        mutation: Mutation,
        guard: MutationGuard,
    ) -> MutationResponse {
        self.send_mutation(mutation, Some(guard)).await
    }

    async fn send_mutation(
        &self,
        mutation: Mutation,
        guard: Option<MutationGuard>,
    ) -> MutationResponse {
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .send(Command {
                mutation,
                response: response_tx,
                _guard: guard,
            })
            .await
            .map_err(|_| CamelliaNexusError::new(ErrorCode::Internal, "Controller stopped"))?;
        response_rx.await.map_err(|_| {
            CamelliaNexusError::new(ErrorCode::Internal, "Controller dropped response")
        })?
    }

    pub async fn spec(&self) -> ProgramSpec {
        self.spec.read().await.clone()
    }

    pub fn state(&self) -> ProgramState {
        self.state.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<ProgramState> {
        self.state.clone()
    }

    pub async fn operation_lease(&self) -> Result<OwnedRwLockReadGuard<()>> {
        let guard = self.operations.clone().read_owned().await;
        if self.removing.load(Ordering::Acquire) {
            Err(CamelliaNexusError::new(
                ErrorCode::ProgramBusy,
                "Program is being removed",
            ))
        } else {
            Ok(guard)
        }
    }

    pub async fn begin_removal(&self) -> Result<ControllerRemovalGuard> {
        let flag = RemovalFlag::acquire(self.removing.clone())?;
        let operations = self.operations.clone().write_owned().await;
        Ok(ControllerRemovalGuard {
            flag,
            _operations: operations,
        })
    }

    pub fn is_removing(&self) -> bool {
        self.removing.load(Ordering::Acquire)
    }
}

pub(crate) struct MutationGuard {
    pending: Arc<AtomicBool>,
}

impl MutationGuard {
    fn acquire(pending: Arc<AtomicBool>) -> Result<Self> {
        pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| CamelliaNexusError::new(ErrorCode::ProgramBusy, "Program is busy"))?;
        Ok(Self { pending })
    }
}

impl Drop for MutationGuard {
    fn drop(&mut self) {
        self.pending.store(false, Ordering::Release);
    }
}

struct RemovalFlag {
    removing: Arc<AtomicBool>,
    keep_set: bool,
}

impl RemovalFlag {
    fn acquire(removing: Arc<AtomicBool>) -> Result<Self> {
        removing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                CamelliaNexusError::new(ErrorCode::ProgramBusy, "Program is being removed")
            })?;
        Ok(Self {
            removing,
            keep_set: false,
        })
    }
}

impl Drop for RemovalFlag {
    fn drop(&mut self) {
        if !self.keep_set {
            self.removing.store(false, Ordering::Release);
        }
    }
}

pub struct ControllerRemovalGuard {
    flag: RemovalFlag,
    _operations: OwnedRwLockWriteGuard<()>,
}

impl ControllerRemovalGuard {
    pub fn commit(mut self) {
        self.flag.keep_set = true;
    }
}

struct ProgramController {
    spec: Arc<RwLock<ProgramSpec>>,
    workspace: std::path::PathBuf,
    driver: DynProcessDriver,
    store: DynProgramStore,
    config_service: Arc<ConfigService>,
    adapters: AdapterRegistry,
    state_tx: watch::Sender<ProgramState>,
    rx: mpsc::Receiver<Command>,
    process: Option<Box<dyn ManagedProcess>>,
    restart_deadline: Option<tokio::time::Instant>,
    restart_attempt: u32,
    started_at: Option<Instant>,
    recent_failures: VecDeque<Instant>,
    desired_running: bool,
    automatic_restarts_enabled: Arc<AtomicBool>,
    automatic_lifecycle_gate: Arc<RwLock<()>>,
}

impl ProgramController {
    async fn run(mut self) {
        loop {
            if let Some(mut process) = self.process.take() {
                tokio::select! {
                    command = self.rx.recv() => {
                        self.process = Some(process);
                        let Some(command) = command else { self.force_shutdown().await; break; };
                        if self.handle_command(command).await { break; }
                    }
                    exit = process.wait() => {
                        self.handle_exit(exit).await;
                    }
                }
            } else if let Some(deadline) = self.restart_deadline {
                tokio::select! {
                    command = self.rx.recv() => {
                        let Some(command) = command else { break; };
                        if self.handle_command(command).await { break; }
                    }
                    _ = tokio::time::sleep_until(deadline) => {
                        self.restart_deadline = None;
                        let automatic_lifecycle_gate = self.automatic_lifecycle_gate.clone();
                        let _automatic_restart = automatic_lifecycle_gate.read().await;
                        if !self.desired_running
                            || !self.automatic_restarts_enabled.load(Ordering::Acquire)
                        {
                            self.set_state(ProgramState::Stopped);
                        } else if let Err(error) = self.start_process(false).await {
                            self.publish_error(error);
                        }
                    }
                }
            } else {
                let Some(command) = self.rx.recv().await else {
                    break;
                };
                if self.handle_command(command).await {
                    break;
                }
            }
        }
    }

    async fn handle_command(&mut self, command: Command) -> bool {
        let Command {
            mutation,
            response,
            _guard,
        } = command;
        let shutdown = matches!(mutation, Mutation::Shutdown);
        let result = match mutation {
            Mutation::Start { interactive } => {
                self.desired_running = true;
                self.start_process(interactive).await.map(|_| None)
            }
            Mutation::Stop => {
                self.desired_running = false;
                self.stop_process().await.map(|_| None)
            }
            Mutation::Restart { interactive } => {
                self.restart_process(interactive).await.map(|_| None)
            }
            Mutation::UpdateSpec {
                expected_spec,
                next_spec,
            } => self
                .update_spec(*expected_spec, *next_spec)
                .await
                .map(|_| None),
            Mutation::UpdateSpecAndRestart {
                expected_spec,
                next_spec,
            } => self
                .update_spec_and_restart(*expected_spec, *next_spec)
                .await
                .map(|_| None),
            Mutation::CommitPreparedPackage {
                expected_spec,
                next_spec,
                staged,
            } => self
                .commit_prepared_package(*expected_spec, *next_spec, staged)
                .await
                .map(|_| None),
            Mutation::ApplyPreparedConfig {
                expected_spec,
                prepared,
                interactive,
            } => self
                .apply_config(*expected_spec, prepared, interactive)
                .await
                .map(Some),
            Mutation::UpdateSpecAndApplyPreparedConfig {
                expected_spec,
                next_spec,
                prepared,
                interactive,
            } => self
                .update_spec_and_apply_config(*expected_spec, *next_spec, prepared, interactive)
                .await
                .map(Some),
            Mutation::Shutdown => {
                self.desired_running = false;
                self.stop_process().await.map(|_| None)
            }
        };
        let terminate = shutdown && result.is_ok();
        let _ = response.send(result);
        drop(_guard);
        terminate
    }

    async fn start_process(&mut self, interactive: bool) -> Result<()> {
        if self.process.is_some() {
            return Ok(());
        }
        self.restart_deadline = None;
        self.set_state(ProgramState::Starting);
        let prepared = async {
            self.refresh_binary_identity().await?;
            let spec = self.spec.read().await.clone();
            let adapter = self.adapters.get(spec.program_type.kind());
            let mut plan = adapter.launch_plan(&spec, &self.workspace)?;
            plan.interactive = interactive;
            self.driver.spawn(plan).await
        }
        .await;
        match prepared {
            Ok(process) => {
                let pid = process.pid();
                self.process = Some(process);
                self.started_at = Some(Instant::now());
                self.set_state(ProgramState::Running {
                    pid,
                    started_unix_ms: ProgramState::unix_ms_now(),
                });
                Ok(())
            }
            Err(error) => {
                self.publish_error(error.clone());
                Err(error)
            }
        }
    }

    async fn stop_process(&mut self) -> Result<()> {
        self.restart_deadline = None;
        self.restart_attempt = 0;
        self.recent_failures.clear();
        if let Some(mut process) = self.process.take() {
            self.set_state(ProgramState::Stopping);
            let stop_result = tokio::time::timeout(PROCESS_STOP_TIMEOUT, process.stop())
                .await
                .unwrap_or_else(|_| {
                    Err(CamelliaNexusError::new(
                        ErrorCode::StopFailed,
                        "Program did not stop within the safety deadline",
                    ))
                });
            match stop_result {
                Ok(_) => self.set_state(ProgramState::Stopped),
                Err(error) => {
                    let pid = process.pid();
                    self.process = Some(process);
                    self.set_state(ProgramState::StopFailed {
                        pid,
                        message: error.message.clone(),
                    });
                    return Err(error);
                }
            }
        } else {
            self.set_state(ProgramState::Stopped);
        }
        Ok(())
    }

    async fn restart_process(&mut self, interactive: bool) -> Result<()> {
        // A restart is two explicit lifecycle operations. Suppress policy-driven restarts
        // throughout the stop phase; only restore the running intent after it completed.
        self.desired_running = false;
        self.stop_process().await?;
        self.desired_running = true;
        self.start_process(interactive).await
    }

    async fn update_spec(&mut self, expected_spec: ProgramSpec, next: ProgramSpec) -> Result<()> {
        if *self.spec.read().await != expected_spec {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Program settings changed while the update was being prepared",
            ));
        }
        self.commit_spec(next).await
    }

    async fn commit_spec(&mut self, next: ProgramSpec) -> Result<()> {
        next.validate()?;
        let current = self.spec.read().await.clone();
        let runtime_changed = runtime_fields_changed(&current, &next);
        if runtime_changed && (self.process.is_some() || self.restart_deadline.is_some()) {
            return Err(CamelliaNexusError::new(
                ErrorCode::InvalidState,
                "Stop the active program or pending retry before changing runtime settings",
            ));
        }
        self.store.save(&next).await?;
        *self.spec.write().await = next;
        if runtime_changed
            && matches!(
                *self.state_tx.borrow(),
                ProgramState::Exited { .. } | ProgramState::Error { .. }
            )
        {
            self.set_state(ProgramState::Stopped);
        }
        Ok(())
    }

    async fn update_spec_and_restart(
        &mut self,
        expected_spec: ProgramSpec,
        next: ProgramSpec,
    ) -> Result<()> {
        let previous = self.spec.read().await.clone();
        if previous != expected_spec {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Program settings changed while the update was being prepared",
            ));
        }
        let was_active = self.process.is_some() || self.restart_deadline.is_some();
        if !was_active {
            return self.commit_spec(next).await;
        }

        self.desired_running = false;
        self.stop_process().await?;
        if let Err(update_error) = self.commit_spec(next).await {
            self.desired_running = true;
            if let Err(restart_error) = self.start_process(true).await {
                tracing::error!(%restart_error, "could not restore the running program after settings validation failed");
            }
            return Err(update_error);
        }

        self.desired_running = true;
        if let Err(start_error) = self.start_process(true).await {
            self.desired_running = false;
            if let Err(rollback_error) = self.commit_spec(previous).await {
                return Err(CamelliaNexusError::new(
                    ErrorCode::Internal,
                    "The program could not start and its previous settings could not be restored",
                )
                .with_details(format!(
                    "start failed: {start_error}; settings rollback failed: {rollback_error}"
                )));
            }
            self.desired_running = true;
            if let Err(restore_error) = self.start_process(true).await {
                return Err(CamelliaNexusError::new(
                    ErrorCode::SpawnFailed,
                    "The program could not start with either the new or restored settings",
                )
                .with_details(format!(
                    "new settings failed: {start_error}; restored settings failed: {restore_error}"
                )));
            }
            return Err(start_error);
        }
        Ok(())
    }

    async fn refresh_binary_identity(&mut self) -> Result<()> {
        let mut spec = self.spec.read().await.clone();
        let mut current = self.store.executable_metadata(&spec).await?;
        let changed = spec.executable.metadata().is_none_or(|recorded| {
            recorded.size != current.size || recorded.modified_unix_ms != current.modified_unix_ms
        });
        if !changed {
            return Ok(());
        }
        current.detected_version = self.config_service.probe_binary(&spec).await?;
        spec.executable.set_metadata(current);
        self.store.save(&spec).await?;
        *self.spec.write().await = spec;
        Ok(())
    }

    async fn commit_prepared_package(
        &mut self,
        expected_spec: ProgramSpec,
        next_spec: ProgramSpec,
        staged: StagedPackage,
    ) -> Result<()> {
        if self.process.is_some() || self.restart_deadline.is_some() {
            let _ = self.store.discard_package(staged).await;
            return Err(CamelliaNexusError::new(
                ErrorCode::InvalidState,
                "Stop the active program or pending retry before replacing its files",
            ));
        }
        let current_spec = self.spec.read().await.clone();
        if current_spec != expected_spec {
            let _ = self.store.discard_package(staged).await;
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Program settings changed while the package was being prepared",
            ));
        }
        if !next_spec.executable.is_managed() {
            let _ = self.store.discard_package(staged).await;
            return Err(CamelliaNexusError::new(
                ErrorCode::InvalidSpec,
                "External executables do not have a managed package",
            ));
        }
        if let Err(error) = next_spec.validate() {
            let _ = self.store.discard_package(staged).await;
            return Err(error);
        }
        let discard = staged.clone();
        if let Err(error) = self
            .store
            .commit_package(staged, &expected_spec, &next_spec)
            .await
        {
            let _ = self.store.discard_package(discard).await;
            return Err(error);
        }
        *self.spec.write().await = next_spec;
        self.set_state(ProgramState::Stopped);
        Ok(())
    }

    async fn apply_config(
        &mut self,
        expected_spec: ProgramSpec,
        prepared: PreparedConfigGuard,
        interactive: bool,
    ) -> Result<String> {
        let state = self.state_tx.borrow().clone();
        if !matches!(
            state,
            ProgramState::Stopped
                | ProgramState::Running { .. }
                | ProgramState::Exited { .. }
                | ProgramState::Error { .. }
        ) {
            let _ = self.config_service.discard(prepared).await;
            return Err(CamelliaNexusError::new(
                ErrorCode::InvalidState,
                "Configuration cannot be applied in the current state",
            ));
        }
        let spec = self.spec.read().await.clone();
        if spec != expected_spec {
            let _ = self.config_service.discard(prepared).await;
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Program settings changed while the configuration was being prepared",
            ));
        }
        let was_running = self.process.is_some();
        if was_running && let Err(error) = self.stop_process().await {
            let _ = self.config_service.discard(prepared).await;
            return Err(error);
        }
        let committed = match self.config_service.commit(prepared).await {
            Ok(committed) => committed,
            Err(error) => {
                if was_running {
                    let _ = self.start_process(interactive).await;
                }
                return Err(error);
            }
        };
        if !was_running {
            self.set_state(ProgramState::Stopped);
            return self.config_service.finalize(&spec, committed).await;
        }

        if let Err(new_error) = self.start_process(interactive).await {
            return self
                .rollback_after_failed_apply(&spec, new_error, interactive)
                .await;
        }

        let early_exit = {
            let process = self.process.as_mut().ok_or_else(|| {
                CamelliaNexusError::new(ErrorCode::Internal, "Started process handle is missing")
            })?;
            tokio::time::timeout(Duration::from_secs(2), process.wait()).await
        };
        match early_exit {
            Err(_) => self.config_service.finalize(&spec, committed).await,
            Ok(exit_result) => {
                self.process = None;
                let error = match exit_result {
                    Ok(exit) => CamelliaNexusError::new(
                        ErrorCode::ConfigInvalid,
                        "Program exited during configuration stabilization",
                    )
                    .with_details(format!("exit code: {:?}", exit.code)),
                    Err(error) => CamelliaNexusError::new(
                        ErrorCode::ConfigInvalid,
                        "Program could not be observed after applying configuration",
                    )
                    .with_details(error.to_string()),
                };
                self.rollback_after_failed_apply(&spec, error, interactive)
                    .await
            }
        }
    }

    async fn update_spec_and_apply_config(
        &mut self,
        expected_spec: ProgramSpec,
        next_spec: ProgramSpec,
        prepared: PreparedConfigGuard,
        interactive: bool,
    ) -> Result<String> {
        let state = self.state_tx.borrow().clone();
        if !matches!(
            state,
            ProgramState::Stopped
                | ProgramState::Running { .. }
                | ProgramState::Exited { .. }
                | ProgramState::Error { .. }
        ) {
            let _ = self.config_service.discard(prepared).await;
            return Err(CamelliaNexusError::new(
                ErrorCode::InvalidState,
                "Configuration cannot be applied in the current state",
            ));
        }
        if let Err(error) = next_spec.validate() {
            let _ = self.config_service.discard(prepared).await;
            return Err(error);
        }
        let current = self.spec.read().await.clone();
        if current != expected_spec {
            let _ = self.config_service.discard(prepared).await;
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Program settings changed while the transaction was being prepared",
            ));
        }
        if runtime_fields_changed(&current, &next_spec)
            && (self.process.is_some() || self.restart_deadline.is_some())
        {
            let _ = self.config_service.discard(prepared).await;
            return Err(CamelliaNexusError::new(
                ErrorCode::InvalidState,
                "Stop the active program or pending retry before changing runtime settings",
            ));
        }

        let was_running = self.process.is_some();
        if was_running && let Err(error) = self.stop_process().await {
            let _ = self.config_service.discard(prepared).await;
            return Err(error);
        }
        let committed = match self
            .config_service
            .commit_program_update(&current, &next_spec, prepared)
            .await
        {
            Ok(committed) => committed,
            Err(error) => {
                if was_running {
                    let _ = self.start_process(interactive).await;
                }
                return Err(error);
            }
        };
        *self.spec.write().await = next_spec;
        if !was_running {
            match self
                .config_service
                .finalize_program_update(&committed)
                .await
            {
                Ok(hash) => {
                    self.set_state(ProgramState::Stopped);
                    return Ok(hash);
                }
                Err(error) => {
                    return self
                        .rollback_program_config_update(
                            &current,
                            committed,
                            error,
                            interactive,
                            false,
                        )
                        .await;
                }
            }
        }

        self.desired_running = true;
        if let Err(new_error) = self.start_process(interactive).await {
            return self
                .rollback_program_config_update(&current, committed, new_error, interactive, true)
                .await;
        }
        let early_exit = {
            let process = self.process.as_mut().ok_or_else(|| {
                CamelliaNexusError::new(ErrorCode::Internal, "Started process handle is missing")
            })?;
            tokio::time::timeout(Duration::from_secs(2), process.wait()).await
        };
        match early_exit {
            Err(_) => match self
                .config_service
                .finalize_program_update(&committed)
                .await
            {
                Ok(hash) => Ok(hash),
                Err(error) => {
                    self.rollback_program_config_update(
                        &current,
                        committed,
                        error,
                        interactive,
                        true,
                    )
                    .await
                }
            },
            Ok(exit_result) => {
                self.process = None;
                let error = match exit_result {
                    Ok(exit) => CamelliaNexusError::new(
                        ErrorCode::ConfigInvalid,
                        "Program exited during configuration stabilization",
                    )
                    .with_details(format!("exit code: {:?}", exit.code)),
                    Err(error) => CamelliaNexusError::new(
                        ErrorCode::ConfigInvalid,
                        "Program could not be observed after applying configuration",
                    )
                    .with_details(error.to_string()),
                };
                self.rollback_program_config_update(&current, committed, error, interactive, true)
                    .await
            }
        }
    }

    async fn rollback_program_config_update(
        &mut self,
        previous_spec: &ProgramSpec,
        committed: crate::config_service::CommittedProgramConfigGuard,
        new_error: CamelliaNexusError,
        interactive: bool,
        restart_previous: bool,
    ) -> Result<String> {
        self.desired_running = false;
        if self.process.is_some()
            && let Err(stop_error) = self.stop_process().await
        {
            return Err(CamelliaNexusError::new(
                ErrorCode::StopFailed,
                "The rejected program/configuration transaction could not be rolled back while active",
            )
            .with_details(format!(
                "transaction failed: {new_error}; stop before rollback failed: {stop_error}"
            )));
        }
        if let Err(restore_error) = self.config_service.rollback_program_update(committed).await {
            let error = CamelliaNexusError::new(
                ErrorCode::Storage,
                "The previous program settings and configuration could not be restored",
            )
            .with_details(format!(
                "transaction failed: {new_error}; restore failed: {restore_error}"
            ));
            self.publish_error(error.clone());
            return Err(error);
        }
        *self.spec.write().await = previous_spec.clone();
        if !matches!(self.state_tx.borrow().clone(), ProgramState::Stopped) {
            self.set_state(ProgramState::Stopped);
        }
        if !restart_previous {
            return Err(new_error);
        }
        self.desired_running = true;
        match self.start_process(interactive).await {
            Ok(()) => Err(new_error),
            Err(old_error) => {
                let error = CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    "New program settings failed and rollback could not restart the old configuration",
                )
                .with_details(format!("new: {new_error}; rollback: {old_error}"));
                self.publish_error(error.clone());
                Err(error)
            }
        }
    }

    async fn rollback_after_failed_apply(
        &mut self,
        spec: &ProgramSpec,
        new_error: CamelliaNexusError,
        interactive: bool,
    ) -> Result<String> {
        self.desired_running = false;
        if self.process.is_some()
            && let Err(stop_error) = self.stop_process().await
        {
            return Err(CamelliaNexusError::new(
                ErrorCode::StopFailed,
                "The rejected configuration could not be rolled back while the program was active",
            )
            .with_details(format!(
                "configuration failed: {new_error}; stop before rollback failed: {stop_error}"
            )));
        }
        if let Err(restore_error) = self.config_service.restore_backup(spec).await {
            let error = CamelliaNexusError::new(
                ErrorCode::Storage,
                "The previous configuration could not be restored",
            )
            .with_details(format!(
                "configuration failed: {new_error}; restore failed: {restore_error}"
            ));
            self.publish_error(error.clone());
            return Err(error);
        }
        self.desired_running = true;
        match self.start_process(interactive).await {
            Ok(()) => Err(new_error),
            Err(old_error) => {
                let error = CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    "New configuration failed and rollback could not restart the old one",
                )
                .with_details(format!("new: {new_error}; rollback: {old_error}"));
                self.publish_error(error.clone());
                Err(error)
            }
        }
    }

    async fn handle_exit(&mut self, result: Result<ProcessExit>) {
        let exit = match result {
            Ok(exit) => exit,
            Err(error) => {
                self.publish_error(error);
                return;
            }
        };
        if !self.desired_running {
            self.restart_deadline = None;
            self.restart_attempt = 0;
            self.recent_failures.clear();
            self.set_state(ProgramState::Stopped);
            return;
        }
        self.set_state(ProgramState::Exited {
            code: exit.code,
            success: exit.success,
        });
        if self
            .started_at
            .is_some_and(|started| started.elapsed() >= STABLE_RESET)
        {
            self.restart_attempt = 0;
            self.recent_failures.clear();
        }
        let policy = self.spec.read().await.restart_policy;
        let should_restart = self.desired_running
            && self.automatic_restarts_enabled.load(Ordering::Acquire)
            && match policy {
                RestartPolicy::Never => false,
                RestartPolicy::OnFailure => !exit.success,
                RestartPolicy::Always => true,
            };
        if !should_restart {
            return;
        }
        let now = Instant::now();
        self.recent_failures.push_back(now);
        while self
            .recent_failures
            .front()
            .is_some_and(|failure| now.duration_since(*failure) > FAILURE_WINDOW)
        {
            self.recent_failures.pop_front();
        }
        if self.recent_failures.len() >= MAX_FAILURES {
            self.set_state(ProgramState::Error {
                code: ErrorCode::SpawnFailed,
                message: "Program failed 10 times within 10 minutes".into(),
            });
            return;
        }
        let delay = BACKOFF_SECONDS
            .get(self.restart_attempt as usize)
            .copied()
            .unwrap_or(30);
        self.restart_attempt = self.restart_attempt.saturating_add(1);
        self.restart_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(delay));
        self.set_state(ProgramState::Backoff {
            attempt: self.restart_attempt,
            delay_seconds: delay,
        });
    }

    async fn force_shutdown(&mut self) {
        self.desired_running = false;
        let _ = self.stop_process().await;
    }

    fn publish_error(&self, error: CamelliaNexusError) {
        self.set_state(ProgramState::Error {
            code: error.code,
            message: error.message,
        });
    }

    fn set_state(&self, state: ProgramState) {
        self.state_tx.send_replace(state);
    }
}

fn runtime_fields_changed(current: &ProgramSpec, next: &ProgramSpec) -> bool {
    current.executable != next.executable
        || current.program_type != next.program_type
        || current.managed_config != next.managed_config
        || current.working_directory != next.working_directory
        || current.environment != next.environment
        || current.privilege_policy != next.privilege_policy
}
