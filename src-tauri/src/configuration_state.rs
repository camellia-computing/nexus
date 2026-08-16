use std::{collections::HashMap, sync::Arc};

use camellia_nexus_core::{
    CamelliaNexusError, CandidateValidationStatus, ConfigurationCandidate, ConfigurationConflict,
    ConfigurationDiagnostic, ConfigurationFormat, ConfigurationIssueScope, ConfigurationRevision,
    ConfigurationState, ConfigurationStateView, ConflictSeverity, CoreCompatibilityProfile,
    CoreTargetIdentity, CoreValidationEvidence, ErrorCode, ProgramId, ProgramKind, ProgramManager,
    ProgramSpec, RawConflictResolution, RawDraftSession, RawManualIntent, Result,
    ShareImportPreview, SourceFreshness, SourceSnapshot, SourceStatus, rebase_raw_document,
    refresh_raw_draft_conflicts, resolve_raw_draft_conflict,
};
use serde_json::Value;
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};
use uuid::Uuid;

use crate::{
    FileStore, config_credentials::CredentialSnapshot, config_sources::SourceRefreshResult,
};

const NATIVE_VALIDATOR_CONTRACT_REVISION: &str = "core-native-validator-v1-20260811";

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
    previous_generation: u64,
}

impl ConfigurationCoordinator {
    pub(crate) fn new(store: Arc<FileStore>) -> Self {
        Self {
            store,
            locks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub(crate) async fn begin_source_update(
        &self,
        id: &ProgramId,
        previous_spec: &ProgramSpec,
        next_spec: &ProgramSpec,
        expected_generation: u64,
    ) -> Result<()> {
        self.store
            .begin_configuration_source_update(id, previous_spec, next_spec, expected_generation)
            .await
    }

    pub(crate) async fn mark_source_update_committed(&self, id: &ProgramId) -> Result<()> {
        self.store
            .mark_configuration_source_update_committed(id)
            .await
    }

    pub(crate) async fn finish_source_update(&self, id: &ProgramId) -> Result<()> {
        self.store.finish_configuration_source_update(id).await
    }

    pub(crate) async fn rollback_source_update(&self, id: &ProgramId) -> Result<()> {
        self.store.rollback_configuration_source_update(id).await
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

    pub(crate) async fn acquire_lease(&self, id: &ProgramId) -> ConfigurationLease {
        self.lock(id).await
    }

    #[allow(dead_code)]
    pub(crate) async fn forget(&self, id: &ProgramId) {
        self.locks.write().await.remove(id);
    }

    pub(crate) async fn load_view(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
    ) -> Result<ConfigurationStateView> {
        let lease = self.lock(id).await;
        self.load_view_with_lease(manager, id, &lease).await
    }

    pub(crate) async fn load_view_with_lease(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        _lease: &ConfigurationLease,
    ) -> Result<ConfigurationStateView> {
        let spec = manager.refresh_binary_identity_if_changed(id).await?;
        let mut state = self.load_or_initialize(manager, id).await?;
        if state.migrate_legacy_schema() {
            state.canonicalize_legacy_managed_raw()?;
            state.rebuild_desired(now_unix_ms())?;
            self.store
                .save_configuration_state(id, &state, None)
                .await?;
        }
        let previous_generation = state.generation;
        if self.retarget_state(id, &spec, &mut state).await? {
            return self
                .validate_existing(manager, id, &spec, state, previous_generation)
                .await;
        }
        Ok(view_for_spec(&spec, &state))
    }

    pub(crate) async fn initialize_created(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        observations: Option<SourceRefreshResult>,
    ) -> Result<ConfigurationStateView> {
        let _lease = self.lock(id).await;
        let (spec, _) = manager.get(id).await?;
        if let Some(mut state) = self.store.load_configuration_state(id).await? {
            if state.migrate_legacy_schema() {
                state.canonicalize_legacy_managed_raw()?;
                state.rebuild_desired(now_unix_ms())?;
                self.store
                    .save_configuration_state(id, &state, None)
                    .await?;
            }
            return Ok(view_for_spec(&spec, &state));
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
        self.persist_initialized(id, &state).await?;
        Ok(view_for_spec(&spec, &state))
    }

    pub(crate) async fn set_guided(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        setting_id: String,
        value: Option<serde_json::Value>,
        expected_generation: u64,
        replace_raw_override: bool,
    ) -> Result<ConfigurationStateView> {
        let _lease = self.lock(id).await;
        let (spec, mut state) = self.load_current(manager, id).await?;
        ensure_generation(&state, expected_generation)?;
        if !replace_raw_override && raw_overrides_setting(&spec, &state.raw_intent, &setting_id) {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "This Guided setting is overridden by Raw configuration",
            ));
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
        if replace_raw_override {
            remove_raw_override(&spec, &mut state.raw_intent, &setting_id);
        }
        let previous_generation = state.generation;
        state.rebuild_desired(now_unix_ms())?;
        self.validate_and_save(manager, id, &spec, state, previous_generation)
            .await
    }

    pub(crate) fn preview_import(
        &self,
        target: &CoreTargetIdentity,
        input: &[u8],
    ) -> Result<ShareImportPreview> {
        camellia_nexus_core::preview_share_import_for_version(input, target)
    }

    pub(crate) async fn get_editor_session(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
    ) -> Result<RawDraftSession> {
        let _lease = self.lock(id).await;
        if let Some(mut draft) = self.store.load_raw_draft(id).await? {
            let (_spec, state) = self.load_current(manager, id).await?;
            if draft.based_on_generation == state.generation {
                refresh_raw_draft_conflicts(&mut draft, state.format);
                return Ok(draft);
            }
            let mut rebased = draft;
            let original = parse_draft_value(state.format, &rebased.base_content)?;
            let user = match parse_draft_value(state.format, &rebased.user_content) {
                Ok(user) => user,
                // Invalid in-progress text is still a durable draft.  Keep it
                // recoverable and defer semantic rebase until the user repairs
                // the syntax; it can never reach commit in this state.
                Err(_) => {
                    refresh_raw_draft_conflicts(&mut rebased, state.format);
                    return Ok(rebased);
                }
            };
            let updated = parse_draft_value(state.format, &state.base.content)?;
            let result = rebase_raw_document(&original, &user, &updated);
            rebased.base_content = state.base.content.clone();
            rebased.based_on_generation = state.generation;
            rebased.working_content = serialize_draft_value(state.format, &result.document)?;
            rebased.conflicts = result.conflicts;
            rebased.resolutions.clear();
            refresh_raw_draft_conflicts(&mut rebased, state.format);
            rebased.draft_revision = rebased.draft_revision.saturating_add(1);
            rebased.updated_unix_ms = now_unix_ms();
            self.store.save_raw_draft(id, &rebased, None).await?;
            return Ok(rebased);
        }
        let (_spec, state) = self.load_current(manager, id).await?;
        Ok(RawDraftSession {
            session_id: Uuid::new_v4().to_string(),
            draft_revision: 0,
            based_on_generation: state.generation,
            base_content: state.base.content.clone(),
            user_content: state.desired.content.clone(),
            working_content: state.desired.content,
            conflicts: Vec::new(),
            resolutions: std::collections::BTreeMap::new(),
            unresolved_conflict_ids: Vec::new(),
            updated_unix_ms: now_unix_ms(),
        })
    }

    pub(crate) async fn save_configuration_draft(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        mut draft: RawDraftSession,
        expected_revision: u64,
    ) -> Result<RawDraftSession> {
        let _lease = self.lock(id).await;
        let (spec, state) = self.load_current(manager, id).await?;
        if draft.draft_revision != expected_revision {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Raw configuration draft revision is stale",
            ));
        }
        match self.store.load_raw_draft(id).await? {
            Some(persisted) => {
                if persisted.draft_revision != expected_revision
                    || persisted.session_id != draft.session_id
                {
                    return Err(CamelliaNexusError::new(
                        ErrorCode::ConfigConflict,
                        "Raw configuration draft revision is stale",
                    ));
                }
                draft.base_content = persisted.base_content;
                draft.based_on_generation = persisted.based_on_generation;
                draft.conflicts = persisted.conflicts;
                draft.resolutions = persisted.resolutions;
                draft.unresolved_conflict_ids = persisted.unresolved_conflict_ids;
            }
            None => {
                if expected_revision != 0
                    || draft.based_on_generation != state.generation
                    || draft.base_content != state.base.content
                {
                    return Err(CamelliaNexusError::new(
                        ErrorCode::ConfigConflict,
                        "Configuration changed before the Raw draft was first saved",
                    ));
                }
                if draft.session_id.trim().is_empty() {
                    return Err(CamelliaNexusError::new(
                        ErrorCode::InvalidSpec,
                        "Raw configuration draft session id is empty",
                    ));
                }
                draft.conflicts.clear();
                draft.resolutions.clear();
                draft.unresolved_conflict_ids.clear();
            }
        }
        if draft.based_on_generation > state.generation {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Raw configuration draft generation is invalid",
            ));
        }
        if draft.based_on_generation != state.generation {
            let original = parse_draft_value(state.format, &draft.base_content)?;
            if let Ok(user) = parse_draft_value(state.format, &draft.user_content) {
                let updated = parse_draft_value(state.format, &state.base.content)?;
                let rebased = rebase_raw_document(&original, &user, &updated);
                draft.base_content = state.base.content.clone();
                draft.based_on_generation = state.generation;
                draft.working_content = serialize_draft_value(state.format, &rebased.document)?;
                draft.conflicts = rebased.conflicts;
                draft.resolutions.clear();
            }
        }
        refresh_raw_draft_conflicts(&mut draft, state.format);
        draft.draft_revision = draft.draft_revision.saturating_add(1);
        draft.updated_unix_ms = now_unix_ms();
        self.store
            .save_raw_draft(id, &draft, Some(expected_revision))
            .await?;
        let _ = spec;
        Ok(draft)
    }

    pub(crate) async fn rebase_configuration_draft(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
    ) -> Result<RawDraftSession> {
        let _lease = self.lock(id).await;
        let (_spec, state) = self.load_current(manager, id).await?;
        let mut draft = self.store.load_raw_draft(id).await?.ok_or_else(|| {
            CamelliaNexusError::new(ErrorCode::NotFound, "Raw configuration draft was not found")
        })?;
        let original = parse_draft_value(state.format, &draft.base_content)?;
        let user = parse_draft_value(state.format, &draft.user_content)?;
        let updated = parse_draft_value(state.format, &state.base.content)?;
        let rebased = rebase_raw_document(&original, &user, &updated);
        draft.base_content = state.base.content.clone();
        draft.based_on_generation = state.generation;
        draft.working_content = serialize_draft_value(state.format, &rebased.document)?;
        draft.conflicts = rebased.conflicts;
        draft.resolutions.clear();
        refresh_raw_draft_conflicts(&mut draft, state.format);
        draft.draft_revision = draft.draft_revision.saturating_add(1);
        draft.updated_unix_ms = now_unix_ms();
        self.store.save_raw_draft(id, &draft, None).await?;
        Ok(draft)
    }

    pub(crate) async fn resolve_configuration_conflict(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        conflict_id: String,
        resolution: RawConflictResolution,
    ) -> Result<RawDraftSession> {
        let _lease = self.lock(id).await;
        let (_spec, state) = self.load_current(manager, id).await?;
        let mut draft = self.store.load_raw_draft(id).await?.ok_or_else(|| {
            CamelliaNexusError::new(ErrorCode::NotFound, "Raw configuration draft was not found")
        })?;
        if draft.based_on_generation != state.generation {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Configuration changed; rebase the draft before resolving conflicts",
            ));
        }
        resolve_raw_draft_conflict(&mut draft, state.format, &conflict_id, resolution)?;
        draft.draft_revision = draft.draft_revision.saturating_add(1);
        draft.updated_unix_ms = now_unix_ms();
        self.store.save_raw_draft(id, &draft, None).await?;
        Ok(draft)
    }

    pub(crate) async fn discard_configuration_draft(&self, id: &ProgramId) -> Result<()> {
        let _lease = self.lock(id).await;
        self.store.discard_raw_draft(id).await
    }

    pub(crate) async fn commit_configuration_draft(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
    ) -> Result<ConfigurationStateView> {
        let _lease = self.lock(id).await;
        let (spec, mut state) = self.load_current(manager, id).await?;
        let mut draft = self.store.load_raw_draft(id).await?.ok_or_else(|| {
            CamelliaNexusError::new(ErrorCode::NotFound, "Raw configuration draft was not found")
        })?;
        refresh_raw_draft_conflicts(&mut draft, state.format);
        if !draft.unresolved_conflict_ids.is_empty()
            || draft.based_on_generation != state.generation
        {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Resolve all Raw configuration conflicts before saving",
            ));
        }
        let previous_generation = state.generation;
        state.replace_raw_from_edited(draft.working_content.as_bytes(), now_unix_ms())?;
        let view = self
            .validate_existing(manager, id, &spec, state, previous_generation)
            .await?;
        // A draft is only needed while it differs from the authoritative
        // Desired candidate.  Invalid Desired is intentionally durable, so a
        // malformed draft that has already been committed (or was repaired to
        // the exact candidate bytes) must not be resurrected on the next
        // restart.  Applied/LKG remain untouched by this cleanup.
        if view.desired.validation != CandidateValidationStatus::Invalid
            || view.desired.content == draft.working_content
        {
            self.store.discard_raw_draft(id).await?;
        }
        Ok(view)
    }

    pub(crate) async fn apply_candidate(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        expected_generation: u64,
        interactive: bool,
    ) -> Result<ConfigurationStateView> {
        let _lease = self.lock(id).await;
        let (_, mut state) = self.load_current(manager, id).await?;
        ensure_generation(&state, expected_generation)?;
        let spec = manager.refresh_binary_identity(id).await?;
        if self.retarget_state(id, &spec, &mut state).await? {
            self.store
                .save_configuration_state(id, &state, Some(expected_generation))
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
        let active = manager.load_config(id).await?;
        self.store.begin_configuration_apply(id, &state).await?;
        let hash = manager
            .apply_config(
                id,
                &spec,
                state.desired.content.clone(),
                active.base_hash,
                interactive,
            )
            .await?;
        if hash != state.desired.revision.content_hash {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Applied configuration hash does not match Desired revision",
            ));
        }
        state.mark_applied()?;
        self.store
            .save_last_known_good(id, state.format, &state.desired.content)
            .await?;
        self.store
            .save_configuration_state(id, &state, Some(expected_generation))
            .await?;
        self.store.finish_configuration_apply(id).await?;
        Ok(view_for_spec(&spec, &state))
    }

    pub(crate) async fn refresh(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        local_base: Option<&std::path::Path>,
        credentials: &CredentialSnapshot,
    ) -> Result<ConfigurationStateView> {
        let lease = self.lock(id).await;
        self.refresh_with_lease(manager, id, local_base, credentials, &lease)
            .await
    }

    pub(crate) async fn refresh_with_lease(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        local_base: Option<&std::path::Path>,
        credentials: &CredentialSnapshot,
        _lease: &ConfigurationLease,
    ) -> Result<ConfigurationStateView> {
        let spec = manager.refresh_binary_identity(id).await?;
        let mut state = self.load_or_initialize(manager, id).await?;
        let previous_generation = state.generation;
        self.retarget_state(id, &spec, &mut state).await?;
        let observed = crate::config_sources::refresh_snapshots(
            &spec,
            local_base,
            credentials,
            &state.source_snapshots,
            now_unix_ms(),
        )
        .await?;
        for snapshot in &observed.snapshots {
            if let (Some(reference), Some(raw)) = (
                snapshot.raw_observation_ref.as_deref(),
                observed.raw_observations.get(&snapshot.source_id),
            ) {
                self.store
                    .save_configuration_sidecar(id, reference, raw)
                    .await?;
            }
        }
        state.source_statuses = observed.statuses;
        reconcile_snapshots(&spec, &mut state.source_snapshots, observed.snapshots);
        let has_enabled_sources = spec
            .managed_config
            .as_ref()
            .is_some_and(|managed| managed.sources.iter().any(|source| source.enabled()));
        if !has_enabled_sources {
            state.rebuild_desired(now_unix_ms())?;
            return self
                .validate_and_save(manager, id, &spec, state, previous_generation)
                .await;
        }
        let snapshots = ordered_snapshots(&spec, &state.source_snapshots);
        // Every enabled source is part of the requested Base.  If one has no
        // previously parsed snapshot, silently merging the remaining sources
        // would manufacture a partial candidate and could apply it while the
        // UI merely shows a small source-status badge.  Keep the current
        // Desired content as an invalid candidate instead; Applied/LKG remain
        // untouched and the user can repair the missing/invalid source from
        // the same workspace.  A failed source with a prior snapshot is
        // reported as Stale by refresh_snapshots and remains safely usable.
        if observed.unavailable {
            state.generation = state.generation.saturating_add(1);
            let invalid = state
                .source_statuses
                .values()
                .any(|status| status.freshness == SourceFreshness::Invalid);
            state.desired = blocked_source_candidate(&state, invalid);
            self.store
                .save_configuration_state(id, &state, Some(previous_generation))
                .await?;
            return Ok(view_for_spec(&spec, &state));
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
        state.rebuild_desired(now_unix_ms())?;
        self.validate_and_save(manager, id, &spec, state, previous_generation)
            .await
    }

    pub(crate) async fn sync_managed_dashboard(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
    ) -> Result<ConfigurationStateView> {
        let _lease = self.lock(id).await;
        let (spec, mut state) = self.load_current(manager, id).await?;
        let previous_generation = state.generation;
        if let Some(managed) = spec.managed_config.as_ref() {
            camellia_nexus_core::sync_managed_dashboard_intent(&mut state.managed_intent, managed);
        }
        state.rebuild_desired(now_unix_ms())?;
        self.validate_and_save(manager, id, &spec, state, previous_generation)
            .await
    }

    pub(crate) async fn prepare_managed_integration_update(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        next_spec: &ProgramSpec,
        expected_generation: Option<u64>,
        replace_overlapping_raw: bool,
    ) -> Result<PreparedManagedIntegrationUpdate> {
        let lease = self.lock(id).await;
        let (_current, mut state) = self.load_current(manager, id).await?;
        if let Some(expected_generation) = expected_generation {
            ensure_generation(&state, expected_generation)?;
        }
        let previous_generation = state.generation;
        let empty = camellia_nexus_core::ManagedConfigSpec::default();
        let managed = next_spec.managed_config.as_ref().unwrap_or(&empty);
        camellia_nexus_core::sync_managed_dashboard_intent(&mut state.managed_intent, managed);
        let overlapping_paths = camellia_nexus_core::managed_raw_override_paths(
            next_spec.program_type.kind(),
            &state.managed_intent,
            &state.raw_intent,
        );
        if !overlapping_paths.is_empty() && !replace_overlapping_raw {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Managed integration is overridden by Raw configuration",
            )
            .with_message_key("CONFIGURATION_RAW_OVERRIDE")
            .with_details(
                serde_json::to_string(&serde_json::json!({
                    "messageKey": "CONFIGURATION_RAW_OVERRIDE",
                    "semanticPaths": overlapping_paths,
                    "appliedAndLastKnownGoodRetained": true,
                }))
                .unwrap_or_else(|_| "CONFIGURATION_RAW_OVERRIDE".into()),
            ));
        }
        if replace_overlapping_raw {
            camellia_nexus_core::remove_managed_raw_overrides(
                next_spec.program_type.kind(),
                &state.managed_intent,
                &mut state.raw_intent,
            );
        }
        state.rebuild_desired(now_unix_ms())?;
        Ok(PreparedManagedIntegrationUpdate {
            _lease: lease,
            state,
            previous_generation,
        })
    }

    pub(crate) async fn commit_managed_integration_update(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        next_spec: &ProgramSpec,
        prepared: PreparedManagedIntegrationUpdate,
    ) -> Result<ConfigurationStateView> {
        let PreparedManagedIntegrationUpdate {
            _lease,
            state,
            previous_generation,
        } = prepared;
        self.validate_and_save(manager, id, next_spec, state, previous_generation)
            .await
    }

    async fn validate_and_save(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        spec: &ProgramSpec,
        state: ConfigurationState,
        expected_generation: u64,
    ) -> Result<ConfigurationStateView> {
        self.validate_existing(manager, id, spec, state, expected_generation)
            .await
    }

    async fn validate_existing(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
        _spec: &ProgramSpec,
        mut state: ConfigurationState,
        expected_generation: u64,
    ) -> Result<ConfigurationStateView> {
        let spec = manager.refresh_binary_identity(id).await?;
        self.retarget_state(id, &spec, &mut state).await?;
        if state.desired.validation == CandidateValidationStatus::Invalid
            && state
                .desired
                .conflicts
                .iter()
                .any(|conflict| conflict.severity == camellia_nexus_core::ConflictSeverity::Error)
        {
            self.store
                .save_configuration_state(id, &state, Some(expected_generation))
                .await?;
            return Ok(view_for_spec(&spec, &state));
        }
        let active = manager.load_config(id).await?;
        let validation = manager
            .validate_config(id, state.desired.content.clone(), active.base_hash)
            .await?;
        let diagnostics = if validation.valid {
            Vec::new()
        } else {
            vec![ConfigurationDiagnostic {
                code: "CORE_INVALID".into(),
                message: "Core validation failed".into(),
                message_key: Some("CORE_INVALID".into()),
                scope: ConfigurationIssueScope::configuration(),
                details: Some(format!("{}\n{}", validation.stdout, validation.stderr)),
            }]
        };
        let evidence = validation
            .valid
            .then(|| native_validation_evidence(&spec, &state))
            .transpose()?;
        state.mark_validation(validation.valid, diagnostics, evidence)?;
        self.store
            .save_configuration_state(id, &state, Some(expected_generation))
            .await?;
        Ok(view_for_spec(&spec, &state))
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
                    rejected_share_sources.push((snapshot.source_id.clone(), error.message));
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
        state.rebuild_desired(now_unix_ms())?;
        for (source_id, message) in rejected_share_sources {
            if let Some(status) = state.source_statuses.get_mut(&source_id) {
                status.freshness = SourceFreshness::Invalid;
                status.message = Some(message.clone());
            }
            state.desired.conflicts.push(ConfigurationConflict {
                semantic_path: format!("/sources/{source_id}"),
                reason: "Share source has no item expressible for the selected Core target".into(),
                severity: ConflictSeverity::Error,
                message_key: Some("CORE_TARGET_SOURCE_REJECTED".into()),
                scope: ConfigurationIssueScope::sources(source_id.clone()),
                source_value: None,
                guided_value: None,
                raw_value: None,
                effective_value: None,
            });
            state.desired.diagnostics.push(ConfigurationDiagnostic {
                code: "CORE_TARGET_SOURCE_REJECTED".into(),
                message: "Share source was retained but has no accepted item for the selected Core target".into(),
                message_key: Some("CORE_TARGET_SOURCE_REJECTED".into()),
                scope: ConfigurationIssueScope::sources("share-import"),
                details: Some(message),
            });
            state.desired.validation = CandidateValidationStatus::Invalid;
            state.desired.validation_evidence = None;
        }
        state.desired.diagnostics.push(ConfigurationDiagnostic {
            code: "CORE_TARGET_CHANGED".into(),
            message: "Core target changed; Sources, Guided intent, and Raw intent were retargeted"
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

    async fn load_current(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
    ) -> Result<(ProgramSpec, ConfigurationState)> {
        let (spec, _) = manager.get(id).await?;
        let mut state = self.load_or_initialize(manager, id).await?;
        if state.migrate_legacy_schema() {
            state.canonicalize_legacy_managed_raw()?;
            state.rebuild_desired(now_unix_ms())?;
            self.store
                .save_configuration_state(id, &state, None)
                .await?;
        }
        Ok((spec, state))
    }

    async fn load_or_initialize(
        &self,
        manager: &ProgramManager,
        id: &ProgramId,
    ) -> Result<ConfigurationState> {
        if let Some(state) = self.store.load_configuration_state(id).await? {
            return Ok(state);
        }
        let (spec, _) = manager.get(id).await?;
        let document = manager.load_config(id).await?;
        let state = state_from_initial_document(&spec, &document.content)?;
        self.persist_initialized(id, &state).await?;
        Ok(state)
    }

    async fn persist_initialized(&self, id: &ProgramId, state: &ConfigurationState) -> Result<()> {
        self.store
            .save_last_known_good(id, state.format, &state.desired.content)
            .await?;
        self.store.save_configuration_state(id, state, None).await?;
        Ok(())
    }
}

fn is_zero_acceptance_share_error(error: &CamelliaNexusError) -> bool {
    error.code == ErrorCode::ConfigInvalid
        && (error.message == "Share source contains no translatable items"
            || error.message == "Share source contains no compatible items for this Core")
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
    state.mark_validation(
        true,
        Vec::new(),
        Some(native_validation_evidence(spec, &state)?),
    )?;
    state.mark_applied()?;
    Ok(state)
}

fn state_from_initial_document(spec: &ProgramSpec, content: &str) -> Result<ConfigurationState> {
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
    // The initial file has already passed the Core's native validator during
    // program creation. Keep its exact bytes as the initial candidate so the
    // candidate-only evidence binds to the file that will actually start;
    // subsequent semantic edits may canonicalize formatting through the normal
    // source/desired pipeline.
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
            message: None,
            observed_unix_ms: Some(snapshot.parsed_unix_ms),
        },
    );
    state.mark_validation(
        true,
        Vec::new(),
        Some(native_validation_evidence(spec, &state)?),
    )?;
    state.mark_applied()?;
    Ok(state)
}

fn ensure_generation(state: &ConfigurationState, expected: u64) -> Result<()> {
    if state.generation == expected {
        Ok(())
    } else {
        Err(CamelliaNexusError::new(
            ErrorCode::ConfigConflict,
            "Configuration changed since it was loaded",
        ))
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
        validator_contract_revision: NATIVE_VALIDATOR_CONTRACT_REVISION.into(),
        native_accepted: true,
        validated_unix_ms: now_unix_ms(),
    })
}

fn blocked_source_candidate(state: &ConfigurationState, invalid: bool) -> ConfigurationCandidate {
    let mut candidate = state.desired.clone();
    candidate.revision =
        ConfigurationRevision::new(state.generation, &candidate.content, now_unix_ms());
    candidate.validation = CandidateValidationStatus::Invalid;
    candidate.validation_evidence = None;
    candidate.diagnostics = vec![ConfigurationDiagnostic {
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

fn parse_draft_value(format: ConfigurationFormat, content: &str) -> Result<Value> {
    camellia_nexus_core::parse_semantic_document(format, content.as_bytes())
}

fn serialize_draft_value(format: ConfigurationFormat, value: &Value) -> Result<String> {
    camellia_nexus_core::serialize_semantic_document(format, value)
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

fn view_for_spec(spec: &ProgramSpec, state: &ConfigurationState) -> ConfigurationStateView {
    let mut view = state.view();
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
    view
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

fn raw_overrides_setting(spec: &ProgramSpec, intent: &RawManualIntent, setting_id: &str) -> bool {
    let Some(path) = guided_path_for_spec(spec, setting_id) else {
        return false;
    };
    intent.operations.iter().any(|operation| {
        let operation_path = match operation {
            camellia_nexus_core::IntentOperation::Set { path, .. }
            | camellia_nexus_core::IntentOperation::Delete { path }
            | camellia_nexus_core::IntentOperation::ReorderIdentities { path, .. }
            | camellia_nexus_core::IntentOperation::ReplaceSequence { path, .. } => path,
        };
        path.starts_with(operation_path) || operation_path.starts_with(&path)
    })
}

fn remove_raw_override(spec: &ProgramSpec, intent: &mut RawManualIntent, setting_id: &str) {
    let Some(path) = guided_path_for_spec(spec, setting_id) else {
        return;
    };
    intent.operations.retain(|operation| {
        let operation_path = match operation {
            camellia_nexus_core::IntentOperation::Set { path, .. }
            | camellia_nexus_core::IntentOperation::Delete { path }
            | camellia_nexus_core::IntentOperation::ReorderIdentities { path, .. }
            | camellia_nexus_core::IntentOperation::ReplaceSequence { path, .. } => path,
        };
        !(path.starts_with(operation_path) || operation_path.starts_with(&path))
    });
}

fn guided_path_for_spec(
    spec: &ProgramSpec,
    setting_id: &str,
) -> Option<Vec<camellia_nexus_core::SemanticPathSegment>> {
    let path = match (spec.program_type.kind(), setting_id) {
        (ProgramKind::SingBox, "logging.level") => vec!["log", "level"],
        (ProgramKind::SingBox, "dns.strategy") => vec!["dns", "strategy"],
        (ProgramKind::SingBox, "routing.autoDetectInterface") => {
            vec!["route", "auto_detect_interface"]
        }
        (ProgramKind::Xray, "logging.level") => vec!["log", "loglevel"],
        (ProgramKind::Xray, "routing.domainStrategy") => vec!["routing", "domainStrategy"],
        (ProgramKind::Mihomo, "logging.level") => vec!["log-level"],
        (ProgramKind::Mihomo, "network.ipv6") => vec!["ipv6"],
        (ProgramKind::Mihomo, "tun.enabled") => vec!["tun", "enable"],
        (ProgramKind::Mihomo, "tun.strictRoute") => vec!["tun", "strict-route"],
        (ProgramKind::Mihomo, "dns.enabled") => vec!["dns", "enable"],
        (ProgramKind::Mihomo, "dns.mode") => vec!["dns", "enhanced-mode"],
        (ProgramKind::Mihomo, "routing.mode") => vec!["mode"],
        _ => return None,
    };
    Some(
        path.into_iter()
            .map(|key| camellia_nexus_core::SemanticPathSegment::Key { key: key.into() })
            .collect(),
    )
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
        ConfigSourceSpec, ConfigurationFormat, CoreBinaryFingerprint, CoreCompatibilityPreference,
        CoreCompatibilityReference, CoreProbeReport, CoreTargetIdentity, ExecutableMetadata,
        ExecutableSpec, ManagedConfigSpec, ProgramId, ProgramType, RestartPolicy, SCHEMA_VERSION,
        embedded_core_compatibility_catalog,
    };

    use super::*;

    fn spec(sources: Vec<ConfigSourceSpec>) -> ProgramSpec {
        let fingerprint = CoreBinaryFingerprint {
            sha256: "a".repeat(64),
            size: 1,
            modified_unix_ms: 1,
        };
        ProgramSpec {
            schema_version: SCHEMA_VERSION,
            id: ProgramId::parse("configuration-test").expect("id"),
            name: "Configuration test".into(),
            executable: ExecutableSpec::External {
                path: PathBuf::from("xray"),
                compatibility: Default::default(),
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
    fn guided_replace_removes_only_overlapping_raw_operations() {
        let program = spec(Vec::new());
        let key =
            |value: &str| vec![camellia_nexus_core::SemanticPathSegment::Key { key: value.into() }];
        let mut intent = RawManualIntent {
            based_on_revision: None,
            operations: vec![
                camellia_nexus_core::IntentOperation::Set {
                    path: [key("log"), key("loglevel")].concat(),
                    value: serde_json::json!("debug"),
                },
                camellia_nexus_core::IntentOperation::Set {
                    path: key("routing"),
                    value: serde_json::json!({"domainStrategy": "IPOnDemand"}),
                },
            ],
        };

        assert!(raw_overrides_setting(&program, &intent, "logging.level"));
        remove_raw_override(&program, &mut intent, "logging.level");
        assert_eq!(intent.operations.len(), 1);
        assert!(matches!(
            &intent.operations[0],
            camellia_nexus_core::IntentOperation::Set { path, .. }
                if path == &key("routing")
        ));
    }

    #[test]
    fn created_managed_state_uses_real_source_observations() {
        let spec = spec(vec![
            inline("base", true),
            inline("override", true),
            inline("disabled", false),
        ]);
        let base = snapshot("base");
        let override_snapshot = snapshot("override");
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
                    message: None,
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
                message: None,
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
    fn blocked_source_candidate_uses_the_new_state_generation_and_reason() {
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
        state
            .mark_validation(
                true,
                Vec::new(),
                Some(native_validation_evidence(&spec, &state).expect("evidence")),
            )
            .expect("validation");
        state.mark_applied().expect("applied");
        state.generation = 2;

        let candidate = blocked_source_candidate(&state, false);

        assert_eq!(candidate.revision.generation, state.generation);
        assert_eq!(
            candidate.revision.content_hash,
            state.desired.revision.content_hash
        );
        assert_eq!(candidate.validation, CandidateValidationStatus::Invalid);
        assert_eq!(candidate.diagnostics[0].code, "SOURCE_UNAVAILABLE");

        let invalid = blocked_source_candidate(&state, true);
        assert_eq!(invalid.validation, CandidateValidationStatus::Invalid);
        assert_eq!(invalid.diagnostics[0].code, "SOURCE_INVALID");
        assert!(state.applied.is_some());
        assert!(state.last_known_good.is_some());
    }

    #[tokio::test]
    async fn unknown_without_reference_retargets_without_replacing_applied_or_lkg() {
        let directory = tempfile::tempdir().expect("tempdir");
        let coordinator = ConfigurationCoordinator::new(Arc::new(
            FileStore::new(directory.path().join("store")).expect("file store"),
        ));
        let mut referenced_spec = spec(Vec::new());
        let fingerprint = referenced_spec
            .executable
            .metadata()
            .expect("metadata")
            .fingerprint
            .clone();
        let catalog = embedded_core_compatibility_catalog().expect("catalog");
        let release = catalog
            .program(ProgramKind::Xray)
            .expect("xray catalog")
            .versions
            .first()
            .expect("catalogued release");
        let referenced_preference = CoreCompatibilityPreference::Unknown {
            reference: Some(CoreCompatibilityReference::Release {
                tag: release.tag.clone(),
            }),
        };
        let probe = CoreProbeReport::from_reported_version(Some("private xray build".into()));
        let referenced_target = catalog
            .resolve_target(
                ProgramKind::Xray,
                &probe,
                &referenced_preference,
                Some(fingerprint.sha256.clone()),
            )
            .expect("referenced target");
        referenced_spec
            .executable
            .set_compatibility(referenced_preference);
        referenced_spec.executable.set_metadata(ExecutableMetadata {
            fingerprint: fingerprint.clone(),
            probe: Some(probe.clone()),
            core_target: Some(referenced_target),
        });

        let merge = camellia_nexus_core::merge_configuration_sources(
            ProgramKind::Xray,
            &[snapshot("initial")],
        )
        .expect("merge");
        let mut state = ConfigurationState::from_merge(
            ProgramKind::Xray,
            1,
            1,
            merge,
            compatibility_profile(&referenced_spec).expect("referenced profile"),
        )
        .expect("configuration state");
        state
            .mark_validation(
                true,
                Vec::new(),
                Some(
                    native_validation_evidence(&referenced_spec, &state)
                        .expect("validation evidence"),
                ),
            )
            .expect("validation");
        state.mark_applied().expect("applied");
        let applied = state.applied.clone();
        let last_known_good = state.last_known_good.clone();

        let mut unreferenced_spec = referenced_spec.clone();
        let unreferenced_preference = CoreCompatibilityPreference::Unknown { reference: None };
        let unreferenced_target = catalog
            .resolve_target(
                ProgramKind::Xray,
                &probe,
                &unreferenced_preference,
                Some(fingerprint.sha256.clone()),
            )
            .expect("unreferenced target");
        unreferenced_spec
            .executable
            .set_compatibility(unreferenced_preference);
        unreferenced_spec
            .executable
            .set_metadata(ExecutableMetadata {
                fingerprint,
                probe: Some(probe),
                core_target: Some(unreferenced_target),
            });

        assert!(
            coordinator
                .retarget_state(&referenced_spec.id, &unreferenced_spec, &mut state)
                .await
                .expect("retarget")
        );
        assert!(matches!(
            state.compatibility_profile.target.coordinate,
            camellia_nexus_core::CoreVersionCoordinate::Unknown
        ));
        assert_eq!(
            state.compatibility_profile.target.basis,
            camellia_nexus_core::CoreCompatibilityBasis::Unknown
        );
        assert_eq!(state.applied, applied);
        assert_eq!(state.last_known_good, last_known_good);
        assert_eq!(state.desired.validation, CandidateValidationStatus::Pending);
        assert!(state.desired.validation_evidence.is_none());
    }
}
