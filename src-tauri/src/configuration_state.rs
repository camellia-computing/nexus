use std::{collections::HashMap, sync::Arc};

use camellia_nexus_core::{
    CamelliaNexusError, CandidateValidationStatus, ConfigurationCandidate, ConfigurationConflict,
    ConfigurationDiagnostic, ConfigurationFormat, ConfigurationIssueScope,
    ConfigurationMutationContext, ConfigurationOperationKind, ConfigurationOperationStatus,
    ConfigurationRevision, ConfigurationState, ConfigurationStateView,
    ConfigurationWorkspaceSnapshot, ConflictSeverity, CoreAdmissionReport, CoreAdmissionStatus,
    CoreCompatibilityProfile, CoreTargetIdentity, CoreValidationEvidence, ErrorCode,
    FinalConflictResolution, FinalEditorSession, ProgramId, ProgramManager, ProgramSpec, Result,
    ShareImportPreview, SourceFreshness, SourceSnapshot, SourceStatus,
    refresh_final_editor_conflicts, resolve_final_editor_conflict,
};
use serde_json::Value;
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};
use uuid::Uuid;

use crate::{
    FileStore, config_credentials::CredentialSnapshot, config_sources::SourceRefreshResult,
};

const NATIVE_VALIDATOR_CONTRACT_REVISION: &str = camellia_nexus_core::CORE_IMPLEMENTATION_REVISION;

pub(crate) struct ConfigurationCoordinator {
    store: Arc<FileStore>,
    locks: Arc<RwLock<HashMap<ProgramId, Arc<Mutex<()>>>>>,
}

pub(crate) struct ConfigurationLease {
    _guard: OwnedMutexGuard<()>,
}

pub(crate) struct PreparedManagedIntegrationUpdate {
    _lease: ConfigurationLease,
    state: ConfigurationState,
    previous_state_revision: u64,
}

pub(crate) struct PreparedSourceRefresh {
    spec: ProgramSpec,
    state: ConfigurationState,
    previous_state_revision: u64,
    sidecars: Vec<(String, Vec<u8>)>,
}

impl ConfigurationCoordinator {
    pub(crate) fn new(store: Arc<FileStore>) -> Self {
        Self {
            store,
            locks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub(crate) async fn begin_workspace_update(
        &self,
        id: &ProgramId,
        previous_spec: &ProgramSpec,
        next_spec: &ProgramSpec,
        expected_state_revision: u64,
    ) -> Result<()> {
        self.store
            .begin_configuration_workspace_update(
                id,
                previous_spec,
                next_spec,
                expected_state_revision,
            )
            .await
    }

    pub(crate) async fn mark_workspace_update_committed(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
    ) -> Result<()> {
        let (spec, _) = manager.get(id).await?;
        self.store
            .mark_configuration_workspace_update_committed(id, &spec)
            .await
    }

    pub(crate) async fn finish_workspace_update(&self, id: &ProgramId) -> Result<()> {
        self.store.finish_configuration_workspace_update(id).await
    }

    pub(crate) async fn rollback_workspace_update(&self, id: &ProgramId) -> Result<()> {
        self.store.rollback_configuration_workspace_update(id).await
    }

    pub(crate) async fn reconcile_workspace_commit(&self, spec: &ProgramSpec) -> Result<()> {
        self.store.reconcile_configuration_workspace(spec).await
    }

    async fn lock(&self, id: &ProgramId) -> ConfigurationLease {
        let lock = if let Some(lock) = self.locks.read().await.get(id).cloned() {
            lock
        } else {
            let mut locks = self.locks.write().await;
            locks
                .entry(id.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        ConfigurationLease {
            _guard: lock.lock_owned().await,
        }
    }

    pub(crate) async fn acquire_lease(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
    ) -> Result<ConfigurationLease> {
        let lease = self.lock(id).await;
        self.recover_interrupted_operation(manager, id).await?;
        Ok(lease)
    }

    pub(crate) async fn load_workspace(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
    ) -> Result<ConfigurationWorkspaceSnapshot> {
        let lease = self.lock(id).await;
        self.recover_interrupted_operation(manager, id).await?;
        match self.load_workspace_with_lease(manager, id, &lease).await {
            Ok(snapshot) => Ok(snapshot),
            Err(error) => {
                let (spec, _) = manager.get(id).await?;
                let Some(report) =
                    CoreAdmissionReport::from_rejection(spec.program_type.kind(), &error)?
                else {
                    return Err(error);
                };
                let Some(state) = self.store.load_configuration_state(id).await? else {
                    return Err(error);
                };
                let mut view = view_for_spec(&spec, &state)?;
                project_admission(&mut view, report);
                Ok(ConfigurationWorkspaceSnapshot {
                    state: view,
                    editor_session: state.editor_session,
                    operation_result: None,
                })
            }
        }
    }

    pub(crate) async fn prepare_package_workspace(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        next_spec: &ProgramSpec,
        expected_state_revision: Option<u64>,
        _lease: &ConfigurationLease,
    ) -> Result<Option<camellia_nexus_core::PackageConfigurationUpdate>> {
        if next_spec.program_type.main_config().is_none() {
            return Ok(None);
        }
        // Use the retained workspace so an unrecognized replaced file cannot prevent recovery.
        let mut state = self.load_or_initialize(manager, id).await?;
        if expected_state_revision != Some(state.state_revision) {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Configuration state changed before package replacement",
            )
            .with_message_key("CONFIGURATION_STATE_STALE"));
        }
        let expected_state_revision = state.state_revision;
        self.retarget_state(id, next_spec, &mut state).await?;
        Ok(Some(camellia_nexus_core::PackageConfigurationUpdate {
            expected_state_revision,
            state,
        }))
    }

    pub(crate) async fn load_workspace_with_lease(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        lease: &ConfigurationLease,
    ) -> Result<ConfigurationWorkspaceSnapshot> {
        let (spec, state) = self.load_state_for_view(manager, id, lease).await?;
        let editor_session = final_editor_session_for_state(&state);
        Ok(ConfigurationWorkspaceSnapshot {
            state: view_for_spec(&spec, &state)?,
            editor_session: Some(editor_session),
            operation_result: None,
        })
    }

    pub(crate) async fn load_view_with_lease(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        lease: &ConfigurationLease,
    ) -> Result<ConfigurationStateView> {
        let (spec, state) = self.load_state_for_view(manager, id, lease).await?;
        view_for_spec(&spec, &state)
    }

    async fn load_state_for_view(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        _lease: &ConfigurationLease,
    ) -> Result<(ProgramSpec, ConfigurationState)> {
        let spec = manager.refresh_binary_identity_if_changed(id).await?;
        let mut state = self.load_or_initialize(manager, id).await?;
        let previous_state_revision = state.state_revision;
        if self.retarget_state(id, &spec, &mut state).await? {
            self.store
                .save_configuration_state(id, &state, Some(previous_state_revision))
                .await?;
        }
        Ok((spec, state))
    }

    pub(crate) async fn initialize_created(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        observations: Option<SourceRefreshResult>,
    ) -> Result<ConfigurationStateView> {
        let _lease = self.lock(id).await;
        let (spec, _) = manager.get(id).await?;
        if let Some(state) = self.store.load_configuration_state(id).await? {
            return view_for_spec(&spec, &state);
        }
        let document = manager.load_config(id).await?;
        let state = if let Some(observations) = observations {
            for snapshot in &observations.snapshots {
                if let (Some(reference), Some(raw)) = (
                    snapshot.raw_observation_ref.as_deref(),
                    observations.raw_observations.get(&snapshot.source_id),
                ) {
                    self.store
                        .save_configuration_sidecar(id, reference, raw)
                        .await?;
                }
            }
            state_from_source_observations(&spec, &document.content, observations)?
        } else {
            state_from_initial_document(&spec, &document.content)?
        };
        self.persist_created(id, &state).await?;
        view_for_spec(&spec, &state)
    }

    pub(crate) async fn set_guided_with_lease(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        setting_id: String,
        value: Option<serde_json::Value>,
        expected_generation: u64,
        lease: &ConfigurationLease,
    ) -> Result<ConfigurationWorkspaceSnapshot> {
        let (spec, mut state) = self.load_current(manager, id).await?;
        ensure_generation(&state, expected_generation)?;
        let previous_state_revision = state.state_revision;
        if state.guided_intent.values.get(&setting_id) == value.as_ref() {
            state.claim_guided_setting(&setting_id)?;
        }
        if let Some(value) = value {
            camellia_nexus_core::validate_guided_value(
                spec.program_type.kind(),
                &setting_id,
                &value,
            )?;
            state.guided_intent.set(setting_id.clone(), value);
        } else {
            state.guided_intent.reset(&setting_id);
        }
        state.rebuild_desired(now_unix_ms())?;
        self.persist_candidate(manager, id, state, previous_state_revision)
            .await?;
        self.load_workspace_with_lease(manager, id, lease).await
    }

    pub(crate) fn preview_import(
        &self,
        target: &CoreTargetIdentity,
        input: &[u8],
    ) -> Result<ShareImportPreview> {
        camellia_nexus_core::preview_share_import_for_version(input, target)
    }

    async fn get_final_editor_session_with_lease(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        _lease: &ConfigurationLease,
    ) -> Result<FinalEditorSession> {
        let (_, state) = self.load_current(manager, id).await?;
        Ok(final_editor_session_for_state(&state))
    }

    pub(crate) async fn update_final_configuration_draft_with_lease(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        mut draft: FinalEditorSession,
        expected_revision: u64,
        lease: &ConfigurationLease,
    ) -> Result<ConfigurationWorkspaceSnapshot> {
        let (_spec, state) = self.load_current(manager, id).await?;
        let candidate_content = state.desired.content.clone();
        if draft.draft_revision != expected_revision {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Final configuration draft revision is stale",
            )
            .with_message_key("CONFIGURATION_DRAFT_STALE"));
        }
        let persisted_before = self.store.load_final_editor_draft(id).await?;
        match persisted_before.clone() {
            Some(persisted) => {
                if persisted.draft_revision != expected_revision
                    || persisted.session_id != draft.session_id
                {
                    return Err(CamelliaNexusError::new(
                        ErrorCode::ConfigConflict,
                        "Final configuration draft revision is stale",
                    )
                    .with_message_key("CONFIGURATION_DRAFT_STALE"));
                }
                draft.conflicts = persisted.conflicts;
                draft.resolutions = persisted.resolutions;
                draft.unresolved_conflict_ids = persisted.unresolved_conflict_ids;
                draft.rebase_required = persisted.rebase_required;
            }
            None => {
                if expected_revision != 0 {
                    return Err(CamelliaNexusError::new(
                        ErrorCode::ConfigConflict,
                        "Configuration changed before the Final editor draft was first saved",
                    )
                    .with_message_key("CONFIGURATION_DRAFT_STALE"));
                }
                if draft.session_id.trim().is_empty() {
                    return Err(CamelliaNexusError::new(
                        ErrorCode::InvalidSpec,
                        "Final configuration draft session id is empty",
                    ));
                }
                draft.conflicts.clear();
                draft.resolutions.clear();
                draft.unresolved_conflict_ids.clear();
            }
        }
        if draft.based_on_candidate_generation > state.generation {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Final configuration draft generation is invalid",
            )
            .with_message_key("CONFIGURATION_GENERATION_STALE"));
        }
        camellia_nexus_core::rebase_final_editor_session(
            &mut draft,
            state.format,
            &candidate_content,
            state.state_revision,
            state.generation,
        )?;
        refresh_final_editor_conflicts(&mut draft, state.format);
        if persisted_before.as_ref() == Some(&draft) {
            return self.load_workspace_with_lease(manager, id, lease).await;
        }
        draft.draft_revision = draft.draft_revision.saturating_add(1);
        draft.updated_unix_ms = now_unix_ms();
        self.store
            .save_final_editor_draft(id, &draft, Some(expected_revision))
            .await?;
        self.load_workspace_with_lease(manager, id, lease).await
    }

    pub(crate) async fn rebase_final_configuration_draft_with_lease(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        expected_revision: u64,
        lease: &ConfigurationLease,
    ) -> Result<ConfigurationWorkspaceSnapshot> {
        let (_spec, state) = self.load_current(manager, id).await?;
        let candidate_content = state.desired.content.clone();
        let mut draft = self
            .store
            .load_final_editor_draft(id)
            .await?
            .ok_or_else(|| {
                CamelliaNexusError::new(
                    ErrorCode::NotFound,
                    "Final configuration draft was not found",
                )
            })?;
        if draft.draft_revision != expected_revision {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Final editor draft changed",
            )
            .with_message_key("CONFIGURATION_DRAFT_STALE"));
        }
        let before = draft.clone();
        camellia_nexus_core::rebase_final_editor_session(
            &mut draft,
            state.format,
            &candidate_content,
            state.state_revision,
            state.generation,
        )?;
        refresh_final_editor_conflicts(&mut draft, state.format);
        if draft == before {
            return self.load_workspace_with_lease(manager, id, lease).await;
        }
        draft.draft_revision = draft.draft_revision.saturating_add(1);
        draft.updated_unix_ms = now_unix_ms();
        self.store
            .save_final_editor_draft(id, &draft, Some(expected_revision))
            .await?;
        self.load_workspace_with_lease(manager, id, lease).await
    }

    pub(crate) async fn resolve_final_draft_conflict_with_lease(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        conflict_id: String,
        resolution: FinalConflictResolution,
        expected_revision: u64,
        lease: &ConfigurationLease,
    ) -> Result<ConfigurationWorkspaceSnapshot> {
        let (_spec, state) = self.load_current(manager, id).await?;
        let mut draft = self
            .store
            .load_final_editor_draft(id)
            .await?
            .ok_or_else(|| {
                CamelliaNexusError::new(
                    ErrorCode::NotFound,
                    "Final configuration draft was not found",
                )
            })?;
        if draft.based_on_candidate_generation != state.generation
            || draft.base_content != state.desired.content
        {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Configuration changed; rebase the draft before resolving conflicts",
            )
            .with_message_key("CONFIGURATION_DRAFT_STALE"));
        }
        resolve_final_editor_conflict(&mut draft, state.format, &conflict_id, resolution)?;
        draft.draft_revision = draft.draft_revision.saturating_add(1);
        draft.updated_unix_ms = now_unix_ms();
        self.store
            .save_final_editor_draft(id, &draft, Some(expected_revision))
            .await?;
        self.load_workspace_with_lease(manager, id, lease).await
    }

    pub(crate) async fn resolve_final_configuration_conflict_with_lease(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        conflict_id: String,
        resolution: FinalConflictResolution,
        expected_generation: u64,
        lease: &ConfigurationLease,
    ) -> Result<ConfigurationWorkspaceSnapshot> {
        let (_spec, mut state) = self.load_current(manager, id).await?;
        ensure_generation(&state, expected_generation)?;
        let previous_state_revision = state.state_revision;
        state.resolve_final_conflict(&conflict_id, resolution, now_unix_ms())?;
        self.persist_candidate(manager, id, state, previous_state_revision)
            .await?;
        self.load_workspace_with_lease(manager, id, lease).await
    }

    pub(crate) async fn discard_final_configuration_draft_with_lease(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        expected_revision: u64,
        lease: &ConfigurationLease,
    ) -> Result<ConfigurationWorkspaceSnapshot> {
        self.store
            .discard_final_editor_draft(id, expected_revision)
            .await?;
        self.load_workspace_with_lease(manager, id, lease).await
    }

    pub(crate) async fn save_workspace_with_lease<F, Fut, Permit>(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        request: ConfigurationMutationContext,
        lease: &ConfigurationLease,
        authorize: F,
    ) -> Result<ConfigurationWorkspaceSnapshot>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<Permit>>,
    {
        request.ensure_kind(ConfigurationOperationKind::Save)?;
        let (_, state) = self.load_current(manager, id).await?;
        if state.operation_receipt(&request)?.is_some() {
            return self.operation_workspace(manager, id, &request, lease).await;
        }
        self.save_configuration_candidate_with_lease(manager, id, lease, &request, &authorize)
            .await?;
        self.operation_workspace(manager, id, &request, lease).await
    }

    async fn save_configuration_candidate_with_lease<F, Fut, Permit>(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        _lease: &ConfigurationLease,
        request: &ConfigurationMutationContext,
        authorize: &F,
    ) -> Result<ConfigurationStateView>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<Permit>>,
    {
        let (_spec, mut state) = self.load_current(manager, id).await?;
        let previous_state_revision = state.state_revision;
        if request.kind == ConfigurationOperationKind::Save {
            state.begin_operation(request.clone())?;
        }
        let draft = self.store.load_final_editor_draft(id).await?;
        if let Some(mut draft) = draft.clone() {
            refresh_final_editor_conflicts(&mut draft, state.format);
            if draft.rebase_required
                || !draft.unresolved_conflict_ids.is_empty()
                || draft.based_on_candidate_generation != state.generation
                || draft.base_content != state.desired.content
            {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigConflict,
                    "Resolve final configuration conflicts before saving",
                )
                .with_message_key("FINAL_EDIT_CONFLICT"));
            }
            state.replace_final_from_edited(draft.working_content.as_bytes(), now_unix_ms())?;
        }
        if state
            .desired
            .conflicts
            .iter()
            .any(|conflict| conflict.severity == camellia_nexus_core::ConflictSeverity::Error)
        {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Resolve all blocking configuration conflicts before saving the candidate",
            )
            .with_message_key("CONFIGURATION_BLOCKING_CONFLICT"));
        }
        state.mark_candidate_saved()?;
        let spec = manager.refresh_binary_identity(id).await?;
        if self.retarget_state(id, &spec, &mut state).await? {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Configuration target changed",
            )
            .with_message_key("CORE_TARGET_CHANGED"));
        }
        if draft.is_some() && state.state_revision == previous_state_revision {
            state.state_revision = state.state_revision.saturating_add(1);
        }
        let _authorization = authorize().await?;
        if manager.get(id).await?.0 != spec {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Configuration target changed before candidate save",
            )
            .with_message_key("CORE_TARGET_CHANGED"));
        }
        state.record_operation_candidate(&request.operation_id)?;
        if request.kind == ConfigurationOperationKind::Save {
            // The saved candidate, consumed draft and receipt share one atomic write.
            state.finish_operation(
                &request.operation_id,
                ConfigurationOperationStatus::Saved,
                state.generation,
                None,
            );
            self.store
                .save_configuration_candidate_state(
                    id,
                    &state,
                    previous_state_revision,
                    draft.as_ref().map(|draft| draft.draft_revision),
                )
                .await?;
        } else {
            self.store
                .save_configuration_state(id, &state, Some(previous_state_revision))
                .await?;
        }
        view_for_spec(&spec, &state)
    }

    async fn validate_candidate_with_lease<F, Fut, Permit>(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        expected_generation: u64,
        _lease: &ConfigurationLease,
        authorize: &F,
    ) -> Result<ConfigurationStateView>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<Permit>>,
    {
        let (_, state) = self.load_current(manager, id).await?;
        ensure_generation(&state, expected_generation)?;
        if !state.candidate_is_saved() {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Save the final candidate before validation",
            )
            .with_message_key("CONFIGURATION_CANDIDATE_UNSAVED"));
        }
        let previous_state_revision = state.state_revision;
        let (spec, state) = self.prepare_validation(manager, id, state).await?;
        if manager.refresh_binary_identity(id).await? != spec {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Validation target changed",
            )
            .with_message_key("CORE_TARGET_CHANGED"));
        }
        let _authorization = authorize().await?;
        if manager.get(id).await?.0 != spec {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Validation target changed",
            )
            .with_message_key("CORE_TARGET_CHANGED"));
        }
        self.store
            .save_configuration_state(id, &state, Some(previous_state_revision))
            .await?;
        view_for_spec(&spec, &state)
    }

    async fn apply_candidate_with_lease<F, Fut, Permit>(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        expected_generation: u64,
        interactive: bool,
        _lease: &ConfigurationLease,
        authorize: &F,
    ) -> Result<ConfigurationStateView>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<Permit>>,
    {
        let (_, mut state) = self.load_current(manager, id).await?;
        ensure_generation(&state, expected_generation)?;
        if !state.candidate_is_saved() {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Save the final candidate before applying",
            )
            .with_message_key("CONFIGURATION_CANDIDATE_UNSAVED"));
        }
        let previous_state_revision = state.state_revision;
        let spec = manager.refresh_binary_identity(id).await?;
        if self.retarget_state(id, &spec, &mut state).await? {
            let _authorization = authorize().await?;
            self.ensure_workspace_matches(manager, &spec, previous_state_revision)
                .await?;
            self.store
                .save_configuration_state(id, &state, Some(previous_state_revision))
                .await?;
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Core executable target changed; review and validate the retargeted candidate",
            ));
        }
        if state.desired.validation != CandidateValidationStatus::Valid {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Desired configuration has not passed Core validation",
            ));
        }
        // Re-check the exact binary/profile/config evidence before beginning
        // any active-file transaction. A stale or corrupted persisted
        // evidence record must fail closed without touching Runtime,
        // Applied, or Last Known Good.
        state.ensure_apply_ready()?;
        let active = manager.load_config(id).await?;
        if state.applied.as_ref() == Some(&state.desired)
            && active.base_hash == state.desired.revision.content_hash
        {
            let _authorization = authorize().await?;
            self.ensure_workspace_matches(manager, &spec, previous_state_revision)
                .await?;
            manager.verify_applied_config(&spec).await?;
            if let Some(draft) = &state.editor_session {
                let revision = draft.draft_revision;
                state.state_revision = state.state_revision.saturating_add(1);
                self.store
                    .save_configuration_candidate_state(
                        id,
                        &state,
                        previous_state_revision,
                        Some(revision),
                    )
                    .await?;
            }
            return view_for_spec(&spec, &state);
        }
        let prepared = manager
            .prepare_config(id, &spec, state.desired.content.clone(), active.base_hash)
            .await?;
        let commit_check = async {
            let permit = authorize().await?;
            self.ensure_workspace_matches(manager, &spec, state.state_revision)
                .await?;
            self.store.begin_configuration_apply(id, &state).await?;
            Ok(permit)
        }
        .await;
        let _authorization = match commit_check {
            Ok(permit) => permit,
            Err(error) => {
                manager.discard_prepared_config(prepared).await?;
                return Err(error);
            }
        };
        let result = manager
            .apply_prepared_config(id, &spec, prepared, interactive)
            .await;
        let hash = match result {
            Ok(hash) => hash,
            Err(error) => {
                if !matches!(
                    error.message_key.as_deref(),
                    Some(
                        "CONFIGURATION_RECOVERY_REQUIRED"
                            | "CONFIGURATION_COMMIT_RECOVERY_REQUIRED"
                    )
                ) {
                    self.store.finish_configuration_apply(id).await?;
                }
                return Err(error);
            }
        };
        if hash != state.desired.revision.content_hash {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Applied configuration hash does not match Desired revision",
            ));
        }
        self.store.reconcile_configuration_apply(&spec).await?;
        let (_, committed) = self.load_current(manager, id).await?;
        view_for_spec(&spec, &committed)
    }

    pub(crate) async fn activate_workspace_with_lease<F, Fut, Permit>(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        request: ConfigurationMutationContext,
        interactive: bool,
        lease: &ConfigurationLease,
        authorize: F,
    ) -> Result<ConfigurationWorkspaceSnapshot>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<Permit>>,
    {
        request.ensure_kind(ConfigurationOperationKind::Apply)?;
        self.load_view_with_lease(manager, id, lease).await?;
        let (_, mut state) = self.load_current(manager, id).await?;
        if state.operation_receipt(&request)?.is_some() {
            return self.operation_workspace(manager, id, &request, lease).await;
        }
        let authorization = authorize().await?;
        let previous_revision = state.state_revision;
        state.begin_operation(request.clone())?;
        self.store
            .save_configuration_state(id, &state, Some(previous_revision))
            .await?;
        drop(authorization);

        let result: Result<bool> = async {
            let mut candidate = self
                .save_configuration_candidate_with_lease(manager, id, lease, &request, &authorize)
                .await?;
            if candidate.desired.validation != CandidateValidationStatus::Valid
                || !candidate.workspace.editor.can_apply
            {
                candidate = self
                    .validate_candidate_with_lease(
                        manager,
                        id,
                        candidate.generation,
                        lease,
                        &authorize,
                    )
                    .await?;
            }
            if candidate.desired.validation != CandidateValidationStatus::Valid
                || !candidate.workspace.editor.can_apply
            {
                return Ok(false);
            }
            self.apply_candidate_with_lease(
                manager,
                id,
                candidate.generation,
                interactive,
                lease,
                &authorize,
            )
            .await?;
            Ok(true)
        }
        .await;
        match result {
            Ok(applied) => {
                self.finish_operation(
                    manager,
                    id,
                    &request.operation_id,
                    if applied {
                        ConfigurationOperationStatus::Applied
                    } else {
                        ConfigurationOperationStatus::Rejected
                    },
                    if applied { None } else { Some("CORE_INVALID") },
                )
                .await?;
                self.operation_workspace(manager, id, &request, lease).await
            }
            Err(error) => {
                if !matches!(
                    error.message_key.as_deref(),
                    Some(
                        "CONFIGURATION_RECOVERY_REQUIRED"
                            | "CONFIGURATION_COMMIT_RECOVERY_REQUIRED"
                    )
                ) {
                    self.finish_operation(
                        manager,
                        id,
                        &request.operation_id,
                        ConfigurationOperationStatus::Rejected,
                        error.message_key.as_deref(),
                    )
                    .await?;
                }
                Err(error)
            }
        }
    }

    pub(crate) async fn load_operation(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        request: ConfigurationMutationContext,
    ) -> Result<ConfigurationWorkspaceSnapshot> {
        let lease = self.lock(id).await;
        self.recover_interrupted_operation(manager, id).await?;
        self.operation_workspace(manager, id, &request, &lease)
            .await
    }

    async fn operation_workspace(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        request: &ConfigurationMutationContext,
        lease: &ConfigurationLease,
    ) -> Result<ConfigurationWorkspaceSnapshot> {
        let mut snapshot = self.load_workspace_with_lease(manager, id, lease).await?;
        let (_, state) = self.load_current(manager, id).await?;
        snapshot.operation_result = state
            .operation_receipt(request)?
            .map(|receipt| receipt.result.clone());
        Ok(snapshot)
    }

    async fn finish_operation(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        operation_id: &str,
        status: ConfigurationOperationStatus,
        message_key: Option<&str>,
    ) -> Result<()> {
        let (_, mut state) = self.load_current(manager, id).await?;
        let previous_revision = state.state_revision;
        if state.finish_operation(operation_id, status, state.generation, message_key) {
            self.store
                .save_configuration_state(id, &state, Some(previous_revision))
                .await?;
        }
        Ok(())
    }

    async fn recover_interrupted_operation(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
    ) -> Result<()> {
        // The caller owns the lease; a pending request cannot still be executing.
        // Committed Apply markers are reconciled by load_current before this check.
        let (spec, _) = manager.get(id).await?;
        self.store.reconcile_configuration_workspace(&spec).await?;
        let (_, state) = self.load_current(manager, id).await?;
        for receipt in state.operation_receipts {
            if receipt.result.status == ConfigurationOperationStatus::Pending {
                self.finish_operation(
                    manager,
                    id,
                    &receipt.request.operation_id,
                    ConfigurationOperationStatus::Interrupted,
                    Some("CONFIGURATION_OPERATION_INTERRUPTED"),
                )
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn prepare_source_refresh(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        local_base: Option<&std::path::Path>,
        credentials: &CredentialSnapshot,
    ) -> Result<PreparedSourceRefresh> {
        let lease = self.acquire_lease(manager, id).await?;
        let spec = manager.refresh_binary_identity(id).await?;
        let state = self.load_or_initialize(manager, id).await?;
        drop(lease);
        self.prepare_source_observations(
            spec,
            state,
            local_base,
            credentials,
            camellia_nexus_core::SourceUpdateKind::Refresh,
        )
        .await
    }

    #[cfg(all(test, unix))]
    async fn commit_source_refresh(
        &self,
        manager: &ProgramManager,
        prepared: PreparedSourceRefresh,
    ) -> Result<ConfigurationWorkspaceSnapshot> {
        let lease = self.acquire_lease(manager, &prepared.spec.id).await?;
        self.commit_source_refresh_with_lease(manager, prepared, &lease)
            .await
    }

    pub(crate) async fn prepare_source_refresh_with_lease(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        local_base: Option<&std::path::Path>,
        credentials: &CredentialSnapshot,
        _lease: &ConfigurationLease,
        source_update: camellia_nexus_core::SourceUpdateKind,
    ) -> Result<PreparedSourceRefresh> {
        let spec = manager.refresh_binary_identity(id).await?;
        let state = self.load_or_initialize(manager, id).await?;
        self.prepare_source_observations(spec, state, local_base, credentials, source_update)
            .await
    }

    async fn prepare_source_observations(
        &self,
        spec: ProgramSpec,
        mut state: ConfigurationState,
        local_base: Option<&std::path::Path>,
        credentials: &CredentialSnapshot,
        source_update: camellia_nexus_core::SourceUpdateKind,
    ) -> Result<PreparedSourceRefresh> {
        let id = &spec.id;
        let previous_state_revision = state.state_revision;
        self.retarget_state(id, &spec, &mut state).await?;
        let observed = crate::config_sources::refresh_snapshots(
            &spec,
            local_base,
            credentials,
            &state.source_snapshots,
            now_unix_ms(),
        )
        .await?;
        let mut sidecars = Vec::new();
        for snapshot in &observed.snapshots {
            if let (Some(reference), Some(raw)) = (
                snapshot.raw_observation_ref.as_deref(),
                observed.raw_observations.get(&snapshot.source_id),
            ) {
                sidecars.push((reference.to_owned(), raw.clone()));
            }
        }
        state.source_statuses = observed.statuses;
        reconcile_snapshots(&spec, &mut state.source_snapshots, observed.snapshots);
        let has_enabled_sources = spec
            .managed_config
            .as_ref()
            .is_some_and(|managed| managed.sources.iter().any(|source| source.enabled()));
        if !has_enabled_sources && source_update == camellia_nexus_core::SourceUpdateKind::Refresh {
            state.rebuild_desired(now_unix_ms())?;
            return Ok(PreparedSourceRefresh {
                spec,
                state,
                previous_state_revision,
                sidecars,
            });
        }
        let snapshots = ordered_snapshots(&spec, &state.source_snapshots);
        // A missing source without a usable snapshot blocks the complete candidate.
        // Failed refreshes with an existing snapshot retain that snapshot as stale.
        if observed.unavailable {
            let invalid = state
                .source_statuses
                .values()
                .any(|status| status.freshness == SourceFreshness::Invalid);
            let candidate_changed = !candidate_has_source_blocker(&state.desired, invalid);
            if candidate_changed {
                state.desired = blocked_source_candidate(&state, invalid);
            }
            // Source freshness/observation metadata is durable state even when
            // the same unavailable candidate is observed again. Keep storage
            // CAS monotonic without manufacturing a new candidate generation.
            state.state_revision = state.state_revision.saturating_add(1);
            return Ok(PreparedSourceRefresh {
                spec,
                state,
                previous_state_revision,
                sidecars,
            });
        }
        let merge =
            camellia_nexus_core::merge_configuration_sources(spec.program_type.kind(), &snapshots)?;
        state.base_provenance = merge.provenance;
        state.base = ConfigurationCandidate {
            revision: ConfigurationRevision::new(
                state.generation.saturating_add(1),
                &merge.content,
                now_unix_ms(),
            ),
            content: merge.content,
            compatibility_profile_hash: state.compatibility_profile.profile_hash.clone(),
            validation: CandidateValidationStatus::Pending,
            validation_evidence: None,
            diagnostics: Vec::new(),
            conflicts: merge.conflicts,
        };
        state.rebuild_desired_with_source_update(now_unix_ms(), source_update)?;
        Ok(PreparedSourceRefresh {
            spec,
            state,
            previous_state_revision,
            sidecars,
        })
    }

    pub(crate) async fn commit_source_refresh_with_lease(
        &self,
        manager: &ProgramManager,
        prepared: PreparedSourceRefresh,
        _lease: &ConfigurationLease,
    ) -> Result<ConfigurationWorkspaceSnapshot> {
        let PreparedSourceRefresh {
            spec,
            state,
            previous_state_revision,
            sidecars,
        } = prepared;
        let id = &spec.id;
        let (current_spec, current_state) = self.load_current(manager, id).await?;
        if current_spec != spec || current_state.state_revision != previous_state_revision {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Configuration changed while sources were being refreshed",
            )
            .with_message_key("CONFIGURATION_STATE_STALE"));
        }
        for (reference, raw) in sidecars {
            self.store
                .save_configuration_sidecar(id, &reference, &raw)
                .await?;
        }
        self.store
            .save_configuration_state(id, &state, Some(previous_state_revision))
            .await?;
        Ok(ConfigurationWorkspaceSnapshot {
            state: view_for_spec(&spec, &state)?,
            editor_session: Some(
                self.get_final_editor_session_with_lease(manager, id, _lease)
                    .await?,
            ),
            operation_result: None,
        })
    }

    pub(crate) async fn sync_managed_dashboard(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
    ) -> Result<ConfigurationStateView> {
        let _lease = self.lock(id).await;
        let (spec, mut state) = self.load_current(manager, id).await?;
        let previous_state_revision = state.state_revision;
        if let Some(managed) = spec.managed_config.as_ref() {
            camellia_nexus_core::sync_managed_dashboard_intent(&mut state.managed_intent, managed);
        }
        state.rebuild_desired(now_unix_ms())?;
        self.persist_candidate(manager, id, state, previous_state_revision)
            .await
    }

    pub(crate) async fn prepare_managed_integration_update(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        next_spec: &ProgramSpec,
        expected_generation: Option<u64>,
        claimed_settings: &[String],
    ) -> Result<PreparedManagedIntegrationUpdate> {
        let lease = self.lock(id).await;
        self.recover_interrupted_operation(manager, id).await?;
        let (_current, mut state) = self.load_current(manager, id).await?;
        if let Some(expected_generation) = expected_generation {
            ensure_generation(&state, expected_generation)?;
        }
        let previous_state_revision = state.state_revision;
        let empty = camellia_nexus_core::ManagedConfigSpec::default();
        let managed = next_spec.managed_config.as_ref().unwrap_or(&empty);
        camellia_nexus_core::sync_managed_dashboard_intent(&mut state.managed_intent, managed);
        state.rebuild_desired(now_unix_ms())?;
        for setting in claimed_settings {
            state.claim_managed_setting(setting)?;
        }
        if !claimed_settings.is_empty() {
            state.rebuild_desired(now_unix_ms())?;
        }
        Ok(PreparedManagedIntegrationUpdate {
            _lease: lease,
            state,
            previous_state_revision,
        })
    }

    pub(crate) async fn commit_managed_integration_update(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        _next_spec: &ProgramSpec,
        prepared: &PreparedManagedIntegrationUpdate,
    ) -> Result<ConfigurationStateView> {
        self.persist_candidate(
            manager,
            id,
            prepared.state.clone(),
            prepared.previous_state_revision,
        )
        .await
    }

    pub(crate) async fn begin_managed_integration_update(
        &self,
        id: &ProgramId,
        previous_spec: &ProgramSpec,
        next_spec: &ProgramSpec,
        prepared: &PreparedManagedIntegrationUpdate,
    ) -> Result<()> {
        self.begin_workspace_update(
            id,
            previous_spec,
            next_spec,
            prepared.previous_state_revision,
        )
        .await
    }

    async fn persist_candidate(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        mut state: ConfigurationState,
        expected_state_revision: u64,
    ) -> Result<ConfigurationStateView> {
        let spec = manager.refresh_binary_identity(id).await?;
        self.retarget_state(id, &spec, &mut state).await?;
        self.store
            .save_configuration_state(id, &state, Some(expected_state_revision))
            .await?;
        view_for_spec(&spec, &state)
    }

    async fn prepare_validation(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        mut state: ConfigurationState,
    ) -> Result<(ProgramSpec, ConfigurationState)> {
        let spec = manager.refresh_binary_identity(id).await?;
        self.retarget_state(id, &spec, &mut state).await?;
        if state.desired.validation == CandidateValidationStatus::Invalid
            && state
                .desired
                .conflicts
                .iter()
                .any(|conflict| conflict.severity == camellia_nexus_core::ConflictSeverity::Error)
        {
            return Ok((spec, state));
        }
        if !state.candidate_is_saved() {
            return Err(CamelliaNexusError::new(
                ErrorCode::InvalidState,
                "Save the current candidate before native validation",
            )
            .with_message_key("CONFIGURATION_CANDIDATE_UNSAVED"));
        }
        let active = manager.load_config(id).await?;
        if let Some(assessment) = manager
            .assess_configuration(id, &state.desired.content)
            .await?
            && !assessment.issues.is_empty()
        {
            let diagnostics = assessment
                .issues
                .iter()
                .map(|issue| {
                    Ok(ConfigurationDiagnostic {
                        location: Some(
                            camellia_nexus_core::ConfigurationDiagnosticLocation::from_pointer(
                                &issue.path,
                            ),
                        ),
                        code: issue.code.clone(),
                        message: "A configuration value is not supported".into(),
                        message_key: Some(issue.message_key.clone()),
                        scope: ConfigurationIssueScope::configuration(),
                        details: Some(serde_json::to_string(issue)?),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            state.mark_validation(false, diagnostics, None)?;
            return Ok((spec, state));
        }
        let validation = manager
            .validate_config(id, state.desired.content.clone(), active.base_hash)
            .await?;
        let diagnostics = if validation.valid {
            Vec::new()
        } else {
            vec![ConfigurationDiagnostic {
                location: None,
                code: "CORE_INVALID".into(),
                message: "Core validation failed".into(),
                message_key: Some(validation.report.message_key.clone()),
                scope: ConfigurationIssueScope::configuration(),
                details: Some(serde_json::to_string(&validation.report)?),
            }]
        };
        let evidence = validation
            .valid
            .then(|| native_validation_evidence(&spec, &state))
            .transpose()?;
        state.mark_validation(validation.valid, diagnostics, evidence)?;
        Ok((spec, state))
    }

    async fn retarget_state(
        &self,
        id: &ProgramId,
        spec: &ProgramSpec,
        state: &mut ConfigurationState,
    ) -> Result<bool> {
        let profile = compatibility_profile(spec)?;
        if state.compatibility_profile.profile_hash == profile.profile_hash {
            return Ok(false);
        }
        let target = profile.target.clone();
        let mut rejected_share_sources = Vec::new();
        for snapshot in state.source_snapshots.values_mut() {
            let Some(reference) = snapshot.raw_observation_ref.clone() else {
                continue;
            };
            let raw = self.load_original_observation(id, &reference).await?;
            let replacement = match SourceSnapshot::parse_share(
                snapshot.source_id.clone(),
                snapshot.source_name.clone(),
                &target,
                &raw,
                now_unix_ms(),
            ) {
                Ok(replacement) => replacement,
                Err(error) if is_zero_acceptance_share_error(&error) => {
                    rejected_share_sources.push(snapshot.source_id.clone());
                    continue;
                }
                Err(error) => return Err(error),
            };
            *snapshot = replacement;
        }
        state.compatibility_profile = profile;
        let ordered = ordered_snapshots(spec, &state.source_snapshots);
        if !ordered.is_empty() {
            let merge = camellia_nexus_core::merge_configuration_sources(
                spec.program_type.kind(),
                &ordered,
            )?;
            state.base_provenance = merge.provenance;
            state.base = ConfigurationCandidate {
                revision: ConfigurationRevision::new(
                    state.generation.saturating_add(1),
                    &merge.content,
                    now_unix_ms(),
                ),
                content: merge.content,
                compatibility_profile_hash: state.compatibility_profile.profile_hash.clone(),
                validation: CandidateValidationStatus::Pending,
                validation_evidence: None,
                diagnostics: Vec::new(),
                conflicts: merge.conflicts,
            };
        } else {
            state.base.compatibility_profile_hash =
                state.compatibility_profile.profile_hash.clone();
            state.base.validation = CandidateValidationStatus::Pending;
            state.base.validation_evidence = None;
        }
        state.rebuild_desired_with_source_update(
            now_unix_ms(),
            camellia_nexus_core::SourceUpdateKind::Refresh,
        )?;
        for source_id in rejected_share_sources {
            if let Some(status) = state.source_statuses.get_mut(&source_id) {
                status.freshness = SourceFreshness::Invalid;
                status.message_key = Some("CORE_TARGET_SOURCE_REJECTED".into());
            }
            state.desired.conflicts.push(ConfigurationConflict {
                semantic_path: format!("/sources/{source_id}"),
                reason: "Share source has no item expressible for the selected Core target".into(),
                severity: ConflictSeverity::Error,
                message_key: Some("CORE_TARGET_SOURCE_REJECTED".into()),
                scope: ConfigurationIssueScope::sources(source_id.clone()),
                source_value: None,
                guided_value: None,
                user_value: None,
                effective_value: None,
            });
            state.desired.diagnostics.push(ConfigurationDiagnostic {
                location: None,
                code: "CORE_TARGET_SOURCE_REJECTED".into(),
                message: "Share source was retained but has no accepted item for the selected Core target".into(),
                message_key: Some("CORE_TARGET_SOURCE_REJECTED".into()),
                scope: ConfigurationIssueScope::sources("share-import"),
                details: None,
            });
            state.desired.validation = CandidateValidationStatus::Invalid;
            state.desired.validation_evidence = None;
        }
        state.desired.diagnostics.push(ConfigurationDiagnostic {
            location: None,
            code: "CORE_TARGET_CHANGED".into(),
            message: "Core target changed; Sources, Intent, Details, and Final configuration were retargeted"
                .into(),
            message_key: Some("CORE_TARGET_CHANGED".into()),
            scope: ConfigurationIssueScope::compatibility(),
            details: None,
        });
        Ok(true)
    }

    async fn load_original_observation(&self, id: &ProgramId, hash: &str) -> Result<Vec<u8>> {
        self.store.load_configuration_sidecar(id, hash).await
    }

    async fn ensure_workspace_matches(
        &self,
        manager: &ProgramManager,
        expected_spec: &ProgramSpec,
        expected_state_revision: u64,
    ) -> Result<()> {
        let (spec, state) = self.load_current(manager, &expected_spec.id).await?;
        if spec != *expected_spec || state.state_revision != expected_state_revision {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Configuration changed before the operation committed",
            )
            .with_message_key("CONFIGURATION_STATE_STALE"));
        }
        Ok(())
    }

    async fn load_current(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
    ) -> Result<(ProgramSpec, ConfigurationState)> {
        let (spec, _) = manager.get(id).await?;
        let state = self.load_or_initialize(manager, id).await?;
        Ok((spec, state))
    }

    async fn load_or_initialize(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
    ) -> Result<ConfigurationState> {
        let (spec, _) = manager.get(id).await?;
        self.store.reconcile_configuration_apply(&spec).await?;
        if let Some(state) = self.store.load_configuration_state(id).await? {
            return Ok(state);
        }
        let (spec, _) = manager.get(id).await?;
        let document = manager.load_config(id).await?;
        let format = ConfigurationFormat::for_kind(spec.program_type.kind()).ok_or_else(|| {
            CamelliaNexusError::invalid_spec("Generic programs have no configuration state")
        })?;
        let last_known_good = self.store.load_last_known_good(id, format).await?;
        let state =
            state_from_existing_document(&spec, &document.content, last_known_good.as_deref())?;
        self.store
            .save_configuration_state(id, &state, None)
            .await?;
        Ok(state)
    }

    async fn persist_created(&self, id: &ProgramId, state: &ConfigurationState) -> Result<()> {
        self.store
            .save_last_known_good(id, state.format, &state.desired.content)
            .await?;
        self.store.save_configuration_state(id, state, None).await?;
        Ok(())
    }
}

fn is_zero_acceptance_share_error(error: &CamelliaNexusError) -> bool {
    error.code == ErrorCode::ConfigInvalid
        && error.message_key.as_deref() == Some("SOURCE_NO_COMPATIBLE_ITEMS")
}

#[test]
fn zero_acceptance_share_classification_uses_stable_code() {
    let error = CamelliaNexusError::new(ErrorCode::ConfigInvalid, "A changed diagnostic")
        .with_message_key("SOURCE_NO_COMPATIBLE_ITEMS");
    assert!(is_zero_acceptance_share_error(&error));
    assert!(!is_zero_acceptance_share_error(&CamelliaNexusError::new(
        ErrorCode::ConfigInvalid,
        "Share source contains no translatable items",
    )));
    assert!(!is_zero_acceptance_share_error(
        &CamelliaNexusError::new(ErrorCode::Storage, "A different failure",)
            .with_message_key("SOURCE_NO_COMPATIBLE_ITEMS")
    ));
}

fn state_from_source_observations(
    spec: &ProgramSpec,
    active_content: &str,
    observations: SourceRefreshResult,
) -> Result<ConfigurationState> {
    if observations.unavailable {
        return Err(CamelliaNexusError::new(
            ErrorCode::InvalidState,
            "Initial configuration source observations are incomplete",
        ));
    }
    let snapshots = observations
        .snapshots
        .into_iter()
        .map(|snapshot| (snapshot.source_id.clone(), snapshot))
        .collect::<std::collections::BTreeMap<_, _>>();
    let ordered = ordered_snapshots(spec, &snapshots);
    let expected_count = spec
        .managed_config
        .as_ref()
        .map(|managed| {
            managed
                .sources
                .iter()
                .filter(|source| source.enabled())
                .count()
        })
        .unwrap_or_default();
    if ordered.len() != expected_count {
        return Err(CamelliaNexusError::new(
            ErrorCode::InvalidState,
            "Initial configuration source observations are incomplete",
        ));
    }
    let merge =
        camellia_nexus_core::merge_configuration_sources(spec.program_type.kind(), &ordered)?;
    if camellia_nexus_core::config_service::hash_bytes(merge.content.as_bytes())
        != camellia_nexus_core::config_service::hash_bytes(active_content.as_bytes())
    {
        return Err(CamelliaNexusError::new(
            ErrorCode::ConfigConflict,
            "Initial active configuration does not match its Source observations",
        ));
    }
    let mut state = ConfigurationState::from_merge(
        spec.program_type.kind(),
        1,
        now_unix_ms(),
        merge,
        compatibility_profile(spec)?,
    )?;
    state.source_snapshots = snapshots;
    state.source_statuses = observations.statuses;
    state.mark_candidate_saved()?;
    state.mark_validation(
        true,
        Vec::new(),
        Some(native_validation_evidence(spec, &state)?),
    )?;
    state.mark_applied()?;
    Ok(state)
}

fn state_from_initial_document(spec: &ProgramSpec, content: &str) -> Result<ConfigurationState> {
    let mut state = state_from_document(spec, content)?;
    state.mark_candidate_saved()?;
    state.mark_validation(
        true,
        Vec::new(),
        Some(native_validation_evidence(spec, &state)?),
    )?;
    state.mark_applied()?;
    Ok(state)
}

fn state_from_existing_document(
    spec: &ProgramSpec,
    active_content: &str,
    last_known_good_content: Option<&str>,
) -> Result<ConfigurationState> {
    let mut state = state_from_document(spec, active_content)?;
    state.applied = Some(state.desired.clone());
    state.last_known_good = last_known_good_content
        .map(|content| candidate_from_existing_content(&state, content))
        .transpose()?
        .or_else(|| state.applied.clone());
    Ok(state)
}

fn state_from_document(spec: &ProgramSpec, content: &str) -> Result<ConfigurationState> {
    let format = camellia_nexus_core::ConfigurationFormat::for_kind(spec.program_type.kind())
        .ok_or_else(|| {
            CamelliaNexusError::invalid_spec("Generic programs have no configuration state")
        })?;
    let snapshot = SourceSnapshot::parse(
        "initial",
        "Initial configuration",
        format,
        content.as_bytes(),
        now_unix_ms(),
        false,
    )?;
    let mut merge = camellia_nexus_core::merge_configuration_sources(
        spec.program_type.kind(),
        std::slice::from_ref(&snapshot),
    )?;
    // Preserve the exact active bytes. Subsequent semantic edits may
    // canonicalize formatting through the normal source/desired pipeline.
    merge.content = content.to_owned();
    merge.content_hash = camellia_nexus_core::config_service::hash_bytes(content.as_bytes());
    let mut state = ConfigurationState::from_merge(
        spec.program_type.kind(),
        1,
        now_unix_ms(),
        merge,
        compatibility_profile(spec)?,
    )?;
    state
        .source_snapshots
        .insert(snapshot.source_id.clone(), snapshot.clone());
    state.source_statuses.insert(
        snapshot.source_id.clone(),
        SourceStatus {
            source_id: snapshot.source_id.clone(),
            source_name: snapshot.source_name.clone(),
            freshness: SourceFreshness::Fresh,
            observed_hash: Some(snapshot.content_hash.clone()),
            snapshot_hash: Some(snapshot.content_hash.clone()),
            message_key: None,
            observed_unix_ms: Some(snapshot.parsed_unix_ms),
        },
    );
    Ok(state)
}

fn candidate_from_existing_content(
    state: &ConfigurationState,
    content: &str,
) -> Result<ConfigurationCandidate> {
    parse_draft_value(state.format, content)?;
    Ok(ConfigurationCandidate {
        revision: ConfigurationRevision::new(state.generation, content, now_unix_ms()),
        content: content.to_owned(),
        compatibility_profile_hash: state.compatibility_profile.profile_hash.clone(),
        validation: CandidateValidationStatus::Pending,
        validation_evidence: None,
        diagnostics: Vec::new(),
        conflicts: Vec::new(),
    })
}

fn ensure_generation(state: &ConfigurationState, expected: u64) -> Result<()> {
    if state.generation == expected {
        Ok(())
    } else {
        Err(CamelliaNexusError::new(
            ErrorCode::ConfigConflict,
            "Configuration changed since it was loaded",
        )
        .with_message_key("CONFIGURATION_GENERATION_STALE"))
    }
}

fn compatibility_profile(spec: &ProgramSpec) -> Result<CoreCompatibilityProfile> {
    let target = spec.core_target_identity().ok_or_else(|| {
        CamelliaNexusError::new(
            ErrorCode::InvalidState,
            "Configuration state requires a Core compatibility target",
        )
    })?;
    CoreCompatibilityProfile::resolve(&target)
}

fn native_validation_evidence(
    spec: &ProgramSpec,
    state: &ConfigurationState,
) -> Result<CoreValidationEvidence> {
    let fingerprint = spec
        .executable
        .metadata()
        .map(|metadata| &metadata.fingerprint)
        .ok_or_else(|| {
            CamelliaNexusError::new(
                ErrorCode::InvalidState,
                "Native validation requires an exact binary fingerprint",
            )
        })?;
    if state
        .compatibility_profile
        .target
        .fingerprint_sha256
        .as_deref()
        != Some(fingerprint.sha256.as_str())
    {
        return Err(CamelliaNexusError::new(
            ErrorCode::ConfigConflict,
            "Configuration compatibility target no longer matches the executable fingerprint",
        ));
    }
    Ok(CoreValidationEvidence {
        binary_sha256: fingerprint.sha256.clone(),
        profile_hash: state.compatibility_profile.profile_hash.clone(),
        config_hash: state.desired.revision.content_hash.clone(),
        candidate_generation: state.generation,
        validator_contract_revision: NATIVE_VALIDATOR_CONTRACT_REVISION.into(),
        native_accepted: true,
        validated_unix_ms: now_unix_ms(),
    })
}

fn blocked_source_candidate(state: &ConfigurationState, invalid: bool) -> ConfigurationCandidate {
    let mut candidate = state.desired.clone();
    candidate.validation = CandidateValidationStatus::Invalid;
    candidate.validation_evidence = None;
    candidate.diagnostics = vec![ConfigurationDiagnostic {
        location: None,
        code: if invalid {
            "SOURCE_INVALID".into()
        } else {
            "SOURCE_UNAVAILABLE".into()
        },
        message: if invalid {
            "An enabled source is invalid and has no successfully parsed snapshot".into()
        } else {
            "An enabled source is unavailable and has no successfully parsed snapshot".into()
        },
        message_key: Some(if invalid {
            "SOURCE_INVALID".into()
        } else {
            "SOURCE_UNAVAILABLE".into()
        }),
        scope: ConfigurationIssueScope::sources("source-refresh"),
        details: None,
    }];
    candidate
}

fn candidate_has_source_blocker(candidate: &ConfigurationCandidate, invalid: bool) -> bool {
    let expected_code = if invalid {
        "SOURCE_INVALID"
    } else {
        "SOURCE_UNAVAILABLE"
    };
    candidate.validation == CandidateValidationStatus::Invalid
        && candidate.validation_evidence.is_none()
        && candidate.diagnostics.len() == 1
        && candidate.diagnostics[0].code == expected_code
}

fn parse_draft_value(format: ConfigurationFormat, content: &str) -> Result<Value> {
    camellia_nexus_core::parse_semantic_document(format, content.as_bytes())
}

fn ordered_snapshots(
    spec: &ProgramSpec,
    snapshots: &std::collections::BTreeMap<String, SourceSnapshot>,
) -> Vec<SourceSnapshot> {
    spec.managed_config
        .as_ref()
        .map(|managed| {
            managed
                .sources
                .iter()
                .filter(|source| source.enabled())
                .filter_map(|source| snapshots.get(source.id()).cloned())
                .collect()
        })
        .unwrap_or_default()
}

fn final_editor_session_for_state(state: &ConfigurationState) -> FinalEditorSession {
    state
        .editor_session
        .clone()
        .unwrap_or_else(|| FinalEditorSession {
            session_id: Uuid::new_v4().to_string(),
            draft_revision: 0,
            based_on_state_revision: state.state_revision,
            based_on_candidate_generation: state.generation,
            base_content: state.desired.content.clone(),
            working_content: state.desired.content.clone(),
            conflicts: Vec::new(),
            resolutions: std::collections::BTreeMap::new(),
            unresolved_conflict_ids: Vec::new(),
            rebase_required: false,
            updated_unix_ms: now_unix_ms(),
        })
}

fn view_for_spec(spec: &ProgramSpec, state: &ConfigurationState) -> Result<ConfigurationStateView> {
    let mut view = state.view();
    let report = match spec
        .executable
        .metadata()
        .and_then(|metadata| metadata.probe.as_ref())
    {
        Some(probe) => camellia_nexus_core::assess_core_probe(spec.program_type.kind(), probe)?,
        None => camellia_nexus_core::embedded_core_knowledge()?.assess(
            spec.program_type.kind(),
            None,
            None,
        )?,
    };
    project_admission(&mut view, report);
    let order = spec
        .managed_config
        .as_ref()
        .map(|managed| {
            managed
                .sources
                .iter()
                .enumerate()
                .map(|(index, source)| (source.id(), index))
                .collect::<std::collections::HashMap<_, _>>()
        })
        .unwrap_or_default();
    view.source_statuses.sort_by_key(|status| {
        order
            .get(status.source_id.as_str())
            .copied()
            .unwrap_or(usize::MAX)
    });
    Ok(view)
}

fn project_admission(view: &mut ConfigurationStateView, report: CoreAdmissionReport) {
    use camellia_nexus_core::{
        ConfigurationGate, ConfigurationGateBlocker, ConfigurationRecoveryAction,
    };
    view.workspace.editor.blockers.retain(|blocker| {
        blocker.recovery_action != ConfigurationRecoveryAction::OpenCompatibility
    });
    if report.status != CoreAdmissionStatus::Admitted {
        let editor = &mut view.workspace.editor;
        editor.can_validate = false;
        editor.can_apply = false;
        editor.blockers.push(ConfigurationGateBlocker {
            code: report.message_key.clone(),
            details: None,
            message_key: report.message_key.clone(),
            scope: ConfigurationIssueScope::compatibility(),
            semantic_path: None,
            blocks: vec![ConfigurationGate::Validate, ConfigurationGate::Apply],
            recovery_action: ConfigurationRecoveryAction::OpenCompatibility,
        });
    }
    view.core_admission = Some(report);
}

fn reconcile_snapshots(
    spec: &ProgramSpec,
    snapshots: &mut std::collections::BTreeMap<String, SourceSnapshot>,
    refreshed: Vec<SourceSnapshot>,
) {
    let configured_source_ids = spec
        .managed_config
        .as_ref()
        .map(|managed| {
            managed
                .sources
                .iter()
                .map(|source| source.id().to_owned())
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();
    snapshots.retain(|source_id, _| configured_source_ids.contains(source_id));
    for snapshot in refreshed {
        snapshots.insert(snapshot.source_id.clone(), snapshot);
    }
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use camellia_nexus_core::{
        ConfigSourceSpec, ConfigurationFormat, CoreBinaryFingerprint, CoreTargetIdentity,
        ExecutableMetadata, ExecutableSpec, ManagedConfigSpec, ProgramId, ProgramKind, ProgramType,
        RestartPolicy,
    };

    use super::*;

    #[cfg(unix)]
    impl ConfigurationCoordinator {
        async fn refresh_with_lease(
            &self,
            manager: &ProgramManager,
            id: &ProgramId,
            local_base: Option<&std::path::Path>,
            credentials: &CredentialSnapshot,
            lease: &ConfigurationLease,
            kind: camellia_nexus_core::SourceUpdateKind,
        ) -> Result<ConfigurationStateView> {
            let prepared = self
                .prepare_source_refresh_with_lease(
                    manager,
                    id,
                    local_base,
                    credentials,
                    lease,
                    kind,
                )
                .await?;
            Ok(self
                .commit_source_refresh_with_lease(manager, prepared, lease)
                .await?
                .state)
        }
    }

    // Exercise coordinator mutations with the same outer lease used by IPC commands.
    macro_rules! workspace_mutations {
        ($( $name:ident => $implementation:ident ( $( $arg:ident : $ty:ty ),* ); )*) => {
            #[cfg(unix)]
            impl ConfigurationCoordinator {
                $(async fn $name(&self, manager: &ProgramManager, id: &ProgramId, $( $arg: $ty ),*) -> Result<ConfigurationWorkspaceSnapshot> {
                    let lease = self.acquire_lease(manager, id).await?;
                    self.$implementation(manager, id, $( $arg, )* &lease).await
                })*
            }
        };
    }

    workspace_mutations! {
        set_guided => set_guided_with_lease(setting: String, value: Option<Value>, generation: u64);
        update_final_configuration_draft => update_final_configuration_draft_with_lease(draft: FinalEditorSession, revision: u64);
        resolve_final_draft_conflict => resolve_final_draft_conflict_with_lease(conflict: String, resolution: FinalConflictResolution, revision: u64);
    }

    #[cfg(unix)]
    impl ConfigurationCoordinator {
        async fn save_configuration_candidate(
            &self,
            manager: &ProgramManager,
            id: &ProgramId,
        ) -> Result<ConfigurationWorkspaceSnapshot> {
            let lease = self.acquire_lease(manager, id).await?;
            let snapshot = self.load_workspace_with_lease(manager, id, &lease).await?;
            let mut request = activation_request(&snapshot);
            request.kind = ConfigurationOperationKind::Save;
            self.save_workspace_with_lease(manager, id, request, &lease, || async { Ok(()) })
                .await
        }

        async fn activate_configuration_candidate(
            &self,
            manager: &ProgramManager,
            id: &ProgramId,
            request: ConfigurationMutationContext,
            interactive: bool,
        ) -> Result<ConfigurationWorkspaceSnapshot> {
            let lease = self.acquire_lease(manager, id).await?;
            self.activate_workspace_with_lease(
                manager,
                id,
                request,
                interactive,
                &lease,
                || async { Ok(()) },
            )
            .await
        }
    }

    #[cfg(unix)]
    fn activation_request(
        snapshot: &ConfigurationWorkspaceSnapshot,
    ) -> ConfigurationMutationContext {
        ConfigurationMutationContext {
            operation_id: Uuid::new_v4().to_string(),
            kind: ConfigurationOperationKind::Apply,
            expected_state_revision: snapshot.state.state_revision,
            editor_session_id: snapshot
                .editor_session
                .as_ref()
                .map(|draft| draft.session_id.clone()),
            expected_draft_revision: snapshot
                .editor_session
                .as_ref()
                .map(|draft| draft.draft_revision),
        }
    }

    fn spec(sources: Vec<ConfigSourceSpec>) -> ProgramSpec {
        let fingerprint = CoreBinaryFingerprint {
            sha256: "a".repeat(64),
            size: 1,
            modified_unix_ms: 1,
        };
        ProgramSpec {
            id: ProgramId::parse("configuration-test").expect("id"),
            name: "Configuration test".into(),
            executable: ExecutableSpec::External {
                path: PathBuf::from("xray"),
                metadata: Some(ExecutableMetadata {
                    fingerprint: fingerprint.clone(),
                    probe: None,
                    core_target: Some(
                        CoreTargetIdentity::unknown(ProgramKind::Xray, None)
                            .bind_fingerprint(&fingerprint),
                    ),
                }),
            },
            program_type: ProgramType::Xray {
                extra_args: Vec::new(),
            },
            managed_config: Some(ManagedConfigSpec {
                sources,
                ..ManagedConfigSpec::default()
            }),
            working_directory: PathBuf::from("."),
            environment: BTreeMap::new(),
            auto_start: false,
            restart_policy: RestartPolicy::Never,
            privilege_policy: Default::default(),
        }
    }

    fn inline(id: &str, enabled: bool) -> ConfigSourceSpec {
        ConfigSourceSpec::Inline {
            id: id.into(),
            name: id.into(),
            enabled,
            content: format!(r#"{{"source":"{id}"}}"#),
        }
    }

    #[cfg(unix)]
    async fn workspace_fixture() -> (
        tempfile::TempDir,
        Arc<FileStore>,
        Arc<ProgramManager>,
        ConfigurationCoordinator,
        ProgramId,
    ) {
        workspace_fixture_with_driver(Arc::new(crate::NativeProcessDriver::default())).await
    }

    #[cfg(unix)]
    async fn workspace_fixture_with_driver(
        driver: Arc<dyn camellia_nexus_core::ProcessDriver>,
    ) -> (
        tempfile::TempDir,
        Arc<FileStore>,
        Arc<ProgramManager>,
        ConfigurationCoordinator,
        ProgramId,
    ) {
        use crate::NativeToolRunner;
        use camellia_nexus_core::CreateProgramRequest;
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("fixture directory");
        let binary = directory.path().join("xray");
        let baseline = camellia_nexus_core::embedded_core_knowledge()
            .unwrap()
            .program(ProgramKind::Xray)
            .unwrap()
            .releases
            .last()
            .unwrap();
        std::fs::write(
            &binary,
            r#"#!/bin/sh
case "$1" in
version) echo 'Xray FIXTURE_VERSION' ;;
help) echo 'run version -test -c -format -dump' ;;
run)
    if [ -f "${0%/*}/hold-validation" ]; then
        touch "${0%/*}/validation-entered"
        attempts=0
        while [ -f "${0%/*}/hold-validation" ]; do
            attempts=$((attempts + 1))
            [ "$attempts" -lt 500 ] || exit 1
            sleep 0.01
        done
    fi
    if [ -f "${0%/*}/reject-validation" ]; then
        cat "${0%/*}/reject-validation"
        cat "${0%/*}/reject-validation" >&2
        exit 1
    fi
    exit 0 ;;
esac
"#
            .replace("FIXTURE_VERSION", &baseline.version),
        )
        .expect("write validator fixture");
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))
            .expect("validator permissions");
        let store = Arc::new(FileStore::new(directory.path().join("data")).expect("store"));
        let manager = ProgramManager::new(
            driver,
            store.clone(),
            store.clone(),
            Arc::new(NativeToolRunner::default()),
        );
        manager
            .initialize_without_auto_start()
            .await
            .expect("initialize stopped");
        let mut program = spec(vec![ConfigSourceSpec::Inline {
            id: "source".into(),
            name: "Source".into(),
            enabled: true,
            content: r#"{"log":{"loglevel":"info"}}"#.into(),
        }]);
        program.executable = ExecutableSpec::External {
            path: binary,
            metadata: None,
        };
        program.working_directory = directory.path().to_owned();
        let id = program.id.clone();
        manager
            .create(CreateProgramRequest {
                spec: program,
                package_source: None,
                initial_config: Some(r#"{"log":{"loglevel":"info"}}"#.into()),
            })
            .await
            .expect("create stopped program");
        let coordinator = ConfigurationCoordinator::new(store.clone());
        coordinator
            .load_workspace(&manager, &id)
            .await
            .expect("workspace");
        (directory, store, manager, coordinator, id)
    }

    #[cfg(unix)]
    async fn managed_workspace_fixture() -> (
        tempfile::TempDir,
        Arc<FileStore>,
        Arc<ProgramManager>,
        ConfigurationCoordinator,
        ProgramId,
        PathBuf,
    ) {
        use camellia_nexus_core::CreateProgramRequest;
        use std::os::unix::fs::PermissionsExt;

        let (directory, store, manager, coordinator, original_id) = workspace_fixture().await;
        let source = directory.path().join("package-source");
        let replacement = directory.path().join("package-replacement");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&replacement).unwrap();
        let original_binary = std::fs::read(directory.path().join("xray")).unwrap();
        std::fs::write(source.join("xray"), &original_binary).unwrap();
        let mut next_binary = original_binary;
        next_binary.extend_from_slice(b"\n# distinct package build\n");
        std::fs::write(replacement.join("xray"), next_binary).unwrap();
        for path in [source.join("xray"), replacement.join("xray")] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut program = manager.get(&original_id).await.unwrap().0;
        program.id = ProgramId::parse("managed-workspace-test").unwrap();
        program.executable = ExecutableSpec::Managed {
            path: PathBuf::from("bin/xray"),
            metadata: None,
        };
        program.working_directory = PathBuf::from("bin");
        let id = program.id.clone();
        manager
            .create(CreateProgramRequest {
                spec: program,
                package_source: Some(source),
                initial_config: Some(r#"{"log":{"loglevel":"info"}}"#.into()),
            })
            .await
            .unwrap();
        coordinator.load_workspace(&manager, &id).await.unwrap();
        (directory, store, manager, coordinator, id, replacement)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn package_workspace_commit_rolls_back_each_boundary_and_retries_with_draft() {
        use camellia_nexus_core::ProgramStore;
        for stage in [1, 2] {
            for invalid in [false, true] {
                let (_directory, store, manager, coordinator, id, source) =
                    managed_workspace_fixture().await;
                let original = coordinator.load_workspace(&manager, &id).await.unwrap();
                let mut draft = original.editor_session.unwrap();
                draft.working_content = if invalid {
                    "{unfinished".into()
                } else {
                    r#"{"log":{"loglevel":"debug"}}"#.into()
                };
                coordinator
                    .update_final_configuration_draft(&manager, &id, draft, 0)
                    .await
                    .unwrap();
                let before = store.load_configuration_state(&id).await.unwrap().unwrap();
                let before_spec = manager.get(&id).await.unwrap().0;
                let active = manager.load_config(&id).await.unwrap();
                let root = store.workspace(&id).await.unwrap();
                let old_binary = std::fs::read(root.join("bin/xray")).unwrap();
                let lease = coordinator.acquire_lease(&manager, &id).await.unwrap();
                let prepared = manager.prepare_package(&id, source.clone()).await.unwrap();
                let update = coordinator
                    .prepare_package_workspace(
                        &manager,
                        &id,
                        prepared.next_spec(),
                        Some(before.state_revision),
                        &lease,
                    )
                    .await
                    .unwrap();
                store.fail_package_commit_at(stage);
                assert_eq!(
                    manager
                        .commit_package(prepared, update)
                        .await
                        .unwrap_err()
                        .code,
                    ErrorCode::Storage
                );
                assert_eq!(manager.get(&id).await.unwrap().0, before_spec);
                assert_eq!(std::fs::read(root.join("bin/xray")).unwrap(), old_binary);
                assert_eq!(
                    store.load_configuration_state(&id).await.unwrap().unwrap(),
                    before
                );
                assert_eq!(
                    manager.load_config(&id).await.unwrap().base_hash,
                    active.base_hash
                );
                for _ in 0..2 {
                    let report = store.load_all().await.unwrap();
                    assert!(report.invalid.is_empty(), "{:?}", report.invalid);
                    assert_eq!(
                        store.load_configuration_state(&id).await.unwrap().unwrap(),
                        before
                    );
                }

                let prepared = manager.prepare_package(&id, source.clone()).await.unwrap();
                let next_spec = prepared.next_spec().clone();
                let update = coordinator
                    .prepare_package_workspace(
                        &manager,
                        &id,
                        &next_spec,
                        Some(before.state_revision),
                        &lease,
                    )
                    .await
                    .unwrap();
                manager.commit_package(prepared, update).await.unwrap();
                let after = coordinator
                    .load_workspace_with_lease(&manager, &id, &lease)
                    .await
                    .unwrap();
                assert_eq!(manager.get(&id).await.unwrap().0, next_spec);
                assert_ne!(std::fs::read(root.join("bin/xray")).unwrap(), old_binary);
                assert_eq!(
                    after.state.applied_revision,
                    before
                        .applied
                        .as_ref()
                        .map(|candidate| candidate.revision.clone())
                );
                assert_eq!(
                    after.state.last_known_good_revision,
                    before
                        .last_known_good
                        .as_ref()
                        .map(|candidate| candidate.revision.clone())
                );
                assert_eq!(
                    manager.load_config(&id).await.unwrap().base_hash,
                    active.base_hash
                );
                assert_eq!(
                    after.state.desired.validation,
                    CandidateValidationStatus::Pending
                );
                let draft = after.editor_session.as_ref().unwrap();
                assert!(draft.working_content.contains(if invalid {
                    "unfinished"
                } else {
                    "debug"
                }));
                assert_eq!(
                    draft.rebase_required,
                    before.editor_session.as_ref().unwrap().rebase_required
                );
                assert!(
                    draft.draft_revision > before.editor_session.as_ref().unwrap().draft_revision
                );
                assert_eq!(
                    manager.get(&id).await.unwrap().1,
                    camellia_nexus_core::ProgramState::Stopped
                );
                let repeated = coordinator
                    .load_workspace_with_lease(&manager, &id, &lease)
                    .await
                    .unwrap();
                assert_eq!(repeated.state.state_revision, after.state.state_revision);
                assert_eq!(repeated.state.generation, after.state.generation);
                assert_eq!(repeated.editor_session, after.editor_session);
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn package_workspace_rejects_stale_state_and_tampered_prepared_binary() {
        use camellia_nexus_core::ProgramStore;
        let (_directory, store, manager, coordinator, id, source) =
            managed_workspace_fixture().await;
        let before = store.load_configuration_state(&id).await.unwrap().unwrap();
        let original_spec = manager.get(&id).await.unwrap().0;
        let lease = coordinator.acquire_lease(&manager, &id).await.unwrap();
        let prepared = manager.prepare_package(&id, source.clone()).await.unwrap();
        let error = coordinator
            .prepare_package_workspace(
                &manager,
                &id,
                prepared.next_spec(),
                Some(before.state_revision + 1),
                &lease,
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.message_key.as_deref(),
            Some("CONFIGURATION_STATE_STALE")
        );
        manager.discard_prepared_package(prepared).await.unwrap();
        let prepared = manager.prepare_package(&id, source.clone()).await.unwrap();
        let update = coordinator
            .prepare_package_workspace(
                &manager,
                &id,
                prepared.next_spec(),
                Some(before.state_revision),
                &lease,
            )
            .await
            .unwrap();
        let mut newer = before.clone();
        newer
            .guided_intent
            .set("logging.level", serde_json::json!("debug"));
        newer.rebuild_desired(now_unix_ms()).unwrap();
        store
            .save_configuration_state(&id, &newer, Some(before.state_revision))
            .await
            .unwrap();
        let error = manager.commit_package(prepared, update).await.unwrap_err();
        assert_eq!(
            error.message_key.as_deref(),
            Some("CONFIGURATION_STATE_STALE")
        );
        assert_eq!(
            store.load_configuration_state(&id).await.unwrap().unwrap(),
            newer
        );
        assert_eq!(manager.get(&id).await.unwrap().0, original_spec);
        let before = newer;
        let prepared = manager.prepare_package(&id, source).await.unwrap();
        let update = coordinator
            .prepare_package_workspace(
                &manager,
                &id,
                prepared.next_spec(),
                Some(before.state_revision),
                &lease,
            )
            .await
            .unwrap();
        let root = store.workspace(&id).await.unwrap();
        std::fs::write(root.join("bin.new/xray"), b"changed after admission").unwrap();
        let error = manager.commit_package(prepared, update).await.unwrap_err();
        assert_eq!(
            error.message_key.as_deref(),
            Some("CORE_BINARY_IDENTITY_MISMATCH")
        );
        assert_eq!(manager.get(&id).await.unwrap().0, original_spec);
        assert_eq!(
            store.load_configuration_state(&id).await.unwrap().unwrap(),
            before
        );
        assert!(!root.join("bin.new").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejected_replacement_does_not_stop_a_running_process_and_stop_remains_available() {
        use camellia_nexus_core::{LaunchPlan, ManagedProcess, ProcessDriver, ProcessExit};
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Process {
            stops: Arc<AtomicUsize>,
        }
        #[async_trait::async_trait]
        impl ManagedProcess for Process {
            fn pid(&self) -> u32 {
                42
            }
            async fn wait(&mut self) -> Result<ProcessExit> {
                std::future::pending().await
            }
            async fn stop(&mut self) -> Result<ProcessExit> {
                self.stops.fetch_add(1, Ordering::SeqCst);
                Ok(ProcessExit {
                    code: Some(0),
                    success: true,
                })
            }
        }
        struct Driver {
            spawns: Arc<AtomicUsize>,
            stops: Arc<AtomicUsize>,
        }
        #[async_trait::async_trait]
        impl ProcessDriver for Driver {
            async fn spawn(&self, _: LaunchPlan) -> Result<Box<dyn ManagedProcess>> {
                self.spawns.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(Process {
                    stops: self.stops.clone(),
                }))
            }
        }

        let spawns = Arc::new(AtomicUsize::new(0));
        let stops = Arc::new(AtomicUsize::new(0));
        let driver = Arc::new(Driver {
            spawns: spawns.clone(),
            stops: stops.clone(),
        });
        let (directory, store, manager, coordinator, id) =
            workspace_fixture_with_driver(driver).await;
        let candidate = coordinator.load_workspace(&manager, &id).await.unwrap();
        let applied = coordinator
            .activate_configuration_candidate(&manager, &id, activation_request(&candidate), false)
            .await
            .unwrap();
        manager.start(&id).await.unwrap();
        let running = manager.get(&id).await.unwrap().1;
        assert!(matches!(
            running,
            camellia_nexus_core::ProgramState::Running { .. }
        ));
        let binary = directory.path().join("xray");
        std::fs::write(&binary, "#!/bin/sh\ncase \"$1\" in\nversion) echo 'Xray 0.0.1' ;;\nhelp) echo 'run version -test -c -format -dump' ;;\nrun) exit 0 ;;\nesac\n").unwrap();
        let error = manager.restart(&id).await.unwrap_err();
        assert_eq!(error.message_key.as_deref(), Some("CORE_VERSION_TOO_OLD"));
        assert_eq!(manager.get(&id).await.unwrap().1, running);
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
        assert_eq!(stops.load(Ordering::SeqCst), 0);
        let state = store.load_configuration_state(&id).await.unwrap().unwrap();
        assert_eq!(
            state.applied.as_ref().map(|value| &value.revision),
            applied.state.applied_revision.as_ref()
        );
        assert_eq!(
            state.last_known_good.as_ref().map(|value| &value.revision),
            applied.state.last_known_good_revision.as_ref()
        );
        manager.stop(&id).await.unwrap();
        assert_eq!(stops.load(Ordering::SeqCst), 1);
        assert_eq!(
            manager.get(&id).await.unwrap().1,
            camellia_nexus_core::ProgramState::Stopped
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unsupported_binary_keeps_workspace_inspectable_and_never_reuses_acceptance() {
        let (directory, store, manager, coordinator, id) = workspace_fixture().await;
        let original = coordinator.load_workspace(&manager, &id).await.unwrap();
        let accepted = coordinator
            .activate_configuration_candidate(&manager, &id, activation_request(&original), false)
            .await
            .unwrap();
        assert_eq!(
            accepted.state.core_admission.as_ref().unwrap().status,
            CoreAdmissionStatus::Admitted
        );
        let (original_spec, _) = manager.get(&id).await.unwrap();
        let mut draft = accepted.editor_session.clone().unwrap();
        draft.working_content = "{unfinished".into();
        coordinator
            .update_final_configuration_draft(&manager, &id, draft, 0)
            .await
            .unwrap();
        let before = store.load_configuration_state(&id).await.unwrap().unwrap();
        let binary = directory.path().join("xray");
        let bytes = std::fs::read(&binary).unwrap();
        let invalid_files = [
            (
                "#!/bin/sh\ncase \"$1\" in\nversion) echo 'Xray 0.0.1' ;;\nhelp) echo 'run version -test -c -format -dump' ;;\nrun) exit 0 ;;\nesac\n",
                CoreAdmissionStatus::TooOld,
            ),
            (
                "#!/bin/sh\necho 'Unrelated program'\n",
                CoreAdmissionStatus::ProbeRejected,
            ),
        ];
        for (invalid_file, expected_status) in invalid_files {
            std::fs::write(&binary, invalid_file).unwrap();

            for _ in 0..2 {
                let snapshot = coordinator.load_workspace(&manager, &id).await.unwrap();
                let report = snapshot.state.core_admission.as_ref().unwrap();
                assert_eq!(report.status, expected_status);
                assert!(report.baseline.is_none());
                assert_eq!(snapshot.state.desired, before.desired);
                assert_eq!(
                    snapshot.state.workspace.editor.document.content,
                    before.desired.content
                );
                assert_eq!(snapshot.editor_session, before.editor_session);
                assert!(!snapshot.state.workspace.editor.can_apply);
                assert!(!snapshot.state.workspace.editor.can_validate);
                assert!(
                    snapshot
                        .state
                        .workspace
                        .editor
                        .blockers
                        .iter()
                        .any(|blocker| {
                            blocker.message_key == expected_status.message_key()
                        && blocker.recovery_action
                            == camellia_nexus_core::ConfigurationRecoveryAction::OpenCompatibility
                        })
                );
                let error = coordinator
                    .activate_configuration_candidate(
                        &manager,
                        &id,
                        activation_request(&snapshot),
                        false,
                    )
                    .await
                    .unwrap_err();
                assert_eq!(
                    CoreAdmissionReport::from_rejection(
                        camellia_nexus_core::ProgramKind::Xray,
                        &error
                    )
                    .unwrap()
                    .unwrap()
                    .status,
                    expected_status,
                );
                assert_eq!(
                    store.load_configuration_state(&id).await.unwrap().unwrap(),
                    before
                );
                assert_eq!(manager.get(&id).await.unwrap().0, original_spec);
                assert_eq!(
                    manager.get(&id).await.unwrap().1,
                    camellia_nexus_core::ProgramState::Stopped
                );
            }
        }

        std::fs::write(&binary, bytes).unwrap();
        let restored = coordinator.load_workspace(&manager, &id).await.unwrap();
        assert_eq!(
            restored.state.core_admission.unwrap().status,
            CoreAdmissionStatus::Admitted
        );
        assert!(
            !restored
                .state
                .workspace
                .editor
                .blockers
                .iter()
                .any(|blocker| blocker.code == "CORE_VERSION_TOO_OLD")
        );
        assert_eq!(restored.editor_session, before.editor_session);
        assert_eq!(
            restored.state.applied_revision,
            accepted.state.applied_revision
        );
        assert_eq!(
            restored.state.last_known_good_revision,
            accepted.state.last_known_good_revision
        );
        assert_eq!(restored.state.state_revision, before.state_revision);
    }

    #[cfg(unix)]
    async fn change_fixture_source(
        coordinator: &ConfigurationCoordinator,
        manager: &ProgramManager,
        id: &ProgramId,
        content: &str,
        kind: camellia_nexus_core::SourceUpdateKind,
    ) -> ConfigurationWorkspaceSnapshot {
        let lease = coordinator.acquire_lease(manager, id).await.unwrap();
        let (mut program, _) = manager.get(id).await.expect("program");
        program.managed_config.as_mut().expect("sources").sources =
            vec![ConfigSourceSpec::Inline {
                id: "source".into(),
                name: "Source".into(),
                enabled: true,
                content: content.into(),
            }];
        manager.update(program).await.expect("source specification");
        coordinator
            .refresh_with_lease(
                manager,
                id,
                None,
                &CredentialSnapshot::empty(),
                &lease,
                kind,
            )
            .await
            .expect("source transaction");
        coordinator
            .load_workspace_with_lease(manager, id, &lease)
            .await
            .expect("snapshot")
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn invalid_source_diagnostics_recover_without_exposing_content_or_changing_applied() {
        let (directory, store, manager, coordinator, id) = workspace_fixture().await;
        let before = coordinator.load_workspace(&manager, &id).await.unwrap();
        let active = manager.load_config(&id).await.unwrap();
        let source = directory.path().join("private-source-token.json");
        std::fs::write(&source, r#"{"log":{"loglevel":"info"}}"#).unwrap();
        let (mut spec, _) = manager.get(&id).await.unwrap();
        spec.managed_config.as_mut().unwrap().sources = vec![ConfigSourceSpec::Local {
            id: "source".into(),
            name: "Source".into(),
            enabled: true,
            path: source.clone(),
        }];
        manager.update(spec).await.unwrap();
        for (content, expected_key) in [
            (r#"{"log":{"loglevel":"info"}}"#, None),
            (
                r#"{"private-configuration-secret":"unfinished"#,
                Some("SOURCE_INVALID"),
            ),
            (r#"{"log":{"loglevel":"debug"}}"#, None),
        ] {
            std::fs::write(&source, content).unwrap();
            let prepared = coordinator
                .prepare_source_refresh(&manager, &id, None, &CredentialSnapshot::empty())
                .await
                .unwrap();
            let workspace = coordinator
                .commit_source_refresh(&manager, prepared)
                .await
                .unwrap();
            let status = &workspace.state.source_statuses[0];
            assert_eq!(status.message_key.as_deref(), expected_key);
            assert_eq!(
                workspace.state.applied_revision,
                before.state.applied_revision
            );
            assert_eq!(
                workspace.state.last_known_good_revision,
                before.state.last_known_good_revision
            );
            assert_eq!(
                manager.load_config(&id).await.unwrap().base_hash,
                active.base_hash
            );
            for diagnostics in [
                serde_json::to_string(status).unwrap(),
                serde_json::to_string(&workspace.state.desired.diagnostics).unwrap(),
            ] {
                assert!(!diagnostics.contains("private-"));
            }
            let reopened = ConfigurationCoordinator::new(store.clone())
                .load_workspace(&manager, &id)
                .await
                .unwrap();
            assert_eq!(
                reopened.state.source_statuses,
                workspace.state.source_statuses
            );
            if expected_key.is_some() {
                assert_eq!(status.freshness, SourceFreshness::Stale);
                assert!(!workspace.state.workspace.editor.can_apply);
                assert!(workspace.state.desired.content.contains("info"));
            } else {
                assert_eq!(status.freshness, SourceFreshness::Fresh);
                assert!(
                    !workspace
                        .state
                        .desired
                        .diagnostics
                        .iter()
                        .any(|issue| issue.code == "SOURCE_INVALID")
                );
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn prepared_source_refresh_preserves_intervening_edits_and_reenters() {
        for change in ["draft", "intent", "program"] {
            let (directory, store, manager, coordinator, id) = workspace_fixture().await;
            let source = directory.path().join("source.json");
            std::fs::write(&source, r#"{"log":{"loglevel":"info"}}"#).unwrap();
            let (mut spec, _) = manager.get(&id).await.unwrap();
            spec.managed_config.as_mut().unwrap().sources = vec![ConfigSourceSpec::Local {
                id: "source".into(),
                name: "Source".into(),
                enabled: true,
                path: source.clone(),
            }];
            manager.update(spec).await.unwrap();
            let prepared = coordinator
                .prepare_source_refresh(&manager, &id, None, &CredentialSnapshot::empty())
                .await
                .unwrap();
            let original = coordinator
                .commit_source_refresh(&manager, prepared)
                .await
                .unwrap();
            let before = store.load_configuration_state(&id).await.unwrap().unwrap();
            let active = manager.load_config(&id).await.unwrap();
            std::fs::write(
                &source,
                r#"{"log":{"loglevel":"error"},"extension":{"fresh":true}}"#,
            )
            .unwrap();
            let prepared = coordinator
                .prepare_source_refresh(&manager, &id, None, &CredentialSnapshot::empty())
                .await
                .unwrap();
            assert_eq!(
                store.load_configuration_state(&id).await.unwrap().unwrap(),
                before
            );
            match change {
                "draft" => {
                    let mut draft = original.editor_session.clone().unwrap();
                    let revision = draft.draft_revision;
                    draft.working_content = r#"{"log":{"loglevel":"warning"}}"#.into();
                    coordinator
                        .update_final_configuration_draft(&manager, &id, draft, revision)
                        .await
                        .unwrap();
                }
                "intent" => {
                    coordinator
                        .set_guided(
                            &manager,
                            &id,
                            "logging.level".into(),
                            Some(serde_json::json!("debug")),
                            original.state.generation,
                        )
                        .await
                        .unwrap();
                }
                _ => {
                    let (mut spec, _) = manager.get(&id).await.unwrap();
                    spec.name = "Changed during refresh".into();
                    manager.update(spec).await.unwrap();
                }
            }
            let edited = store.load_configuration_state(&id).await.unwrap().unwrap();
            let error = coordinator
                .commit_source_refresh(&manager, prepared)
                .await
                .unwrap_err();
            assert_eq!(
                error.message_key.as_deref(),
                Some("CONFIGURATION_STATE_STALE")
            );
            assert_eq!(
                store.load_configuration_state(&id).await.unwrap().unwrap(),
                edited
            );
            let prepared = coordinator
                .prepare_source_refresh(&manager, &id, None, &CredentialSnapshot::empty())
                .await
                .unwrap();
            let refreshed = coordinator
                .commit_source_refresh(&manager, prepared)
                .await
                .unwrap();
            let content: Value = serde_json::from_str(&refreshed.state.desired.content).unwrap();
            assert_eq!(content["extension"]["fresh"], true);
            if change == "draft" {
                let draft = refreshed.editor_session.as_ref().unwrap();
                assert!(!draft.unresolved_conflict_ids.is_empty());
                assert!(!refreshed.state.workspace.editor.can_apply);
            } else if change == "intent" {
                assert_eq!(content["log"]["loglevel"], "debug");
            }
            assert_eq!(
                refreshed.state.applied_revision,
                original.state.applied_revision
            );
            assert_eq!(
                refreshed.state.last_known_good_revision,
                original.state.last_known_good_revision
            );
            assert_eq!(
                manager.load_config(&id).await.unwrap().base_hash,
                active.base_hash
            );
            assert_eq!(
                manager.get(&id).await.unwrap().1,
                camellia_nexus_core::ProgramState::Stopped
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn source_refresh_and_queued_edit_allow_authorization_transition() {
        let (_directory, store, manager, coordinator, id) = workspace_fixture().await;
        let coordinator = Arc::new(coordinator);
        let authorization = Arc::new(crate::RuntimeAuthorizationCoordinator::new());
        let prepared = coordinator
            .prepare_source_refresh(&manager, &id, None, &CredentialSnapshot::empty())
            .await
            .unwrap();
        let lease = coordinator.acquire_lease(&manager, &id).await.unwrap();
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let edit = {
            let coordinator = coordinator.clone();
            let authorization = authorization.clone();
            let manager = manager.clone();
            let id = id.clone();
            let barrier = barrier.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                let lease = coordinator.acquire_lease(&manager, &id).await.unwrap();
                let _permit = authorization
                    .authorized_mutation_permit(|| Ok(()))
                    .await
                    .unwrap();
                let (_, state) = coordinator.load_current(&manager, &id).await.unwrap();
                coordinator
                    .set_guided_with_lease(
                        &manager,
                        &id,
                        "logging.level".into(),
                        Some(Value::from("debug")),
                        state.generation,
                        &lease,
                    )
                    .await
                    .unwrap()
            })
        };
        barrier.wait().await;
        let transition = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            authorization.transition_permit(),
        )
        .await
        .expect("queued edit does not hold authorization while waiting for configuration");
        drop(transition);
        let permit = authorization
            .authorized_mutation_permit(|| Ok(()))
            .await
            .unwrap();
        let refreshed = coordinator
            .commit_source_refresh_with_lease(&manager, prepared, &lease)
            .await
            .unwrap();
        drop(permit);
        drop(lease);
        let edited = tokio::time::timeout(std::time::Duration::from_secs(5), edit)
            .await
            .expect("queued edit resumed")
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&edited.state.desired.content).unwrap()["log"]["loglevel"],
            "debug"
        );
        assert_eq!(
            edited.state.applied_revision,
            refreshed.state.applied_revision
        );
        assert_eq!(
            edited.state.last_known_good_revision,
            refreshed.state.last_known_good_revision
        );
        assert_eq!(
            store
                .load_configuration_state(&id)
                .await
                .unwrap()
                .unwrap()
                .generation,
            edited.state.generation
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn source_preparation_can_be_abandoned_and_failed_commit_can_retry() {
        let (_directory, store, manager, coordinator, id) = workspace_fixture().await;
        let before = store.load_configuration_state(&id).await.unwrap().unwrap();
        let prepared = coordinator
            .prepare_source_refresh(&manager, &id, None, &CredentialSnapshot::empty())
            .await
            .unwrap();
        let authorization = crate::RuntimeAuthorizationCoordinator::new();
        let denied = authorization
            .authorized_mutation_permit(|| {
                Err(CamelliaNexusError::new(
                    ErrorCode::InvalidState,
                    "Fixture authorization denied",
                ))
            })
            .await;
        assert!(denied.is_err());
        drop(prepared);
        assert_eq!(
            store.load_configuration_state(&id).await.unwrap().unwrap(),
            before
        );
        let prepared = coordinator
            .prepare_source_refresh(&manager, &id, None, &CredentialSnapshot::empty())
            .await
            .unwrap();
        store.fail_next_configuration_write();
        let error = coordinator
            .commit_source_refresh(&manager, prepared)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Storage);
        assert_eq!(
            store.load_configuration_state(&id).await.unwrap().unwrap(),
            before
        );
        let prepared = coordinator
            .prepare_source_refresh(&manager, &id, None, &CredentialSnapshot::empty())
            .await
            .unwrap();
        let saved = coordinator
            .commit_source_refresh(&manager, prepared)
            .await
            .unwrap();
        assert_eq!(
            saved.state.applied_revision,
            before.applied.as_ref().map(|value| value.revision.clone())
        );
        assert_eq!(
            saved.state.workspace.editor.document.content,
            saved.state.desired.content
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn coordinator_latest_writes_and_refresh_preserve_runtime_and_unrelated_paths() {
        use camellia_nexus_core::SourceUpdateKind::{Refresh, UserEdit};
        let (_directory, store, manager, coordinator, id) = workspace_fixture().await;
        let original = coordinator.load_workspace(&manager, &id).await.unwrap();
        let active = manager.load_config(&id).await.unwrap();
        let intent = coordinator
            .set_guided(
                &manager,
                &id,
                "logging.level".into(),
                Some(Value::from("warning")),
                original.state.generation,
            )
            .await
            .unwrap();
        assert!(intent.state.desired.content.contains("warning"));
        let source = change_fixture_source(
            &coordinator,
            &manager,
            &id,
            r#"{"log":{"loglevel":"debug"},"extension":{"enabled":true}}"#,
            UserEdit,
        )
        .await;
        assert!(source.state.desired.content.contains("debug"));
        let intent = coordinator
            .set_guided(
                &manager,
                &id,
                "logging.level".into(),
                Some(Value::from("warning")),
                source.state.generation,
            )
            .await
            .unwrap();
        let revision = intent.state.state_revision;
        let repeated = coordinator
            .set_guided(
                &manager,
                &id,
                "logging.level".into(),
                Some(Value::from("warning")),
                intent.state.generation,
            )
            .await
            .unwrap();
        assert_eq!(repeated.state.state_revision, revision);
        let refreshed = change_fixture_source(
            &coordinator,
            &manager,
            &id,
            r#"{"log":{"loglevel":"error"},"extension":{"enabled":true,"fresh":true}}"#,
            Refresh,
        )
        .await;
        let document: Value = serde_json::from_str(&refreshed.state.desired.content).unwrap();
        assert_eq!(document["log"]["loglevel"], "warning");
        assert_eq!(document["extension"]["fresh"], true);
        assert_eq!(
            refreshed.state.workspace.editor.document.content,
            refreshed.state.desired.content
        );
        assert_eq!(
            refreshed.editor_session.as_ref().unwrap().working_content,
            refreshed.state.desired.content
        );
        assert_eq!(
            refreshed.state.applied_revision,
            original.state.applied_revision
        );
        assert_eq!(
            refreshed.state.last_known_good_revision,
            original.state.last_known_good_revision
        );
        assert_eq!(
            manager.load_config(&id).await.unwrap().base_hash,
            active.base_hash
        );
        assert!(matches!(
            manager.get(&id).await.unwrap().1,
            camellia_nexus_core::ProgramState::Stopped
        ));
        assert_eq!(
            store
                .load_configuration_state(&id)
                .await
                .unwrap()
                .unwrap()
                .state_revision,
            refreshed.state.state_revision
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn coordinator_first_autosave_and_continuous_conflicts_keep_original_user_value() {
        let (_directory, _store, manager, coordinator, id) = workspace_fixture().await;
        let original = coordinator.load_workspace(&manager, &id).await.unwrap();
        let mut draft = original.editor_session.unwrap();
        draft.working_content = r#"{"log":{"loglevel":"debug"},"mine":true}"#.into();
        let upstream = coordinator
            .set_guided(
                &manager,
                &id,
                "logging.level".into(),
                Some(Value::from("warning")),
                original.state.generation,
            )
            .await
            .unwrap();
        let first = coordinator
            .update_final_configuration_draft(&manager, &id, draft.clone(), 0)
            .await
            .unwrap();
        let conflict = &first.editor_session.as_ref().unwrap().conflicts[0];
        assert_eq!(
            conflict.user_value,
            camellia_nexus_core::SemanticValue::Present(Value::from("debug"))
        );
        assert_eq!(
            conflict.upstream_value,
            camellia_nexus_core::SemanticValue::Present(Value::from("warning"))
        );
        let latest = coordinator
            .set_guided(
                &manager,
                &id,
                "logging.level".into(),
                Some(Value::from("error")),
                upstream.state.generation,
            )
            .await
            .unwrap();
        let session = latest.editor_session.unwrap();
        assert_eq!(session.conflicts.len(), 1);
        assert_eq!(session.conflicts[0].conflict_id, conflict.conflict_id);
        assert_eq!(session.conflicts[0].user_value, conflict.user_value);
        assert_eq!(
            session.conflicts[0].upstream_value,
            camellia_nexus_core::SemanticValue::Present(Value::from("error"))
        );
        let value: Value = serde_json::from_str(&session.working_content).unwrap();
        assert_eq!(value["log"]["loglevel"], "error");
        assert_eq!(value["mine"], true);
        let stale = coordinator
            .update_final_configuration_draft(&manager, &id, draft, 0)
            .await
            .unwrap_err();
        assert_eq!(
            stale.message_key.as_deref(),
            Some("CONFIGURATION_DRAFT_STALE")
        );
        let resolved = coordinator
            .resolve_final_draft_conflict(
                &manager,
                &id,
                session.conflicts[0].conflict_id.clone(),
                FinalConflictResolution::KeepMine,
                session.draft_revision,
            )
            .await
            .unwrap();
        let session = resolved.editor_session.unwrap();
        assert!(session.unresolved_conflict_ids.is_empty());
        assert!(session.working_content.contains("debug"));
        let saved = coordinator
            .save_configuration_candidate(&manager, &id)
            .await
            .unwrap();
        let again = coordinator
            .save_configuration_candidate(&manager, &id)
            .await
            .unwrap();
        assert!(saved.state.state_revision < again.state.state_revision);
        assert_eq!(saved.state.generation, again.state.generation);
        assert_eq!(
            saved.state.applied_revision,
            original.state.applied_revision
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn committed_operation_basis_allows_continued_typing_without_self_conflicts() {
        for action in ["save", "apply", "reject"] {
            let (directory, store, manager, coordinator, id) = workspace_fixture().await;
            let before = coordinator.load_workspace(&manager, &id).await.unwrap();
            let active = manager.load_config(&id).await.unwrap();
            let submitted = r#"{"log":{"loglevel":"debug"}}"#;
            let mut draft = before.editor_session.clone().unwrap();
            draft.working_content = submitted.into();
            let edited = coordinator
                .update_final_configuration_draft(&manager, &id, draft, 0)
                .await
                .unwrap();
            let mut request = activation_request(&edited);
            if action == "reject" {
                std::fs::write(
                    directory.path().join("reject-validation"),
                    "invalid configuration",
                )
                .unwrap();
            }
            let result = if action == "save" {
                request.kind = ConfigurationOperationKind::Save;
                let lease = coordinator.acquire_lease(&manager, &id).await.unwrap();
                coordinator
                    .save_workspace_with_lease(&manager, &id, request.clone(), &lease, || async {
                        Ok(())
                    })
                    .await
                    .unwrap()
            } else {
                coordinator
                    .activate_configuration_candidate(&manager, &id, request.clone(), false)
                    .await
                    .unwrap()
            };
            let receipt = result.operation_result.clone().unwrap();
            assert_eq!(
                receipt.saved_candidate.as_ref(),
                Some(&result.state.desired.revision)
            );
            let mut continued = result.editor_session.clone().unwrap();
            let revision = continued.draft_revision;
            continued.base_content = submitted.into();
            continued.working_content = "{unfinished".into();
            let unfinished = coordinator
                .update_final_configuration_draft(&manager, &id, continued, revision)
                .await
                .unwrap();
            let mut continued = unfinished.editor_session.unwrap();
            assert!(!continued.rebase_required, "{action}");
            assert_eq!(continued.working_content, "{unfinished");
            assert_eq!(unfinished.state.desired, result.state.desired);
            let revision = continued.draft_revision;
            continued.working_content = r#"{"log":{"loglevel":"error"},"continued":true}"#.into();
            let continued = coordinator
                .update_final_configuration_draft(&manager, &id, continued, revision)
                .await
                .unwrap();
            let draft = continued.editor_session.as_ref().unwrap();
            assert!(draft.conflicts.is_empty(), "{action}");
            assert!(draft.working_content.contains("error"));
            assert!(draft.working_content.contains("continued"));
            assert_eq!(continued.state.desired, result.state.desired);
            assert_eq!(
                continued.state.applied_revision,
                result.state.applied_revision
            );
            assert_eq!(
                continued.state.last_known_good_revision,
                result.state.last_known_good_revision
            );
            if action != "apply" {
                assert_eq!(
                    manager.load_config(&id).await.unwrap().base_hash,
                    active.base_hash
                );
                assert_eq!(
                    continued.state.applied_revision,
                    before.state.applied_revision
                );
                assert_eq!(
                    continued.state.last_known_good_revision,
                    before.state.last_known_good_revision
                );
            }
            let restarted = ConfigurationCoordinator::new(store);
            let replay = restarted
                .load_operation(&manager, &id, request)
                .await
                .unwrap();
            assert_eq!(replay.operation_result, Some(receipt));
            assert_eq!(replay.editor_session, continued.editor_session);
            assert_eq!(replay.state, continued.state);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn save_receipt_replays_after_restart_without_consuming_later_edits() {
        let (_directory, store, manager, coordinator, id) = workspace_fixture().await;
        let before = coordinator.load_workspace(&manager, &id).await.unwrap();
        let active = manager.load_config(&id).await.unwrap();
        let mut draft = before.editor_session.clone().unwrap();
        draft.working_content = r#"{"log":{"loglevel":"debug"}}"#.into();
        let edited = coordinator
            .update_final_configuration_draft(&manager, &id, draft, 0)
            .await
            .unwrap();
        let mut request = activation_request(&edited);
        request.kind = ConfigurationOperationKind::Save;
        let saved = {
            let lease = coordinator.acquire_lease(&manager, &id).await.unwrap();
            coordinator
                .save_workspace_with_lease(&manager, &id, request.clone(), &lease, || async {
                    Ok(())
                })
                .await
                .unwrap()
        };
        assert_eq!(
            saved.operation_result.as_ref().unwrap().status,
            ConfigurationOperationStatus::Saved
        );
        assert_eq!(saved.state.applied_revision, before.state.applied_revision);
        assert_eq!(
            saved.state.last_known_good_revision,
            before.state.last_known_good_revision
        );
        assert!(store.load_final_editor_draft(&id).await.unwrap().is_none());

        let mut later_draft = saved.editor_session.clone().unwrap();
        later_draft.working_content = r#"{"log":{"loglevel":"error"}}"#.into();
        let later = coordinator
            .update_final_configuration_draft(&manager, &id, later_draft, 0)
            .await
            .unwrap();
        let restarted = ConfigurationCoordinator::new(store.clone());
        for _ in 0..2 {
            let lease = restarted.acquire_lease(&manager, &id).await.unwrap();
            let replay = restarted
                .save_workspace_with_lease(&manager, &id, request.clone(), &lease, || async {
                    Ok(())
                })
                .await
                .unwrap();
            assert_eq!(replay.operation_result, saved.operation_result);
            assert_eq!(replay.state, later.state);
            assert_eq!(replay.editor_session, later.editor_session);
        }
        let error = restarted
            .activate_configuration_candidate(&manager, &id, request.clone(), false)
            .await
            .unwrap_err();
        assert_eq!(
            error.message_key.as_deref(),
            Some("CONFIGURATION_OPERATION_MISMATCH")
        );
        request.kind = ConfigurationOperationKind::Apply;
        let error = restarted
            .activate_configuration_candidate(&manager, &id, request, false)
            .await
            .unwrap_err();
        assert_eq!(
            error.message_key.as_deref(),
            Some("CONFIGURATION_OPERATION_MISMATCH")
        );
        assert_eq!(
            manager.load_config(&id).await.unwrap().base_hash,
            active.base_hash
        );
        assert!(matches!(
            manager.get(&id).await.unwrap().1,
            camellia_nexus_core::ProgramState::Stopped
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn save_request_rejects_stale_drafts_and_commits_receipt_atomically() {
        let (_directory, store, manager, coordinator, id) = workspace_fixture().await;
        let before = coordinator.load_workspace(&manager, &id).await.unwrap();
        let active = manager.load_config(&id).await.unwrap();
        let mut request = activation_request(&before);
        request.kind = ConfigurationOperationKind::Save;
        let mut draft = before.editor_session.clone().unwrap();
        draft.working_content = r#"{"log":{"loglevel":"debug"}}"#.into();
        let edited = coordinator
            .update_final_configuration_draft(&manager, &id, draft, 0)
            .await
            .unwrap();
        let lease = coordinator.acquire_lease(&manager, &id).await.unwrap();
        let error = coordinator
            .save_workspace_with_lease(&manager, &id, request.clone(), &lease, || async { Ok(()) })
            .await
            .unwrap_err();
        assert_eq!(
            error.message_key.as_deref(),
            Some("CONFIGURATION_STATE_STALE")
        );
        request.expected_state_revision = edited.state.state_revision;
        let error = coordinator
            .save_workspace_with_lease(&manager, &id, request.clone(), &lease, || async { Ok(()) })
            .await
            .unwrap_err();
        assert_eq!(
            error.message_key.as_deref(),
            Some("CONFIGURATION_DRAFT_STALE")
        );
        request = activation_request(&edited);
        request.kind = ConfigurationOperationKind::Save;
        store.fail_next_configuration_write();
        let error = coordinator
            .save_workspace_with_lease(&manager, &id, request.clone(), &lease, || async { Ok(()) })
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Storage);
        assert_eq!(
            coordinator
                .load_workspace_with_lease(&manager, &id, &lease)
                .await
                .unwrap(),
            edited
        );
        assert!(
            store
                .load_configuration_state(&id)
                .await
                .unwrap()
                .unwrap()
                .operation_receipts
                .is_empty()
        );
        let saved = coordinator
            .save_workspace_with_lease(&manager, &id, request.clone(), &lease, || async { Ok(()) })
            .await
            .unwrap();
        assert_eq!(
            saved.operation_result.as_ref().unwrap().status,
            ConfigurationOperationStatus::Saved
        );
        let persisted = store.load_configuration_state(&id).await.unwrap().unwrap();
        assert!(persisted.editor_session.is_none());
        assert!(persisted.candidate_is_saved());
        assert_eq!(
            persisted
                .operation_receipt(&request)
                .unwrap()
                .unwrap()
                .result,
            saved.operation_result.unwrap()
        );
        assert_eq!(saved.state.applied_revision, before.state.applied_revision);
        assert_eq!(
            saved.state.last_known_good_revision,
            before.state.last_known_good_revision
        );
        assert_eq!(
            manager.load_config(&id).await.unwrap().base_hash,
            active.base_hash
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn coordinator_apply_receipt_replays_after_restart_without_applying_new_changes() {
        let (_directory, store, manager, coordinator, id) = workspace_fixture().await;
        let before = coordinator.load_workspace(&manager, &id).await.unwrap();
        let request = activation_request(&before);
        let applied = coordinator
            .activate_configuration_candidate(&manager, &id, request.clone(), false)
            .await
            .unwrap();
        assert_eq!(
            applied.operation_result.as_ref().unwrap().status,
            ConfigurationOperationStatus::Applied
        );
        let changed = coordinator
            .set_guided(
                &manager,
                &id,
                "logging.level".into(),
                Some(Value::from("error")),
                applied.state.generation,
            )
            .await
            .unwrap();
        let restarted = ConfigurationCoordinator::new(store.clone());
        let replayed = restarted
            .activate_configuration_candidate(&manager, &id, request.clone(), false)
            .await
            .unwrap();
        assert_eq!(replayed.state, changed.state);
        assert_eq!(replayed.operation_result, applied.operation_result);
        assert_eq!(
            replayed.state.applied_revision,
            applied.state.applied_revision
        );
        assert!(replayed.state.desired.content.contains("error"));
        assert!(matches!(
            manager.get(&id).await.unwrap().1,
            camellia_nexus_core::ProgramState::Stopped
        ));
        let mut mismatched = request;
        mismatched.expected_state_revision = changed.state.state_revision;
        let error = restarted
            .activate_configuration_candidate(&manager, &id, mismatched, false)
            .await
            .unwrap_err();
        assert_eq!(
            error.message_key.as_deref(),
            Some("CONFIGURATION_OPERATION_MISMATCH")
        );
        assert_eq!(
            restarted.load_workspace(&manager, &id).await.unwrap().state,
            changed.state
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn coordinator_apply_checks_draft_state_even_when_candidate_generation_is_unchanged() {
        let (_directory, store, manager, coordinator, id) = workspace_fixture().await;
        let before = coordinator.load_workspace(&manager, &id).await.unwrap();
        let request = activation_request(&before);
        let mut draft = before.editor_session.clone().unwrap();
        draft.working_content = "{".into();
        let updated = coordinator
            .update_final_configuration_draft(&manager, &id, draft, 0)
            .await
            .unwrap();
        assert_eq!(updated.state.generation, before.state.generation);
        let error = coordinator
            .activate_configuration_candidate(&manager, &id, request, false)
            .await
            .unwrap_err();
        assert_eq!(
            error.message_key.as_deref(),
            Some("CONFIGURATION_STATE_STALE")
        );
        assert!(
            store
                .load_configuration_state(&id)
                .await
                .unwrap()
                .unwrap()
                .operation_receipts
                .is_empty()
        );
        assert_eq!(
            coordinator.load_workspace(&manager, &id).await.unwrap(),
            updated
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn coordinator_interrupted_request_is_reported_without_running_it_again() {
        let (_directory, store, manager, coordinator, id) = workspace_fixture().await;
        let before = coordinator.load_workspace(&manager, &id).await.unwrap();
        let request = activation_request(&before);
        let mut state = store.load_configuration_state(&id).await.unwrap().unwrap();
        let revision = state.state_revision;
        state.begin_operation(request.clone()).unwrap();
        store
            .save_configuration_state(&id, &state, Some(revision))
            .await
            .unwrap();
        let restarted = ConfigurationCoordinator::new(store.clone());
        let recovered = restarted
            .load_operation(&manager, &id, request.clone())
            .await
            .unwrap();
        assert_eq!(
            recovered.operation_result.as_ref().unwrap().status,
            ConfigurationOperationStatus::Interrupted
        );
        assert_eq!(
            recovered.state.applied_revision,
            before.state.applied_revision
        );
        let repeated = restarted
            .activate_configuration_candidate(&manager, &id, request, false)
            .await
            .unwrap();
        assert_eq!(repeated.state, recovered.state);
        assert_eq!(repeated.operation_result, recovered.operation_result);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn activation_releases_authorization_during_native_check_and_reenters_after_denial() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let (directory, store, manager, coordinator, id) = workspace_fixture().await;
        let original = coordinator.load_workspace(&manager, &id).await.unwrap();
        let active = manager.load_config(&id).await.unwrap();
        let mut draft = original.editor_session.clone().unwrap();
        let revision = draft.draft_revision;
        draft.working_content = r#"{"log":{"loglevel":"debug"}}"#.into();
        let edited = coordinator
            .update_final_configuration_draft(&manager, &id, draft, revision)
            .await
            .unwrap();
        let authorization = crate::RuntimeAuthorizationCoordinator::new();
        let allowed = AtomicBool::new(true);
        let hold = directory.path().join("hold-validation");
        let entered = directory.path().join("validation-entered");
        std::fs::write(&hold, "").unwrap();
        let lease = coordinator.acquire_lease(&manager, &id).await.unwrap();
        let request = activation_request(&edited);
        let activation = coordinator.activate_workspace_with_lease(
            &manager,
            &id,
            request.clone(),
            false,
            &lease,
            || {
                authorization.authorized_mutation_permit(|| {
                    if allowed.load(Ordering::SeqCst) {
                        Ok(())
                    } else {
                        Err(CamelliaNexusError::new(
                            ErrorCode::LicenseExpired,
                            "Authorization expired",
                        )
                        .with_message_key("LICENSE_EXPIRED"))
                    }
                })
            },
        );
        let revoke = async {
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                while !entered.exists() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("native check started");
            let transition = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                authorization.transition_permit(),
            )
            .await;
            if transition.is_ok() {
                allowed.store(false, Ordering::SeqCst);
            }
            std::fs::remove_file(&hold).unwrap();
            assert!(
                transition.is_ok(),
                "native check must not hold the authorization gate"
            );
        };
        let (result, ()) = tokio::join!(activation, revoke);
        assert_eq!(result.unwrap_err().code, ErrorCode::LicenseExpired);
        drop(lease);
        let rejected = coordinator
            .load_operation(&manager, &id, request)
            .await
            .unwrap();
        assert_eq!(
            rejected.operation_result.unwrap().status,
            ConfigurationOperationStatus::Rejected
        );
        assert_eq!(
            rejected.state.applied_revision,
            original.state.applied_revision
        );
        assert_eq!(
            rejected.state.last_known_good_revision,
            original.state.last_known_good_revision
        );
        let retained = rejected.editor_session.as_ref().unwrap();
        let submitted = edited.editor_session.as_ref().unwrap();
        assert_eq!(retained.session_id, submitted.session_id);
        assert_eq!(
            serde_json::from_str::<Value>(&retained.working_content).unwrap(),
            serde_json::from_str::<Value>(&submitted.working_content).unwrap()
        );
        assert_eq!(
            manager.load_config(&id).await.unwrap().base_hash,
            active.base_hash
        );
        assert_ne!(
            rejected.state.desired.validation,
            CandidateValidationStatus::Valid
        );
        assert!(store.load_final_editor_draft(&id).await.unwrap().is_some());

        allowed.store(true, Ordering::SeqCst);
        let current = coordinator.load_workspace(&manager, &id).await.unwrap();
        let lease = coordinator.acquire_lease(&manager, &id).await.unwrap();
        let applied = coordinator
            .activate_workspace_with_lease(
                &manager,
                &id,
                activation_request(&current),
                false,
                &lease,
                || {
                    authorization.authorized_mutation_permit(|| {
                        assert!(allowed.load(Ordering::SeqCst));
                        Ok(())
                    })
                },
            )
            .await
            .unwrap();
        assert_eq!(
            applied.operation_result.unwrap().status,
            ConfigurationOperationStatus::Applied
        );
        assert_eq!(
            manager.get(&id).await.unwrap().1,
            camellia_nexus_core::ProgramState::Stopped
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn activation_commit_denial_discards_staging_and_preserves_candidate_for_retry() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let (_directory, _store, manager, coordinator, id) = workspace_fixture().await;
        let original = coordinator.load_workspace(&manager, &id).await.unwrap();
        let active = manager.load_config(&id).await.unwrap();
        let edited = coordinator
            .set_guided(
                &manager,
                &id,
                "logging.level".into(),
                Some(Value::from("debug")),
                original.state.generation,
            )
            .await
            .unwrap();
        let calls = AtomicUsize::new(0);
        let lease = coordinator.acquire_lease(&manager, &id).await.unwrap();
        let error = coordinator
            .activate_workspace_with_lease(
                &manager,
                &id,
                activation_request(&edited),
                false,
                &lease,
                || async {
                    if calls.fetch_add(1, Ordering::SeqCst) == 3 {
                        Err(CamelliaNexusError::new(
                            ErrorCode::LicenseExpired,
                            "Authorization expired",
                        ))
                    } else {
                        Ok(())
                    }
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::LicenseExpired);
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        drop(lease);
        let rejected = coordinator.load_workspace(&manager, &id).await.unwrap();
        assert_eq!(
            rejected.state.applied_revision,
            original.state.applied_revision
        );
        assert_eq!(
            rejected.state.last_known_good_revision,
            original.state.last_known_good_revision
        );
        assert_eq!(rejected.state.desired.content, edited.state.desired.content);
        assert_eq!(
            manager.load_config(&id).await.unwrap().base_hash,
            active.base_hash
        );
        let spec = manager.get(&id).await.unwrap().0;
        let config = manager
            .workspace(&id)
            .await
            .unwrap()
            .join(spec.program_type.main_config().unwrap());
        assert!(
            !std::fs::read_dir(config.parent().unwrap())
                .unwrap()
                .any(|entry| {
                    entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".camellia-nexus-staged-")
                })
        );
        let applied = coordinator
            .activate_configuration_candidate(&manager, &id, activation_request(&rejected), false)
            .await
            .unwrap();
        assert_eq!(
            applied.operation_result.unwrap().status,
            ConfigurationOperationStatus::Applied
        );
        assert_eq!(
            manager.get(&id).await.unwrap().1,
            camellia_nexus_core::ProgramState::Stopped
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn activation_prepared_commit_rejects_intervening_workspace_changes() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        for (change, already_applied) in [
            ("draft", false),
            ("program", false),
            ("draft", true),
            ("program", true),
        ] {
            let (_directory, store, manager, coordinator, id) = workspace_fixture().await;
            let mut original = coordinator.load_workspace(&manager, &id).await.unwrap();
            if already_applied {
                original = coordinator
                    .activate_configuration_candidate(
                        &manager,
                        &id,
                        activation_request(&original),
                        false,
                    )
                    .await
                    .unwrap();
            }
            let final_authorization = if already_applied { 2 } else { 3 };
            let active = manager.load_config(&id).await.unwrap();
            let calls = AtomicUsize::new(0);
            let lease = coordinator.acquire_lease(&manager, &id).await.unwrap();
            let error = coordinator
                .activate_workspace_with_lease(
                    &manager,
                    &id,
                    activation_request(&original),
                    false,
                    &lease,
                    || async {
                        if calls.fetch_add(1, Ordering::SeqCst) == final_authorization {
                            if change == "draft" {
                                let mut draft = coordinator
                                    .get_final_editor_session_with_lease(&manager, &id, &lease)
                                    .await?;
                                let revision = draft.draft_revision;
                                draft.working_content = r#"{"log":{"loglevel":"warning"}}"#.into();
                                store
                                    .save_final_editor_draft(&id, &draft, Some(revision))
                                    .await?;
                            } else {
                                let (mut spec, _) = manager.get(&id).await?;
                                spec.name = "Changed during native preparation".into();
                                manager.update(spec).await?;
                            }
                        }
                        Ok(())
                    },
                )
                .await
                .unwrap_err();
            assert_eq!(
                error.message_key.as_deref(),
                Some("CONFIGURATION_STATE_STALE")
            );
            assert_eq!(calls.load(Ordering::SeqCst), final_authorization + 1);
            drop(lease);
            let rejected = coordinator.load_workspace(&manager, &id).await.unwrap();
            assert_eq!(
                rejected.state.applied_revision,
                original.state.applied_revision
            );
            assert_eq!(
                rejected.state.last_known_good_revision,
                original.state.last_known_good_revision
            );
            assert_eq!(
                manager.load_config(&id).await.unwrap().base_hash,
                active.base_hash
            );
            if change == "draft" {
                assert_eq!(
                    serde_json::from_str::<Value>(
                        &rejected.editor_session.as_ref().unwrap().working_content,
                    )
                    .unwrap()["log"]["loglevel"],
                    "warning"
                );
            } else {
                assert_eq!(
                    manager.get(&id).await.unwrap().0.name,
                    "Changed during native preparation"
                );
            }
            let applied = coordinator
                .activate_configuration_candidate(
                    &manager,
                    &id,
                    activation_request(&rejected),
                    false,
                )
                .await
                .unwrap();
            assert_eq!(
                applied.operation_result.unwrap().status,
                ConfigurationOperationStatus::Applied
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn repeated_apply_rechecks_files_after_authorization_and_retries_after_repair() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        for change_binary in [false, true] {
            let (directory, store, manager, coordinator, id) = workspace_fixture().await;
            let initial = coordinator.load_workspace(&manager, &id).await.unwrap();
            let applied = coordinator
                .activate_configuration_candidate(
                    &manager,
                    &id,
                    activation_request(&initial),
                    false,
                )
                .await
                .unwrap();
            let (spec, runtime) = manager.get(&id).await.unwrap();
            let target = if change_binary {
                directory.path().join("xray")
            } else {
                manager
                    .workspace(&id)
                    .await
                    .unwrap()
                    .join(spec.program_type.main_config().unwrap())
            };
            let original = std::fs::read(&target).unwrap();
            let changed = if change_binary {
                let mut bytes = original.clone();
                bytes.extend_from_slice(b"\n# replaced during authorization\n");
                bytes
            } else {
                br#"{"log":{"loglevel":"error"}}"#.to_vec()
            };
            let calls = AtomicUsize::new(0);
            let lease = coordinator.acquire_lease(&manager, &id).await.unwrap();
            let error = coordinator
                .activate_workspace_with_lease(
                    &manager,
                    &id,
                    activation_request(&applied),
                    false,
                    &lease,
                    || async {
                        if calls.fetch_add(1, Ordering::SeqCst) == 2 {
                            std::fs::write(&target, &changed)?;
                        }
                        Ok(())
                    },
                )
                .await
                .unwrap_err();
            assert_eq!(calls.load(Ordering::SeqCst), 3);
            assert_eq!(
                error.message_key.as_deref(),
                Some(if change_binary {
                    "CORE_TARGET_CHANGED"
                } else {
                    "CORE_VALIDATION_EVIDENCE_STALE"
                })
            );
            drop(lease);
            assert_eq!(std::fs::read(&target).unwrap(), changed);
            assert_eq!(manager.get(&id).await.unwrap().1, runtime);
            let retained = store.load_configuration_state(&id).await.unwrap().unwrap();
            assert_eq!(
                retained.applied.as_ref().map(|value| &value.revision),
                applied.state.applied_revision.as_ref()
            );
            assert_eq!(
                retained
                    .last_known_good
                    .as_ref()
                    .map(|value| &value.revision),
                applied.state.last_known_good_revision.as_ref()
            );
            std::fs::write(&target, original).unwrap();
            let current = coordinator.load_workspace(&manager, &id).await.unwrap();
            let result = coordinator
                .activate_configuration_candidate(
                    &manager,
                    &id,
                    activation_request(&current),
                    false,
                )
                .await
                .unwrap();
            assert_eq!(
                result.operation_result.unwrap().status,
                ConfigurationOperationStatus::Applied
            );
            assert_eq!(manager.get(&id).await.unwrap().1, runtime);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn native_success_cannot_accept_replaced_binary_or_staged_content() {
        for prepare_apply in [false, true] {
            for tamper_binary in [false, true] {
                let (directory, _store, manager, coordinator, id) = workspace_fixture().await;
                let original = coordinator.load_workspace(&manager, &id).await.unwrap();
                let active = manager.load_config(&id).await.unwrap();
                let spec = manager.get(&id).await.unwrap().0;
                let config = manager
                    .workspace(&id)
                    .await
                    .unwrap()
                    .join(spec.program_type.main_config().unwrap());
                let workspace = config.parent().unwrap();
                let binary = directory.path().join("xray");
                let binary_bytes = std::fs::read(&binary).unwrap();
                let hold = directory.path().join("hold-validation");
                let entered = directory.path().join("validation-entered");
                std::fs::write(&hold, "").unwrap();
                let validate = async {
                    let content = r#"{"log":{"loglevel":"debug"}}"#.to_owned();
                    if prepare_apply {
                        manager
                            .prepare_config(&id, &spec, content, active.base_hash.clone())
                            .await
                            .map(|_| ())
                    } else {
                        manager
                            .validate_config(&id, content, active.base_hash.clone())
                            .await
                            .map(|_| ())
                    }
                };
                let tamper = async {
                    tokio::time::timeout(std::time::Duration::from_secs(3), async {
                        while !entered.exists() {
                            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                        }
                    })
                    .await
                    .expect("native validator reached staged file");
                    if tamper_binary {
                        let mut bytes = binary_bytes.clone();
                        bytes.extend_from_slice(b"\n# modified during validation\n");
                        std::fs::write(&binary, bytes).unwrap();
                    } else {
                        let staged = std::fs::read_dir(workspace)
                            .unwrap()
                            .map(|entry| entry.unwrap().path())
                            .find(|path| {
                                path.file_name()
                                    .unwrap()
                                    .to_string_lossy()
                                    .starts_with(".camellia-nexus-staged-")
                            })
                            .unwrap();
                        std::fs::write(staged, r#"{"log":{"loglevel":"error"}}"#).unwrap();
                    }
                    std::fs::remove_file(&hold).unwrap();
                };
                let (result, ()) = tokio::join!(validate, tamper);
                assert_eq!(
                    result.unwrap_err().message_key.as_deref(),
                    Some(if tamper_binary {
                        "CORE_TARGET_CHANGED"
                    } else {
                        "CORE_VALIDATION_EVIDENCE_STALE"
                    })
                );
                assert_eq!(
                    manager.load_config(&id).await.unwrap().base_hash,
                    active.base_hash
                );
                assert!(!std::fs::read_dir(workspace).unwrap().any(|entry| {
                    entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".camellia-nexus-staged-")
                }));
                if tamper_binary {
                    std::fs::write(&binary, &binary_bytes).unwrap();
                    manager.refresh_binary_identity(&id).await.unwrap();
                }
                let current = coordinator.load_workspace(&manager, &id).await.unwrap();
                assert_eq!(
                    current.state.applied_revision,
                    original.state.applied_revision
                );
                assert_eq!(
                    current.state.last_known_good_revision,
                    original.state.last_known_good_revision
                );
                let applied = coordinator
                    .activate_configuration_candidate(
                        &manager,
                        &id,
                        activation_request(&current),
                        false,
                    )
                    .await
                    .unwrap();
                assert_eq!(
                    applied.operation_result.unwrap().status,
                    ConfigurationOperationStatus::Applied
                );
                assert_eq!(
                    manager.get(&id).await.unwrap().1,
                    camellia_nexus_core::ProgramState::Stopped
                );
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn coordinator_native_rejection_preserves_draft_and_allows_in_place_retry() {
        let (directory, store, manager, coordinator, id) = workspace_fixture().await;
        let original = coordinator.load_workspace(&manager, &id).await.unwrap();
        let active = manager.load_config(&id).await.unwrap();
        let mut draft = original.editor_session.unwrap();
        draft.working_content = r#"{"log":{"loglevel":"debug"}}"#.into();
        coordinator
            .update_final_configuration_draft(&manager, &id, draft, 0)
            .await
            .unwrap();
        let rejection = directory.path().join("reject-validation");
        let native_output = concat!(
            "json: cannot unmarshal https://fixture-user:fixture-password@private.example/fixture-token\n",
            "-----BEGIN ",
            "PRIVATE KEY-----\nfixture-key-material\n-----END PRIVATE KEY-----\n",
            "32e48a9f-0326-4f37-92fe-ad489fe055f6\ncHJpdmF0ZS10b2tlbg==",
        );
        std::fs::write(&rejection, native_output).unwrap();
        let checked = manager
            .validate_config(
                &id,
                r#"{"log":{"loglevel":"debug"}}"#.into(),
                active.base_hash.clone(),
            )
            .await
            .unwrap();
        assert!(!checked.valid);
        assert_eq!(checked.report.message_key, "CORE_NATIVE_TYPE_REJECTED");
        assert_eq!(checked.report.exit_code, Some(1));
        assert_eq!(checked.report.stdout_bytes, native_output.len());
        assert_eq!(checked.report.stderr_bytes, native_output.len());
        let before_apply = coordinator.load_workspace(&manager, &id).await.unwrap();
        let rejected = coordinator
            .activate_configuration_candidate(
                &manager,
                &id,
                activation_request(&before_apply),
                false,
            )
            .await
            .unwrap();
        assert_eq!(
            rejected.state.desired.validation,
            CandidateValidationStatus::Invalid
        );
        assert!(store.load_final_editor_draft(&id).await.unwrap().is_some());
        assert_eq!(
            rejected.state.applied_revision,
            original.state.applied_revision
        );
        assert_eq!(
            rejected.state.last_known_good_revision,
            original.state.last_known_good_revision
        );
        assert_eq!(
            manager.load_config(&id).await.unwrap().base_hash,
            active.base_hash
        );
        let native_issue = rejected
            .state
            .desired
            .diagnostics
            .iter()
            .find(|issue| issue.message_key.as_deref() == Some("CORE_NATIVE_TYPE_REJECTED"))
            .expect("localized native rejection");
        assert_eq!(
            serde_json::from_str::<camellia_nexus_core::NativeDiagnosticReport>(
                native_issue.details.as_deref().unwrap()
            )
            .unwrap(),
            checked.report
        );
        assert!(
            rejected
                .state
                .workspace
                .editor
                .blockers
                .iter()
                .any(|blocker| {
                    Some(blocker.message_key.as_str()) == native_issue.message_key.as_deref()
                        && blocker.details == native_issue.details
                })
        );
        let reopened = ConfigurationCoordinator::new(store.clone())
            .load_workspace(&manager, &id)
            .await
            .unwrap();
        for encoded in [
            serde_json::to_string(&checked).unwrap(),
            serde_json::to_string(&rejected).unwrap(),
            serde_json::to_string(&reopened).unwrap(),
            serde_json::to_string(&store.load_configuration_state(&id).await.unwrap()).unwrap(),
        ] {
            for private_value in [
                "fixture-password",
                "private.example",
                "fixture-token",
                "fixture-key-material",
                "32e48a9f",
                "cHJpdmF0ZS10b2tlbg",
            ] {
                assert!(!encoded.contains(private_value));
            }
        }
        std::fs::remove_file(rejection).unwrap();
        let accepted = coordinator
            .activate_configuration_candidate(&manager, &id, activation_request(&rejected), false)
            .await
            .unwrap();
        assert_eq!(
            accepted.state.applied_revision,
            Some(accepted.state.desired.revision.clone())
        );
        assert!(store.load_final_editor_draft(&id).await.unwrap().is_none());
        assert!(accepted.state.desired.diagnostics.is_empty());
        assert!(
            !accepted
                .state
                .workspace
                .editor
                .blockers
                .iter()
                .any(|blocker| { blocker.message_key == "CORE_NATIVE_TYPE_REJECTED" })
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn coordinator_missing_build_feature_blocks_accepting_validator_and_keeps_recovery_reentrant()
     {
        use camellia_nexus_core::CreateProgramRequest;
        use std::os::unix::fs::PermissionsExt;

        let (directory, store, manager, coordinator, _) = workspace_fixture().await;
        let baseline = camellia_nexus_core::embedded_core_knowledge()
            .unwrap()
            .program(ProgramKind::SingBox)
            .unwrap()
            .releases
            .last()
            .unwrap();
        let binary = directory.path().join("sing-box");
        let script = r#"#!/bin/sh
case "$1" in
version) echo 'sing-box version FIXTURE_VERSION'; echo 'Tags: with_gvisor' ;;
check) if [ "$2" = "--help" ]; then echo '-c --config -D --directory'; else touch "${0%/*}/native-check-ran"; fi ;;
format) echo '-w' ;;
schema) exit 1 ;;
esac
"#
        .replace("FIXTURE_VERSION", &baseline.version);
        std::fs::write(&binary, script).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut program = spec(vec![ConfigSourceSpec::Inline {
            id: "source".into(),
            name: "Source".into(),
            enabled: true,
            content: "{}".into(),
        }]);
        program.id = ProgramId::parse("build-feature-test").unwrap();
        program.name = "Build feature fixture".into();
        program.program_type = ProgramType::SingBox {
            extra_args: Vec::new(),
        };
        program.executable = ExecutableSpec::External {
            path: binary,
            metadata: None,
        };
        program.working_directory = directory.path().to_owned();
        let id = program.id.clone();
        manager
            .create(CreateProgramRequest {
                spec: program,
                package_source: None,
                initial_config: Some("{}".into()),
            })
            .await
            .unwrap();
        let original = coordinator.load_workspace(&manager, &id).await.unwrap();
        let active = manager.load_config(&id).await.unwrap();
        std::fs::remove_file(directory.path().join("native-check-ran")).unwrap();
        let mut draft = original.editor_session.clone().unwrap();
        draft.working_content =
            r#"{"outbounds":[{"type":"tuic","tag":"private-config-label"}]}"#.into();
        coordinator
            .update_final_configuration_draft(&manager, &id, draft, 0)
            .await
            .unwrap();
        let candidate = coordinator.load_workspace(&manager, &id).await.unwrap();
        let rejected = coordinator
            .activate_configuration_candidate(&manager, &id, activation_request(&candidate), false)
            .await
            .unwrap();
        assert_eq!(
            rejected.state.desired.validation,
            CandidateValidationStatus::Invalid
        );
        assert_eq!(
            rejected.state.desired.diagnostics[0].code,
            "CORE_BUILD_CAPABILITY_UNAVAILABLE"
        );
        assert!(!directory.path().join("native-check-ran").exists());
        assert!(store.load_final_editor_draft(&id).await.unwrap().is_some());
        assert_eq!(
            rejected.state.applied_revision,
            original.state.applied_revision
        );
        assert_eq!(
            rejected.state.last_known_good_revision,
            original.state.last_known_good_revision
        );
        assert_eq!(
            manager.load_config(&id).await.unwrap().base_hash,
            active.base_hash
        );
        assert!(
            !serde_json::to_string(&rejected.state.desired.diagnostics)
                .unwrap()
                .contains("private-config-label")
        );
        let mut draft = rejected.editor_session.unwrap();
        let revision = draft.draft_revision;
        draft.working_content = r#"{"outbounds":[{"type":"direct","tag":"test"}]}"#.into();
        coordinator
            .update_final_configuration_draft(&manager, &id, draft, revision)
            .await
            .unwrap();
        let repaired = coordinator.load_workspace(&manager, &id).await.unwrap();
        let applied = coordinator
            .activate_configuration_candidate(&manager, &id, activation_request(&repaired), false)
            .await
            .unwrap();
        assert_eq!(
            applied.state.applied_revision,
            Some(applied.state.desired.revision.clone())
        );
        assert!(applied.state.desired.diagnostics.is_empty());
        assert!(directory.path().join("native-check-ran").exists());
        assert_eq!(
            manager.get(&id).await.unwrap().1,
            camellia_nexus_core::ProgramState::Stopped
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn undeclared_entry_blocks_accepting_native_validator_and_repairs_without_losing_draft() {
        let (_directory, store, manager, coordinator, id) = workspace_fixture().await;
        let original = coordinator.load_workspace(&manager, &id).await.unwrap();
        let active = manager.load_config(&id).await.unwrap();
        let mut draft = original.editor_session.clone().unwrap();
        draft.working_content = r#"{"privateExtension":"private-fixture-value"}"#.into();
        let candidate = coordinator
            .update_final_configuration_draft(&manager, &id, draft, 0)
            .await
            .unwrap();
        let rejected = coordinator
            .activate_configuration_candidate(&manager, &id, activation_request(&candidate), false)
            .await
            .unwrap();
        assert_eq!(
            rejected.state.desired.validation,
            CandidateValidationStatus::Invalid
        );
        assert_eq!(
            rejected.state.desired.diagnostics[0].code,
            "CORE_CONFIGURATION_FIELD_UNCONFIRMED"
        );
        assert!(
            !serde_json::to_string(&rejected.state.desired.diagnostics)
                .unwrap()
                .contains("private-fixture-value")
        );
        assert_eq!(
            rejected.state.applied_revision,
            original.state.applied_revision
        );
        assert_eq!(
            rejected.state.last_known_good_revision,
            original.state.last_known_good_revision
        );
        assert_eq!(
            manager.load_config(&id).await.unwrap().base_hash,
            active.base_hash
        );
        assert!(store.load_final_editor_draft(&id).await.unwrap().is_some());
        let mut draft = rejected.editor_session.unwrap();
        let revision = draft.draft_revision;
        draft.working_content = r#"{"log":{"loglevel":"warning"}}"#.into();
        let corrected = coordinator
            .update_final_configuration_draft(&manager, &id, draft, revision)
            .await
            .unwrap();
        let applied = coordinator
            .activate_configuration_candidate(&manager, &id, activation_request(&corrected), false)
            .await
            .unwrap();
        assert!(applied.state.desired.diagnostics.is_empty());
        assert_eq!(
            applied.state.applied_revision,
            Some(applied.state.desired.revision.clone())
        );
        assert!(store.load_final_editor_draft(&id).await.unwrap().is_none());
        assert_eq!(
            manager.get(&id).await.unwrap().1,
            camellia_nexus_core::ProgramState::Stopped
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn nested_field_rejections_retain_workspace_and_reenter_single_apply() {
        let (_directory, store, manager, coordinator, id) = workspace_fixture().await;
        let original = coordinator.load_workspace(&manager, &id).await.unwrap();
        let active = manager.load_config(&id).await.unwrap();
        let mut workspace = original.clone();
        for (content, path) in [
            (
                r#"{"log":{"privateExtension":"private-fixture-value"}}"#,
                vec!["log", "privateExtension"],
            ),
            (
                r#"{"inbounds":[{"protocol":"socks","settings":{"accounts":[{"privateExtension":"private-fixture-value"}]}}]}"#,
                vec![
                    "inbounds",
                    "0",
                    "settings",
                    "accounts",
                    "0",
                    "privateExtension",
                ],
            ),
        ] {
            let mut draft = workspace.editor_session.clone().unwrap();
            let revision = draft.draft_revision;
            draft.working_content = content.into();
            workspace = coordinator
                .update_final_configuration_draft(&manager, &id, draft, revision)
                .await
                .unwrap();
            workspace = coordinator
                .activate_configuration_candidate(
                    &manager,
                    &id,
                    activation_request(&workspace),
                    false,
                )
                .await
                .unwrap();
            assert_eq!(
                workspace.state.desired.validation,
                CandidateValidationStatus::Invalid
            );
            let diagnostic = workspace
                .state
                .desired
                .diagnostics
                .iter()
                .find(|issue| issue.code == "CORE_CONFIGURATION_FIELD_UNCONFIRMED")
                .unwrap();
            assert_eq!(diagnostic.location.as_ref().unwrap().document_path, path);
            assert!(
                !serde_json::to_string(diagnostic)
                    .unwrap()
                    .contains("private-fixture-value")
            );
            assert_eq!(
                workspace.state.applied_revision,
                original.state.applied_revision
            );
            assert_eq!(
                workspace.state.last_known_good_revision,
                original.state.last_known_good_revision
            );
            assert_eq!(
                manager.load_config(&id).await.unwrap().base_hash,
                active.base_hash
            );
            assert!(store.load_final_editor_draft(&id).await.unwrap().is_some());
        }
        let mut draft = workspace.editor_session.clone().unwrap();
        let revision = draft.draft_revision;
        draft.working_content = r#"{"log":{"loglevel":"warning"}}"#.into();
        workspace = coordinator
            .update_final_configuration_draft(&manager, &id, draft, revision)
            .await
            .unwrap();
        let applied = coordinator
            .activate_configuration_candidate(&manager, &id, activation_request(&workspace), false)
            .await
            .unwrap();
        assert!(applied.state.desired.diagnostics.is_empty());
        assert_eq!(
            applied.state.applied_revision,
            Some(applied.state.desired.revision.clone())
        );
        assert_eq!(
            manager.get(&id).await.unwrap().1,
            camellia_nexus_core::ProgramState::Stopped
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sing_box_flat_decoder_rejects_unproven_fields_before_native_and_reenters_apply() {
        use camellia_nexus_core::CreateProgramRequest;
        use std::os::unix::fs::PermissionsExt;

        let (directory, store, manager, coordinator, _) = workspace_fixture().await;
        let baseline = camellia_nexus_core::embedded_core_knowledge()
            .unwrap()
            .program(ProgramKind::SingBox)
            .unwrap()
            .releases
            .last()
            .unwrap();
        let binary = directory.path().join("sing-box-flat-fields");
        let script = r#"#!/bin/sh
case "$1" in
version) echo 'sing-box version FIXTURE_VERSION'; echo 'Tags: ' ;;
check) if [ "$2" = "--help" ]; then echo '-c --config -D --directory'; else printf 'check\n' >> "${0%/*}/flat-native-checks"; fi ;;
format) echo '-w' ;;
schema) exit 1 ;;
esac
"#.replace("FIXTURE_VERSION", &baseline.version);
        std::fs::write(&binary, script).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut program = spec(vec![ConfigSourceSpec::Inline {
            id: "source".into(),
            name: "Source".into(),
            enabled: true,
            content: "{}".into(),
        }]);
        program.id = ProgramId::parse("flat-fields-test").unwrap();
        program.program_type = ProgramType::SingBox {
            extra_args: Vec::new(),
        };
        program.executable = ExecutableSpec::External {
            path: binary,
            metadata: None,
        };
        program.working_directory = directory.path().to_owned();
        let id = program.id.clone();
        manager
            .create(CreateProgramRequest {
                spec: program,
                package_source: None,
                initial_config: Some("{}".into()),
            })
            .await
            .unwrap();
        let original = coordinator.load_workspace(&manager, &id).await.unwrap();
        let active = manager.load_config(&id).await.unwrap();
        let checks = directory.path().join("flat-native-checks");
        std::fs::remove_file(&checks).unwrap();
        let mut workspace = original.clone();
        for (content, path) in [
            (
                r#"{"outbounds":[{"type":"socks","TAG":"private-fixture-value"}]}"#,
                vec!["outbounds", "0", "TAG"],
            ),
            (
                r#"{"outbounds":[{"type":"vless","multiplex":{"privateExtension":true}}]}"#,
                vec!["outbounds", "0", "multiplex", "privateExtension"],
            ),
            (
                r#"{"inbounds":[{"type":"socks","UDPFragmentDefault":true}]}"#,
                vec!["inbounds", "0", "UDPFragmentDefault"],
            ),
            (
                r#"{"outbounds":[{"type":"unregistered-fixture"}]}"#,
                vec!["outbounds", "0"],
            ),
            (
                r#"{"outbounds":[{"type":"socks","udp_over_tcp":{"privateExtension":"private-fixture-value"}}]}"#,
                vec!["outbounds", "0", "udp_over_tcp", "privateExtension"],
            ),
        ] {
            let mut draft = workspace.editor_session.clone().unwrap();
            let revision = draft.draft_revision;
            draft.working_content = content.into();
            workspace = coordinator
                .update_final_configuration_draft(&manager, &id, draft, revision)
                .await
                .unwrap();
            workspace = coordinator
                .activate_configuration_candidate(
                    &manager,
                    &id,
                    activation_request(&workspace),
                    false,
                )
                .await
                .unwrap();
            assert_eq!(
                workspace.state.desired.validation,
                CandidateValidationStatus::Invalid
            );
            let diagnostic = workspace
                .state
                .desired
                .diagnostics
                .iter()
                .find(|issue| issue.code == "CORE_CONFIGURATION_FIELD_UNCONFIRMED")
                .unwrap();
            assert_eq!(diagnostic.location.as_ref().unwrap().document_path, path);
            assert!(
                !serde_json::to_string(diagnostic)
                    .unwrap()
                    .contains("private-fixture-value")
            );
            assert!(!checks.exists());
            assert_eq!(
                workspace.state.applied_revision,
                original.state.applied_revision
            );
            assert_eq!(
                workspace.state.last_known_good_revision,
                original.state.last_known_good_revision
            );
            assert_eq!(
                manager.load_config(&id).await.unwrap().base_hash,
                active.base_hash
            );
            let draft = store.load_final_editor_draft(&id).await.unwrap().unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&draft.working_content).unwrap(),
                serde_json::from_str::<serde_json::Value>(content).unwrap()
            );
            workspace = ConfigurationCoordinator::new(store.clone())
                .load_workspace(&manager, &id)
                .await
                .unwrap();
        }
        let mut draft = workspace.editor_session.clone().unwrap();
        let revision = draft.draft_revision;
        draft.working_content = r#"{"outbounds":[{"type":"socks","tag":"fixture","SERVER":"127.0.0.1","server_port":1080,"udp_over_tcp":{"enabled":true,"version":2}}]}"#.into();
        workspace = coordinator
            .update_final_configuration_draft(&manager, &id, draft, revision)
            .await
            .unwrap();
        let request = activation_request(&workspace);
        let applied = coordinator
            .activate_configuration_candidate(&manager, &id, request.clone(), false)
            .await
            .unwrap();
        assert_eq!(
            applied.state.applied_revision,
            Some(applied.state.desired.revision.clone())
        );
        let native_checks = std::fs::read_to_string(&checks).unwrap();
        assert_eq!(native_checks, "check\ncheck\n");
        let repeated = ConfigurationCoordinator::new(store.clone())
            .activate_configuration_candidate(&manager, &id, request, false)
            .await
            .unwrap();
        assert_eq!(
            repeated.state.applied_revision,
            applied.state.applied_revision
        );
        assert_eq!(std::fs::read_to_string(&checks).unwrap(), native_checks);
        assert_eq!(
            manager.get(&id).await.unwrap().1,
            camellia_nexus_core::ProgramState::Stopped
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mihomo_composed_fields_preserve_failed_candidates_and_reenter_single_apply() {
        use camellia_nexus_core::CreateProgramRequest;
        use std::os::unix::fs::PermissionsExt;

        let (directory, store, manager, coordinator, _) = workspace_fixture().await;
        let baseline = camellia_nexus_core::embedded_core_knowledge()
            .unwrap()
            .program(ProgramKind::Mihomo)
            .unwrap()
            .releases
            .last()
            .unwrap();
        let binary = directory.path().join("mihomo-fields");
        let script = r#"#!/bin/sh
case "$1" in
-v) echo 'Mihomo Meta vFIXTURE_VERSION' ;;
-h) echo '-d -f -t' ;;
-t) printf 'check\n' >> "${0%/*}/mihomo-native-check" ;;
esac
"#
        .replace("FIXTURE_VERSION", &baseline.version);
        std::fs::write(&binary, script).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut program = spec(vec![ConfigSourceSpec::Inline {
            id: "source".into(),
            name: "Source".into(),
            enabled: true,
            content: "{}".into(),
        }]);
        program.id = ProgramId::parse("composed-fields-test").unwrap();
        program.name = "Composed fields fixture".into();
        program.program_type = ProgramType::Mihomo {
            extra_args: Vec::new(),
        };
        program.executable = ExecutableSpec::External {
            path: binary,
            metadata: None,
        };
        program.working_directory = directory.path().to_owned();
        let id = program.id.clone();
        manager
            .create(CreateProgramRequest {
                spec: program,
                package_source: None,
                initial_config: Some("{}".into()),
            })
            .await
            .unwrap();
        let original = coordinator.load_workspace(&manager, &id).await.unwrap();
        let active = manager.load_config(&id).await.unwrap();
        let native_checks = directory.path().join("mihomo-native-check");
        std::fs::remove_file(&native_checks).unwrap();
        let mut workspace = original.clone();
        for (content, path) in [
            (
                "proxies:\n  - name: fixture\n    type: socks5\n    smux:\n      privateExtension: private-fixture-value\n",
                vec!["proxies", "0", "smux", "privateExtension"],
            ),
            (
                "proxies:\n  - name: fixture\n    type: socks5\n    SMUX:\n      enabled: true\n",
                vec!["proxies", "0", "SMUX"],
            ),
            (
                "proxies:\n  - name: fixture\n    type: private-fixture-protocol\n",
                vec!["proxies", "0"],
            ),
            (
                "proxies:\n  - name: fixture\n    Type: socks5\n    privateExtension: private-fixture-value\n",
                vec!["proxies", "0"],
            ),
        ] {
            let mut draft = workspace.editor_session.clone().unwrap();
            let revision = draft.draft_revision;
            draft.working_content = content.into();
            workspace = coordinator
                .update_final_configuration_draft(&manager, &id, draft, revision)
                .await
                .unwrap();
            workspace = coordinator
                .activate_configuration_candidate(
                    &manager,
                    &id,
                    activation_request(&workspace),
                    false,
                )
                .await
                .unwrap();
            assert_eq!(
                workspace.state.desired.validation,
                CandidateValidationStatus::Invalid
            );
            let diagnostic = workspace
                .state
                .desired
                .diagnostics
                .iter()
                .find(|issue| issue.code == "CORE_CONFIGURATION_FIELD_UNCONFIRMED")
                .unwrap();
            assert_eq!(diagnostic.location.as_ref().unwrap().document_path, path);
            assert!(
                !serde_json::to_string(diagnostic)
                    .unwrap()
                    .contains("private-fixture")
            );
            assert!(!native_checks.exists());
            assert_eq!(
                workspace.state.applied_revision,
                original.state.applied_revision
            );
            assert_eq!(
                workspace.state.last_known_good_revision,
                original.state.last_known_good_revision
            );
            assert_eq!(
                manager.load_config(&id).await.unwrap().base_hash,
                active.base_hash
            );
            let retained = store.load_final_editor_draft(&id).await.unwrap().unwrap();
            assert_eq!(
                serde_yaml_ng::from_str::<serde_json::Value>(&retained.working_content).unwrap(),
                serde_yaml_ng::from_str::<serde_json::Value>(content).unwrap()
            );
            workspace = ConfigurationCoordinator::new(store.clone())
                .load_workspace(&manager, &id)
                .await
                .unwrap();
        }
        let mut draft = workspace.editor_session.clone().unwrap();
        let revision = draft.draft_revision;
        draft.working_content = "proxies:\n  - name: fixture\n    type: socks5\n    server: 127.0.0.1\n    port: 1080\n    skip_cert_verify: false\n    INTERFACE_NAME: fixture-interface\n    smux:\n      enabled: false\n      max_connections: 4\n".into();
        workspace = coordinator
            .update_final_configuration_draft(&manager, &id, draft, revision)
            .await
            .unwrap();
        let request = activation_request(&workspace);
        let applied = coordinator
            .activate_configuration_candidate(&manager, &id, request.clone(), false)
            .await
            .unwrap();
        assert!(applied.state.desired.diagnostics.is_empty());
        assert_eq!(
            applied.state.applied_revision,
            Some(applied.state.desired.revision.clone())
        );
        assert!(store.load_final_editor_draft(&id).await.unwrap().is_none());
        let checks = std::fs::read(&native_checks).unwrap();
        assert!(!checks.is_empty());
        let repeated = ConfigurationCoordinator::new(store.clone())
            .activate_configuration_candidate(&manager, &id, request, false)
            .await
            .unwrap();
        assert_eq!(repeated.state, applied.state);
        assert_eq!(std::fs::read(&native_checks).unwrap(), checks);
        assert_eq!(
            manager.get(&id).await.unwrap().1,
            camellia_nexus_core::ProgramState::Stopped
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn binary_field_declaration_is_required_and_cannot_survive_a_changed_executable() {
        use camellia_nexus_core::CreateProgramRequest;
        use std::os::unix::fs::PermissionsExt;

        let (directory, store, manager, coordinator, _) = workspace_fixture().await;
        let baseline = camellia_nexus_core::embedded_core_knowledge()
            .unwrap()
            .program(ProgramKind::SingBox)
            .unwrap()
            .releases
            .last()
            .unwrap();
        let binary = directory.path().join("sing-box-fields");
        let schema_path = directory.path().join("binary-schema.json");
        std::fs::write(&schema_path, r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","additionalProperties":true}"#).unwrap();
        let script = r#"#!/bin/sh
case "$1" in
version) echo 'sing-box version FIXTURE_VERSION' ;;
check) if [ "$2" = "--help" ]; then echo '-c --config -D --directory'; else touch "${0%/*}/field-native-check"; fi ;;
format) echo '-w' ;;
schema) if [ "$2" = "--help" ]; then echo 'schema'; else cat "${0%/*}/binary-schema.json"; fi ;;
esac
"#.replace("FIXTURE_VERSION", &baseline.version);
        std::fs::write(&binary, &script).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut program = spec(vec![ConfigSourceSpec::Inline {
            id: "source".into(),
            name: "Source".into(),
            enabled: true,
            content: "{}".into(),
        }]);
        program.id = ProgramId::parse("binary-field-test").unwrap();
        program.program_type = ProgramType::SingBox {
            extra_args: Vec::new(),
        };
        program.executable = ExecutableSpec::External {
            path: binary.clone(),
            metadata: None,
        };
        program.working_directory = directory.path().to_owned();
        let id = program.id.clone();
        manager
            .create(CreateProgramRequest {
                spec: program,
                package_source: None,
                initial_config: Some("{}".into()),
            })
            .await
            .unwrap();
        let original = coordinator.load_workspace(&manager, &id).await.unwrap();
        let active = manager.load_config(&id).await.unwrap();
        std::fs::remove_file(directory.path().join("field-native-check")).unwrap();
        let mut draft = original.editor_session.clone().unwrap();
        draft.working_content = r#"{"privateExtension":{"enabled":true}}"#.into();
        let candidate = coordinator
            .update_final_configuration_draft(&manager, &id, draft, 0)
            .await
            .unwrap();
        let rejected = coordinator
            .activate_configuration_candidate(&manager, &id, activation_request(&candidate), false)
            .await
            .unwrap();
        assert_eq!(
            rejected.state.desired.diagnostics[0].code,
            "CORE_CONFIGURATION_FIELD_UNCONFIRMED"
        );
        assert!(!directory.path().join("field-native-check").exists());
        assert_eq!(
            manager.load_config(&id).await.unwrap().base_hash,
            active.base_hash
        );
        assert_eq!(
            rejected.state.applied_revision,
            original.state.applied_revision
        );
        assert_eq!(
            rejected.state.last_known_good_revision,
            original.state.last_known_good_revision
        );
        assert!(store.load_final_editor_draft(&id).await.unwrap().is_some());

        std::fs::write(&schema_path, r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"privateExtension":{"type":"object","properties":{"enabled":{"type":"boolean"}}}}}"#).unwrap();
        std::fs::write(
            &binary,
            format!("{script}\n# explicit extension declaration\n"),
        )
        .unwrap();
        let changed = coordinator.load_workspace(&manager, &id).await.unwrap();
        assert_ne!(
            changed.state.compatibility_profile.profile_hash,
            rejected.state.compatibility_profile.profile_hash
        );
        let applied = coordinator
            .activate_configuration_candidate(&manager, &id, activation_request(&changed), false)
            .await
            .unwrap();
        assert!(applied.state.desired.diagnostics.is_empty());
        assert_eq!(
            applied.state.applied_revision,
            Some(applied.state.desired.revision.clone())
        );
        assert!(directory.path().join("field-native-check").exists());

        std::fs::write(&schema_path, r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","additionalProperties":true}"#).unwrap();
        std::fs::write(
            &binary,
            format!("{script}\n# extension declaration removed\n"),
        )
        .unwrap();
        let changed = coordinator.load_workspace(&manager, &id).await.unwrap();
        assert!(changed.state.desired.validation_evidence.is_none());
        let blocked = coordinator
            .activate_configuration_candidate(&manager, &id, activation_request(&changed), false)
            .await
            .unwrap();
        assert_eq!(
            blocked.state.desired.diagnostics[0].code,
            "CORE_CONFIGURATION_FIELD_UNCONFIRMED"
        );
        assert_eq!(
            blocked.state.applied_revision,
            applied.state.applied_revision
        );
        assert_eq!(
            blocked.state.last_known_good_revision,
            applied.state.last_known_good_revision
        );
        assert_eq!(
            manager.get(&id).await.unwrap().1,
            camellia_nexus_core::ProgramState::Stopped
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn coordinator_source_rule_rejects_even_when_native_accepts_and_recovers_after_edit() {
        let (_directory, store, manager, coordinator, id) = workspace_fixture().await;
        let original = coordinator.load_workspace(&manager, &id).await.unwrap();
        let active = manager.load_config(&id).await.unwrap();
        let mut draft = original.editor_session.clone().unwrap();
        draft.working_content = r#"{"outbounds":[{"protocol":"freedom","mux":{"xudpProxyUDP443":"private-invalid-value"}}]}"#.into();
        coordinator
            .update_final_configuration_draft(&manager, &id, draft, 0)
            .await
            .unwrap();
        let candidate = coordinator.load_workspace(&manager, &id).await.unwrap();
        let rejected = coordinator
            .activate_configuration_candidate(&manager, &id, activation_request(&candidate), false)
            .await
            .unwrap();
        assert_eq!(
            rejected.state.desired.validation,
            CandidateValidationStatus::Invalid
        );
        assert!(rejected.state.desired.validation_evidence.is_none());
        assert_eq!(
            rejected.state.desired.diagnostics[0].code,
            "CONFIGURATION_VALUE_NOT_ALLOWED"
        );
        assert!(
            !serde_json::to_string(&rejected.state.desired.diagnostics)
                .unwrap()
                .contains("private-invalid-value")
        );
        assert_eq!(
            rejected.state.applied_revision,
            original.state.applied_revision
        );
        assert_eq!(
            rejected.state.last_known_good_revision,
            original.state.last_known_good_revision
        );
        assert_eq!(
            manager.load_config(&id).await.unwrap().base_hash,
            active.base_hash
        );
        assert_eq!(
            manager.get(&id).await.unwrap().1,
            camellia_nexus_core::ProgramState::Stopped
        );
        let mut draft = rejected.editor_session.unwrap();
        let revision = draft.draft_revision;
        draft.working_content = draft
            .working_content
            .replace("private-invalid-value", "allow");
        coordinator
            .update_final_configuration_draft(&manager, &id, draft, revision)
            .await
            .unwrap();
        let corrected = coordinator.load_workspace(&manager, &id).await.unwrap();
        let applied = coordinator
            .activate_configuration_candidate(&manager, &id, activation_request(&corrected), false)
            .await
            .unwrap();
        assert_eq!(
            applied.state.applied_revision,
            Some(applied.state.desired.revision.clone())
        );
        assert!(applied.state.desired.diagnostics.is_empty());
        assert!(store.load_final_editor_draft(&id).await.unwrap().is_none());
        assert_eq!(
            manager.get(&id).await.unwrap().1,
            camellia_nexus_core::ProgramState::Stopped
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn coordinator_apply_distinguishes_failed_commit_from_committed_result_recovery() {
        for committed in [false, true] {
            let (_directory, store, manager, coordinator, id) = workspace_fixture().await;
            let original = coordinator.load_workspace(&manager, &id).await.unwrap();
            let active = manager.load_config(&id).await.unwrap();
            let candidate = coordinator
                .set_guided(
                    &manager,
                    &id,
                    "logging.level".into(),
                    Some(Value::from("error")),
                    original.state.generation,
                )
                .await
                .unwrap();
            store.fail_apply_commit(committed);
            let request = activation_request(&candidate);
            let error = coordinator
                .activate_configuration_candidate(&manager, &id, request.clone(), false)
                .await
                .unwrap_err();
            assert_eq!(error.code, ErrorCode::Storage);
            let after = coordinator.load_workspace(&manager, &id).await.unwrap();
            assert!(matches!(
                manager.get(&id).await.unwrap().1,
                camellia_nexus_core::ProgramState::Stopped
            ));
            if committed {
                assert_eq!(
                    error.message_key.as_deref(),
                    Some("CONFIGURATION_COMMIT_RECOVERY_REQUIRED")
                );
                assert_eq!(
                    after.state.applied_revision,
                    Some(after.state.desired.revision.clone())
                );
                assert_eq!(
                    after.state.last_known_good_revision,
                    after.state.applied_revision
                );
                let again = coordinator
                    .activate_configuration_candidate(&manager, &id, request, false)
                    .await
                    .unwrap();
                assert_eq!(again.state.state_revision, after.state.state_revision);
                assert_eq!(again.state.generation, after.state.generation);
            } else {
                assert_eq!(
                    manager.load_config(&id).await.unwrap().base_hash,
                    active.base_hash
                );
                assert_eq!(
                    after.state.applied_revision,
                    original.state.applied_revision
                );
                assert_eq!(
                    after.state.last_known_good_revision,
                    original.state.last_known_good_revision
                );
                let retry = coordinator
                    .activate_configuration_candidate(
                        &manager,
                        &id,
                        activation_request(&after),
                        false,
                    )
                    .await
                    .unwrap();
                assert_eq!(
                    retry.state.applied_revision,
                    Some(retry.state.desired.revision.clone())
                );
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn coordinator_workspace_commit_failure_preserves_candidate_and_draft_together() {
        let (_directory, store, manager, coordinator, id) = workspace_fixture().await;
        let original = coordinator.load_workspace(&manager, &id).await.unwrap();
        let mut draft = original.editor_session.unwrap();
        draft.working_content = r#"{"log":{"loglevel":"debug"}}"#.into();
        coordinator
            .update_final_configuration_draft(&manager, &id, draft, 0)
            .await
            .unwrap();
        let before = store.load_configuration_state(&id).await.unwrap().unwrap();
        store.fail_next_configuration_write();
        let error = coordinator
            .save_configuration_candidate(&manager, &id)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Storage);
        let after = store.load_configuration_state(&id).await.unwrap().unwrap();
        assert_eq!(before, after);
        assert!(after.editor_session.is_some());
        let saved = coordinator
            .save_configuration_candidate(&manager, &id)
            .await
            .unwrap();
        let after = store.load_configuration_state(&id).await.unwrap().unwrap();
        assert!(after.editor_session.is_none());
        assert!(saved.state.desired.content.contains("debug"));
        assert_eq!(before.applied, after.applied);
        assert_eq!(before.last_known_good, after.last_known_good);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn coordinator_source_rollback_restores_the_entire_editor_workspace() {
        let (_directory, store, manager, coordinator, id) = workspace_fixture().await;
        let original = coordinator.load_workspace(&manager, &id).await.unwrap();
        let mut draft = original.editor_session.unwrap();
        draft.working_content = r#"{"log":{"loglevel":"debug"}}"#.into();
        coordinator
            .update_final_configuration_draft(&manager, &id, draft, 0)
            .await
            .unwrap();
        let before = store.load_configuration_state(&id).await.unwrap().unwrap();
        let lease = coordinator.acquire_lease(&manager, &id).await.unwrap();
        let (previous_spec, _) = manager.get(&id).await.unwrap();
        let mut next_spec = previous_spec.clone();
        next_spec.managed_config.as_mut().unwrap().sources = vec![ConfigSourceSpec::Inline {
            id: "source".into(),
            name: "Source".into(),
            enabled: true,
            content: r#"{"log":{"loglevel":"error"}}"#.into(),
        }];
        coordinator
            .begin_workspace_update(&id, &previous_spec, &next_spec, before.state_revision)
            .await
            .unwrap();
        manager.update(next_spec).await.unwrap();
        store.fail_next_configuration_write();
        let error = coordinator
            .refresh_with_lease(
                &manager,
                &id,
                None,
                &CredentialSnapshot::empty(),
                &lease,
                camellia_nexus_core::SourceUpdateKind::UserEdit,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Storage);
        manager.update(previous_spec.clone()).await.unwrap();
        coordinator.rollback_workspace_update(&id).await.unwrap();
        assert_eq!(
            store.load_configuration_state(&id).await.unwrap().unwrap(),
            before
        );
        assert_eq!(manager.get(&id).await.unwrap().0, previous_spec);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn coordinator_details_commit_failure_restores_spec_draft_and_candidate_then_retries() {
        let (_directory, store, manager, coordinator, id) = workspace_fixture().await;
        let original = coordinator.load_workspace(&manager, &id).await.unwrap();
        let mut draft = original.editor_session.unwrap();
        draft.working_content = r#"{"log":{"loglevel":"debug"}}"#.into();
        coordinator
            .update_final_configuration_draft(&manager, &id, draft, 0)
            .await
            .unwrap();
        let before = store.load_configuration_state(&id).await.unwrap().unwrap();
        assert_ne!(before.state_revision, before.generation);
        let (previous_spec, _) = manager.get(&id).await.unwrap();
        let active = manager.load_config(&id).await.unwrap();
        let mut next_spec = previous_spec.clone();
        next_spec.managed_config.as_mut().unwrap().xray_dashboard =
            Some(camellia_nexus_core::XrayDashboardSpec {
                api_port: 10085,
                metrics_port: 11111,
            });

        for fail in [true, false] {
            let prepared = coordinator
                .prepare_managed_integration_update(
                    &manager,
                    &id,
                    &next_spec,
                    Some(before.generation),
                    &[],
                )
                .await
                .unwrap();
            coordinator
                .begin_managed_integration_update(&id, &previous_spec, &next_spec, &prepared)
                .await
                .unwrap();
            let update = manager.prepare_update(next_spec.clone()).await.unwrap();
            manager.commit_update(update, false).await.unwrap();
            if fail {
                store.fail_next_configuration_write();
            }
            let result = coordinator
                .commit_managed_integration_update(&manager, &id, &next_spec, &prepared)
                .await;
            if fail {
                assert_eq!(result.unwrap_err().code, ErrorCode::Storage);
                manager.update(previous_spec.clone()).await.unwrap();
                coordinator.rollback_workspace_update(&id).await.unwrap();
                assert_eq!(
                    store.load_configuration_state(&id).await.unwrap().unwrap(),
                    before
                );
                assert_eq!(manager.get(&id).await.unwrap().0, previous_spec);
            } else {
                let committed = result.unwrap();
                coordinator
                    .mark_workspace_update_committed(&manager, &id)
                    .await
                    .unwrap();
                drop(prepared);
                // A completed write with no response is read back without repeating it.
                let recovered = coordinator.load_workspace(&manager, &id).await.unwrap();
                assert_eq!(recovered.state.state_revision, committed.state_revision);
                assert!(recovered.state.desired.content.contains("10085"));
                assert!(
                    recovered
                        .editor_session
                        .unwrap()
                        .working_content
                        .contains("debug")
                );
                assert_eq!(
                    recovered.state.applied_revision,
                    original.state.applied_revision
                );
                assert_eq!(
                    recovered.state.last_known_good_revision,
                    original.state.last_known_good_revision
                );
            }
        }
        assert_eq!(
            manager.load_config(&id).await.unwrap().base_hash,
            active.base_hash
        );
        assert!(matches!(
            manager.get(&id).await.unwrap().1,
            camellia_nexus_core::ProgramState::Stopped
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn coordinator_binary_replacement_failure_restores_workspace_and_retries_without_applying()
     {
        let (_directory, store, manager, coordinator, id) = workspace_fixture().await;
        let original = coordinator.load_workspace(&manager, &id).await.unwrap();
        let mut draft = original.editor_session.unwrap();
        draft.working_content = "{unfinished".into();
        coordinator
            .update_final_configuration_draft(&manager, &id, draft, 0)
            .await
            .unwrap();
        let before = store.load_configuration_state(&id).await.unwrap().unwrap();
        let (previous_spec, _) = manager.get(&id).await.unwrap();
        let mut next_spec = previous_spec.clone();
        let replacement = _directory.path().join("replacement-xray");
        let mut bytes = std::fs::read(previous_spec.executable.path()).unwrap();
        bytes.extend_from_slice(b"\n# fixture binary identity\n");
        std::fs::write(&replacement, bytes).unwrap();
        std::fs::set_permissions(
            &replacement,
            std::fs::metadata(previous_spec.executable.path())
                .unwrap()
                .permissions(),
        )
        .unwrap();
        next_spec.executable = ExecutableSpec::External {
            path: replacement,
            metadata: None,
        };

        for fail in [true, false] {
            let lease = coordinator.acquire_lease(&manager, &id).await.unwrap();
            coordinator
                .begin_workspace_update(&id, &previous_spec, &next_spec, before.state_revision)
                .await
                .unwrap();
            manager.update(next_spec.clone()).await.unwrap();
            if fail {
                store.fail_next_configuration_write();
            }
            let result = coordinator
                .load_workspace_with_lease(&manager, &id, &lease)
                .await;
            if fail {
                assert_eq!(result.unwrap_err().code, ErrorCode::Storage);
                manager.update(previous_spec.clone()).await.unwrap();
                coordinator.rollback_workspace_update(&id).await.unwrap();
                assert_eq!(
                    store.load_configuration_state(&id).await.unwrap().unwrap(),
                    before
                );
            } else {
                let snapshot = result.unwrap();
                coordinator
                    .mark_workspace_update_committed(&manager, &id)
                    .await
                    .unwrap();
                drop(lease);
                let recovered = coordinator.load_workspace(&manager, &id).await.unwrap();
                assert_eq!(
                    recovered.state.state_revision,
                    snapshot.state.state_revision
                );
                assert_eq!(recovered.state.desired.content, before.desired.content);
                assert_eq!(
                    recovered.editor_session.unwrap().working_content,
                    "{unfinished"
                );
                assert_eq!(
                    recovered.state.applied_revision,
                    original.state.applied_revision
                );
                assert_eq!(
                    recovered.state.last_known_good_revision,
                    original.state.last_known_good_revision
                );
            }
        }
        assert!(matches!(
            manager.get(&id).await.unwrap().1,
            camellia_nexus_core::ProgramState::Stopped
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn coordinator_source_credentials_follow_the_workspace_commit_phase() {
        use crate::config_credentials::ConfigCredentialVault;
        use camellia_nexus_core::ConfigSourceAuthentication;
        use camellia_nexus_licensing::SessionSecureStore;

        for committed in [false, true] {
            let (_directory, store, manager, coordinator, id) = workspace_fixture().await;
            let vault = ConfigCredentialVault::with_store(Arc::new(SessionSecureStore::default()));
            let (mut previous_spec, _) = manager.get(&id).await.unwrap();
            previous_spec
                .managed_config
                .as_mut()
                .unwrap()
                .sources
                .push(ConfigSourceSpec::Remote {
                    id: "private-source".into(),
                    name: "Private source".into(),
                    enabled: false,
                    url: "https://example.test/config.json".into(),
                    authentication: Some(ConfigSourceAuthentication::Basic {
                        username: "test-user".into(),
                        credential_id: None,
                        password: Some("first".into()),
                    }),
                });
            vault
                .reconcile(&mut previous_spec)
                .await
                .unwrap()
                .commit()
                .unwrap();
            manager.update(previous_spec.clone()).await.unwrap();
            let mut next_spec = previous_spec.clone();
            if let ConfigSourceSpec::Remote {
                authentication: Some(ConfigSourceAuthentication::Basic { password, .. }),
                ..
            } = &mut next_spec.managed_config.as_mut().unwrap().sources[1]
            {
                *password = Some("second".into());
            }
            let transaction = vault.reconcile(&mut next_spec).await.unwrap();
            let lease = coordinator.acquire_lease(&manager, &id).await.unwrap();
            let before = store.load_configuration_state(&id).await.unwrap().unwrap();
            coordinator
                .begin_workspace_update(&id, &previous_spec, &next_spec, before.state_revision)
                .await
                .unwrap();
            manager.update(next_spec.clone()).await.unwrap();
            coordinator
                .refresh_with_lease(
                    &manager,
                    &id,
                    None,
                    transaction.snapshot(),
                    &lease,
                    camellia_nexus_core::SourceUpdateKind::UserEdit,
                )
                .await
                .unwrap();
            if committed {
                coordinator
                    .mark_workspace_update_committed(&manager, &id)
                    .await
                    .unwrap();
            }
            transaction.retain_for_recovery();
            if !committed {
                assert_eq!(
                    vault
                        .recover(&manager, &coordinator)
                        .await
                        .unwrap_err()
                        .message_key
                        .as_deref(),
                    Some("CONFIGURATION_RECOVERY_REQUIRED")
                );
                manager.update(previous_spec.clone()).await.unwrap();
                coordinator.rollback_workspace_update(&id).await.unwrap();
                assert_eq!(
                    store.load_configuration_state(&id).await.unwrap().unwrap(),
                    before
                );
            }
            assert!(vault.recover(&manager, &coordinator).await.unwrap());
            assert!(!vault.recover(&manager, &coordinator).await.unwrap());
            let (current, program_state) = manager.get(&id).await.unwrap();
            let ConfigSourceSpec::Remote {
                authentication:
                    Some(ConfigSourceAuthentication::Basic {
                        credential_id,
                        username,
                        ..
                    }),
                ..
            } = &current.managed_config.as_ref().unwrap().sources[1]
            else {
                panic!("credential source")
            };
            assert_eq!(
                vault
                    .snapshot()
                    .await
                    .unwrap()
                    .basic_password(credential_id.as_deref(), username)
                    .unwrap()
                    .as_str(),
                if committed { "second" } else { "first" }
            );
            assert!(matches!(
                program_state,
                camellia_nexus_core::ProgramState::Stopped
            ));
            let after = store.load_configuration_state(&id).await.unwrap().unwrap();
            assert_eq!(before.applied, after.applied);
            assert_eq!(before.last_known_good, after.last_known_good);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn coordinator_invalid_draft_is_retained_separately_from_latest_candidate() {
        let (_directory, store, manager, coordinator, id) = workspace_fixture().await;
        let original = coordinator.load_workspace(&manager, &id).await.unwrap();
        let mut draft = original.editor_session.unwrap();
        draft.working_content = "{unfinished".into();
        coordinator
            .update_final_configuration_draft(&manager, &id, draft, 0)
            .await
            .unwrap();
        let latest = coordinator
            .set_guided(
                &manager,
                &id,
                "logging.level".into(),
                Some(Value::from("error")),
                original.state.generation,
            )
            .await
            .unwrap();
        assert!(
            latest
                .state
                .workspace
                .editor
                .document
                .content
                .contains("error")
        );
        let session = latest.editor_session.unwrap();
        assert!(session.rebase_required);
        assert_eq!(session.working_content, "{unfinished");
        assert!(
            coordinator
                .save_configuration_candidate(&manager, &id)
                .await
                .is_err()
        );
        let restored = store.load_final_editor_draft(&id).await.unwrap().unwrap();
        assert_eq!(restored, session);
        assert_eq!(
            latest.state.applied_revision,
            original.state.applied_revision
        );
        assert_eq!(
            latest.state.last_known_good_revision,
            original.state.last_known_good_revision
        );
    }

    fn snapshot(id: &str) -> SourceSnapshot {
        SourceSnapshot::parse(
            id,
            id,
            ConfigurationFormat::Jsonc,
            format!(r#"{{"source":"{id}"}}"#).as_bytes(),
            1,
            false,
        )
        .expect("snapshot")
    }

    fn snapshot_field(id: &str, field: &str) -> SourceSnapshot {
        SourceSnapshot::parse(
            id,
            id,
            ConfigurationFormat::Jsonc,
            format!(r#"{{"{field}":true}}"#).as_bytes(),
            1,
            false,
        )
        .expect("snapshot")
    }

    #[test]
    fn source_order_is_the_program_order_not_the_snapshot_map_order() {
        let spec = spec(vec![inline("z-source", true), inline("a-source", true)]);
        let snapshots = BTreeMap::from([
            ("a-source".into(), snapshot("a-source")),
            ("z-source".into(), snapshot("z-source")),
        ]);

        let ordered = ordered_snapshots(&spec, &snapshots);

        assert_eq!(
            ordered
                .iter()
                .map(|snapshot| snapshot.source_id.as_str())
                .collect::<Vec<_>>(),
            ["z-source", "a-source"]
        );
    }

    #[test]
    fn disabled_source_snapshot_is_retained_but_excluded_from_merge() {
        let spec = spec(vec![inline("enabled", true), inline("disabled", false)]);
        let mut snapshots = BTreeMap::from([
            ("disabled".into(), snapshot("disabled")),
            ("removed".into(), snapshot("removed")),
        ]);

        reconcile_snapshots(&spec, &mut snapshots, vec![snapshot("enabled")]);

        assert!(snapshots.contains_key("disabled"));
        assert!(!snapshots.contains_key("removed"));
        assert_eq!(
            ordered_snapshots(&spec, &snapshots)
                .iter()
                .map(|snapshot| snapshot.source_id.as_str())
                .collect::<Vec<_>>(),
            ["enabled"]
        );
    }

    #[test]
    fn created_managed_state_uses_real_source_observations() {
        let spec = spec(vec![
            inline("base", true),
            inline("override", true),
            inline("disabled", false),
        ]);
        let base = snapshot_field("base", "base");
        let override_snapshot = snapshot_field("override", "override");
        let merge = camellia_nexus_core::merge_configuration_sources(
            ProgramKind::Xray,
            &[base.clone(), override_snapshot.clone()],
        )
        .expect("merge");
        let statuses = [
            (&base, SourceFreshness::Fresh),
            (&override_snapshot, SourceFreshness::Fresh),
        ]
        .into_iter()
        .map(|(snapshot, freshness)| {
            (
                snapshot.source_id.clone(),
                SourceStatus {
                    source_id: snapshot.source_id.clone(),
                    source_name: snapshot.source_name.clone(),
                    freshness,
                    observed_hash: Some(snapshot.content_hash.clone()),
                    snapshot_hash: Some(snapshot.content_hash.clone()),
                    message_key: None,
                    observed_unix_ms: Some(snapshot.parsed_unix_ms),
                },
            )
        })
        .chain(std::iter::once((
            "disabled".into(),
            SourceStatus {
                source_id: "disabled".into(),
                source_name: "disabled".into(),
                freshness: SourceFreshness::Disabled,
                observed_hash: None,
                snapshot_hash: None,
                message_key: None,
                observed_unix_ms: None,
            },
        )))
        .collect();

        let state = state_from_source_observations(
            &spec,
            &merge.content,
            SourceRefreshResult {
                snapshots: vec![base, override_snapshot],
                statuses,
                unavailable: false,
                raw_observations: Default::default(),
            },
        )
        .expect("state");

        assert_eq!(
            state
                .source_snapshots
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["base", "override"]
        );
        assert!(!state.source_snapshots.contains_key("initial"));
        assert_eq!(state.source_statuses.len(), 3);
        assert_eq!(state.desired.content, merge.content);
        assert_eq!(state.applied.as_ref(), Some(&state.desired));
        assert_eq!(state.last_known_good.as_ref(), Some(&state.desired));
    }

    #[test]
    fn recreated_workspace_preserves_active_and_lkg_without_validation_evidence() {
        let spec = spec(Vec::new());
        let active = r#"{"log":{"loglevel":"info"}}"#;
        let last_known_good = r#"{"log":{"loglevel":"warning"}}"#;

        let state = state_from_existing_document(&spec, active, Some(last_known_good))
            .expect("recreated state");

        assert_eq!(state.desired.content, active);
        assert_eq!(state.desired.validation, CandidateValidationStatus::Pending);
        assert!(state.desired.validation_evidence.is_none());
        assert!(!state.candidate_is_saved());
        assert_eq!(
            state
                .applied
                .as_ref()
                .map(|candidate| candidate.content.as_str()),
            Some(active)
        );
        assert_eq!(
            state
                .last_known_good
                .as_ref()
                .map(|candidate| candidate.content.as_str()),
            Some(last_known_good)
        );
        assert!(state.ensure_apply_ready().is_err());
    }

    #[test]
    fn recreated_workspace_uses_active_as_lkg_when_no_lkg_file_exists() {
        let spec = spec(Vec::new());
        let active = r#"{"log":{"loglevel":"info"}}"#;

        let state = state_from_existing_document(&spec, active, None).expect("recreated state");

        assert_eq!(state.applied, state.last_known_good);
        assert_eq!(
            state
                .applied
                .as_ref()
                .map(|candidate| candidate.content.as_str()),
            Some(active)
        );
        assert!(state.desired.validation_evidence.is_none());
    }

    #[test]
    fn blocked_source_candidate_keeps_content_revision_and_updates_reason() {
        let merge = camellia_nexus_core::merge_configuration_sources(
            ProgramKind::Xray,
            &[snapshot("initial")],
        )
        .expect("merge");
        let spec = spec(Vec::new());
        let mut state = ConfigurationState::from_merge(
            ProgramKind::Xray,
            1,
            1,
            merge,
            compatibility_profile(&spec).expect("profile"),
        )
        .expect("configuration state");
        state.mark_candidate_saved().expect("save candidate");
        state
            .mark_validation(
                true,
                Vec::new(),
                Some(native_validation_evidence(&spec, &state).expect("evidence")),
            )
            .expect("validation");
        state.mark_applied().expect("applied");

        let candidate = blocked_source_candidate(&state, false);

        assert_eq!(candidate.revision.generation, state.generation);
        assert_eq!(candidate.revision, state.desired.revision);
        assert_eq!(
            candidate.revision.content_hash,
            state.desired.revision.content_hash
        );
        assert_eq!(candidate.validation, CandidateValidationStatus::Invalid);
        assert_eq!(candidate.diagnostics[0].code, "SOURCE_UNAVAILABLE");

        let invalid = blocked_source_candidate(&state, true);
        assert_eq!(invalid.validation, CandidateValidationStatus::Invalid);
        assert_eq!(invalid.diagnostics[0].code, "SOURCE_INVALID");
        assert!(candidate_has_source_blocker(&candidate, false));
        assert!(!candidate_has_source_blocker(&candidate, true));
        assert!(candidate_has_source_blocker(&invalid, true));
        assert!(state.applied.is_some());
        assert!(state.last_known_good.is_some());
    }
}
