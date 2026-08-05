use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use camellia_nexus_core::{
    CamelliaNexusError, ConfigDocument, ErrorCode, PreparedConfigGuard, PreparedProgramUpdate,
    ProgramId, ProgramManager, ProgramSpec, Result,
};
use camellia_nexus_licensing::{ProtectedOperation, RestrictedOperation};
use serde::Serialize;
use tauri::{Emitter, Manager};

const SCHEDULER_TICK: Duration = Duration::from_secs(30);
use crate::config_update_schedule::ScheduleBook;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigUpdateResult {
    pub source_count: usize,
    pub document: camellia_nexus_core::ConfigDocument,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AutomaticConfigUpdateEvent {
    program_id: ProgramId,
    succeeded: bool,
}

#[derive(Default)]
pub struct RefreshCoordinator {
    active: Mutex<HashSet<ProgramId>>,
    completed: Mutex<HashMap<ProgramId, Instant>>,
    shutdown_requested: AtomicBool,
}

pub(crate) struct RefreshLease<'a> {
    coordinator: &'a RefreshCoordinator,
    id: ProgramId,
}

impl RefreshCoordinator {
    pub(crate) fn try_acquire(&self, id: &ProgramId) -> Result<RefreshLease<'_>> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.shutdown_requested.load(Ordering::Acquire) {
            return Err(CamelliaNexusError::new(
                ErrorCode::InvalidState,
                "Application shutdown is in progress",
            ));
        }
        if !active.insert(id.clone()) {
            return Err(CamelliaNexusError::new(
                ErrorCode::ProgramBusy,
                "Configuration sources are already being updated",
            ));
        }
        Ok(RefreshLease {
            coordinator: self,
            id: id.clone(),
        })
    }

    pub(crate) fn begin_shutdown(&self) {
        let _active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.shutdown_requested.store(true, Ordering::Release);
    }

    fn is_shutting_down(&self) -> bool {
        self.shutdown_requested.load(Ordering::Acquire)
    }

    fn mark_completed(&self, id: &ProgramId) {
        self.completed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id.clone(), Instant::now());
    }

    fn last_completed(&self, id: &ProgramId) -> Option<Instant> {
        self.completed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .copied()
    }

    fn retain_completed(&self, active: &HashSet<ProgramId>) {
        self.completed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|id, _| active.contains(id));
    }
}

impl Drop for RefreshLease<'_> {
    fn drop(&mut self) {
        self.coordinator
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.id);
    }
}

pub async fn refresh(
    state: &crate::AppState,
    program_id: &ProgramId,
) -> Result<ConfigUpdateResult> {
    let _lease = state.config_refreshes.try_acquire(program_id)?;
    let prepared = prepare_refresh(state, program_id).await?;
    // Source I/O stays outside the runtime gate. Re-check immediately before the only commit that
    // can restart a running program, then hold the read permit until that commit has settled.
    let runtime_operation = match crate::commands::authorize_runtime_protected(
        state,
        ProtectedOperation::UseManagedConfigSources,
    )
    .await
    {
        Ok(operation) => operation,
        Err(error) => {
            discard_prepared_refresh(state, prepared).await;
            return Err(error);
        }
    };
    commit_refresh(state, program_id, prepared, &runtime_operation).await
}

pub(crate) struct PreparedRefresh {
    expected_spec: ProgramSpec,
    was_active: bool,
    requires_privilege_broker: bool,
    source_count: usize,
    document: ConfigDocument,
    content: String,
    config: PreparedConfigGuard,
}

pub(crate) async fn prepare_refresh(
    state: &crate::AppState,
    program_id: &ProgramId,
) -> Result<PreparedRefresh> {
    let manager = &state.manager;
    let (spec, program_state) = manager.get(program_id).await?;
    prepare_refresh_for_specs(state, program_id, spec.clone(), spec, program_state).await
}

pub(crate) async fn prepare_refresh_for_update(
    state: &crate::AppState,
    update: &PreparedProgramUpdate,
) -> Result<PreparedRefresh> {
    let program_id = &update.next_spec().id;
    let (_, program_state) = state.manager.get(program_id).await?;
    prepare_refresh_for_specs(
        state,
        program_id,
        update.expected_spec().clone(),
        update.next_spec().clone(),
        program_state,
    )
    .await
}

async fn prepare_refresh_for_specs(
    state: &crate::AppState,
    program_id: &ProgramId,
    expected_spec: ProgramSpec,
    target_spec: ProgramSpec,
    program_state: camellia_nexus_core::ProgramState,
) -> Result<PreparedRefresh> {
    target_spec.validate()?;
    let source_count = target_spec
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
    let document = state
        .manager
        .load_config_for_spec(program_id, &expected_spec, &target_spec)
        .await?;
    let local_base = state.manager.working_directory(program_id).await?;
    let credentials = if crate::config_credentials::has_credentials(&target_spec) {
        state.config_credentials.snapshot().await?
    } else {
        crate::config_credentials::CredentialSnapshot::empty()
    };
    let content = crate::config_sources::materialize(
        &target_spec,
        Some(document.content.clone()),
        false,
        Some(&local_base),
        &credentials,
    )
    .await?;
    let was_active = matches!(
        program_state,
        camellia_nexus_core::ProgramState::Starting
            | camellia_nexus_core::ProgramState::Running { .. }
            | camellia_nexus_core::ProgramState::Stopping
            | camellia_nexus_core::ProgramState::Backoff { .. }
            | camellia_nexus_core::ProgramState::StopFailed { .. }
    );
    let requires_privilege_broker = if was_active
        && !matches!(
            target_spec.privilege_policy,
            camellia_nexus_core::PrivilegePolicy::Standard
        ) {
        let (requirement, _) = crate::privileges::assess_configuration(
            target_spec.program_type.kind(),
            content.as_bytes(),
        )?;
        requirement == camellia_nexus_core::PrivilegeRequirement::Elevated
            || matches!(
                target_spec.privilege_policy,
                camellia_nexus_core::PrivilegePolicy::Elevated
            )
    } else {
        false
    };
    let config = state
        .manager
        .prepare_config(
            program_id,
            &expected_spec,
            &target_spec,
            content.clone(),
            document.base_hash.clone(),
        )
        .await?;
    Ok(PreparedRefresh {
        expected_spec,
        was_active,
        requires_privilege_broker,
        source_count,
        document,
        content,
        config,
    })
}

pub(crate) async fn commit_refresh(
    state: &crate::AppState,
    program_id: &ProgramId,
    prepared: PreparedRefresh,
    _runtime_operation: &crate::commands::RuntimeMutationPermit<'_>,
) -> Result<ConfigUpdateResult> {
    let PreparedRefresh {
        expected_spec,
        was_active,
        requires_privilege_broker,
        source_count,
        document,
        content,
        config,
    } = prepared;
    if was_active && requires_privilege_broker && !crate::privilege_broker::has_active_session() {
        let _ = state.manager.discard_prepared_config(config).await;
        return Err(camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::PrivilegeRequired,
            "Automatic configuration refresh was deferred until an administrator session is opened by an explicit start",
        ));
    }
    let new_hash = state
        .manager
        .apply_prepared_config(program_id, &expected_spec, config, false)
        .await?;
    let result = completed_refresh_result(source_count, document, content, new_hash);
    state.config_refreshes.mark_completed(program_id);
    Ok(result)
}

pub(crate) async fn commit_update_refresh(
    state: &crate::AppState,
    update: PreparedProgramUpdate,
    prepared: PreparedRefresh,
    _runtime_operation: &crate::commands::RuntimeMutationPermit<'_>,
) -> Result<ConfigUpdateResult> {
    let program_id = update.next_spec().id.clone();
    let PreparedRefresh {
        expected_spec: _,
        was_active,
        requires_privilege_broker,
        source_count,
        document,
        content,
        config,
    } = prepared;
    if was_active && requires_privilege_broker && !crate::privilege_broker::has_active_session() {
        let _ = state.manager.discard_prepared_config(config).await;
        return Err(camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::PrivilegeRequired,
            "Automatic configuration refresh was deferred until an administrator session is opened by an explicit start",
        ));
    }
    let new_hash = state
        .manager
        .commit_update_and_apply_config(update, config, false)
        .await?;
    let result = completed_refresh_result(source_count, document, content, new_hash);
    state.config_refreshes.mark_completed(&program_id);
    Ok(result)
}

pub(crate) async fn discard_prepared_refresh(state: &crate::AppState, prepared: PreparedRefresh) {
    let _ = state.manager.discard_prepared_config(prepared.config).await;
}

fn completed_refresh_result(
    source_count: usize,
    document: ConfigDocument,
    content: String,
    new_hash: String,
) -> ConfigUpdateResult {
    ConfigUpdateResult {
        source_count,
        document: ConfigDocument {
            content,
            base_hash: new_hash,
            language: document.language,
            documentation_url: document.documentation_url,
            configuration_schema: document.configuration_schema,
        },
    }
}

pub fn spawn_scheduler(
    app: tauri::AppHandle,
    manager: Arc<ProgramManager>,
    coordinator: Arc<RefreshCoordinator>,
) {
    tauri::async_runtime::spawn(async move {
        let mut schedule = ScheduleBook::default();
        let mut ticker = tokio::time::interval(SCHEDULER_TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            if coordinator.is_shutting_down() {
                return;
            }
            let mut policies = Vec::new();
            for summary in manager.list().await {
                let Ok((spec, _)) = manager.get(&summary.id).await else {
                    continue;
                };
                let Some(minutes) = spec
                    .managed_config
                    .as_ref()
                    .and_then(|managed| managed.automatic_remote_update_minutes())
                else {
                    continue;
                };
                let last_completed = coordinator.last_completed(&summary.id);
                policies.push((
                    summary.id,
                    Duration::from_secs(u64::from(minutes) * 60),
                    last_completed,
                ));
            }
            let active: HashSet<_> = policies.iter().map(|(id, _, _)| id.clone()).collect();
            coordinator.retain_completed(&active);
            for program_id in schedule.due(policies, Instant::now()) {
                if coordinator.is_shutting_down() {
                    return;
                }
                let Some(state) = app.try_state::<crate::AppState>() else {
                    return;
                };
                if let Err(error) = state.authorization.authorize(
                    RestrictedOperation::Protected(ProtectedOperation::UseManagedConfigSources),
                    crate::licensing::unix_now(),
                ) {
                    tracing::debug!(program = %program_id, %error, "automatic configuration update deferred until the license is active");
                    schedule.retry_soon(&program_id, Instant::now());
                    continue;
                }
                match refresh(&state, &program_id).await {
                    Ok(_) => {
                        let _ = app.emit(
                            "automatic-config-update",
                            AutomaticConfigUpdateEvent {
                                program_id,
                                succeeded: true,
                            },
                        );
                    }
                    Err(error) => {
                        if coordinator.is_shutting_down() {
                            return;
                        }
                        tracing::warn!(program = %program_id, %error, "automatic configuration update failed");
                        schedule.retry_soon(&program_id, Instant::now());
                        let _ = app.emit(
                            "automatic-config-update",
                            AutomaticConfigUpdateEvent {
                                program_id,
                                succeeded: false,
                            },
                        );
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use camellia_nexus_core::{ErrorCode, ProgramId};

    use super::RefreshCoordinator;

    #[test]
    fn shutdown_prevents_new_configuration_refresh_leases() {
        let coordinator = RefreshCoordinator::default();
        let active_id = ProgramId::parse("active-refresh").expect("active id");
        let next_id = ProgramId::parse("next-refresh").expect("next id");
        let active = coordinator.try_acquire(&active_id).expect("active refresh");

        coordinator.begin_shutdown();
        assert!(coordinator.is_shutting_down());
        let error = coordinator
            .try_acquire(&next_id)
            .err()
            .expect("shutdown must reject a new refresh");
        assert_eq!(error.code, ErrorCode::InvalidState);

        drop(active);
        let error = coordinator
            .try_acquire(&active_id)
            .err()
            .expect("dropping an in-flight lease must not reopen scheduling");
        assert_eq!(error.code, ErrorCode::InvalidState);
    }
}
