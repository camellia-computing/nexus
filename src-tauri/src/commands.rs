use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsStr,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use camellia_nexus_core::{
    ActionDescriptor, ActionResult, CommandPlan, ConfigDocument, CreateProgramRequest, ErrorCode,
    ExecutableSpec, LogChunk, LogStream, ProgramId, ProgramSpec, ProgramState, ProgramSummary,
    ProgramType, Result, ValidationResult,
};
use camellia_nexus_licensing::{
    DeviceState, EntitlementState, NumericLimit, ProtectedOperation, RestrictedOperation,
    SafetyOperation, SecretValue,
};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;

use crate::AppState;
use crate::license_session::{
    AUTHORIZATION_SESSION_CAPACITY, PendingAuthorizationError, PendingAuthorizationStore,
};
use crate::settings::AppSettings;

const LICENSE_AUTHORIZATION_CALLBACK_TIMEOUT: Duration = Duration::from_secs(3 * 60);
static AUTHORIZATION_CALLBACK_THREADS: CallbackThreadLimiter =
    CallbackThreadLimiter::new(AUTHORIZATION_SESSION_CAPACITY);

struct CallbackThreadLimiter {
    active: AtomicUsize,
    capacity: usize,
}

impl CallbackThreadLimiter {
    const fn new(capacity: usize) -> Self {
        Self {
            active: AtomicUsize::new(0),
            capacity,
        }
    }

    fn try_acquire(&self) -> Option<CallbackThreadPermit<'_>> {
        self.active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < self.capacity).then_some(active + 1)
            })
            .ok()
            .map(|_| CallbackThreadPermit { limiter: self })
    }
}

struct CallbackThreadPermit<'a> {
    limiter: &'a CallbackThreadLimiter,
}

impl Drop for CallbackThreadPermit<'_> {
    fn drop(&mut self) {
        let previous = self.limiter.active.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "callback thread permit underflow");
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgramDetail {
    pub spec: ProgramSpec,
    pub state: ProgramState,
    pub working_directory: std::path::PathBuf,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XrayDashboardSnapshot {
    pub api_url: String,
    pub metrics_url: String,
    pub metrics: Option<Value>,
    pub metrics_error: Option<String>,
    pub system_stats: Option<XraySystemStats>,
    pub system_stats_error: Option<String>,
    pub topology: Option<XrayRuntimeTopology>,
    pub topology_error: Option<String>,
    pub online_users: Option<XrayOnlineUsersSummary>,
    pub online_users_error: Option<String>,
    pub balancers: Option<Vec<XrayBalancerInfo>>,
    pub routing_error: Option<String>,
    pub fetched_unix_ms: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XraySystemStats {
    pub uptime_seconds: u64,
    pub allocated_bytes: u64,
    pub system_bytes: u64,
    pub goroutines: u64,
    pub live_objects: u64,
    pub garbage_collections: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XrayRuntimeTopology {
    pub inbound_tags: Vec<String>,
    pub outbound_tags: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LicenseAuthorizationCallbackEvent {
    pub state: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LicenseAuthorizationFailedEvent {
    pub state: String,
    pub message: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementSnapshot {
    pub generation: u64,
    pub entitlement_state: EntitlementState,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LicenseStateChangedEvent {
    #[serde(flatten)]
    pub snapshot: EntitlementSnapshot,
    pub reason: &'static str,
    pub runtime_impact: &'static str,
    pub stopped_programs: usize,
    pub failed_programs: usize,
    pub failed_program_ids: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalLicenseDevice {
    pub device_id: String,
    pub display_name: Option<String>,
    pub platform: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LicenseRuntimeImpact {
    Active,
    RestrictedOffline,
    HardInactive,
}

impl LicenseRuntimeImpact {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::RestrictedOffline => "restrictedOffline",
            Self::HardInactive => "hardInactive",
        }
    }

    const fn requires_runtime_stop(self) -> bool {
        matches!(self, Self::HardInactive)
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XrayOnlineUsersSummary {
    pub policy_enabled: bool,
    pub status_available: bool,
    pub loopback_only: bool,
    pub user_count: usize,
    pub address_count: usize,
    pub users: Vec<XrayOnlineUser>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XrayOnlineUser {
    pub email: String,
    pub online: Option<bool>,
    pub addresses: Vec<XrayOnlineAddress>,
    pub uplink: u64,
    pub downlink: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XrayOnlineAddress {
    pub ip: String,
    pub last_seen_unix: i64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XrayBalancerInfo {
    pub tag: String,
    pub selectors: Vec<String>,
    pub candidates: Vec<String>,
    pub available_candidates: Vec<String>,
    pub current_target: Option<String>,
    pub principle_targets: Vec<String>,
    pub strategy: Option<String>,
    pub fallback_target: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone)]
struct ConfiguredXrayBalancer {
    tag: String,
    selectors: Vec<String>,
    candidates: Vec<String>,
    strategy: Option<String>,
    fallback_target: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub author: &'static str,
    pub copyright: &'static str,
    pub license: &'static str,
    pub description: &'static str,
    pub signature_status: &'static str,
}

#[derive(Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum UiIntent {
    CreateProgram,
    SelectProgram { program_id: String },
    About,
}

fn id(value: String) -> Result<ProgramId> {
    ProgramId::parse(value)
}

#[tauri::command]
pub async fn list_programs(state: State<'_, AppState>) -> Result<Vec<ProgramSummary>> {
    authorize_safety(&state, SafetyOperation::View)?;
    Ok(state.manager.list().await)
}

#[tauri::command]
pub fn get_application_info() -> ApplicationInfo {
    ApplicationInfo {
        name: "Camellia Nexus",
        version: env!("CARGO_PKG_VERSION"),
        author: crate::APP_AUTHOR,
        copyright: crate::APP_COPYRIGHT,
        license: crate::APP_LICENSE,
        description: env!("CARGO_PKG_DESCRIPTION"),
        signature_status: application_signature_status(),
    }
}

#[tauri::command]
pub fn log_frontend_event(
    state: State<'_, AppState>,
    level: String,
    message: String,
) -> Result<()> {
    if !valid_frontend_log_event(&level, &message) {
        return Err(camellia_nexus_core::CamelliaNexusError::invalid_spec(
            "Frontend log event is invalid",
        ));
    }
    let mut limiter = state
        .frontend_log_limiter
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if limiter.0.elapsed() >= Duration::from_secs(60) {
        *limiter = (Instant::now(), 0);
    }
    if limiter.1 >= 120 {
        return Err(camellia_nexus_core::CamelliaNexusError::new(
            ErrorCode::ProgramBusy,
            "Frontend logging is temporarily rate limited",
        ));
    }
    limiter.1 += 1;
    drop(limiter);
    match level.as_str() {
        "warn" => {
            tracing::warn!(target: "camellia_nexus_desktop::frontend", event = %message, "frontend event")
        }
        "debug" => {
            tracing::debug!(target: "camellia_nexus_desktop::frontend", event = %message, "frontend event")
        }
        _ => {
            tracing::info!(target: "camellia_nexus_desktop::frontend", event = %message, "frontend event")
        }
    }
    Ok(())
}

fn valid_frontend_log_event(level: &str, message: &str) -> bool {
    matches!(
        (level, message),
        ("warn", "license.state-reconcile-failed")
            | ("info", "license.runtime-sync")
            | ("warn", "license.timeout")
            | ("warn", "license.callback-failed")
            | ("info", "license.callback-polled")
            | ("info", "license.begin")
            | ("info", "license.request-ready")
            | ("warn", "license.begin-failed")
            | ("info", "license.callback-received-by-ui")
            | ("debug", "license.callback-waiting-for-idle")
            | ("info", "license.cancel")
            | ("debug", "license.complete-duplicate-ignored")
            | ("info", "license.complete-start")
            | ("debug", "license.complete-invoke")
            | ("info", "license.complete-success")
            | ("warn", "license.complete-failed")
    )
}

#[cfg(test)]
mod frontend_log_tests {
    use super::valid_frontend_log_event;

    #[test]
    fn frontend_logging_accepts_only_fixed_safe_events_and_levels() {
        for (level, event) in [
            ("warn", "license.state-reconcile-failed"),
            ("info", "license.runtime-sync"),
            ("warn", "license.timeout"),
            ("warn", "license.callback-failed"),
            ("info", "license.callback-polled"),
            ("info", "license.begin"),
            ("info", "license.request-ready"),
            ("warn", "license.begin-failed"),
            ("info", "license.callback-received-by-ui"),
            ("debug", "license.callback-waiting-for-idle"),
            ("info", "license.cancel"),
            ("debug", "license.complete-duplicate-ignored"),
            ("info", "license.complete-start"),
            ("debug", "license.complete-invoke"),
            ("info", "license.complete-success"),
            ("warn", "license.complete-failed"),
        ] {
            assert!(valid_frontend_log_event(level, event), "{level} {event}");
        }
        for (level, event) in [
            ("info", "license.timeout"),
            ("error", "license.begin"),
            ("info", "oauth-state-secret"),
            ("info", "license.callback.received.secret"),
        ] {
            assert!(!valid_frontend_log_event(level, event), "{level} {event}");
        }
    }
}

#[tauri::command]
pub async fn get_entitlement_state(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<EntitlementSnapshot> {
    let _session_operation = state.license_session_operation.lock().await;
    let event = synchronize_runtime_with_entitlement(&app, &state, "entitlement_state_read").await;
    Ok(event.snapshot)
}

#[tauri::command]
pub async fn get_local_license_device(
    state: State<'_, AppState>,
) -> Result<Option<LocalLicenseDevice>> {
    // Keep this secure-store read in the same outer queue as the rest of the
    // licensing surface. Otherwise a concurrent entitlement read can consume
    // this operation's credential-store timeout while it is only waiting.
    let _session_operation = state.license_session_operation.lock().await;
    state
        .authorization
        .device_registration_metadata()
        .await
        .map(|metadata| {
            metadata.map(|metadata| LocalLicenseDevice {
                device_id: metadata.device_id,
                display_name: metadata.display_name,
                platform: metadata.platform,
            })
        })
        .map_err(license_error)
}

#[tauri::command]
pub fn get_license_service_settings() -> crate::licensing::LicenseServiceSettings {
    crate::licensing::service_settings()
}

#[tauri::command]
pub async fn begin_license_authorization(
    app: AppHandle,
    state: State<'_, AppState>,
    open_browser: bool,
) -> Result<crate::licensing::LicenseAuthorizationRequest> {
    let _session_operation = state.license_session_operation.lock().await;
    tracing::info!(open_browser, "starting license authorization");
    let (listener, redirect_uri) = bind_loopback_authorization_callback()?;
    let session = crate::licensing::begin_loopback_license_authorization(&redirect_uri)
        .map_err(license_error)?;
    let request = session.request;
    let redirect_uri = session.redirect_uri;
    let callback_thread = AUTHORIZATION_CALLBACK_THREADS
        .try_acquire()
        .ok_or_else(|| {
            camellia_nexus_core::CamelliaNexusError::new(
                camellia_nexus_core::ErrorCode::RateLimited,
                "Too many device activation callbacks are already active",
            )
        })?;
    tracing::debug!(
        callback_mode = %request.callback_mode,
        "license authorization request prepared"
    );
    let evicted_states = state
        .pending_license_authorizations
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(
            request.state.clone(),
            session.pkce_verifier,
            redirect_uri.clone(),
        );
    {
        let mut callbacks = state
            .license_authorization_callbacks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        callbacks.remove(&request.state);
        for evicted in evicted_states {
            callbacks.remove(&evicted);
        }
    }
    if let Err(error) = spawn_loopback_authorization_callback(
        app.clone(),
        state.license_authorization_callbacks.clone(),
        state.pending_license_authorizations.clone(),
        listener,
        request.state.clone(),
        redirect_uri,
        callback_thread,
    ) {
        cancel_pending_authorization(&state, &request.state);
        return Err(error);
    }
    if open_browser {
        tracing::debug!("opening license authorization URL in system browser");
        if let Err(error) = open_system_url(&request.authorization_url) {
            let removed = cancel_pending_authorization(&state, &request.state);
            tracing::warn!(removed, %error, "could not open the license authorization URL");
            return Err(error);
        }
    }
    Ok(request)
}

#[tauri::command]
pub fn take_license_authorization_callback(
    state: State<'_, AppState>,
    authorization_state: String,
) -> Option<LicenseAuthorizationCallbackEvent> {
    let found = state
        .license_authorization_callbacks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains_key(&authorization_state);
    tracing::debug!(found, "checked pending license authorization callback");
    found.then_some(LicenseAuthorizationCallbackEvent {
        state: authorization_state,
    })
}

#[tauri::command]
pub async fn cancel_license_authorization(
    state: State<'_, AppState>,
    authorization_state: String,
) -> Result<()> {
    let _session_operation = state.license_session_operation.lock().await;
    let removed = cancel_pending_authorization(&state, &authorization_state);
    tracing::info!(removed, "cancelled license authorization");
    Ok(())
}

fn cancel_pending_authorization(state: &AppState, authorization_state: &str) -> bool {
    let removed = state
        .pending_license_authorizations
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .cancel(authorization_state);
    state
        .license_authorization_callbacks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(authorization_state);
    removed
}

fn cancel_all_pending_authorizations(state: &AppState) -> usize {
    let removed = state
        .pending_license_authorizations
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    state
        .license_authorization_callbacks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    removed.len()
}

fn pending_authorization_error(
    error: PendingAuthorizationError,
) -> camellia_nexus_core::CamelliaNexusError {
    let details = match error {
        PendingAuthorizationError::Missing => {
            "The device activation session is no longer available. Start activation again."
        }
        PendingAuthorizationError::Expired => {
            "The device activation session expired. Start activation again."
        }
        PendingAuthorizationError::CompletionInProgress => {
            "This device activation response is already being processed."
        }
    };
    license_authorization_request_error(details)
}

fn license_authorization_request_error(
    details: impl Into<String>,
) -> camellia_nexus_core::CamelliaNexusError {
    camellia_nexus_core::CamelliaNexusError::new(
        camellia_nexus_core::ErrorCode::InvalidSpec,
        "License service operation failed",
    )
    .with_details(details)
}

#[tauri::command]
pub async fn complete_license_authorization(
    app: AppHandle,
    state: State<'_, AppState>,
    expected_state: String,
    display_name: Option<String>,
) -> Result<EntitlementSnapshot> {
    tracing::info!("completing license authorization callback");
    let credentials = state
        .pending_license_authorizations
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .begin_completion(&expected_state)
        .map_err(pending_authorization_error)?;
    let callback_url = state
        .license_authorization_callbacks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&expected_state)
        .cloned();
    let _session_operation = state.license_session_operation.lock().await;
    let runtime_transition = state.runtime_authorization.transition_permit().await;
    let result = async {
        let callback_url = callback_url.ok_or_else(|| {
            license_authorization_request_error(
                "The browser has not returned a device activation response yet.",
            )
        })?;
        let callback = url::Url::parse(callback_url.expose()).map_err(|_| {
            license_authorization_request_error("The device activation callback URL is invalid.")
        })?;
        let redirect = url::Url::parse(&credentials.redirect_uri).map_err(|_| {
            license_authorization_request_error("The authorization redirect URI is invalid.")
        })?;
        let code = camellia_nexus_licensing::complete_authorization_callback(
            &callback,
            &expected_state,
            &redirect,
        )
        .map_err(license_error)?;
        tracing::debug!("license authorization callback validated");
        if !state
            .pending_license_authorizations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_active(&expected_state)
        {
            return Err(pending_authorization_error(
                PendingAuthorizationError::Missing,
            ));
        }
        activate_license_with_code(
            &state,
            code.code.clone(),
            credentials.pkce_verifier,
            code.redirect_uri.clone(),
            display_name,
        )
        .await
    }
    .await;
    let succeeded = result.is_ok();
    state
        .pending_license_authorizations
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .finish_completion(&expected_state, succeeded);
    if succeeded {
        state
            .license_authorization_callbacks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&expected_state);
    }
    let event = synchronize_runtime_with_entitlement_locked(
        &app,
        &state,
        "license_activation",
        runtime_transition,
    )
    .await;
    result.map(|_| event.snapshot)
}

async fn activate_license_with_code(
    state: &AppState,
    authorization_code: String,
    pkce_verifier: camellia_nexus_licensing::SecretValue,
    redirect_uri: String,
    display_name: Option<String>,
) -> Result<camellia_nexus_licensing::EntitlementState> {
    tracing::debug!("acquiring shared license service API client");
    let api = license_api_for_command(state, "license_activation").await?;
    let platform = current_platform_name().to_owned();
    tracing::info!(%platform, "starting license device registration");
    let registration = camellia_nexus_licensing::DeviceRegistrationFlow {
        authorization_code: camellia_nexus_licensing::SecretValue(authorization_code),
        pkce_verifier,
        redirect_uri,
        platform,
        display_name,
        local_unix: crate::licensing::unix_now(),
    };
    let had_registered_identity = state
        .authorization
        .device_registration_metadata()
        .await
        .map_err(license_error)?
        .is_some();
    let first_attempt = state
        .authorization
        .register_device_with_api(&api, registration.clone())
        .await;
    let registration_result = match first_attempt {
        Err(
            camellia_nexus_licensing::LicensingError::Network
            | camellia_nexus_licensing::LicensingError::Timeout,
        ) => {
            tracing::warn!(
                "device registration response was ambiguous; retrying the same bound authorization once"
            );
            state
                .authorization
                .register_device_with_api(&api, registration)
                .await
        }
        result => result,
    };
    if let Err(error) = registration_result {
        if let Some(conflict) =
            identity_registration_conflict_error(had_registered_identity, &error)
        {
            return Err(conflict);
        }
        let persistence_error = apply_refresh_failure_state(state, &error).await.err();
        return Err(persistence_error.unwrap_or_else(|| license_error(error)));
    }
    tracing::info!("license device registration completed");
    let now = crate::licensing::unix_now();
    if let Err(error) = resume_pending_activation(state, &api, now).await {
        if transient_license_error(&error) {
            tracing::warn!(
                %error,
                "device registration succeeded; activation will resume when the license service is reachable"
            );
            return Ok(state.authorization.state_at(now));
        }
        let persistence_error = apply_refresh_failure_state(state, &error).await.err();
        return Err(persistence_error.unwrap_or_else(|| license_error(error)));
    }
    let next_state = state.authorization.state();
    log_entitlement_state("license entitlement refresh completed", &next_state);
    Ok(next_state)
}

fn identity_registration_conflict_error(
    had_registered_identity: bool,
    error: &camellia_nexus_licensing::LicensingError,
) -> Option<camellia_nexus_core::CamelliaNexusError> {
    (had_registered_identity
        && matches!(
            error,
            camellia_nexus_licensing::LicensingError::DeviceDenied
        ))
    .then(|| {
        camellia_nexus_core::CamelliaNexusError::new(
            ErrorCode::LicenseIdentityAlreadyRegistered,
            "License service operation failed",
        )
    })
}

fn current_platform_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        platform => platform,
    }
}

async fn resume_pending_activation(
    state: &AppState,
    api: &dyn camellia_nexus_licensing::LicenseApi,
    local_unix: i64,
) -> camellia_nexus_licensing::Result<()> {
    match state
        .authorization
        .complete_pending_activation_with_api(api, local_unix)
        .await
    {
        Ok(_) => Ok(()),
        Err(
            camellia_nexus_licensing::LicensingError::AuthorizationRequired
            | camellia_nexus_licensing::LicensingError::RefreshSessionReused,
        ) => {
            state
                .authorization
                .recover_session_with_api(api, local_unix)
                .await?;
            state
                .authorization
                .complete_pending_activation_with_api(api, local_unix)
                .await
                .map(|_| ())
        }
        Err(error) => Err(error),
    }
}

async fn license_api_for_command(
    state: &AppState,
    stage: &'static str,
) -> Result<camellia_nexus_licensing::HttpLicenseApi> {
    let started = Instant::now();
    let api = state
        .license_http_api
        .get_or_try_init(|| async move {
            match tokio::time::timeout(
                Duration::from_secs(8),
                tokio::task::spawn_blocking(crate::licensing::http_api),
            )
            .await
            {
                Ok(Ok(Ok(api))) => {
                    tracing::debug!(
                        %stage,
                        elapsed_ms = started.elapsed().as_millis(),
                        "shared license service API client initialized"
                    );
                    Ok(api)
                }
                Ok(Ok(Err(error))) => {
                    tracing::warn!(%stage, %error, "license service API client could not be created");
                    Err(license_error(error))
                }
                Ok(Err(error)) => {
                    tracing::warn!(%stage, %error, "license service API client task failed");
                    Err(camellia_nexus_core::CamelliaNexusError::new(
                        camellia_nexus_core::ErrorCode::SystemIntegration,
                        "License service operation failed",
                    )
                    .with_details("The license service client could not be initialized."))
                }
                Err(_) => {
                    tracing::warn!(
                        %stage,
                        elapsed_ms = started.elapsed().as_millis(),
                        "license service API client initialization timed out"
                    );
                    Err(camellia_nexus_core::CamelliaNexusError::new(
                        camellia_nexus_core::ErrorCode::Timeout,
                        "License service operation failed",
                    )
                    .with_details("The license service client did not initialize in time."))
                }
            }
        })
        .await?;
    tracing::trace!(%stage, "shared license service API client acquired");
    Ok(api.clone())
}

fn open_system_url(url: &str) -> Result<()> {
    let parsed = url::Url::parse(url).map_err(|_| {
        camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::InvalidSpec,
            "License service operation failed",
        )
        .with_details("The authorization URL is invalid.")
    })?;
    if parsed.scheme() != "https"
        && !(parsed.scheme() == "http"
            && matches!(
                parsed.host_str(),
                Some("127.0.0.1" | "localhost" | "::1" | "[::1]")
            ))
    {
        return Err(camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::InvalidSpec,
            "License service operation failed",
        )
        .with_details("The authorization URL must use HTTPS or loopback HTTP."));
    }

    open_external(OsStr::new(parsed.as_str())).map_err(|error| {
        camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::SystemIntegration,
            "License service operation failed",
        )
        .with_details(error.to_string())
    })
}

fn bind_loopback_authorization_callback() -> Result<(TcpListener, String)> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|error| {
        camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::SystemIntegration,
            "License service operation failed",
        )
        .with_details(format!(
            "The local activation callback could not be started: {error}"
        ))
    })?;
    let port = listener
        .local_addr()
        .map_err(|error| {
            camellia_nexus_core::CamelliaNexusError::new(
                camellia_nexus_core::ErrorCode::SystemIntegration,
                "License service operation failed",
            )
            .with_details(format!(
                "The local activation callback address is unavailable: {error}"
            ))
        })?
        .port();
    listener.set_nonblocking(true).map_err(|error| {
        camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::SystemIntegration,
            "License service operation failed",
        )
        .with_details(format!(
            "The local activation callback could not be prepared: {error}"
        ))
    })?;
    let redirect_uri = format!("http://127.0.0.1:{port}/auth/callback");
    tracing::debug!(%redirect_uri, "license loopback callback listener bound");
    Ok((listener, redirect_uri))
}

fn spawn_loopback_authorization_callback(
    app: AppHandle,
    callbacks: Arc<Mutex<BTreeMap<String, SecretValue>>>,
    pending: Arc<Mutex<PendingAuthorizationStore>>,
    listener: TcpListener,
    expected_state: String,
    redirect_uri: String,
    callback_thread: CallbackThreadPermit<'static>,
) -> Result<()> {
    std::thread::Builder::new()
        .name("camellia-license-callback".to_owned())
        .spawn(move || {
            let _callback_thread = callback_thread;
            run_loopback_authorization_callback(
                app,
                callbacks,
                pending,
                listener,
                expected_state,
                redirect_uri,
            );
        })
        .map(|_| ())
        .map_err(|error| {
            camellia_nexus_core::CamelliaNexusError::new(
                camellia_nexus_core::ErrorCode::SystemIntegration,
                "The device activation callback could not be started",
            )
            .with_details(error.to_string())
        })
}

fn run_loopback_authorization_callback(
    app: AppHandle,
    callbacks: Arc<Mutex<BTreeMap<String, SecretValue>>>,
    pending: Arc<Mutex<PendingAuthorizationStore>>,
    listener: TcpListener,
    expected_state: String,
    redirect_uri: String,
) {
    tracing::debug!("license loopback callback listener started");
    let deadline = Instant::now() + LICENSE_AUTHORIZATION_CALLBACK_TIMEOUT;
    loop {
        if !pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_active(&expected_state)
        {
            tracing::debug!("license loopback callback listener stopped");
            break;
        }
        if Instant::now() >= deadline {
            tracing::warn!("license loopback callback listener timed out");
            if pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .cancel(&expected_state)
            {
                emit_license_authorization_failure(
                    &app,
                    &expected_state,
                    "Device activation timed out before the browser returned to Camellia Nexus.",
                );
            }
            break;
        }
        match listener.accept() {
            Ok((mut stream, peer)) => {
                tracing::debug!(peer = %peer, "license loopback callback connection accepted");
                if handle_loopback_authorization_connection(
                    &app,
                    &callbacks,
                    &pending,
                    &mut stream,
                    &expected_state,
                    &redirect_uri,
                ) {
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                tracing::warn!(%error, "license loopback callback listener failed");
                emit_license_authorization_failure(
                    &app,
                    &expected_state,
                    format!("The local activation callback failed: {error}"),
                );
                pending
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .cancel(&expected_state);
                break;
            }
        }
    }
}

fn handle_loopback_authorization_connection(
    app: &AppHandle,
    callbacks: &Arc<Mutex<BTreeMap<String, SecretValue>>>,
    pending: &Arc<Mutex<PendingAuthorizationStore>>,
    stream: &mut std::net::TcpStream,
    expected_state: &str,
    redirect_uri: &str,
) -> bool {
    if let Err(error) = stream.set_nonblocking(false) {
        tracing::warn!(%error, "could not prepare license authorization callback connection");
        return false;
    }
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let request_line = match read_loopback_request_line(stream) {
        Ok(line) => line,
        Err(error) => {
            tracing::warn!(%error, "could not read license authorization callback");
            let _ = write_loopback_response(stream, StatusLine::BadRequest);
            return false;
        }
    };
    let Some(target) = loopback_request_target(&request_line) else {
        let _ = write_loopback_response(stream, StatusLine::BadRequest);
        return false;
    };
    let Ok(incoming) = url::Url::parse(&format!("http://127.0.0.1{target}")) else {
        let _ = write_loopback_response(stream, StatusLine::BadRequest);
        return false;
    };
    if incoming.path() != "/auth/callback" {
        tracing::debug!(path = %incoming.path(), "ignored non-authorization loopback request");
        let _ = write_loopback_response(stream, StatusLine::NotFound);
        return false;
    }
    let Ok(redirect) = url::Url::parse(redirect_uri) else {
        let _ = write_loopback_response(stream, StatusLine::ServerError);
        emit_license_authorization_failure(
            app,
            expected_state,
            "The local activation callback configuration is invalid.",
        );
        pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cancel(expected_state);
        return true;
    };
    let mut callback = redirect.clone();
    callback.set_query(incoming.query());
    callback.set_fragment(None);
    let callback_url = callback.to_string();
    let callback_is_valid = camellia_nexus_licensing::complete_authorization_callback(
        &callback,
        expected_state,
        &redirect,
    )
    .is_ok();
    if callback_is_valid {
        // Keep the lock order consistent with cancellation: session first, callback second. This
        // prevents a callback containing an authorization code from being reinserted after the
        // user has cancelled or the session has expired.
        let stored = {
            let mut pending = pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !pending.contains_active(expected_state) {
                false
            } else {
                let mut callbacks = callbacks
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                callbacks.insert(expected_state.to_owned(), SecretValue(callback_url));
                while callbacks.len() > 16 {
                    callbacks.pop_first();
                }
                true
            }
        };
        let _ = write_loopback_response(
            stream,
            if stored {
                StatusLine::Ok
            } else {
                StatusLine::BadRequest
            },
        );
        if !stored {
            tracing::debug!("ignored callback for inactive license authorization");
            return true;
        }
        tracing::info!("license authorization callback received");
        let event = LicenseAuthorizationCallbackEvent {
            state: expected_state.to_owned(),
        };
        if let Err(error) = app.emit("license-authorization-callback", event) {
            tracing::warn!(%error, "could not emit license authorization callback to frontend");
        } else {
            tracing::debug!("license authorization callback emitted to frontend");
        }
    } else {
        let _ = write_loopback_response(stream, StatusLine::BadRequest);
        tracing::warn!("ignored invalid license authorization callback");
        return false;
    }
    true
}

const MAX_LOOPBACK_REQUEST_LINE_BYTES: usize = 4 * 1024;

fn read_loopback_request_line(reader: &mut impl Read) -> std::io::Result<String> {
    let mut line = Vec::with_capacity(256);
    let mut chunk = [0_u8; 512];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "HTTP request ended before its request line",
            ));
        }
        line.extend_from_slice(&chunk[..read]);
        if line.len() > MAX_LOOPBACK_REQUEST_LINE_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "HTTP request line is too long",
            ));
        }
        if let Some(end) = line.windows(2).position(|bytes| bytes == b"\r\n") {
            line.truncate(end);
            return String::from_utf8(line).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "HTTP request line is not UTF-8",
                )
            });
        }
    }
}

fn loopback_request_target(request_line: &str) -> Option<&str> {
    let mut parts = request_line.split(' ');
    let method = parts.next()?;
    let target = parts.next()?;
    let version = parts.next()?;
    (method == "GET"
        && target.starts_with('/')
        && matches!(version, "HTTP/1.0" | "HTTP/1.1")
        && parts.next().is_none())
    .then_some(target)
}

#[cfg(test)]
mod loopback_callback_tests {
    use std::io::{Cursor, Read};

    use super::{CallbackThreadLimiter, loopback_request_target, read_loopback_request_line};

    struct FragmentedReader {
        inner: Cursor<Vec<u8>>,
        maximum_chunk: usize,
    }

    impl Read for FragmentedReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let length = buffer.len().min(self.maximum_chunk);
            self.inner.read(&mut buffer[..length])
        }
    }

    #[test]
    fn fragmented_loopback_request_line_is_read_to_crlf() {
        let mut reader = FragmentedReader {
            inner: Cursor::new(
                b"GET /auth/callback?code=abc&state=expected HTTP/1.1\r\nHost: 127.0.0.1\r\n"
                    .to_vec(),
            ),
            maximum_chunk: 3,
        };
        let line = read_loopback_request_line(&mut reader).expect("request line");
        assert_eq!(
            loopback_request_target(&line),
            Some("/auth/callback?code=abc&state=expected")
        );
    }

    #[test]
    fn loopback_request_line_parser_is_strict() {
        for invalid in [
            "POST /auth/callback HTTP/1.1",
            "GET http://example.test/ HTTP/1.1",
            "GET /auth/callback HTTP/2",
            "GET  /auth/callback HTTP/1.1",
            "GET /auth/callback HTTP/1.1 extra",
        ] {
            assert_eq!(loopback_request_target(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn callback_thread_limiter_releases_capacity() {
        let limiter = CallbackThreadLimiter::new(2);
        let first = limiter.try_acquire().expect("first callback");
        let second = limiter.try_acquire().expect("second callback");
        assert!(limiter.try_acquire().is_none());
        drop(first);
        let replacement = limiter.try_acquire().expect("released callback capacity");
        assert!(limiter.try_acquire().is_none());
        drop((second, replacement));
        assert!(limiter.try_acquire().is_some());
    }
}

enum StatusLine {
    Ok,
    BadRequest,
    NotFound,
    ServerError,
}

fn write_loopback_response(
    stream: &mut std::net::TcpStream,
    status: StatusLine,
) -> std::io::Result<()> {
    let (line, title, message) = match status {
        StatusLine::Ok => (
            "HTTP/1.1 200 OK",
            "Activation complete",
            "Camellia Nexus is completing device activation. You can return to the desktop app.",
        ),
        StatusLine::BadRequest => (
            "HTTP/1.1 400 Bad Request",
            "Activation could not continue",
            "Return to Camellia Nexus and start device activation again.",
        ),
        StatusLine::NotFound => (
            "HTTP/1.1 404 Not Found",
            "Not found",
            "This local endpoint only handles Camellia Nexus device activation.",
        ),
        StatusLine::ServerError => (
            "HTTP/1.1 500 Internal Server Error",
            "Activation could not continue",
            "Return to Camellia Nexus and start device activation again.",
        ),
    };
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{title}</title><style>body{{margin:0;min-height:100vh;display:grid;place-items:center;background:#f5f5f7;color:#16181d;font-family:\"Segoe UI\",system-ui,sans-serif}}main{{max-width:460px;padding:32px;border:1px solid rgba(0,0,0,.12);border-radius:18px;background:white;box-shadow:0 18px 60px rgba(0,0,0,.16)}}h1{{margin:0 0 10px;font-size:22px}}p{{margin:0;color:#596170;line-height:1.5}}@media (prefers-color-scheme:dark){{body{{background:#121418;color:#f4f6f8}}main{{background:#1c2027;border-color:rgba(255,255,255,.14)}}p{{color:#aab2bf}}}}</style></head><body><main><h1>{title}</h1><p>{message}</p></main></body></html>"
    );
    let response = format!(
        "{line}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nPragma: no-cache\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())
}

fn emit_license_authorization_failure(app: &AppHandle, state: &str, message: impl Into<String>) {
    let message = message.into();
    tracing::warn!(%message, "license authorization failed");
    let _ = app.emit(
        "license-authorization-failed",
        LicenseAuthorizationFailedEvent {
            state: state.to_owned(),
            message,
        },
    );
}

#[tauri::command]
pub async fn refresh_license_entitlement(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<EntitlementSnapshot> {
    tracing::info!("refreshing license entitlement");
    let api = license_api_for_command(&state, "license_refresh").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let runtime_transition = state.runtime_authorization.transition_permit().await;
    let now = crate::licensing::unix_now();
    let pending_activation = matches!(
        state.authorization.state_at(now),
        EntitlementState::ActivationPending
    );
    let result = if pending_activation {
        resume_pending_activation(&state, &api, now).await
    } else {
        state
            .authorization
            .refresh_entitlement_with_api(&api, "entitlement:refresh", now)
            .await
            .map(|_| ())
    };
    let result = match result {
        Err(
            camellia_nexus_licensing::LicensingError::AuthorizationRequired
            | camellia_nexus_licensing::LicensingError::RefreshSessionReused,
        ) if !pending_activation => recover_and_refresh_license(&state, &api, now).await,
        result => result,
    };
    if let Err(error) = result {
        let persistence_error = apply_refresh_failure_state(&state, &error).await.err();
        synchronize_runtime_with_entitlement_locked(
            &app,
            &state,
            "entitlement_refresh_failed",
            runtime_transition,
        )
        .await;
        return Err(persistence_error.unwrap_or_else(|| license_error(error)));
    }
    let next_state = state.authorization.state_at(crate::licensing::unix_now());
    log_entitlement_state("license entitlement refreshed", &next_state);
    let event = synchronize_runtime_with_entitlement_locked(
        &app,
        &state,
        "entitlement_refresh",
        runtime_transition,
    )
    .await;
    Ok(event.snapshot)
}

#[tauri::command]
pub async fn reconnect_license_device(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<EntitlementSnapshot> {
    let api = license_api_for_command(&state, "license_device_reconnect").await?;
    let _session_operation = state.license_session_operation.lock().await;
    if state
        .authorization
        .device_registration_metadata()
        .await
        .map_err(license_error)?
        .is_none()
    {
        return Err(camellia_nexus_core::CamelliaNexusError::new(
            ErrorCode::LicenseRequired,
            "No registered local device identity is available",
        ));
    }
    let runtime_transition = state.runtime_authorization.transition_permit().await;
    let now = crate::licensing::unix_now();
    if let Err(error) = recover_and_refresh_license(&state, &api, now).await {
        let persistence_error = apply_refresh_failure_state(&state, &error).await.err();
        synchronize_runtime_with_entitlement_locked(
            &app,
            &state,
            "license_device_reconnect_failed",
            runtime_transition,
        )
        .await;
        return Err(persistence_error.unwrap_or_else(|| license_error(error)));
    }
    let event = synchronize_runtime_with_entitlement_locked(
        &app,
        &state,
        "license_device_reconnected",
        runtime_transition,
    )
    .await;
    Ok(event.snapshot)
}

async fn recover_and_refresh_license(
    state: &AppState,
    api: &dyn camellia_nexus_licensing::LicenseApi,
    local_unix: i64,
) -> camellia_nexus_licensing::Result<()> {
    state
        .authorization
        .recover_session_with_api(api, local_unix)
        .await?;
    state
        .authorization
        .refresh_entitlement_with_api(api, "entitlement:refresh", local_unix)
        .await
        .map(|_| ())
}

async fn apply_refresh_failure_state(
    state: &AppState,
    error: &camellia_nexus_licensing::LicensingError,
) -> Result<()> {
    match error {
        camellia_nexus_licensing::LicensingError::RefreshSessionReused => {
            state
                .authorization
                .clear_session()
                .await
                .map_err(license_error)?;
        }
        camellia_nexus_licensing::LicensingError::DeviceActivationPending => {
            state
                .authorization
                .apply_device_state_persisted(
                    DeviceState::PendingActivation,
                    crate::licensing::unix_now(),
                )
                .await
                .map_err(license_error)?;
        }
        camellia_nexus_licensing::LicensingError::DeviceDenied
        | camellia_nexus_licensing::LicensingError::DeviceRevoked => {
            state
                .authorization
                .apply_device_state_persisted(DeviceState::Revoked, crate::licensing::unix_now())
                .await
                .map_err(license_error)?;
        }
        camellia_nexus_licensing::LicensingError::DeviceRemoved => {
            state
                .authorization
                .apply_device_state_persisted(DeviceState::Removed, crate::licensing::unix_now())
                .await
                .map_err(license_error)?;
        }
        camellia_nexus_licensing::LicensingError::DeviceSuspicious => {
            state
                .authorization
                .apply_device_state_persisted(DeviceState::Suspicious, crate::licensing::unix_now())
                .await
                .map_err(license_error)?;
        }
        camellia_nexus_licensing::LicensingError::AuthorizationRequired => {
            state
                .authorization
                .clear_session()
                .await
                .map_err(license_error)?;
        }
        camellia_nexus_licensing::LicensingError::ActivationCodeInvalid
        | camellia_nexus_licensing::LicensingError::ActivationCodeExpired
        | camellia_nexus_licensing::LicensingError::ActivationCodeConsumed
        | camellia_nexus_licensing::LicensingError::ActivationCodeRevoked
        | camellia_nexus_licensing::LicensingError::ActivationPendingExpired => {
            // These failures cannot become successful by retrying the same pending session.
            // Persist fail-closed before clearing it so a crash cannot resurrect a permanently
            // unusable activation; the device identity is intentionally retained for a new code.
            state
                .authorization
                .deauthorize_locally(crate::licensing::unix_now())
                .await
                .map_err(license_error)?;
        }
        camellia_nexus_licensing::LicensingError::AccountSuspended => {
            state
                .authorization
                .apply_license_inactive(
                    camellia_nexus_licensing::LicenseInactiveReason::AccountSuspended,
                    crate::licensing::unix_now(),
                )
                .await
                .map_err(license_error)?;
        }
        camellia_nexus_licensing::LicensingError::AccountDenylisted => {
            state
                .authorization
                .apply_license_inactive(
                    camellia_nexus_licensing::LicenseInactiveReason::AccountDenylisted,
                    crate::licensing::unix_now(),
                )
                .await
                .map_err(license_error)?;
        }
        camellia_nexus_licensing::LicensingError::LicensePastDue => {
            state
                .authorization
                .apply_license_inactive(
                    camellia_nexus_licensing::LicenseInactiveReason::LicensePastDue,
                    crate::licensing::unix_now(),
                )
                .await
                .map_err(license_error)?;
        }
        camellia_nexus_licensing::LicensingError::LicenseCanceled => {
            state
                .authorization
                .apply_license_inactive(
                    camellia_nexus_licensing::LicenseInactiveReason::LicenseCanceled,
                    crate::licensing::unix_now(),
                )
                .await
                .map_err(license_error)?;
        }
        camellia_nexus_licensing::LicensingError::LicenseExpired => {
            state
                .authorization
                .apply_license_inactive(
                    camellia_nexus_licensing::LicenseInactiveReason::LicenseExpired,
                    crate::licensing::unix_now(),
                )
                .await
                .map_err(license_error)?;
        }
        camellia_nexus_licensing::LicensingError::LicenseUnavailable => {
            state
                .authorization
                .apply_license_inactive(
                    camellia_nexus_licensing::LicenseInactiveReason::LicenseUnavailable,
                    crate::licensing::unix_now(),
                )
                .await
                .map_err(license_error)?;
        }
        camellia_nexus_licensing::LicensingError::ClientUpgradeRequired { policy } => {
            state
                .authorization
                .apply_client_upgrade_required(policy.clone(), crate::licensing::unix_now())
                .await
                .map_err(license_error)?;
        }
        camellia_nexus_licensing::LicensingError::EntitlementExpired => {
            state
                .authorization
                .apply_revalidation_required(
                    camellia_nexus_licensing::RevalidationReason::InvalidServerProof,
                    crate::licensing::unix_now(),
                )
                .await
                .map_err(license_error)?;
        }
        camellia_nexus_licensing::LicensingError::ClockRollback => {
            state
                .authorization
                .apply_revalidation_required(
                    camellia_nexus_licensing::RevalidationReason::ClockRollback,
                    crate::licensing::unix_now(),
                )
                .await
                .map_err(license_error)?;
        }
        camellia_nexus_licensing::LicensingError::ObsoleteLicenseEpoch => {
            state
                .authorization
                .apply_revalidation_required(
                    camellia_nexus_licensing::RevalidationReason::ObsoleteEpoch,
                    crate::licensing::unix_now(),
                )
                .await
                .map_err(license_error)?;
        }
        camellia_nexus_licensing::LicensingError::DeviceIdentityUnavailable
        | camellia_nexus_licensing::LicensingError::SecureStoreUnavailable
        | camellia_nexus_licensing::LicensingError::SecureStoreBackend
        | camellia_nexus_licensing::LicensingError::SecureStoreCorrupt => {
            state
                .authorization
                .apply_revalidation_required(
                    camellia_nexus_licensing::RevalidationReason::CorruptSecureStore,
                    crate::licensing::unix_now(),
                )
                .await
                .map_err(license_error)?;
        }
        camellia_nexus_licensing::LicensingError::MalformedEntitlement
        | camellia_nexus_licensing::LicensingError::InvalidSignature
        | camellia_nexus_licensing::LicensingError::UnsupportedAlgorithm
        | camellia_nexus_licensing::LicensingError::UnknownSigningKey
        | camellia_nexus_licensing::LicensingError::WrongIssuer
        | camellia_nexus_licensing::LicensingError::WrongAudience
        | camellia_nexus_licensing::LicensingError::DeviceMismatch
        | camellia_nexus_licensing::LicensingError::DeviceKeyMismatch
        | camellia_nexus_licensing::LicensingError::InvalidClaims => {
            state
                .authorization
                .apply_revalidation_required(
                    camellia_nexus_licensing::RevalidationReason::InvalidServerProof,
                    crate::licensing::unix_now(),
                )
                .await
                .map_err(license_error)?;
        }
        _ => {}
    }
    Ok(())
}

#[tauri::command]
pub async fn get_license_devices(
    app: AppHandle,
    state: State<'_, AppState>,
    cursor: Option<String>,
    page_size: Option<u32>,
) -> Result<camellia_nexus_licensing::RegisteredDevicePage> {
    tracing::debug!("loading registered license devices");
    let api = license_api_for_command(&state, "license_devices").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .list_devices_with_api(
            &api,
            cursor.as_deref(),
            page_size.unwrap_or(50),
            crate::licensing::unix_now(),
        )
        .await;
    license_api_result(&app, &state, result, "license_device_list_denied").await
}

#[tauri::command]
pub async fn get_license_billing_summary(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<camellia_nexus_licensing::BillingSummary> {
    tracing::debug!("loading license billing summary");
    let api = license_api_for_command(&state, "license_billing_summary").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .billing_summary_with_api(&api, crate::licensing::unix_now())
        .await;
    license_api_result(&app, &state, result, "license_billing_denied").await
}

#[tauri::command]
pub async fn submit_license_payment_claim(
    app: AppHandle,
    state: State<'_, AppState>,
    submission: camellia_nexus_licensing::CustomerPaymentSubmission,
) -> Result<camellia_nexus_licensing::ManualPaymentClaim> {
    tracing::info!("submitting manual license payment claim");
    let api = license_api_for_command(&state, "license_payment_claim").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .submit_customer_payment_with_api(&api, submission, crate::licensing::unix_now())
        .await;
    license_api_result(&app, &state, result, "license_payment_denied").await
}

#[tauri::command]
pub async fn get_license_team_profile(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<camellia_nexus_licensing::TeamProfile> {
    let api = license_api_for_command(&state, "license_team_profile").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .team_profile_with_api(&api, crate::licensing::unix_now())
        .await;
    license_api_result(&app, &state, result, "license_team_profile_denied").await
}

#[tauri::command]
pub async fn get_license_team_members(
    app: AppHandle,
    state: State<'_, AppState>,
    request: camellia_nexus_licensing::TeamMemberPageRequest,
) -> Result<camellia_nexus_licensing::TeamMemberPage> {
    let api = license_api_for_command(&state, "license_team_members").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .team_members_with_api(&api, request, crate::licensing::unix_now())
        .await;
    license_api_result(&app, &state, result, "license_team_members_denied").await
}

#[tauri::command]
pub async fn create_license_team_invitation(
    app: AppHandle,
    state: State<'_, AppState>,
    request: camellia_nexus_licensing::CreateTeamInvitation,
) -> Result<camellia_nexus_licensing::TeamInvitation> {
    let api = license_api_for_command(&state, "license_team_invitation").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .create_team_invitation_with_api(&api, request, crate::licensing::unix_now())
        .await;
    license_api_result(&app, &state, result, "license_team_invitation_denied").await
}

#[tauri::command]
pub async fn accept_license_team_invitation(
    app: AppHandle,
    state: State<'_, AppState>,
    request: camellia_nexus_licensing::AcceptTeamInvitation,
) -> Result<camellia_nexus_licensing::TeamProfile> {
    let api = license_api_for_command(&state, "license_team_invitation_accept").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .accept_team_invitation_with_api(&api, request, crate::licensing::unix_now())
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_team_invitation_accept_denied",
    )
    .await
}

#[tauri::command]
pub async fn update_license_team_member(
    app: AppHandle,
    state: State<'_, AppState>,
    member_id: String,
    request: camellia_nexus_licensing::UpdateWorkspaceMember,
) -> Result<camellia_nexus_licensing::WorkspaceMember> {
    let api = license_api_for_command(&state, "license_team_member_update").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .update_team_member_with_api(&api, &member_id, request, crate::licensing::unix_now())
        .await;
    license_api_result(&app, &state, result, "license_team_member_update_denied").await
}

#[tauri::command]
pub async fn create_license_team_device_enrollment(
    app: AppHandle,
    state: State<'_, AppState>,
    request: camellia_nexus_licensing::TeamOperationRequest,
) -> Result<camellia_nexus_licensing::MemberDeviceEnrollment> {
    let api = license_api_for_command(&state, "license_team_device_enrollment").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .create_team_device_enrollment_with_api(&api, request, crate::licensing::unix_now())
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_team_device_enrollment_denied",
    )
    .await
}

#[tauri::command]
pub async fn create_license_team_member_device_enrollment(
    app: AppHandle,
    state: State<'_, AppState>,
    member_id: String,
    request: camellia_nexus_licensing::TeamOperationRequest,
) -> Result<camellia_nexus_licensing::MemberDeviceEnrollment> {
    let api = license_api_for_command(&state, "license_team_member_device_enrollment").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .create_team_member_device_enrollment_with_api(
            &api,
            &member_id,
            request,
            crate::licensing::unix_now(),
        )
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_team_member_device_enrollment_denied",
    )
    .await
}

#[tauri::command]
pub async fn accept_license_team_device_enrollment(
    app: AppHandle,
    state: State<'_, AppState>,
    request: camellia_nexus_licensing::AcceptMemberDeviceEnrollment,
) -> Result<camellia_nexus_licensing::TeamProfile> {
    let api = license_api_for_command(&state, "license_team_device_enrollment_accept").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .accept_team_device_enrollment_with_api(&api, request, crate::licensing::unix_now())
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_team_device_enrollment_accept_denied",
    )
    .await
}

#[tauri::command]
pub async fn leave_license_team_workspace(
    app: AppHandle,
    state: State<'_, AppState>,
    request: camellia_nexus_licensing::LeaveWorkspace,
) -> Result<()> {
    let api = license_api_for_command(&state, "license_team_leave").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let remote_result = state
        .authorization
        .leave_team_workspace_with_api(&api, request, crate::licensing::unix_now())
        .await;
    license_api_result(&app, &state, remote_result, "license_team_leave_denied").await?;
    let runtime_transition = state.runtime_authorization.transition_permit().await;
    let local_result = state
        .authorization
        .deauthorize_locally(crate::licensing::unix_now())
        .await;
    synchronize_runtime_with_entitlement_locked(
        &app,
        &state,
        "license_team_workspace_left",
        runtime_transition,
    )
    .await;
    local_result.map_err(license_error)?;
    Ok(())
}

#[tauri::command]
pub async fn transfer_license_team_ownership(
    app: AppHandle,
    state: State<'_, AppState>,
    request: camellia_nexus_licensing::TransferWorkspaceOwnership,
) -> Result<camellia_nexus_licensing::OwnershipTransferResult> {
    let api = license_api_for_command(&state, "license_team_ownership_transfer").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .transfer_team_ownership_with_api(&api, request, crate::licensing::unix_now())
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_team_ownership_transfer_denied",
    )
    .await
}

#[tauri::command]
pub async fn get_license_workspace_configurations(
    app: AppHandle,
    state: State<'_, AppState>,
    request: camellia_nexus_licensing::SharedConfigurationPageRequest,
) -> Result<camellia_nexus_licensing::SharedConfigurationPage> {
    let api = license_api_for_command(&state, "license_workspace_configurations_list").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .shared_configurations_with_api(&api, request, crate::licensing::unix_now())
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_configurations_list_denied",
    )
    .await
}

#[tauri::command]
pub async fn get_license_workspace_configuration(
    app: AppHandle,
    state: State<'_, AppState>,
    document_id: String,
    request: camellia_nexus_licensing::SharedConfigurationContentRequest,
) -> Result<camellia_nexus_licensing::SharedConfigurationContent> {
    let api = license_api_for_command(&state, "license_workspace_configuration_read").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .shared_configuration_content_with_api(
            &api,
            &document_id,
            request,
            crate::licensing::unix_now(),
        )
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_configuration_read_denied",
    )
    .await
}

#[tauri::command]
pub async fn create_license_workspace_configuration(
    app: AppHandle,
    state: State<'_, AppState>,
    request: camellia_nexus_licensing::CreateSharedConfiguration,
) -> Result<camellia_nexus_licensing::WorkspaceMutationReceipt> {
    let api = license_api_for_command(&state, "license_workspace_configuration_create").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .create_shared_configuration_with_api(&api, request, crate::licensing::unix_now())
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_configuration_create_denied",
    )
    .await
}

#[tauri::command]
pub async fn revise_license_workspace_configuration(
    app: AppHandle,
    state: State<'_, AppState>,
    document_id: String,
    request: camellia_nexus_licensing::ReviseSharedConfiguration,
) -> Result<camellia_nexus_licensing::WorkspaceMutationReceipt> {
    let api = license_api_for_command(&state, "license_workspace_configuration_revise").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .revise_shared_configuration_with_api(
            &api,
            &document_id,
            request,
            crate::licensing::unix_now(),
        )
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_configuration_revise_denied",
    )
    .await
}

#[tauri::command]
pub async fn publish_license_workspace_configuration(
    app: AppHandle,
    state: State<'_, AppState>,
    document_id: String,
    request: camellia_nexus_licensing::PublishSharedConfiguration,
) -> Result<camellia_nexus_licensing::WorkspaceMutationReceipt> {
    let api = license_api_for_command(&state, "license_workspace_configuration_publish").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .publish_shared_configuration_with_api(
            &api,
            &document_id,
            request,
            crate::licensing::unix_now(),
        )
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_configuration_publish_denied",
    )
    .await
}

#[tauri::command]
pub async fn delete_license_workspace_configuration(
    app: AppHandle,
    state: State<'_, AppState>,
    document_id: String,
    request: camellia_nexus_licensing::VersionedWorkspaceMutation,
) -> Result<camellia_nexus_licensing::WorkspaceMutationReceipt> {
    let api = license_api_for_command(&state, "license_workspace_configuration_delete").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .delete_shared_configuration_with_api(
            &api,
            &document_id,
            request,
            crate::licensing::unix_now(),
        )
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_configuration_delete_denied",
    )
    .await
}

#[tauri::command]
pub async fn restore_license_workspace_configuration(
    app: AppHandle,
    state: State<'_, AppState>,
    document_id: String,
    request: camellia_nexus_licensing::VersionedWorkspaceMutation,
) -> Result<camellia_nexus_licensing::WorkspaceMutationReceipt> {
    let api = license_api_for_command(&state, "license_workspace_configuration_restore").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .restore_shared_configuration_with_api(
            &api,
            &document_id,
            request,
            crate::licensing::unix_now(),
        )
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_configuration_restore_denied",
    )
    .await
}

#[tauri::command]
pub async fn purge_license_workspace_configuration(
    app: AppHandle,
    state: State<'_, AppState>,
    document_id: String,
    request: camellia_nexus_licensing::VersionedWorkspaceMutation,
) -> Result<camellia_nexus_licensing::WorkspaceMutationReceipt> {
    let api = license_api_for_command(&state, "license_workspace_configuration_purge").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .purge_shared_configuration_with_api(
            &api,
            &document_id,
            request,
            crate::licensing::unix_now(),
        )
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_configuration_purge_denied",
    )
    .await
}

#[tauri::command]
pub async fn get_license_workspace_sync_feed(
    app: AppHandle,
    state: State<'_, AppState>,
    request: camellia_nexus_licensing::WorkspaceSyncFeedRequest,
) -> Result<camellia_nexus_licensing::WorkspaceSyncFeed> {
    let api = license_api_for_command(&state, "license_workspace_sync_feed").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .workspace_sync_feed_with_api(&api, request, crate::licensing::unix_now())
        .await;
    license_api_result(&app, &state, result, "license_workspace_sync_feed_denied").await
}

#[tauri::command]
pub async fn get_license_workspace_checkpoint(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<camellia_nexus_licensing::WorkspaceDeviceCheckpoint>> {
    let api = license_api_for_command(&state, "license_workspace_checkpoint").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .workspace_checkpoint_with_api(&api, crate::licensing::unix_now())
        .await;
    license_api_result(&app, &state, result, "license_workspace_checkpoint_denied").await
}

#[tauri::command]
pub async fn advance_license_workspace_checkpoint(
    app: AppHandle,
    state: State<'_, AppState>,
    request: camellia_nexus_licensing::AdvanceWorkspaceCheckpoint,
) -> Result<camellia_nexus_licensing::WorkspaceMutationReceipt> {
    let api = license_api_for_command(&state, "license_workspace_checkpoint_advance").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .advance_workspace_checkpoint_with_api(&api, request, crate::licensing::unix_now())
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_checkpoint_advance_denied",
    )
    .await
}

#[tauri::command]
pub async fn get_license_workspace_alert_rules(
    app: AppHandle,
    state: State<'_, AppState>,
    request: camellia_nexus_licensing::WorkspaceAlertRulePageRequest,
) -> Result<camellia_nexus_licensing::WorkspaceAlertRulePage> {
    let api = license_api_for_command(&state, "license_workspace_alert_rules_list").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .workspace_alert_rules_with_api(&api, request, crate::licensing::unix_now())
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_alert_rules_list_denied",
    )
    .await
}

#[tauri::command]
pub async fn create_license_workspace_alert_rule(
    app: AppHandle,
    state: State<'_, AppState>,
    request: camellia_nexus_licensing::CreateWorkspaceAlertRule,
) -> Result<camellia_nexus_licensing::WorkspaceMutationReceipt> {
    let api = license_api_for_command(&state, "license_workspace_alert_rule_create").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .create_workspace_alert_rule_with_api(&api, request, crate::licensing::unix_now())
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_alert_rule_create_denied",
    )
    .await
}

#[tauri::command]
pub async fn update_license_workspace_alert_rule(
    app: AppHandle,
    state: State<'_, AppState>,
    rule_id: String,
    request: camellia_nexus_licensing::UpdateWorkspaceAlertRule,
) -> Result<camellia_nexus_licensing::WorkspaceMutationReceipt> {
    let api = license_api_for_command(&state, "license_workspace_alert_rule_update").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .update_workspace_alert_rule_with_api(&api, &rule_id, request, crate::licensing::unix_now())
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_alert_rule_update_denied",
    )
    .await
}

#[tauri::command]
pub async fn delete_license_workspace_alert_rule(
    app: AppHandle,
    state: State<'_, AppState>,
    rule_id: String,
    request: camellia_nexus_licensing::VersionedWorkspaceMutation,
) -> Result<camellia_nexus_licensing::WorkspaceMutationReceipt> {
    let api = license_api_for_command(&state, "license_workspace_alert_rule_delete").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .delete_workspace_alert_rule_with_api(&api, &rule_id, request, crate::licensing::unix_now())
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_alert_rule_delete_denied",
    )
    .await
}

#[tauri::command]
pub async fn get_license_workspace_alert_incidents(
    app: AppHandle,
    state: State<'_, AppState>,
    request: camellia_nexus_licensing::WorkspaceIncidentPageRequest,
) -> Result<camellia_nexus_licensing::WorkspaceIncidentPage> {
    let api = license_api_for_command(&state, "license_workspace_alert_incidents_list").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .workspace_alert_incidents_with_api(&api, request, crate::licensing::unix_now())
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_alert_incidents_list_denied",
    )
    .await
}

#[tauri::command]
pub async fn acknowledge_license_workspace_alert_incident(
    app: AppHandle,
    state: State<'_, AppState>,
    incident_id: String,
    request: camellia_nexus_licensing::VersionedWorkspaceMutation,
) -> Result<camellia_nexus_licensing::WorkspaceMutationReceipt> {
    let api =
        license_api_for_command(&state, "license_workspace_alert_incident_acknowledge").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .acknowledge_workspace_alert_incident_with_api(
            &api,
            &incident_id,
            request,
            crate::licensing::unix_now(),
        )
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_alert_incident_acknowledge_denied",
    )
    .await
}

#[tauri::command]
pub async fn resolve_license_workspace_alert_incident(
    app: AppHandle,
    state: State<'_, AppState>,
    incident_id: String,
    request: camellia_nexus_licensing::VersionedWorkspaceMutation,
) -> Result<camellia_nexus_licensing::WorkspaceMutationReceipt> {
    let api = license_api_for_command(&state, "license_workspace_alert_incident_resolve").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .resolve_workspace_alert_incident_with_api(
            &api,
            &incident_id,
            request,
            crate::licensing::unix_now(),
        )
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_alert_incident_resolve_denied",
    )
    .await
}

#[tauri::command]
pub async fn get_license_workspace_audit_events(
    app: AppHandle,
    state: State<'_, AppState>,
    request: camellia_nexus_licensing::WorkspaceAuditPageRequest,
) -> Result<camellia_nexus_licensing::WorkspaceAuditPage> {
    let api = license_api_for_command(&state, "license_workspace_audit_events_list").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .workspace_audit_events_with_api(&api, request, crate::licensing::unix_now())
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_audit_events_list_denied",
    )
    .await
}

#[tauri::command]
pub async fn get_license_workspace_audit_event_types(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<camellia_nexus_licensing::WorkspaceAuditEventTypes> {
    let api = license_api_for_command(&state, "license_workspace_audit_event_types").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .workspace_audit_event_types_with_api(&api, crate::licensing::unix_now())
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_audit_event_types_denied",
    )
    .await
}

#[tauri::command]
pub async fn export_license_workspace_audit_events(
    app: AppHandle,
    state: State<'_, AppState>,
    request: camellia_nexus_licensing::WorkspaceAuditPageRequest,
) -> Result<camellia_nexus_licensing::WorkspaceAuditExport> {
    let api = license_api_for_command(&state, "license_workspace_audit_events_export").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .export_workspace_audit_events_with_api(&api, request, crate::licensing::unix_now())
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_audit_events_export_denied",
    )
    .await
}

#[tauri::command]
pub async fn get_license_workspace_webhook_endpoints(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<camellia_nexus_licensing::WorkspaceWebhookEndpoint>> {
    let api = license_api_for_command(&state, "license_workspace_webhooks_list").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .workspace_webhook_endpoints_with_api(&api, crate::licensing::unix_now())
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_webhooks_list_denied",
    )
    .await
}

#[tauri::command]
pub async fn create_license_workspace_webhook_endpoint(
    app: AppHandle,
    state: State<'_, AppState>,
    request: camellia_nexus_licensing::CreateWorkspaceWebhookEndpoint,
) -> Result<camellia_nexus_licensing::WorkspaceWebhookSecretResult> {
    let api = license_api_for_command(&state, "license_workspace_webhook_create").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .create_workspace_webhook_endpoint_with_api(&api, request, crate::licensing::unix_now())
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_webhook_create_denied",
    )
    .await
}

#[tauri::command]
pub async fn update_license_workspace_webhook_endpoint(
    app: AppHandle,
    state: State<'_, AppState>,
    endpoint_id: String,
    request: camellia_nexus_licensing::UpdateWorkspaceWebhookEndpoint,
) -> Result<camellia_nexus_licensing::WorkspaceWebhookEndpoint> {
    let api = license_api_for_command(&state, "license_workspace_webhook_update").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .update_workspace_webhook_endpoint_with_api(
            &api,
            &endpoint_id,
            request,
            crate::licensing::unix_now(),
        )
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_webhook_update_denied",
    )
    .await
}

#[tauri::command]
pub async fn rotate_license_workspace_webhook_endpoint(
    app: AppHandle,
    state: State<'_, AppState>,
    endpoint_id: String,
    request: camellia_nexus_licensing::RotateWorkspaceWebhookSecret,
) -> Result<camellia_nexus_licensing::WorkspaceWebhookSecretResult> {
    let api = license_api_for_command(&state, "license_workspace_webhook_rotate").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .rotate_workspace_webhook_secret_with_api(
            &api,
            &endpoint_id,
            request,
            crate::licensing::unix_now(),
        )
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_webhook_rotate_denied",
    )
    .await
}

#[tauri::command]
pub async fn delete_license_workspace_webhook_endpoint(
    app: AppHandle,
    state: State<'_, AppState>,
    endpoint_id: String,
    request: camellia_nexus_licensing::DeleteWorkspaceWebhookEndpoint,
) -> Result<camellia_nexus_licensing::WorkspaceWebhookDeletion> {
    let api = license_api_for_command(&state, "license_workspace_webhook_delete").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .delete_workspace_webhook_endpoint_with_api(
            &api,
            &endpoint_id,
            request,
            crate::licensing::unix_now(),
        )
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_webhook_delete_denied",
    )
    .await
}

#[tauri::command]
pub async fn get_license_workspace_webhook_deliveries(
    app: AppHandle,
    state: State<'_, AppState>,
    endpoint_id: Option<String>,
    limit: u16,
) -> Result<Vec<camellia_nexus_licensing::WorkspaceWebhookDelivery>> {
    let api = license_api_for_command(&state, "license_workspace_webhook_deliveries").await?;
    let _session_operation = state.license_session_operation.lock().await;
    let result = state
        .authorization
        .workspace_webhook_deliveries_with_api(
            &api,
            endpoint_id.as_deref(),
            limit,
            crate::licensing::unix_now(),
        )
        .await;
    license_api_result(
        &app,
        &state,
        result,
        "license_workspace_webhook_deliveries_denied",
    )
    .await
}

async fn license_api_result<T>(
    app: &AppHandle,
    state: &AppState,
    result: std::result::Result<T, camellia_nexus_licensing::LicensingError>,
    reason: &'static str,
) -> Result<T> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            let runtime_transition = state.runtime_authorization.transition_permit().await;
            let persistence_error = apply_refresh_failure_state(state, &error).await.err();
            synchronize_runtime_with_entitlement_locked(app, state, reason, runtime_transition)
                .await;
            Err(persistence_error.unwrap_or_else(|| license_error(error)))
        }
    }
}

#[tauri::command]
pub async fn remove_license_device(
    app: AppHandle,
    state: State<'_, AppState>,
    device_id: String,
    operation_id: String,
) -> Result<()> {
    tracing::info!(%device_id, "removing registered license device");
    let _session_operation = state.license_session_operation.lock().await;
    let removing_current_device = state
        .authorization
        .state()
        .entitlement()
        .is_some_and(|entitlement| entitlement.claims.device_id == device_id);
    let api = match license_api_for_command(&state, "license_remove_device").await {
        Ok(api) => api,
        Err(error) if removing_current_device => return Err(device_removal_incomplete(error)),
        Err(error) => return Err(error),
    };
    if let Err(error) = state
        .authorization
        .remove_device_with_api(
            &api,
            &device_id,
            &operation_id,
            crate::licensing::unix_now(),
        )
        .await
    {
        return if removing_current_device {
            Err(device_removal_incomplete(error))
        } else {
            Err(license_error(error))
        };
    }
    if removing_current_device {
        let runtime_transition = state.runtime_authorization.transition_permit().await;
        let local_deauthorization = state
            .authorization
            .deauthorize_locally(crate::licensing::unix_now())
            .await;
        synchronize_runtime_with_entitlement_locked(
            &app,
            &state,
            "current_device_removal",
            runtime_transition,
        )
        .await;
        local_deauthorization.map_err(license_error)?;
    }
    Ok(())
}

fn device_removal_incomplete(
    error: impl std::fmt::Display,
) -> camellia_nexus_core::CamelliaNexusError {
    camellia_nexus_core::CamelliaNexusError::new(
        ErrorCode::LicenseDeviceRemovalIncomplete,
        "License service operation failed",
    )
    .with_details(error.to_string())
}

#[tauri::command]
pub async fn logout_license_session(app: AppHandle, state: State<'_, AppState>) -> Result<()> {
    tracing::info!("signing out license session");
    let _session_operation = state.license_session_operation.lock().await;
    let runtime_transition = state.runtime_authorization.transition_permit().await;
    let local_deauthorization = state
        .authorization
        .deauthorize_locally(crate::licensing::unix_now())
        .await;
    synchronize_runtime_with_entitlement_locked(&app, &state, "license_logout", runtime_transition)
        .await;
    let session = local_deauthorization.map_err(license_error)?;
    let mut remote_failure = None;
    if let Some(session) = session {
        match license_api_for_command(&state, "license_logout").await {
            Ok(api) => {
                if let Err(error) = state
                    .authorization
                    .logout_session_with_api(&api, session)
                    .await
                {
                    tracing::warn!(%error, "could not notify the license service during sign-out");
                    remote_failure = Some(error.to_string());
                }
            }
            Err(error) => {
                tracing::warn!(%error, "could not notify the license service during sign-out");
                remote_failure = Some(error.to_string());
            }
        }
    }
    if let Some(details) = remote_failure {
        Err(camellia_nexus_core::CamelliaNexusError::new(
            ErrorCode::LicenseRemoteSignoutIncomplete,
            "License service operation failed",
        )
        .with_details(details))
    } else {
        Ok(())
    }
}

#[tauri::command]
pub async fn reset_license_device_identity(
    app: AppHandle,
    state: State<'_, AppState>,
    operation_id: String,
) -> Result<EntitlementSnapshot> {
    tracing::warn!("replacing local license device identity");
    let _session_operation = state.license_session_operation.lock().await;
    let cancelled = cancel_all_pending_authorizations(&state);
    if cancelled > 0 {
        tracing::info!(
            cancelled,
            "cancelled pending activation sessions before identity reset"
        );
    }

    let metadata = state
        .authorization
        .device_registration_metadata()
        .await
        .map_err(license_error)?
        .ok_or_else(|| {
            license_error(camellia_nexus_licensing::LicensingError::DeviceIdentityUnavailable)
        })?;
    let api = license_api_for_command(&state, "license_identity_reset_retire").await?;
    state
        .authorization
        .remove_device_with_api(
            &api,
            &metadata.device_id,
            &operation_id,
            crate::licensing::unix_now(),
        )
        .await
        .map_err(|error| {
            camellia_nexus_core::CamelliaNexusError::new(
                ErrorCode::LicenseDeviceRemovalIncomplete,
                "The current device could not be retired before replacing its identity",
            )
            .with_details(error.to_string())
        })?;

    let runtime_transition = state.runtime_authorization.transition_permit().await;
    let local_deauthorization = state
        .authorization
        .deauthorize_locally(crate::licensing::unix_now())
        .await;
    let snapshot = synchronize_runtime_with_entitlement_locked(
        &app,
        &state,
        "license_identity_reset",
        runtime_transition,
    )
    .await
    .snapshot;

    match local_deauthorization {
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(%error, "could not fully persist local deauthorization before identity reset");
        }
    }
    let result = state
        .authorization
        .reset_device_identity(crate::licensing::unix_now())
        .await;
    result.map_err(license_error)?;
    Ok(snapshot)
}

pub(crate) async fn synchronize_license_runtime(
    app: &AppHandle,
    reason: &'static str,
) -> Option<EntitlementState> {
    let state = app.try_state::<AppState>()?;
    let event = synchronize_runtime_with_entitlement(app, &state, reason).await;
    let entitlement = event.snapshot.entitlement_state.clone();
    Some(entitlement)
}

pub(crate) fn next_license_enforcement_delay(app: &AppHandle) -> Duration {
    const MAX_MONITOR_DELAY: Duration = Duration::from_secs(30);
    const MIN_MONITOR_DELAY: Duration = Duration::from_millis(100);

    let Some(state) = app.try_state::<AppState>() else {
        return MAX_MONITOR_DELAY;
    };
    let local_now = crate::licensing::unix_now();
    let entitlement = state.authorization.state_at(local_now);
    let trusted_now = state
        .authorization
        .trusted_now(local_now)
        .unwrap_or(local_now);
    let deadline = match entitlement {
        EntitlementState::Active { entitlement } => entitlement.claims.expires_at,
        EntitlementState::RestrictedOffline {
            safety_window_ends_at,
            ..
        } => safety_window_ends_at,
        _ => return MAX_MONITOR_DELAY,
    };
    let remaining = deadline.saturating_sub(trusted_now);
    if remaining <= 0 {
        MIN_MONITOR_DELAY
    } else {
        Duration::from_secs(remaining as u64)
            .min(MAX_MONITOR_DELAY)
            .max(MIN_MONITOR_DELAY)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LicenseMaintenanceOutcome {
    Idle,
    Succeeded,
    TransientFailure { retry_after_seconds: Option<u64> },
}

/// Performs the online part of license maintenance without making the WebView a security
/// boundary. Refresh-session consumers are single-flight with manual license operations.
pub(crate) async fn maintain_license_online(app: &AppHandle) -> LicenseMaintenanceOutcome {
    let Some(state) = app.try_state::<AppState>() else {
        return LicenseMaintenanceOutcome::Idle;
    };
    let Ok(_session_operation) = state.license_session_operation.try_lock() else {
        return LicenseMaintenanceOutcome::Idle;
    };
    let now = crate::licensing::unix_now();
    let entitlement_state = state.authorization.state_at(now);
    let schedule_now = state.authorization.trusted_now(now).unwrap_or(now);
    let pending_activation = matches!(entitlement_state, EntitlementState::ActivationPending);
    let denied_device_recovery = matches!(
        entitlement_state,
        EntitlementState::DeviceDenied {
            state: DeviceState::Revoked | DeviceState::Suspicious
        }
    );
    let refresh_due = match &entitlement_state {
        EntitlementState::Active { entitlement } => {
            schedule_now >= entitlement.claims.refresh_after
                || entitlement.claims.expires_at.saturating_sub(schedule_now) <= 60 * 60
        }
        EntitlementState::ActivationPending
        | EntitlementState::RestrictedOffline { .. }
        | EntitlementState::Expired { .. }
        | EntitlementState::RevalidationRequired { .. }
        | EntitlementState::LicenseInactive { .. } => true,
        // The running binary cannot satisfy a minimum-version denial by retrying. A newly
        // installed supported build clears the persisted marker during initialization and then
        // resumes normal maintenance.
        EntitlementState::ClientUpgradeRequired { .. } => {
            return LicenseMaintenanceOutcome::Idle;
        }
        EntitlementState::DeviceDenied { .. } if denied_device_recovery => true,
        EntitlementState::DeviceDenied { .. } => return LicenseMaintenanceOutcome::Idle,
        EntitlementState::Unauthenticated | EntitlementState::SessionOnly => {
            return LicenseMaintenanceOutcome::Idle;
        }
    };
    let api = match license_api_for_command(&state, "license_background_maintenance").await {
        Ok(api) => api,
        Err(error) => {
            tracing::debug!(%error, "background license service connection is unavailable");
            return LicenseMaintenanceOutcome::TransientFailure {
                retry_after_seconds: None,
            };
        }
    };
    let runtime_transition = state.runtime_authorization.transition_permit().await;
    let mut status_triggered_refresh = false;
    let result = if pending_activation {
        resume_pending_activation(&state, &api, now).await
    } else if denied_device_recovery {
        recover_and_refresh_license(&state, &api, now).await
    } else if refresh_due {
        state
            .authorization
            .refresh_entitlement_with_api(&api, "entitlement:refresh", now)
            .await
            .map(|_| ())
    } else {
        let status_result = state
            .authorization
            .entitlement_status_with_api(&api, now)
            .await;
        match status_result {
            Ok(_)
                if matches!(
                    state.authorization.state_at(now),
                    EntitlementState::RevalidationRequired { .. }
                ) =>
            {
                status_triggered_refresh = true;
                state
                    .authorization
                    .refresh_entitlement_with_api(&api, "entitlement:refresh", now)
                    .await
                    .map(|_| ())
            }
            result => result.map(|_| ()),
        }
    };
    let (result, recovered_session) = match result {
        Err(
            camellia_nexus_licensing::LicensingError::AuthorizationRequired
            | camellia_nexus_licensing::LicensingError::RefreshSessionReused,
        ) if !pending_activation => (recover_and_refresh_license(&state, &api, now).await, true),
        result => (result, denied_device_recovery),
    };
    match result {
        Ok(()) => {
            let reason = if recovered_session {
                "license_background_session_recovered"
            } else if refresh_due {
                "license_background_refresh"
            } else if status_triggered_refresh {
                "license_background_status_refresh"
            } else {
                "license_background_status"
            };
            synchronize_runtime_with_entitlement_locked(app, &state, reason, runtime_transition)
                .await;
            LicenseMaintenanceOutcome::Succeeded
        }
        Err(error) if transient_license_error(&error) => {
            tracing::warn!(
                %error,
                refresh_attempted = refresh_due || status_triggered_refresh,
                "background license maintenance will retry"
            );
            let retry_after_seconds = match &error {
                camellia_nexus_licensing::LicensingError::TooManyRequests {
                    retry_after_seconds,
                } => *retry_after_seconds,
                _ => None,
            };
            synchronize_runtime_with_entitlement_locked(
                app,
                &state,
                "license_network_unavailable",
                runtime_transition,
            )
            .await;
            LicenseMaintenanceOutcome::TransientFailure {
                retry_after_seconds,
            }
        }
        Err(error) => {
            if let Err(state_error) = apply_refresh_failure_state(&state, &error).await {
                tracing::error!(%state_error, "could not apply license service denial locally");
            }
            synchronize_runtime_with_entitlement_locked(
                app,
                &state,
                "license_background_denied",
                runtime_transition,
            )
            .await;
            LicenseMaintenanceOutcome::Succeeded
        }
    }
}

fn transient_license_error(error: &camellia_nexus_licensing::LicensingError) -> bool {
    matches!(
        error,
        camellia_nexus_licensing::LicensingError::Network
            | camellia_nexus_licensing::LicensingError::Timeout
            | camellia_nexus_licensing::LicensingError::SecureStoreTimeout
            | camellia_nexus_licensing::LicensingError::TooManyRequests { .. }
            | camellia_nexus_licensing::LicensingError::ServiceUnconfigured
            | camellia_nexus_licensing::LicensingError::InvalidServerResponse
    )
}

pub(crate) fn license_runtime_impact(state: &EntitlementState) -> LicenseRuntimeImpact {
    match state {
        EntitlementState::Active { .. } => LicenseRuntimeImpact::Active,
        EntitlementState::RestrictedOffline { .. } => LicenseRuntimeImpact::RestrictedOffline,
        EntitlementState::Unauthenticated
        | EntitlementState::SessionOnly
        | EntitlementState::ActivationPending
        | EntitlementState::Expired { .. }
        | EntitlementState::RevalidationRequired { .. }
        | EntitlementState::ClientUpgradeRequired { .. }
        | EntitlementState::DeviceDenied { .. }
        | EntitlementState::LicenseInactive { .. } => LicenseRuntimeImpact::HardInactive,
    }
}

async fn synchronize_runtime_with_entitlement(
    app: &AppHandle,
    state: &AppState,
    reason: &'static str,
) -> LicenseStateChangedEvent {
    let runtime_transition = state.runtime_authorization.transition_permit().await;
    synchronize_runtime_with_entitlement_locked(app, state, reason, runtime_transition).await
}

async fn synchronize_runtime_with_entitlement_locked(
    app: &AppHandle,
    state: &AppState,
    reason: &'static str,
    runtime_transition: tokio::sync::RwLockWriteGuard<'_, ()>,
) -> LicenseStateChangedEvent {
    // A caller that changes entitlement state acquires this writer before publishing the change
    // and retains it through runtime enforcement. Protected commits hold read permits, so the
    // entitlement snapshot and the runtime side effects share one total order.
    let entitlement = state.authorization.state_at(crate::licensing::unix_now());
    let generation = state
        .license_state_generation
        .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
        + 1;
    let runtime_impact = license_runtime_impact(&entitlement);
    let should_auto_start =
        update_auto_start_arming(&state.license_auto_start_armed, runtime_impact);
    let stop_report = if runtime_impact.requires_runtime_stop() {
        state
            .manager
            .disable_automatic_restarts_and_stop_active()
            .await
    } else {
        state
            .manager
            .set_automatic_restarts_enabled(runtime_impact == LicenseRuntimeImpact::Active)
            .await;
        camellia_nexus_core::StopActiveReport::default()
    };
    if stop_report.attempted > 0 {
        tracing::warn!(
            reason,
            attempted = stop_report.attempted,
            stopped = stop_report.stopped,
            failed = stop_report.failed,
            status = entitlement_status_name(&entitlement),
            "stopped active programs because license is not active"
        );
    }
    let event = LicenseStateChangedEvent {
        snapshot: EntitlementSnapshot {
            generation,
            entitlement_state: entitlement,
        },
        reason,
        runtime_impact: runtime_impact.as_str(),
        stopped_programs: stop_report.stopped,
        failed_programs: stop_report.failed,
        failed_program_ids: stop_report
            .failed_program_ids
            .into_iter()
            .map(|id| id.to_string())
            .collect(),
    };
    // Emitting while the writer is held preserves the same total order as runtime enforcement.
    // Commands return this event's snapshot, so one backend generation orders both responses and
    // asynchronous WebView delivery.
    emit_license_state_changed(app, &event);
    drop(runtime_transition);
    if should_auto_start {
        schedule_program_auto_start(
            state.manager.clone(),
            std::time::Duration::from_millis(state.settings.current().program_startup_delay_ms),
            reason,
        );
    }
    event
}

fn update_auto_start_arming(
    armed: &std::sync::atomic::AtomicBool,
    runtime_impact: LicenseRuntimeImpact,
) -> bool {
    if runtime_impact == LicenseRuntimeImpact::HardInactive {
        armed.store(true, std::sync::atomic::Ordering::Release);
        return false;
    }
    runtime_impact == LicenseRuntimeImpact::Active
        && armed.swap(false, std::sync::atomic::Ordering::AcqRel)
}

pub(crate) fn schedule_program_auto_start(
    manager: std::sync::Arc<camellia_nexus_core::ProgramManager>,
    startup_delay: std::time::Duration,
    reason: &'static str,
) {
    tauri::async_runtime::spawn(async move {
        let report = manager.reconcile_auto_start_programs(startup_delay).await;
        if report.failed > 0 {
            tracing::warn!(
                reason,
                eligible = report.eligible,
                started = report.started,
                already_active = report.already_active,
                skipped = report.skipped,
                failed = report.failed,
                failed_program_ids = ?report
                    .failed_program_ids
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
                "managed program autostart completed with failures"
            );
        } else {
            tracing::info!(
                reason,
                eligible = report.eligible,
                started = report.started,
                already_active = report.already_active,
                skipped = report.skipped,
                "managed program autostart reconciliation completed"
            );
        }
    });
}

fn emit_license_state_changed(app: &AppHandle, event: &LicenseStateChangedEvent) {
    if !should_emit_license_state_changed(event) {
        return;
    }
    if let Err(error) = app.emit("license-state-changed", event) {
        tracing::warn!(
            reason = event.reason,
            stopped = event.stopped_programs,
            failed = event.failed_programs,
            %error,
            "could not emit license runtime state to frontend"
        );
    }
}

fn should_emit_license_state_changed(event: &LicenseStateChangedEvent) -> bool {
    if event.stopped_programs > 0 || event.failed_programs > 0 {
        return true;
    }
    if matches!(
        &event.snapshot.entitlement_state,
        EntitlementState::ActivationPending
            | EntitlementState::RestrictedOffline { .. }
            | EntitlementState::Expired { .. }
            | EntitlementState::RevalidationRequired { .. }
            | EntitlementState::ClientUpgradeRequired { .. }
            | EntitlementState::DeviceDenied { .. }
            | EntitlementState::LicenseInactive { .. }
    ) {
        return true;
    }

    // Reads already return the state directly, while the regular monitor would otherwise emit an
    // unauthenticated/session-only event every 30 seconds on a fresh installation. State-changing
    // online/manual operations use distinct reasons and must reach the frontend, including
    // recovery to Active and invalidation to Unauthenticated or SessionOnly.
    !matches!(event.reason, "license_monitor" | "entitlement_state_read")
}

#[cfg(test)]
mod license_state_event_tests {
    use std::sync::atomic::AtomicBool;

    use super::{
        LicenseRuntimeImpact, LicenseStateChangedEvent, should_emit_license_state_changed,
        update_auto_start_arming,
    };
    use camellia_nexus_licensing::EntitlementState;

    fn event(
        entitlement_state: EntitlementState,
        reason: &'static str,
    ) -> LicenseStateChangedEvent {
        LicenseStateChangedEvent {
            snapshot: super::EntitlementSnapshot {
                generation: 1,
                entitlement_state,
            },
            reason,
            runtime_impact: "hardInactive",
            stopped_programs: 0,
            failed_programs: 0,
            failed_program_ids: Vec::new(),
        }
    }

    #[test]
    fn emits_state_changing_unauthenticated_and_session_only_events() {
        assert!(should_emit_license_state_changed(&event(
            EntitlementState::Unauthenticated,
            "license_background_denied",
        )));
        assert!(should_emit_license_state_changed(&event(
            EntitlementState::SessionOnly,
            "license_session_recovered",
        )));
    }

    #[test]
    fn suppresses_routine_initial_unauthenticated_reads_and_monitoring() {
        assert!(!should_emit_license_state_changed(&event(
            EntitlementState::Unauthenticated,
            "license_monitor",
        )));
        assert!(!should_emit_license_state_changed(&event(
            EntitlementState::SessionOnly,
            "entitlement_state_read",
        )));
    }

    #[test]
    fn event_serializes_the_same_generation_bearing_snapshot_contract() {
        let event = event(EntitlementState::Unauthenticated, "test");
        let snapshot = serde_json::to_value(&event.snapshot).expect("snapshot JSON");
        let serialized = serde_json::to_value(event).expect("event JSON");

        assert_eq!(serialized["generation"], snapshot["generation"]);
        assert_eq!(serialized["entitlementState"], snapshot["entitlementState"]);
        assert!(serialized.get("snapshot").is_none());
    }

    #[test]
    fn auto_start_is_armed_only_for_initial_or_hard_inactive_recovery() {
        let armed = AtomicBool::new(true);
        assert!(!update_auto_start_arming(
            &armed,
            LicenseRuntimeImpact::RestrictedOffline,
        ));
        assert!(update_auto_start_arming(
            &armed,
            LicenseRuntimeImpact::Active,
        ));
        assert!(!update_auto_start_arming(
            &armed,
            LicenseRuntimeImpact::RestrictedOffline,
        ));
        assert!(!update_auto_start_arming(
            &armed,
            LicenseRuntimeImpact::Active,
        ));
        assert!(!update_auto_start_arming(
            &armed,
            LicenseRuntimeImpact::HardInactive,
        ));
        assert!(update_auto_start_arming(
            &armed,
            LicenseRuntimeImpact::Active,
        ));
        assert!(!update_auto_start_arming(
            &armed,
            LicenseRuntimeImpact::Active,
        ));
    }
}

fn entitlement_status_name(state: &EntitlementState) -> &'static str {
    match state {
        EntitlementState::Unauthenticated => "unauthenticated",
        EntitlementState::SessionOnly => "session_only",
        EntitlementState::ActivationPending => "activation_pending",
        EntitlementState::Active { .. } => "active",
        EntitlementState::RestrictedOffline { .. } => "restricted_offline",
        EntitlementState::Expired { .. } => "expired",
        EntitlementState::RevalidationRequired { .. } => "revalidation_required",
        EntitlementState::ClientUpgradeRequired { .. } => "client_upgrade_required",
        EntitlementState::DeviceDenied { .. } => "device_denied",
        EntitlementState::LicenseInactive { .. } => "license_inactive",
    }
}

fn log_entitlement_state(message: &'static str, state: &EntitlementState) {
    match state {
        EntitlementState::Active { entitlement }
        | EntitlementState::RestrictedOffline { entitlement, .. }
        | EntitlementState::Expired { entitlement }
        | EntitlementState::ClientUpgradeRequired {
            entitlement: Some(entitlement),
            ..
        }
        | EntitlementState::LicenseInactive {
            entitlement: Some(entitlement),
            ..
        } => tracing::info!(
            status = entitlement_status_name(state),
            license_id = %entitlement.claims.license_id,
            device_id = %entitlement.claims.device_id,
            plan = ?entitlement.claims.plan,
            issued_at = entitlement.claims.issued_at,
            refresh_after = entitlement.claims.refresh_after,
            license_expires_at = ?entitlement.claims.license_expires_at,
            lease_expires_at = entitlement.claims.expires_at,
            key_id = %entitlement.key_id,
            "{message}"
        ),
        EntitlementState::DeviceDenied {
            state: device_state,
        } => tracing::info!(
            status = entitlement_status_name(state),
            device_state = ?device_state,
            "{message}"
        ),
        EntitlementState::RevalidationRequired { reason } => tracing::info!(
            status = entitlement_status_name(state),
            reason = ?reason,
            "{message}"
        ),
        _ => tracing::info!(status = entitlement_status_name(state), "{message}"),
    }
}

fn authorize_protected(state: &AppState, operation: ProtectedOperation) -> Result<()> {
    authorize_restricted(state, RestrictedOperation::Protected(operation))
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeAuthorizationRequirements {
    operations: Vec<ProtectedOperation>,
}

impl RuntimeAuthorizationRequirements {
    fn for_program(primary: ProtectedOperation, spec: &ProgramSpec) -> Self {
        let mut operations = vec![primary];
        if spec.managed_config.is_some() {
            operations.push(ProtectedOperation::UseManagedConfigSources);
        }
        if matches!(spec.executable, ExecutableSpec::Managed { .. }) {
            operations.push(ProtectedOperation::UseManagedProgramPackages);
        }
        Self { operations }
    }

    fn for_configuration(primary: ProtectedOperation, spec: &ProgramSpec) -> Self {
        let mut operations = vec![primary];
        if spec.managed_config.is_some() {
            operations.push(ProtectedOperation::UseManagedConfigSources);
        }
        Self { operations }
    }

    fn single(operation: ProtectedOperation) -> Self {
        Self {
            operations: vec![operation],
        }
    }

    fn authorize(&self, state: &AppState) -> Result<()> {
        for operation in &self.operations {
            authorize_restricted(state, RestrictedOperation::Protected(*operation))?;
        }
        Ok(())
    }
}

pub(crate) struct RuntimeMutationPermit<'a> {
    _guard: tokio::sync::RwLockReadGuard<'a, ()>,
}

pub(crate) async fn authorize_runtime_requirements<'a>(
    state: &'a AppState,
    requirements: &RuntimeAuthorizationRequirements,
) -> Result<RuntimeMutationPermit<'a>> {
    let operation_guard = state.runtime_authorization.mutation_permit().await;
    requirements.authorize(state)?;
    Ok(RuntimeMutationPermit {
        _guard: operation_guard,
    })
}

pub(crate) async fn authorize_runtime_protected(
    state: &AppState,
    operation: ProtectedOperation,
) -> Result<RuntimeMutationPermit<'_>> {
    authorize_runtime_requirements(state, &RuntimeAuthorizationRequirements::single(operation))
        .await
}

fn authorize_safety(state: &AppState, operation: SafetyOperation) -> Result<()> {
    authorize_restricted(state, RestrictedOperation::Safety(operation))
}

fn authorize_restricted(state: &AppState, operation: RestrictedOperation) -> Result<()> {
    match state
        .authorization
        .authorize(operation, crate::licensing::unix_now())
    {
        Ok(()) => {
            tracing::debug!(?operation, "license operation authorized");
            Ok(())
        }
        Err(error) => {
            tracing::warn!(?operation, %error, "license operation denied");
            Err(license_error(error))
        }
    }
}

fn require_license_limit(
    state: &State<'_, AppState>,
    limit: NumericLimit,
    requested_count: u64,
) -> Result<()> {
    match state
        .authorization
        .guard()
        .require_limit(limit, requested_count)
    {
        Ok(()) => {
            tracing::debug!(?limit, requested_count, "license limit authorized");
            Ok(())
        }
        Err(error) => {
            tracing::warn!(?limit, requested_count, %error, "license limit denied");
            Err(license_error(error))
        }
    }
}

fn managed_config_source_count(spec: &ProgramSpec) -> u64 {
    spec.managed_config
        .as_ref()
        .map(|managed| managed.sources.len() as u64)
        .unwrap_or_default()
}

fn license_error(
    error: camellia_nexus_licensing::LicensingError,
) -> camellia_nexus_core::CamelliaNexusError {
    use camellia_nexus_core::ErrorCode;

    let code = match error {
        camellia_nexus_licensing::LicensingError::Network => ErrorCode::Network,
        camellia_nexus_licensing::LicensingError::Timeout
        | camellia_nexus_licensing::LicensingError::SecureStoreTimeout => ErrorCode::Timeout,
        camellia_nexus_licensing::LicensingError::TooManyRequests { .. } => ErrorCode::RateLimited,
        camellia_nexus_licensing::LicensingError::SecureStoreUnavailable
        | camellia_nexus_licensing::LicensingError::SecureStoreBackend
        | camellia_nexus_licensing::LicensingError::SecureStoreCorrupt => ErrorCode::Storage,
        camellia_nexus_licensing::LicensingError::ServiceUnconfigured
        | camellia_nexus_licensing::LicensingError::InvalidRequest
        | camellia_nexus_licensing::LicensingError::InvalidOAuthCallback
        | camellia_nexus_licensing::LicensingError::InvalidServerResponse
        | camellia_nexus_licensing::LicensingError::MalformedEntitlement
        | camellia_nexus_licensing::LicensingError::InvalidSignature
        | camellia_nexus_licensing::LicensingError::UnsupportedAlgorithm
        | camellia_nexus_licensing::LicensingError::UnknownSigningKey
        | camellia_nexus_licensing::LicensingError::WrongIssuer
        | camellia_nexus_licensing::LicensingError::WrongAudience
        | camellia_nexus_licensing::LicensingError::DeviceMismatch
        | camellia_nexus_licensing::LicensingError::DeviceKeyMismatch
        | camellia_nexus_licensing::LicensingError::InvalidClaims
        | camellia_nexus_licensing::LicensingError::InvalidClientBuild => ErrorCode::InvalidSpec,
        camellia_nexus_licensing::LicensingError::AuthorizationRequired => {
            ErrorCode::LicenseRequired
        }
        camellia_nexus_licensing::LicensingError::DeviceActivationPending => {
            ErrorCode::LicenseActivationPending
        }
        camellia_nexus_licensing::LicensingError::ActivationPendingExpired => {
            ErrorCode::LicenseActivationPendingExpired
        }
        camellia_nexus_licensing::LicensingError::ActivationCodeInvalid => {
            ErrorCode::LicenseActivationCodeInvalid
        }
        camellia_nexus_licensing::LicensingError::ActivationCodeExpired => {
            ErrorCode::LicenseActivationCodeExpired
        }
        camellia_nexus_licensing::LicensingError::ActivationCodeConsumed => {
            ErrorCode::LicenseActivationCodeConsumed
        }
        camellia_nexus_licensing::LicensingError::ActivationCodeRevoked => {
            ErrorCode::LicenseActivationCodeRevoked
        }
        camellia_nexus_licensing::LicensingError::CapabilityDenied => {
            ErrorCode::LicensePlanRequired
        }
        camellia_nexus_licensing::LicensingError::PermissionDenied => {
            ErrorCode::LicensePermissionDenied
        }
        camellia_nexus_licensing::LicensingError::TeamInvitationInvalid => {
            ErrorCode::LicenseTeamInvitationInvalid
        }
        camellia_nexus_licensing::LicensingError::TeamDeviceEnrollmentInvalid => {
            ErrorCode::LicenseTeamDeviceEnrollmentInvalid
        }
        camellia_nexus_licensing::LicensingError::WorkspaceVersionConflict => {
            ErrorCode::LicenseWorkspaceConflict
        }
        camellia_nexus_licensing::LicensingError::IdempotencyConflict => {
            ErrorCode::LicenseOperationConflict
        }
        camellia_nexus_licensing::LicensingError::WorkspaceQuotaExceeded => {
            ErrorCode::LicenseWorkspaceQuotaExceeded
        }
        camellia_nexus_licensing::LicensingError::WorkspaceDocumentLimitReached => {
            ErrorCode::LicenseWorkspaceDocumentLimitReached
        }
        camellia_nexus_licensing::LicensingError::WorkspaceAlertRuleLimitReached => {
            ErrorCode::LicenseWorkspaceAlertRuleLimitReached
        }
        camellia_nexus_licensing::LicensingError::WorkspaceRetentionActive => {
            ErrorCode::LicenseWorkspaceRetentionActive
        }
        camellia_nexus_licensing::LicensingError::WorkspaceNotFound => {
            ErrorCode::LicenseWorkspaceNotFound
        }
        camellia_nexus_licensing::LicensingError::WorkspaceIntegrity => {
            ErrorCode::LicenseWorkspaceIntegrityFailed
        }
        camellia_nexus_licensing::LicensingError::WorkspaceKeyUnavailable => {
            ErrorCode::LicenseWorkspaceKeyUnavailable
        }
        camellia_nexus_licensing::LicensingError::WebhookInvalidUrl => {
            ErrorCode::LicenseWebhookInvalidUrl
        }
        camellia_nexus_licensing::LicensingError::WebhookEndpointLimitReached => {
            ErrorCode::LicenseWebhookEndpointLimitReached
        }
        camellia_nexus_licensing::LicensingError::WebhookNotFound => {
            ErrorCode::LicenseWebhookNotFound
        }
        camellia_nexus_licensing::LicensingError::WebhookKeyUnavailable => {
            ErrorCode::LicenseWebhookKeyUnavailable
        }
        camellia_nexus_licensing::LicensingError::RequestTooLarge => ErrorCode::RequestTooLarge,
        camellia_nexus_licensing::LicensingError::LimitExceeded
        | camellia_nexus_licensing::LicensingError::ActivationLimitReached => {
            ErrorCode::LicenseLimitExceeded
        }
        camellia_nexus_licensing::LicensingError::DeviceDenied
        | camellia_nexus_licensing::LicensingError::DeviceRemoved
        | camellia_nexus_licensing::LicensingError::DeviceRevoked
        | camellia_nexus_licensing::LicensingError::DeviceSuspicious
        | camellia_nexus_licensing::LicensingError::RefreshSessionReused => {
            ErrorCode::LicenseDeviceDenied
        }
        camellia_nexus_licensing::LicensingError::AccountSuspended => {
            ErrorCode::LicenseAccountSuspended
        }
        camellia_nexus_licensing::LicensingError::AccountDenylisted => {
            ErrorCode::LicenseAccountDenylisted
        }
        camellia_nexus_licensing::LicensingError::LicensePastDue => {
            ErrorCode::LicensePaymentPastDue
        }
        camellia_nexus_licensing::LicensingError::LicenseCanceled => ErrorCode::LicenseCanceled,
        camellia_nexus_licensing::LicensingError::LicenseExpired => ErrorCode::LicenseExpired,
        camellia_nexus_licensing::LicensingError::LicenseUnavailable => ErrorCode::LicenseRequired,
        camellia_nexus_licensing::LicensingError::ClientUpgradeRequired { .. } => {
            ErrorCode::LicenseClientUpgradeRequired
        }
        camellia_nexus_licensing::LicensingError::ClockRollback
        | camellia_nexus_licensing::LicensingError::ObsoleteLicenseEpoch
        | camellia_nexus_licensing::LicensingError::EntitlementExpired
        | camellia_nexus_licensing::LicensingError::InvalidChallenge
        | camellia_nexus_licensing::LicensingError::ChallengeReplay => {
            ErrorCode::LicenseRevalidationRequired
        }
        _ => ErrorCode::Internal,
    };
    camellia_nexus_core::CamelliaNexusError::new(code, "License service operation failed")
        .with_details(error.to_string())
}

#[cfg(test)]
mod license_error_tests {
    use super::{identity_registration_conflict_error, license_error};
    use camellia_nexus_core::ErrorCode;
    use camellia_nexus_licensing::LicensingError;

    #[test]
    fn activation_code_failures_keep_actionable_ipc_codes() {
        for (error, expected) in [
            (
                LicensingError::ActivationCodeInvalid,
                ErrorCode::LicenseActivationCodeInvalid,
            ),
            (
                LicensingError::ActivationCodeExpired,
                ErrorCode::LicenseActivationCodeExpired,
            ),
            (
                LicensingError::ActivationCodeConsumed,
                ErrorCode::LicenseActivationCodeConsumed,
            ),
            (
                LicensingError::ActivationCodeRevoked,
                ErrorCode::LicenseActivationCodeRevoked,
            ),
            (
                LicensingError::ActivationPendingExpired,
                ErrorCode::LicenseActivationPendingExpired,
            ),
        ] {
            assert_eq!(license_error(error).code, expected);
        }
    }

    #[test]
    fn only_a_registration_denial_for_an_existing_identity_is_a_license_conflict() {
        assert_eq!(
            identity_registration_conflict_error(true, &LicensingError::DeviceDenied)
                .expect("existing identity conflict")
                .code,
            ErrorCode::LicenseIdentityAlreadyRegistered
        );
        assert!(
            identity_registration_conflict_error(false, &LicensingError::DeviceDenied).is_none()
        );
        assert!(
            identity_registration_conflict_error(true, &LicensingError::DeviceRemoved).is_none()
        );
    }

    #[test]
    fn team_credential_failures_are_recoverable_input_errors() {
        for (error, expected) in [
            (
                LicensingError::TeamInvitationInvalid,
                ErrorCode::LicenseTeamInvitationInvalid,
            ),
            (
                LicensingError::TeamDeviceEnrollmentInvalid,
                ErrorCode::LicenseTeamDeviceEnrollmentInvalid,
            ),
        ] {
            assert_eq!(license_error(error).code, expected);
        }
    }

    #[test]
    fn workspace_failures_keep_actionable_ipc_codes() {
        for (error, expected) in [
            (
                LicensingError::WorkspaceQuotaExceeded,
                ErrorCode::LicenseWorkspaceQuotaExceeded,
            ),
            (
                LicensingError::WorkspaceDocumentLimitReached,
                ErrorCode::LicenseWorkspaceDocumentLimitReached,
            ),
            (
                LicensingError::WorkspaceAlertRuleLimitReached,
                ErrorCode::LicenseWorkspaceAlertRuleLimitReached,
            ),
            (
                LicensingError::WorkspaceRetentionActive,
                ErrorCode::LicenseWorkspaceRetentionActive,
            ),
            (
                LicensingError::WorkspaceIntegrity,
                ErrorCode::LicenseWorkspaceIntegrityFailed,
            ),
            (
                LicensingError::WebhookInvalidUrl,
                ErrorCode::LicenseWebhookInvalidUrl,
            ),
            (
                LicensingError::WebhookEndpointLimitReached,
                ErrorCode::LicenseWebhookEndpointLimitReached,
            ),
            (
                LicensingError::WebhookKeyUnavailable,
                ErrorCode::LicenseWebhookKeyUnavailable,
            ),
        ] {
            assert_eq!(license_error(error).code, expected);
        }
    }
}

#[tauri::command]
pub async fn get_program(state: State<'_, AppState>, program_id: String) -> Result<ProgramDetail> {
    authorize_safety(&state, SafetyOperation::View)?;
    let program_id = id(program_id)?;
    let (spec, program_state) = state.manager.get(&program_id).await?;
    let working_directory = state.manager.working_directory(&program_id).await?;
    Ok(ProgramDetail {
        spec,
        state: program_state,
        working_directory,
    })
}

#[tauri::command]
pub async fn get_program_privilege_assessment(
    state: State<'_, AppState>,
    program_id: String,
) -> Result<camellia_nexus_core::PrivilegeAssessment> {
    authorize_safety(&state, SafetyOperation::View)?;
    let program_id = id(program_id)?;
    let (spec, _) = state.manager.get(&program_id).await?;
    let workspace = state.manager.workspace(&program_id).await?;
    let adapter = camellia_nexus_core::AdapterRegistry::default().get(spec.program_type.kind());
    let plan = adapter.launch_plan(&spec, &workspace)?;
    crate::privileges::assess_launch_plan(&plan)
}

#[tauri::command]
pub fn list_invalid_programs(
    state: State<'_, AppState>,
) -> Result<Vec<camellia_nexus_core::InvalidProgram>> {
    authorize_safety(&state, SafetyOperation::View)?;
    Ok(state
        .invalid_programs
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone())
}

#[tauri::command]
pub async fn create_program(
    state: State<'_, AppState>,
    mut request: CreateProgramRequest,
) -> Result<()> {
    request.spec.validate()?;
    let authorization_requirements = RuntimeAuthorizationRequirements::for_program(
        ProtectedOperation::CreatePremiumProgram,
        &request.spec,
    );
    authorization_requirements.authorize(&state)?;
    let mut credential_transaction = if crate::config_credentials::has_credentials(&request.spec) {
        state.config_credentials.recover(&state.manager).await?;
        Some(
            state
                .config_credentials
                .reconcile(&mut request.spec)
                .await?,
        )
    } else {
        None
    };
    let credential_snapshot = credential_transaction
        .as_ref()
        .map(crate::config_credentials::CredentialTransaction::snapshot)
        .cloned()
        .unwrap_or_else(crate::config_credentials::CredentialSnapshot::empty);
    if request.spec.managed_config.is_some() {
        let local_base = create_source_base(&request);
        let require_sources = request.initial_config.is_none();
        request.initial_config = Some(
            crate::config_sources::materialize(
                &request.spec,
                request.initial_config.take(),
                require_sources,
                local_base.as_deref(),
                &credential_snapshot,
            )
            .await?,
        );
    }
    let source_count = managed_config_source_count(&request.spec);
    let program_id = request.spec.id.clone();
    let prepared = match state.manager.prepare_create(request).await {
        Ok(prepared) => prepared,
        Err(error) => {
            if let Some(transaction) = credential_transaction.take() {
                transaction.rollback()?;
            }
            return Err(error);
        }
    };
    // Source downloads, package copying, executable probing and native validation all happen
    // before this gate. Re-authorize and reserve the live limits at the final registration
    // boundary so a concurrent sign-out, denial or downgrade cannot commit the prepared profile.
    let license_operation =
        match authorize_runtime_requirements(&state, &authorization_requirements).await {
            Ok(operation) => operation,
            Err(error) => {
                if let Err(discard_error) = state.manager.discard_prepared_create(prepared).await {
                    tracing::warn!(
                        program = %program_id,
                        %discard_error,
                        "prepared program cleanup requires startup recovery"
                    );
                }
                if let Some(transaction) = credential_transaction.take() {
                    transaction.rollback()?;
                }
                return Err(error);
            }
        };
    let current_count = state.manager.list().await.len() as u64;
    let program_limit =
        match state
            .authorization
            .guard()
            .reserve_limit(NumericLimit::MaxPrograms, current_count, 1)
        {
            Ok(reservation) => reservation,
            Err(error) => {
                drop(license_operation);
                if let Err(discard_error) = state.manager.discard_prepared_create(prepared).await {
                    tracing::warn!(
                        program = %program_id,
                        %discard_error,
                        "prepared program cleanup requires startup recovery"
                    );
                }
                if let Some(transaction) = credential_transaction.take() {
                    transaction.rollback()?;
                }
                tracing::warn!(
                    current_count,
                    requested_count = 1,
                    %error,
                    "license program limit denied"
                );
                return Err(license_error(error));
            }
        };
    if source_count > 0
        && let Err(error) = require_license_limit(
            &state,
            NumericLimit::MaxConfigSourcesPerProgram,
            source_count,
        )
    {
        drop(program_limit);
        drop(license_operation);
        if let Err(discard_error) = state.manager.discard_prepared_create(prepared).await {
            tracing::warn!(
                program = %program_id,
                %discard_error,
                "prepared program cleanup requires startup recovery"
            );
        }
        if let Some(transaction) = credential_transaction.take() {
            transaction.rollback()?;
        }
        return Err(error);
    }
    let create_result = state.manager.commit_create(prepared).await;
    drop(program_limit);
    drop(license_operation);
    if let Err(error) = create_result {
        if let Some(transaction) = credential_transaction.take() {
            transaction.rollback()?;
        }
        return Err(error);
    }
    if let Some(transaction) = credential_transaction
        && let Err(error) = transaction.commit()
    {
        tracing::warn!(program = %program_id, %error, "credential commit cleanup requires recovery");
        state.config_credentials.recover(&state.manager).await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn update_program(state: State<'_, AppState>, mut spec: ProgramSpec) -> Result<()> {
    update_program_transaction(&state, &mut spec, false, false)
        .await
        .map(|_| ())
}

#[tauri::command]
pub async fn update_program_and_restart(
    state: State<'_, AppState>,
    mut spec: ProgramSpec,
) -> Result<()> {
    update_program_transaction(&state, &mut spec, false, true)
        .await
        .map(|_| ())
}

#[tauri::command]
pub async fn update_program_and_refresh_config(
    state: State<'_, AppState>,
    mut spec: ProgramSpec,
) -> Result<crate::config_updates::ConfigUpdateResult> {
    update_program_transaction(&state, &mut spec, true, false)
        .await?
        .ok_or_else(|| {
            camellia_nexus_core::CamelliaNexusError::internal("configuration refresh did not run")
        })
}

async fn update_program_transaction(
    state: &State<'_, AppState>,
    spec: &mut ProgramSpec,
    refresh_managed_config: bool,
    restart_after_update: bool,
) -> Result<Option<crate::config_updates::ConfigUpdateResult>> {
    spec.validate()?;
    if refresh_managed_config && spec.managed_config.is_none() {
        return Err(camellia_nexus_core::CamelliaNexusError::invalid_spec(
            "Managed configuration is required for an atomic source refresh",
        ));
    }
    if refresh_managed_config && restart_after_update {
        return Err(camellia_nexus_core::CamelliaNexusError::invalid_spec(
            "A settings restart and managed configuration refresh must use one commit mode",
        ));
    }
    let authorization_requirements = RuntimeAuthorizationRequirements::for_program(
        ProtectedOperation::EditPremiumConfiguration,
        spec,
    );
    authorization_requirements.authorize(state)?;
    let (current, _) = state.manager.get(&spec.id).await?;
    let mut credential_transaction = if crate::config_credentials::has_credentials(&current)
        || crate::config_credentials::has_credentials(spec)
    {
        state.config_credentials.recover(&state.manager).await?;
        Some(state.config_credentials.reconcile(spec).await?)
    } else {
        None
    };
    let source_count = managed_config_source_count(spec);
    if source_count > 0 {
        require_license_limit(
            state,
            NumericLimit::MaxConfigSourcesPerProgram,
            source_count,
        )?;
    }
    let program_id = spec.id.clone();
    let prepared_update = match state.manager.prepare_update(spec.clone()).await {
        Ok(prepared) => prepared,
        Err(error) => {
            if let Some(transaction) = credential_transaction.take() {
                transaction.rollback()?;
            }
            return Err(error);
        }
    };
    let _refresh_lease = if refresh_managed_config {
        match state.config_refreshes.try_acquire(&program_id) {
            Ok(lease) => Some(lease),
            Err(error) => {
                if let Some(transaction) = credential_transaction.take() {
                    transaction.rollback()?;
                }
                return Err(error);
            }
        }
    } else {
        None
    };
    let mut prepared_refresh = if refresh_managed_config {
        match crate::config_updates::prepare_refresh_for_update(state, &prepared_update).await {
            Ok(prepared) => Some(prepared),
            Err(error) => {
                if let Some(transaction) = credential_transaction.take() {
                    transaction.rollback()?;
                }
                return Err(error);
            }
        }
    } else {
        None
    };
    let license_operation =
        match authorize_runtime_requirements(state, &authorization_requirements).await {
            Ok(operation) => operation,
            Err(error) => {
                if let Some(prepared) = prepared_refresh.take() {
                    crate::config_updates::discard_prepared_refresh(state, prepared).await;
                }
                if let Some(transaction) = credential_transaction.take() {
                    transaction.rollback()?;
                }
                return Err(error);
            }
        };
    if source_count > 0
        && let Err(error) = require_license_limit(
            state,
            NumericLimit::MaxConfigSourcesPerProgram,
            source_count,
        )
    {
        drop(license_operation);
        if let Some(prepared) = prepared_refresh.take() {
            crate::config_updates::discard_prepared_refresh(state, prepared).await;
        }
        if let Some(transaction) = credential_transaction.take() {
            transaction.rollback()?;
        }
        return Err(error);
    }
    let update_result = if let Some(prepared) = prepared_refresh {
        crate::config_updates::commit_update_refresh(
            state,
            prepared_update,
            prepared,
            &license_operation,
        )
        .await
        .map(Some)
    } else {
        state
            .manager
            .commit_update(prepared_update, restart_after_update)
            .await
            .map(|_| None)
    };
    let refresh_result = match update_result {
        Ok(result) => result,
        Err(error) => {
            if let Some(transaction) = credential_transaction.take() {
                transaction.rollback()?;
            }
            return Err(error);
        }
    };
    if let Some(transaction) = credential_transaction
        && let Err(error) = transaction.commit()
    {
        tracing::warn!(program = %program_id, %error, "credential commit cleanup requires recovery");
        state.config_credentials.recover(&state.manager).await?;
    }
    Ok(refresh_result)
}

#[tauri::command]
pub async fn remove_program(state: State<'_, AppState>, program_id: String) -> Result<()> {
    authorize_safety(&state, SafetyOperation::Remove)?;
    let program_id = id(program_id)?;
    state.config_credentials.recover(&state.manager).await?;
    let transaction = state
        .config_credentials
        .remove_program(program_id.as_str())
        .await?;
    if let Err(error) = state.manager.remove(&program_id).await {
        transaction.rollback()?;
        return Err(error);
    }
    if let Err(error) = transaction.commit() {
        tracing::warn!(program = %program_id, %error, "credential removal cleanup requires recovery");
        state.config_credentials.recover(&state.manager).await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn start_program(state: State<'_, AppState>, program_id: String) -> Result<()> {
    let _license_operation =
        authorize_runtime_protected(&state, ProtectedOperation::Activate).await?;
    state.manager.start(&id(program_id)?).await
}

#[tauri::command]
pub async fn stop_program(state: State<'_, AppState>, program_id: String) -> Result<()> {
    authorize_safety(&state, SafetyOperation::Stop)?;
    state.manager.stop(&id(program_id)?).await
}

#[tauri::command]
pub async fn restart_program(state: State<'_, AppState>, program_id: String) -> Result<()> {
    let _license_operation =
        authorize_runtime_protected(&state, ProtectedOperation::Activate).await?;
    state.manager.restart(&id(program_id)?).await
}

#[tauri::command]
pub async fn replace_package(
    state: State<'_, AppState>,
    program_id: String,
    package_source: std::path::PathBuf,
) -> Result<()> {
    let operation = ProtectedOperation::UseManagedProgramPackages;
    authorize_protected(&state, operation)?;
    let program_id = id(program_id)?;
    let prepared = state
        .manager
        .prepare_package(&program_id, package_source)
        .await?;
    // Copying and probing a managed package can be slow. Only the final checked swap holds the
    // runtime authorization gate, and a denial leaves the active package untouched.
    let _license_operation = match authorize_runtime_protected(&state, operation).await {
        Ok(permit) => permit,
        Err(error) => {
            if let Err(discard_error) = state.manager.discard_prepared_package(prepared).await {
                tracing::warn!(
                    program = %program_id,
                    %discard_error,
                    "prepared package cleanup requires startup recovery"
                );
            }
            return Err(error);
        }
    };
    state.manager.commit_package(prepared).await
}

#[tauri::command]
pub async fn list_actions(
    state: State<'_, AppState>,
    program_id: String,
) -> Result<Vec<ActionDescriptor>> {
    authorize_safety(&state, SafetyOperation::View)?;
    state.manager.list_actions(&id(program_id)?).await
}

#[tauri::command]
pub async fn load_config(state: State<'_, AppState>, program_id: String) -> Result<ConfigDocument> {
    authorize_safety(&state, SafetyOperation::View)?;
    state.manager.load_config(&id(program_id)?).await
}

#[tauri::command]
pub async fn load_configuration_schema(
    state: State<'_, AppState>,
    program_id: String,
) -> Result<Option<camellia_nexus_core::ConfigurationSchemaDocument>> {
    authorize_safety(&state, SafetyOperation::View)?;
    state
        .manager
        .load_configuration_schema(&id(program_id)?)
        .await
}

#[tauri::command]
pub async fn validate_config(
    state: State<'_, AppState>,
    program_id: String,
    content: String,
    base_hash: String,
) -> Result<ValidationResult> {
    authorize_protected(&state, ProtectedOperation::RunAdvancedDiagnostics)?;
    let program_id = id(program_id)?;
    let (spec, _) = state.manager.get(&program_id).await?;
    if spec.managed_config.is_some() {
        authorize_protected(&state, ProtectedOperation::UseManagedConfigSources)?;
    }
    let content = crate::config_sources::apply_managed_features(&spec, content)?;
    let result = state
        .manager
        .validate_config(&program_id, content, base_hash)
        .await?;
    authorize_protected(&state, ProtectedOperation::RunAdvancedDiagnostics)?;
    Ok(result)
}

#[tauri::command]
pub async fn apply_config(
    state: State<'_, AppState>,
    program_id: String,
    content: String,
    base_hash: String,
) -> Result<String> {
    let program_id = id(program_id)?;
    let (spec, _) = state.manager.get(&program_id).await?;
    let authorization_requirements = RuntimeAuthorizationRequirements::for_configuration(
        ProtectedOperation::EditPremiumConfiguration,
        &spec,
    );
    authorization_requirements.authorize(&state)?;
    let content = crate::config_sources::apply_managed_features(&spec, content)?;
    let prepared = state
        .manager
        .prepare_config(&program_id, &spec, &spec, content, base_hash)
        .await?;
    let _license_operation =
        match authorize_runtime_requirements(&state, &authorization_requirements).await {
            Ok(operation) => operation,
            Err(error) => {
                let _ = state.manager.discard_prepared_config(prepared).await;
                return Err(error);
            }
        };
    state
        .manager
        .apply_prepared_config(&program_id, &spec, prepared, true)
        .await
}

#[tauri::command]
pub async fn refresh_config_sources(
    state: State<'_, AppState>,
    program_id: String,
) -> Result<crate::config_updates::ConfigUpdateResult> {
    authorize_protected(&state, ProtectedOperation::UseManagedConfigSources)?;
    let program_id = id(program_id)?;
    crate::config_updates::refresh(&state, &program_id).await
}

fn create_source_base(request: &CreateProgramRequest) -> Option<PathBuf> {
    match &request.spec.executable {
        camellia_nexus_core::ExecutableSpec::Managed { path, .. } => {
            let relative = path.strip_prefix("bin").unwrap_or(path);
            let executable_directory = relative
                .parent()
                .unwrap_or_else(|| std::path::Path::new(""));
            request
                .package_source
                .as_ref()
                .map(|package| package.join(executable_directory))
        }
        camellia_nexus_core::ExecutableSpec::External { path, .. } => {
            path.parent().map(|directory| directory.to_path_buf())
        }
    }
}

#[tauri::command]
pub async fn run_action(
    state: State<'_, AppState>,
    program_id: String,
    action_id: String,
    content: String,
    base_hash: String,
) -> Result<ActionResult> {
    authorize_protected(&state, ProtectedOperation::RunAdvancedDiagnostics)?;
    let program_id = id(program_id)?;
    let (spec, _) = state.manager.get(&program_id).await?;
    let content = crate::config_sources::apply_managed_features(&spec, content)?;
    let result = state
        .manager
        .run_action(&program_id, action_id, content, base_hash)
        .await?;
    // Diagnostics do not commit program state, so they must not delay logout/expiry enforcement
    // while an external tool runs. Re-check before releasing premium output to the caller.
    authorize_protected(&state, ProtectedOperation::RunAdvancedDiagnostics)?;
    Ok(result)
}

#[tauri::command]
pub async fn read_logs(
    state: State<'_, AppState>,
    program_id: String,
    stream: LogStream,
    max_bytes: usize,
) -> Result<LogChunk> {
    authorize_safety(&state, SafetyOperation::View)?;
    state
        .manager
        .read_log(&id(program_id)?, stream, max_bytes)
        .await
}

#[tauri::command]
pub async fn clear_logs(state: State<'_, AppState>, program_id: String) -> Result<()> {
    authorize_safety(&state, SafetyOperation::DeleteLocalConfiguration)?;
    state.manager.clear_logs(&id(program_id)?).await
}

#[tauri::command]
pub async fn open_working_directory(state: State<'_, AppState>, program_id: String) -> Result<()> {
    authorize_safety(&state, SafetyOperation::View)?;
    let path = state.manager.working_directory(&id(program_id)?).await?;
    open_external(&path)
}

#[tauri::command]
pub fn open_data_directory(state: State<'_, AppState>) -> Result<()> {
    authorize_safety(&state, SafetyOperation::View)?;
    open_external(&state.data_dir)
}

#[tauri::command]
pub fn open_app_log_directory(state: State<'_, AppState>) -> Result<()> {
    authorize_safety(&state, SafetyOperation::View)?;
    let logs_dir = state.data_dir.join("logs");
    std::fs::create_dir_all(&logs_dir).map_err(camellia_nexus_core::CamelliaNexusError::storage)?;
    open_external(&logs_dir)
}

#[tauri::command]
pub fn get_app_settings(state: State<'_, AppState>) -> Result<AppSettings> {
    state.settings.current_result()
}

#[tauri::command]
pub fn frontend_ready(state: State<'_, AppState>) -> Option<UiIntent> {
    use std::sync::atomic::Ordering;

    let mut pending = state
        .pending_ui_intent
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.ui_ready.store(true, Ordering::Release);
    pending.take()
}

#[tauri::command]
pub async fn set_app_settings(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    settings: AppSettings,
) -> Result<()> {
    state.settings.update(settings)?;
    if let Err(error) = crate::tray::refresh(&app, state.manager.clone()).await {
        tracing::warn!(%error, "could not refresh the tray after saving settings");
    }
    Ok(())
}

#[tauri::command]
pub async fn open_documentation(state: State<'_, AppState>, program_id: String) -> Result<()> {
    authorize_safety(&state, SafetyOperation::View)?;
    let document = state.manager.load_config(&id(program_id)?).await?;
    let url = document.documentation_url;
    let valid_https_url = url
        .strip_prefix("https://")
        .is_some_and(|remainder| !remainder.is_empty());
    if !valid_https_url {
        return Err(camellia_nexus_core::CamelliaNexusError::invalid_spec(
            "Adapter documentation URL must use HTTPS",
        ));
    }
    open_external(url)
}

#[tauri::command]
pub async fn open_sing_box_dashboard(
    state: State<'_, AppState>,
    program_id: String,
    dashboard_kind: String,
) -> Result<()> {
    authorize_protected(&state, ProtectedOperation::RemoteControl)?;
    let program_id = id(program_id)?;
    let (spec, program_state) = state.manager.get(&program_id).await?;
    if !matches!(program_state, ProgramState::Running { .. }) {
        return Err(camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::InvalidState,
            "Start sing-box before opening its Dashboard",
        ));
    }
    let managed = spec.managed_config.as_ref().ok_or_else(|| {
        camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::InvalidState,
            "sing-box Dashboard is not enabled",
        )
    })?;
    let url = match dashboard_kind.as_str() {
        "native" => managed
            .sing_box_dashboard
            .as_ref()
            .map(|dashboard| format!("http://127.0.0.1:{}/dashboard/", dashboard.listen_port)),
        "clash" => managed
            .sing_box_clash_dashboard
            .as_ref()
            .map(|dashboard| format!("http://127.0.0.1:{}/ui/", dashboard.listen_port)),
        _ => {
            return Err(camellia_nexus_core::CamelliaNexusError::invalid_spec(
                "Unknown Dashboard type",
            ));
        }
    }
    .ok_or_else(|| {
        camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::InvalidState,
            "The selected sing-box Dashboard is not enabled",
        )
    })?;
    open_external(url)
}

#[tauri::command]
pub async fn open_mihomo_dashboard(state: State<'_, AppState>, program_id: String) -> Result<()> {
    authorize_protected(&state, ProtectedOperation::RemoteControl)?;
    let program_id = id(program_id)?;
    let (spec, program_state) = state.manager.get(&program_id).await?;
    if !matches!(&spec.program_type, ProgramType::Mihomo { .. }) {
        return Err(camellia_nexus_core::CamelliaNexusError::invalid_spec(
            "Mihomo Dashboard is only available for Mihomo programs",
        ));
    }
    if !matches!(program_state, ProgramState::Running { .. }) {
        return Err(camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::InvalidState,
            "Start Mihomo before opening its Dashboard",
        ));
    }
    let dashboard = spec
        .managed_config
        .as_ref()
        .and_then(|managed| managed.mihomo_dashboard.as_ref())
        .ok_or_else(|| {
            camellia_nexus_core::CamelliaNexusError::new(
                camellia_nexus_core::ErrorCode::InvalidState,
                "Mihomo Dashboard is not enabled",
            )
        })?;
    open_external(format!("http://127.0.0.1:{}/ui/", dashboard.listen_port))
}

#[tauri::command]
pub async fn get_xray_dashboard_snapshot(
    state: State<'_, AppState>,
    program_id: String,
    include_routing: bool,
    include_topology: bool,
) -> Result<XrayDashboardSnapshot> {
    authorize_protected(&state, ProtectedOperation::RemoteControl)?;
    let program_id = id(program_id)?;
    let (spec, program_state) = state.manager.get(&program_id).await?;
    if !matches!(&spec.program_type, ProgramType::Xray { .. }) {
        return Err(camellia_nexus_core::CamelliaNexusError::invalid_spec(
            "Xray Dashboard is only available for Xray programs",
        ));
    }
    if !matches!(program_state, ProgramState::Running { .. }) {
        return Err(camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::InvalidState,
            "Start Xray before refreshing its Dashboard",
        ));
    }
    let dashboard = spec
        .managed_config
        .as_ref()
        .and_then(|managed| managed.xray_dashboard.as_ref())
        .ok_or_else(|| {
            camellia_nexus_core::CamelliaNexusError::new(
                camellia_nexus_core::ErrorCode::InvalidState,
                "Xray Dashboard is not enabled",
            )
        })?;
    let api_url = format!("127.0.0.1:{}", dashboard.api_port);
    let metrics_url = format!("http://127.0.0.1:{}/debug/vars", dashboard.metrics_port);
    let (metrics, metrics_error) = match load_xray_metrics(&metrics_url).await {
        Ok(metrics) => (Some(metrics), None),
        Err(error) => (None, Some(error.to_string())),
    };
    let alive_outbounds = xray_alive_outbounds(metrics.as_ref());
    let system_stats_request = load_xray_system_stats(&state, &program_id, &spec, &api_url);
    let topology_request = async {
        if include_topology {
            load_xray_runtime_topology(&state, &program_id, &spec, &api_url)
                .await
                .map(Some)
        } else {
            Ok(None)
        }
    };
    let online_users_request =
        load_xray_online_users(&state, &program_id, &spec, &api_url, metrics.as_ref());
    let routing_request = async {
        if include_routing {
            load_xray_balancers(&state, &program_id, &spec, &api_url, &alive_outbounds).await
        } else {
            Ok(Vec::new())
        }
    };
    let (system_stats_result, topology_result, online_users_result, routing_result) = tokio::join!(
        system_stats_request,
        topology_request,
        online_users_request,
        routing_request,
    );
    let (system_stats, system_stats_error) = match system_stats_result {
        Ok(stats) => (Some(stats), None),
        Err(error) => (None, Some(error.to_string())),
    };
    let (topology, topology_error) = match topology_result {
        Ok(topology) => (topology, None),
        Err(error) => (None, Some(error.to_string())),
    };
    let (online_users, online_users_error) = match online_users_result {
        Ok(summary) => (Some(summary), None),
        Err(error) => (None, Some(error.to_string())),
    };
    let (balancers, routing_error) = if include_routing {
        match routing_result {
            Ok(balancers) => (Some(balancers), None),
            Err(error) => (Some(Vec::new()), Some(error.to_string())),
        }
    } else {
        (None, None)
    };
    let fetched_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default();
    // A snapshot may involve several bounded local API calls. Do not return premium runtime data
    // if the entitlement became inactive while they were in flight.
    authorize_protected(&state, ProtectedOperation::RemoteControl)?;
    Ok(XrayDashboardSnapshot {
        api_url,
        metrics_url,
        metrics,
        metrics_error,
        system_stats,
        system_stats_error,
        topology,
        topology_error,
        online_users,
        online_users_error,
        balancers,
        routing_error,
        fetched_unix_ms,
    })
}

async fn load_xray_metrics(metrics_url: &str) -> Result<Value> {
    const MAX_XRAY_METRICS_BYTES: usize = 4 * 1024 * 1024;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(3))
        .user_agent(concat!("camellia-nexus/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(camellia_nexus_core::CamelliaNexusError::internal)?;
    let mut response = client
        .get(metrics_url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| {
            camellia_nexus_core::CamelliaNexusError::new(
                camellia_nexus_core::ErrorCode::InvalidState,
                "Xray Metrics is unavailable",
            )
            .with_details(error.without_url().to_string())
        })?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_XRAY_METRICS_BYTES as u64)
    {
        return Err(camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::UnsupportedBinary,
            "Xray Metrics response exceeds the 4 MiB limit",
        ));
    }
    let mut content = Vec::with_capacity(
        response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or_default()
            .min(MAX_XRAY_METRICS_BYTES),
    );
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::InvalidState,
            "Failed to read Xray Metrics",
        )
        .with_details(error.without_url().to_string())
    })? {
        if content.len().saturating_add(chunk.len()) > MAX_XRAY_METRICS_BYTES {
            return Err(camellia_nexus_core::CamelliaNexusError::new(
                camellia_nexus_core::ErrorCode::UnsupportedBinary,
                "Xray Metrics response exceeds the 4 MiB limit",
            ));
        }
        content.extend_from_slice(&chunk);
    }
    serde_json::from_slice::<Value>(&content).map_err(|error| {
        camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::UnsupportedBinary,
            "Xray returned unsupported Metrics data",
        )
        .with_details(error.to_string())
    })
}

fn xray_alive_outbounds(metrics: Option<&Value>) -> HashSet<String> {
    metrics
        .and_then(|metrics| metrics.get("observatory"))
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|observatory| observatory.iter())
        .filter_map(|(name, status)| {
            if status.get("alive").and_then(Value::as_bool) != Some(true) {
                return None;
            }
            Some(
                status
                    .get("outbound_tag")
                    .or_else(|| status.get("outboundTag"))
                    .and_then(Value::as_str)
                    .filter(|tag| !tag.is_empty())
                    .unwrap_or(name)
                    .to_owned(),
            )
        })
        .collect()
}

async fn load_xray_runtime_topology(
    state: &State<'_, AppState>,
    program_id: &ProgramId,
    spec: &ProgramSpec,
    api_url: &str,
) -> Result<XrayRuntimeTopology> {
    let workspace = state.manager.workspace(program_id).await?;
    let executable = spec.executable_path(&workspace);
    let working_directory = spec.working_directory_path(&workspace);
    let inbound_arguments = vec![
        "api".to_owned(),
        "lsi".to_owned(),
        format!("--server={api_url}"),
        "--timeout=3".to_owned(),
        "--isOnlyTags=true".to_owned(),
        "--json".to_owned(),
    ];
    let outbound_arguments = vec![
        "api".to_owned(),
        "lso".to_owned(),
        format!("--server={api_url}"),
        "--timeout=3".to_owned(),
        "--json".to_owned(),
    ];
    let (inbounds, outbounds) = tokio::join!(
        run_xray_api(
            &state.tool_runner,
            &executable,
            &working_directory,
            &inbound_arguments
        ),
        run_xray_api(
            &state.tool_runner,
            &executable,
            &working_directory,
            &outbound_arguments
        ),
    );
    Ok(XrayRuntimeTopology {
        inbound_tags: xray_handler_tags(&inbounds?, "inbounds")?,
        outbound_tags: xray_handler_tags(&outbounds?, "outbounds")?,
    })
}

fn xray_handler_tags(output: &[u8], field: &str) -> Result<Vec<String>> {
    let response: Value = serde_json::from_slice(output).map_err(|error| {
        camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::UnsupportedBinary,
            "Xray returned unsupported handler information",
        )
        .with_details(error.to_string())
    })?;
    let mut tags: Vec<String> = response
        .get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|handler| handler.get("tag").and_then(Value::as_str))
        .filter(|tag| !tag.is_empty())
        .map(str::to_owned)
        .collect();
    tags.sort();
    tags.dedup();
    Ok(tags)
}

async fn load_xray_online_users(
    state: &State<'_, AppState>,
    program_id: &ProgramId,
    spec: &ProgramSpec,
    api_url: &str,
    metrics: Option<&Value>,
) -> Result<XrayOnlineUsersSummary> {
    let document = state.manager.load_config(program_id).await?;
    let root =
        crate::config_sources::parse_object("Xray configuration", document.content.as_bytes())?;
    let policy_enabled = xray_online_policy_enabled(&root);
    let (configured_users, loopback_only) = xray_configured_users(&root);
    let workspace = state.manager.workspace(program_id).await?;
    let executable = spec.executable_path(&workspace);
    let working_directory = spec.working_directory_path(&workspace);
    let (reported_users, status_available) = if policy_enabled && !loopback_only {
        query_xray_online_users(&state.tool_runner, &executable, &working_directory, api_url).await
    } else {
        (Vec::new(), policy_enabled)
    };
    let mut users: BTreeMap<String, XrayOnlineUser> = configured_users
        .into_iter()
        .map(|email| {
            (
                email.clone(),
                XrayOnlineUser {
                    email,
                    online: status_available.then_some(false),
                    addresses: Vec::new(),
                    uplink: 0,
                    downlink: 0,
                },
            )
        })
        .collect();
    for user in reported_users {
        users.insert(user.email.clone(), user);
    }
    for user in users.values_mut() {
        if let Some((uplink, downlink)) = xray_user_metrics(metrics, &user.email) {
            user.uplink = uplink;
            user.downlink = downlink;
        }
    }
    let users: Vec<XrayOnlineUser> = users.into_values().collect();
    let user_count = users
        .iter()
        .filter(|user| user.online == Some(true))
        .count();
    let address_count = users.iter().map(|user| user.addresses.len()).sum();
    Ok(XrayOnlineUsersSummary {
        policy_enabled,
        status_available,
        loopback_only,
        user_count,
        address_count,
        users,
    })
}

async fn query_xray_online_users(
    tool_runner: &camellia_nexus_core::DynToolRunner,
    executable: &Path,
    working_directory: &Path,
    api_url: &str,
) -> (Vec<XrayOnlineUser>, bool) {
    let arguments = vec![
        "api".to_owned(),
        "statsonlineiplist".to_owned(),
        format!("--server={api_url}"),
        "--timeout=3".to_owned(),
        "--all".to_owned(),
        "--include-traffic".to_owned(),
        "--json".to_owned(),
    ];
    if let Ok(output) = run_xray_api(tool_runner, executable, working_directory, &arguments).await
        && let Ok(response) = serde_json::from_slice::<Value>(&output)
    {
        return (parse_xray_online_users(&response), true);
    }

    let fallback_arguments = vec![
        "api".to_owned(),
        "statsgetallonlineusers".to_owned(),
        format!("--server={api_url}"),
        "--timeout=3".to_owned(),
        "--json".to_owned(),
    ];
    let Ok(output) = run_xray_api(
        tool_runner,
        executable,
        working_directory,
        &fallback_arguments,
    )
    .await
    else {
        return (Vec::new(), false);
    };
    let Ok(response) = serde_json::from_slice::<Value>(&output) else {
        return (Vec::new(), false);
    };
    let users = response
        .get("users")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(xray_online_map_email)
        .map(|email| XrayOnlineUser {
            email,
            online: Some(true),
            addresses: Vec::new(),
            uplink: 0,
            downlink: 0,
        })
        .collect();
    (users, true)
}

fn parse_xray_online_users(response: &Value) -> Vec<XrayOnlineUser> {
    let mut users: Vec<XrayOnlineUser> = response
        .get("users")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|user| {
            let email = user.get("email").and_then(Value::as_str)?.to_owned();
            let addresses = user
                .get("ips")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|address| {
                    Some(XrayOnlineAddress {
                        ip: address.get("ip").and_then(Value::as_str)?.to_owned(),
                        last_seen_unix: xray_signed_stat_value(address, &["lastSeen", "last_seen"]),
                    })
                })
                .collect();
            let traffic = user.get("traffic").unwrap_or(&Value::Null);
            Some(XrayOnlineUser {
                email,
                online: Some(true),
                addresses,
                uplink: xray_unsigned_stat_value(traffic, &["uplink"]),
                downlink: xray_unsigned_stat_value(traffic, &["downlink"]),
            })
        })
        .collect();
    users.sort_by(|left, right| left.email.cmp(&right.email));
    users
}

fn xray_online_map_email(value: &str) -> Option<String> {
    value
        .strip_prefix("user>>>")
        .and_then(|value| value.strip_suffix(">>>online"))
        .or_else(|| (!value.is_empty() && !value.contains(">>>")).then_some(value))
        .filter(|email| !email.is_empty())
        .map(str::to_owned)
}

fn xray_configured_users(root: &serde_json::Map<String, Value>) -> (Vec<String>, bool) {
    let mut users = BTreeMap::<String, bool>::new();
    for inbound in root
        .get("inbounds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let loopback = inbound
            .get("listen")
            .and_then(Value::as_str)
            .is_some_and(|listen| matches!(listen, "127.0.0.1" | "::1" | "localhost"));
        let Some(settings) = inbound.get("settings").and_then(Value::as_object) else {
            continue;
        };
        for field in ["clients", "users"] {
            for user in settings
                .get(field)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(email) = user
                    .get("email")
                    .and_then(Value::as_str)
                    .filter(|email| !email.is_empty())
                {
                    users
                        .entry(email.to_owned())
                        .and_modify(|only_loopback| *only_loopback &= loopback)
                        .or_insert(loopback);
                }
            }
        }
        if let Some(email) = settings
            .get("email")
            .and_then(Value::as_str)
            .filter(|email| !email.is_empty())
        {
            users
                .entry(email.to_owned())
                .and_modify(|only_loopback| *only_loopback &= loopback)
                .or_insert(loopback);
        }
    }
    let loopback_only = !users.is_empty() && users.values().all(|loopback| *loopback);
    (users.into_keys().collect(), loopback_only)
}

fn xray_user_metrics(metrics: Option<&Value>, email: &str) -> Option<(u64, u64)> {
    let traffic = metrics?.get("stats")?.get("user")?.get(email)?;
    Some((
        xray_unsigned_stat_value(traffic, &["uplink"]),
        xray_unsigned_stat_value(traffic, &["downlink"]),
    ))
}

fn xray_unsigned_stat_value(value: &Value, names: &[&str]) -> u64 {
    names
        .iter()
        .find_map(|name| value.get(*name))
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        })
        .unwrap_or_default()
}

fn xray_signed_stat_value(value: &Value, names: &[&str]) -> i64 {
    names
        .iter()
        .find_map(|name| value.get(*name))
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        })
        .unwrap_or_default()
}

fn xray_online_policy_enabled(root: &serde_json::Map<String, Value>) -> bool {
    root.get("policy")
        .and_then(Value::as_object)
        .and_then(|policy| policy.get("levels"))
        .and_then(Value::as_object)
        .is_some_and(|levels| {
            levels.values().any(|level| {
                level
                    .get("statsUserOnline")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
        })
}

async fn load_xray_system_stats(
    state: &State<'_, AppState>,
    program_id: &ProgramId,
    spec: &ProgramSpec,
    api_url: &str,
) -> Result<XraySystemStats> {
    let workspace = state.manager.workspace(program_id).await?;
    let executable = spec.executable_path(&workspace);
    let working_directory = spec.working_directory_path(&workspace);
    let arguments = vec![
        "api".to_owned(),
        "statssys".to_owned(),
        format!("--server={api_url}"),
        "--timeout=3".to_owned(),
        "--json".to_owned(),
    ];
    let output = run_xray_api(
        &state.tool_runner,
        &executable,
        &working_directory,
        &arguments,
    )
    .await?;
    let response: Value = serde_json::from_slice(&output).map_err(|error| {
        camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::UnsupportedBinary,
            "Xray returned unsupported system statistics",
        )
        .with_details(error.to_string())
    })?;
    let object = response.as_object().ok_or_else(|| {
        camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::UnsupportedBinary,
            "Xray returned unsupported system statistics",
        )
    })?;
    Ok(XraySystemStats {
        uptime_seconds: xray_stat_value(object, &["uptime", "Uptime"]),
        allocated_bytes: xray_stat_value(object, &["alloc", "Alloc"]),
        system_bytes: xray_stat_value(object, &["sys", "Sys"]),
        goroutines: xray_stat_value(object, &["numGoroutine", "NumGoroutine"]),
        live_objects: xray_stat_value(object, &["liveObjects", "LiveObjects"]),
        garbage_collections: xray_stat_value(object, &["numGC", "NumGC"]),
    })
}

fn xray_stat_value(object: &serde_json::Map<String, Value>, names: &[&str]) -> u64 {
    names
        .iter()
        .find_map(|name| object.get(*name))
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        })
        .unwrap_or_default()
}

#[tauri::command]
pub async fn set_xray_balancer_target(
    state: State<'_, AppState>,
    program_id: String,
    balancer_tag: String,
    target: Option<String>,
) -> Result<XrayBalancerInfo> {
    let _license_operation =
        authorize_runtime_protected(&state, ProtectedOperation::RemoteControl).await?;
    let program_id = id(program_id)?;
    let (spec, program_state) = state.manager.get(&program_id).await?;
    let dashboard = require_running_xray_dashboard(&spec, &program_state)?;
    let api_url = format!("127.0.0.1:{}", dashboard.api_port);
    let configured = configured_xray_balancers(&state, &program_id).await?;
    let balancer = configured
        .into_iter()
        .find(|balancer| balancer.tag == balancer_tag)
        .ok_or_else(|| {
            camellia_nexus_core::CamelliaNexusError::new(
                camellia_nexus_core::ErrorCode::NotFound,
                "Routing balancer not found",
            )
        })?;
    let target = target.filter(|target| !target.is_empty());
    if let Some(target) = target.as_deref()
        && !balancer
            .candidates
            .iter()
            .any(|candidate| candidate == target)
    {
        return Err(camellia_nexus_core::CamelliaNexusError::invalid_spec(
            "The selected outbound does not belong to this balancer",
        ));
    }
    let metrics_url = format!("http://127.0.0.1:{}/debug/vars", dashboard.metrics_port);
    let alive_outbounds = match load_xray_metrics(&metrics_url).await {
        Ok(metrics) => xray_alive_outbounds(Some(&metrics)),
        Err(error) if target.is_some() => return Err(error),
        Err(_) => HashSet::new(),
    };
    if let Some(target) = target.as_deref()
        && !alive_outbounds.contains(target)
    {
        return Err(camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::InvalidState,
            "The selected outbound is not currently healthy",
        ));
    }
    let workspace = state.manager.workspace(&program_id).await?;
    let executable = spec.executable_path(&workspace);
    let working_directory = spec.working_directory_path(&workspace);
    let mut arguments = vec![
        "api".to_owned(),
        "bo".to_owned(),
        format!("--server={api_url}"),
        "--timeout=3".to_owned(),
        "--balancer".to_owned(),
        balancer.tag.clone(),
    ];
    if let Some(target) = target {
        arguments.push(target);
    } else {
        arguments.push("--remove".to_owned());
    }
    run_xray_api(
        &state.tool_runner,
        &executable,
        &working_directory,
        &arguments,
    )
    .await?;
    query_xray_balancer(
        &state.tool_runner,
        &executable,
        &working_directory,
        &api_url,
        balancer,
        &alive_outbounds,
    )
    .await
}

#[tauri::command]
pub async fn restart_xray_logger(state: State<'_, AppState>, program_id: String) -> Result<()> {
    let _license_operation =
        authorize_runtime_protected(&state, ProtectedOperation::RunAdvancedDiagnostics).await?;
    let program_id = id(program_id)?;
    let (spec, program_state) = state.manager.get(&program_id).await?;
    let dashboard = require_running_xray_dashboard(&spec, &program_state)?;
    let workspace = state.manager.workspace(&program_id).await?;
    let executable = spec.executable_path(&workspace);
    let working_directory = spec.working_directory_path(&workspace);
    let arguments = vec![
        "api".to_owned(),
        "restartlogger".to_owned(),
        format!("--server=127.0.0.1:{}", dashboard.api_port),
        "--timeout=3".to_owned(),
    ];
    run_xray_api(
        &state.tool_runner,
        &executable,
        &working_directory,
        &arguments,
    )
    .await?;
    Ok(())
}

fn require_running_xray_dashboard<'a>(
    spec: &'a ProgramSpec,
    state: &ProgramState,
) -> Result<&'a camellia_nexus_core::XrayDashboardSpec> {
    if !matches!(&spec.program_type, ProgramType::Xray { .. }) {
        return Err(camellia_nexus_core::CamelliaNexusError::invalid_spec(
            "Xray Dashboard is only available for Xray programs",
        ));
    }
    if !matches!(state, ProgramState::Running { .. }) {
        return Err(camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::InvalidState,
            "Start Xray before changing routing",
        ));
    }
    spec.managed_config
        .as_ref()
        .and_then(|managed| managed.xray_dashboard.as_ref())
        .ok_or_else(|| {
            camellia_nexus_core::CamelliaNexusError::new(
                camellia_nexus_core::ErrorCode::InvalidState,
                "Xray Dashboard is not enabled",
            )
        })
}

async fn load_xray_balancers(
    state: &State<'_, AppState>,
    program_id: &ProgramId,
    spec: &ProgramSpec,
    api_url: &str,
    alive_outbounds: &HashSet<String>,
) -> Result<Vec<XrayBalancerInfo>> {
    let configured = configured_xray_balancers(state, program_id).await?;
    if configured.is_empty() {
        return Ok(Vec::new());
    }
    let workspace = state.manager.workspace(program_id).await?;
    let executable = spec.executable_path(&workspace);
    let working_directory = spec.working_directory_path(&workspace);
    let tool_runner = state.tool_runner.clone();
    let query = move |balancer: ConfiguredXrayBalancer| {
        let tool_runner = tool_runner.clone();
        let executable = executable.clone();
        let working_directory = working_directory.clone();
        let api_url = api_url.to_owned();
        let alive_outbounds = alive_outbounds.clone();
        async move {
            let fallback = balancer.clone();
            query_xray_balancer(
                &tool_runner,
                &executable,
                &working_directory,
                &api_url,
                balancer,
                &alive_outbounds,
            )
            .await
            .unwrap_or_else(|error| {
                xray_balancer_with_error(fallback, &alive_outbounds, error.to_string())
            })
        }
    };
    let mut configured = configured.into_iter();
    let mut tasks = tokio::task::JoinSet::new();
    for balancer in configured.by_ref().take(2) {
        tasks.spawn(query(balancer));
    }
    let mut balancers = Vec::new();
    while let Some(result) = tasks.join_next().await {
        balancers.push(result.map_err(camellia_nexus_core::CamelliaNexusError::internal)?);
        if let Some(balancer) = configured.next() {
            tasks.spawn(query(balancer));
        }
    }
    balancers.sort_by(|left, right| left.tag.cmp(&right.tag));
    Ok(balancers)
}

async fn configured_xray_balancers(
    state: &State<'_, AppState>,
    program_id: &ProgramId,
) -> Result<Vec<ConfiguredXrayBalancer>> {
    let document = state.manager.load_config(program_id).await?;
    let root =
        crate::config_sources::parse_object("Xray configuration", document.content.as_bytes())?;
    Ok(parse_configured_xray_balancers(&root))
}

fn parse_configured_xray_balancers(
    root: &serde_json::Map<String, Value>,
) -> Vec<ConfiguredXrayBalancer> {
    let outbound_tags: Vec<String> = root
        .get("outbounds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|outbound| outbound.get("tag").and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    let mut balancers = Vec::new();
    for value in root
        .get("routing")
        .and_then(Value::as_object)
        .and_then(|routing| routing.get("balancers"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(object) = value.as_object() else {
            continue;
        };
        let Some(tag) = object.get("tag").and_then(Value::as_str) else {
            continue;
        };
        let selectors: Vec<String> = object
            .get("selector")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let mut candidates: Vec<String> = outbound_tags
            .iter()
            .filter(|outbound| {
                selectors
                    .iter()
                    .any(|selector| outbound.starts_with(selector))
            })
            .cloned()
            .collect();
        candidates.sort();
        candidates.dedup();
        balancers.push(ConfiguredXrayBalancer {
            tag: tag.to_owned(),
            selectors,
            candidates,
            strategy: object
                .get("strategy")
                .and_then(Value::as_object)
                .and_then(|strategy| strategy.get("type"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            fallback_target: object
                .get("fallbackTag")
                .and_then(Value::as_str)
                .filter(|target| !target.is_empty())
                .map(str::to_owned),
        });
    }
    balancers.sort_by(|left, right| left.tag.cmp(&right.tag));
    balancers.dedup_by(|left, right| left.tag == right.tag);
    balancers
}

async fn query_xray_balancer(
    tool_runner: &camellia_nexus_core::DynToolRunner,
    executable: &Path,
    working_directory: &Path,
    api_url: &str,
    configured: ConfiguredXrayBalancer,
    alive_outbounds: &HashSet<String>,
) -> Result<XrayBalancerInfo> {
    let arguments = vec![
        "api".to_owned(),
        "bi".to_owned(),
        format!("--server={api_url}"),
        "--timeout=3".to_owned(),
        "--json".to_owned(),
        configured.tag.clone(),
    ];
    let output = run_xray_api(tool_runner, executable, working_directory, &arguments).await?;
    let response: Value = serde_json::from_slice(&output).map_err(|error| {
        camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::UnsupportedBinary,
            "Xray returned an unsupported routing response",
        )
        .with_details(error.to_string())
    })?;
    let balancer = response.get("balancer").and_then(Value::as_object);
    let current_target = balancer
        .and_then(|balancer| balancer.get("override"))
        .and_then(Value::as_object)
        .and_then(|override_info| override_info.get("target"))
        .and_then(Value::as_str)
        .filter(|target| !target.is_empty())
        .map(str::to_owned);
    let principle_targets = balancer
        .and_then(|balancer| {
            balancer
                .get("principleTarget")
                .or_else(|| balancer.get("principle_target"))
        })
        .and_then(Value::as_object)
        .and_then(|targets| targets.get("tag"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|target| !target.is_empty())
        .map(str::to_owned)
        .collect();
    let available_candidates = configured
        .candidates
        .iter()
        .filter(|candidate| alive_outbounds.contains(candidate.as_str()))
        .cloned()
        .collect();
    Ok(XrayBalancerInfo {
        tag: configured.tag,
        selectors: configured.selectors,
        candidates: configured.candidates,
        available_candidates,
        current_target,
        principle_targets,
        strategy: configured.strategy,
        fallback_target: configured.fallback_target,
        error: None,
    })
}

fn xray_balancer_with_error(
    configured: ConfiguredXrayBalancer,
    alive_outbounds: &HashSet<String>,
    error: String,
) -> XrayBalancerInfo {
    let available_candidates = configured
        .candidates
        .iter()
        .filter(|candidate| alive_outbounds.contains(candidate.as_str()))
        .cloned()
        .collect();
    XrayBalancerInfo {
        tag: configured.tag,
        selectors: configured.selectors,
        candidates: configured.candidates,
        available_candidates,
        current_target: None,
        principle_targets: Vec::new(),
        strategy: configured.strategy,
        fallback_target: configured.fallback_target,
        error: Some(error),
    }
}

async fn run_xray_api(
    tool_runner: &camellia_nexus_core::DynToolRunner,
    executable: &Path,
    working_directory: &Path,
    arguments: &[String],
) -> Result<Vec<u8>> {
    let mut plan = CommandPlan::tool(
        executable.to_path_buf(),
        arguments.to_vec(),
        working_directory.to_path_buf(),
    );
    plan.timeout = std::time::Duration::from_secs(5);
    plan.max_output_bytes = 1024 * 1024;
    let output = tool_runner.run(plan).await?;
    if !output.success {
        let details = output.stderr.trim().to_owned();
        return Err(camellia_nexus_core::CamelliaNexusError::new(
            camellia_nexus_core::ErrorCode::InvalidState,
            "Xray rejected the API operation",
        )
        .with_details(details));
    }
    Ok(output.stdout.into_bytes())
}

#[cfg(test)]
mod xray_dashboard_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn available_outbounds_require_a_healthy_observation() {
        let metrics = json!({
            "observatory": {
                "healthy": { "alive": true, "outbound_tag": "proxy-a" },
                "failed": { "alive": false, "outbound_tag": "proxy-b" }
            }
        });
        assert_eq!(
            xray_alive_outbounds(Some(&metrics)),
            HashSet::from(["proxy-a".to_owned()])
        );
    }

    #[test]
    fn online_stats_are_used_only_when_a_level_enables_them() {
        let disabled = json!({ "policy": { "levels": { "0": {} } } });
        let enabled = json!({
            "policy": { "levels": { "0": { "statsUserOnline": true } } }
        });
        assert!(!xray_online_policy_enabled(
            disabled.as_object().expect("object")
        ));
        assert!(xray_online_policy_enabled(
            enabled.as_object().expect("object")
        ));
    }

    #[test]
    fn handler_topology_returns_sorted_unique_tags() {
        let response = br#"{"outbounds":[{"tag":"z"},{"tag":"a"},{"tag":"a"}]}"#;
        assert_eq!(
            xray_handler_tags(response, "outbounds").expect("handler response"),
            ["a", "z"]
        );
    }

    #[test]
    fn online_user_details_preserve_identity_addresses_and_traffic() {
        let response = json!({
            "users": [{
                "email": "user@example.com",
                "ips": [{ "ip": "203.0.113.9", "lastSeen": "1710000000" }],
                "traffic": { "uplink": "1024", "downlink": "2048" }
            }]
        });
        let users = parse_xray_online_users(&response);
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].email, "user@example.com");
        assert_eq!(users[0].online, Some(true));
        assert_eq!(users[0].addresses[0].ip, "203.0.113.9");
        assert_eq!(users[0].addresses[0].last_seen_unix, 1_710_000_000);
        assert_eq!(users[0].uplink, 1024);
        assert_eq!(users[0].downlink, 2048);
    }

    #[test]
    fn configured_shadowsocks_users_and_loopback_scope_are_detected() {
        let config = json!({
            "inbounds": [{
                "listen": "127.0.0.1",
                "protocol": "shadowsocks",
                "settings": {
                    "clients": [
                        { "email": "b@example.com" },
                        { "email": "a@example.com" }
                    ]
                }
            }]
        });
        let (users, loopback_only) = xray_configured_users(config.as_object().expect("object"));
        assert_eq!(users, ["a@example.com", "b@example.com"]);
        assert!(loopback_only);
    }

    #[test]
    fn fallback_online_map_names_are_parsed() {
        assert_eq!(
            xray_online_map_email("user>>>user@example.com>>>online").as_deref(),
            Some("user@example.com")
        );
    }

    #[test]
    fn user_metrics_are_read_independently_from_online_sessions() {
        let metrics = json!({
            "stats": { "user": {
                "user@example.com": { "uplink": 42, "downlink": 84 }
            }}
        });
        assert_eq!(
            xray_user_metrics(Some(&metrics), "user@example.com"),
            Some((42, 84))
        );
    }
}

#[tauri::command]
pub fn get_autostart(app: tauri::AppHandle) -> Result<bool> {
    app.autolaunch().is_enabled().map_err(|error| {
        camellia_nexus_core::CamelliaNexusError::system_integration(
            "Failed to inspect Start at login",
            error,
        )
    })
}

#[tauri::command]
pub fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<()> {
    let manager = app.autolaunch();
    if enabled {
        manager.enable()
    } else {
        manager.disable()
    }
    .map_err(|error| {
        camellia_nexus_core::CamelliaNexusError::system_integration(
            "Failed to update Start at login",
            error,
        )
    })
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(Some(0))
        .collect()
}

#[cfg(windows)]
fn application_signature_status() -> &'static str {
    if std::env::current_exe()
        .ok()
        .as_deref()
        .is_some_and(embedded_signature_is_valid)
    {
        "verified"
    } else {
        "notVerified"
    }
}

#[cfg(windows)]
pub(crate) fn embedded_signature_is_valid(path: &std::path::Path) -> bool {
    embedded_signer_sha256(path).is_some()
}

#[cfg(windows)]
pub(crate) fn embedded_signer_sha256(path: &std::path::Path) -> Option<[u8; 32]> {
    use sha2::{Digest, Sha256};
    use windows::{
        Win32::{
            Foundation::{HANDLE, HWND},
            Security::WinTrust::{
                WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0,
                WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE,
                WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE,
                WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvCertFromChain,
                WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
            },
        },
        core::PCWSTR,
    };

    let executable = wide(&path.to_string_lossy());
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(executable.as_ptr()),
        hFile: HANDLE::default(),
        pgKnownSubject: std::ptr::null_mut(),
    };
    let mut trust = WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL | WTD_REVOCATION_CHECK_NONE,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let status = unsafe {
        WinVerifyTrust(
            HWND::default(),
            &mut action,
            (&mut trust as *mut WINTRUST_DATA).cast(),
        )
    };
    let fingerprint = if status == 0 {
        let provider = unsafe { WTHelperProvDataFromStateData(trust.hWVTStateData) };
        let signer = if provider.is_null() {
            std::ptr::null_mut()
        } else {
            unsafe { WTHelperGetProvSignerFromChain(provider, 0, false, 0) }
        };
        let certificate = if signer.is_null() {
            std::ptr::null_mut()
        } else {
            unsafe { WTHelperGetProvCertFromChain(signer, 0) }
        };
        if certificate.is_null() || unsafe { (*certificate).pCert }.is_null() {
            None
        } else {
            let context = unsafe { &*(*certificate).pCert };
            if context.pbCertEncoded.is_null() || context.cbCertEncoded == 0 {
                None
            } else {
                let encoded = unsafe {
                    std::slice::from_raw_parts(
                        context.pbCertEncoded,
                        context.cbCertEncoded as usize,
                    )
                };
                Some(Sha256::digest(encoded).into())
            }
        }
    } else {
        None
    };
    trust.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe {
        let _ = WinVerifyTrust(
            HWND::default(),
            &mut action,
            (&mut trust as *mut WINTRUST_DATA).cast(),
        );
    }
    fingerprint
}

#[cfg(not(windows))]
fn application_signature_status() -> &'static str {
    "notChecked"
}

pub(crate) fn open_external(target: impl AsRef<OsStr>) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::{
            Win32::{UI::Shell::ShellExecuteW, UI::WindowsAndMessaging::SW_SHOWNORMAL},
            core::{PCWSTR, w},
        };

        let target: Vec<u16> = target.as_ref().encode_wide().chain(Some(0)).collect();
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(target.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        let status = result.0 as isize;
        if status > 32 {
            Ok(())
        } else {
            Err(camellia_nexus_core::CamelliaNexusError::new(
                camellia_nexus_core::ErrorCode::Storage,
                "Windows could not open the requested location",
            )
            .with_details(format!("ShellExecuteW status: {status}")))
        }
    }
    #[cfg(not(windows))]
    {
        #[cfg(target_os = "macos")]
        let mut command = std::process::Command::new("open");
        #[cfg(all(unix, not(target_os = "macos")))]
        let mut command = std::process::Command::new("xdg-open");
        command
            .arg(target.as_ref())
            .spawn()
            .map(|_| ())
            .map_err(camellia_nexus_core::CamelliaNexusError::storage)
    }
}
