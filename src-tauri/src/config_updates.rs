use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use camellia_nexus_core::{CamelliaNexusError, ErrorCode, ProgramId, ProgramManager, Result};
use camellia_nexus_licensing::{ProtectedOperation, RestrictedOperation};
use serde::Serialize;
use tauri::{Emitter, Manager};

const SCHEDULER_TICK: Duration = Duration::from_secs(30);
use crate::config_update_schedule::ScheduleBook;

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
) -> Result<camellia_nexus_core::ConfigurationStateView> {
    let _lease = state.config_refreshes.try_acquire(program_id)?;
    let (spec, _) = state.manager.get(program_id).await?;
    let local_base = state.manager.working_directory(program_id).await?;
    let credentials = if crate::config_credentials::has_credentials(&spec) {
        state.config_credentials.snapshot().await?
    } else {
        crate::config_credentials::CredentialSnapshot::empty()
    };
    let view = state
        .configuration_state
        .refresh(&state.manager, program_id, Some(&local_base), &credentials)
        .await?;
    // Automatic refresh updates Sources/Base and creates a reviewable
    // candidate only. Native validation and Apply are explicit workspace
    // stages, so a background timer can never replace Applied/LKG.
    state.config_refreshes.mark_completed(program_id);
    Ok(view)
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
