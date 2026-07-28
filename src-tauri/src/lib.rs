#[cfg(all(feature = "desktop-e2e", not(debug_assertions)))]
compile_error!(
    "desktop-e2e embeds a local WebDriver and must never be built with release assertions"
);

#[cfg(feature = "desktop")]
mod app_logging;
#[cfg(feature = "desktop")]
mod config_credentials;
#[cfg(feature = "desktop")]
mod config_sources;
#[cfg(any(feature = "desktop", test))]
mod config_update_schedule;
#[cfg(feature = "desktop")]
mod config_updates;
#[cfg(any(feature = "desktop", test))]
mod license_session;
#[cfg(feature = "desktop")]
mod licensing;
mod platform;
mod privilege_broker;
mod privileges;
#[cfg(feature = "desktop")]
mod programs;
mod storage;

#[cfg(feature = "desktop")]
mod commands;
#[cfg(any(feature = "desktop", test))]
mod settings;
#[cfg(feature = "desktop")]
mod tray;
#[cfg(feature = "desktop")]
mod window_state;

pub use platform::{NativeProcessDriver, NativeToolRunner};
pub use storage::FileStore;

pub const APP_AUTHOR: &str = "Camellia Computing";
pub const APP_COPYRIGHT: &str = "Copyright © 2026 Camellia Computing";
pub const APP_LICENSE: &str = "Proprietary";

#[cfg(feature = "desktop")]
const AUTOSTART_ARGUMENT: &str = "--autostart";

#[cfg(feature = "desktop")]
fn has_autostart_argument<I, S>(arguments: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    arguments
        .into_iter()
        .any(|argument| argument.as_ref() == AUTOSTART_ARGUMENT)
}

#[cfg(feature = "desktop")]
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

#[cfg(feature = "desktop")]
use camellia_nexus_core::ProgramManager;
#[cfg(feature = "desktop")]
use tauri::{Emitter, Manager};

#[cfg(any(feature = "desktop", all(test, unix)))]
pub(crate) struct RuntimeAuthorizationCoordinator {
    gate: tokio::sync::RwLock<()>,
}

#[cfg(any(feature = "desktop", all(test, unix)))]
impl RuntimeAuthorizationCoordinator {
    fn new() -> Self {
        Self {
            gate: tokio::sync::RwLock::new(()),
        }
    }

    pub(crate) async fn mutation_permit(&self) -> tokio::sync::RwLockReadGuard<'_, ()> {
        self.gate.read().await
    }

    pub(crate) async fn transition_permit(&self) -> tokio::sync::RwLockWriteGuard<'_, ()> {
        self.gate.write().await
    }
}

#[cfg(feature = "desktop")]
pub(crate) struct AppState {
    pub(crate) manager: Arc<ProgramManager>,
    pub(crate) tool_runner: camellia_nexus_core::DynToolRunner,
    pub(crate) invalid_programs: std::sync::Mutex<Vec<camellia_nexus_core::InvalidProgram>>,
    pub(crate) data_dir: std::path::PathBuf,
    pub(crate) quitting: AtomicBool,
    pub(crate) ui_ready: AtomicBool,
    pub(crate) pending_ui_intent: std::sync::Mutex<Option<commands::UiIntent>>,
    pub(crate) license_authorization_callbacks: Arc<
        std::sync::Mutex<std::collections::BTreeMap<String, camellia_nexus_licensing::SecretValue>>,
    >,
    pub(crate) pending_license_authorizations:
        Arc<std::sync::Mutex<license_session::PendingAuthorizationStore>>,
    pub(crate) license_http_api: tokio::sync::OnceCell<camellia_nexus_licensing::HttpLicenseApi>,
    pub(crate) license_session_operation: tokio::sync::Mutex<()>,
    pub(crate) runtime_authorization: RuntimeAuthorizationCoordinator,
    pub(crate) license_auto_start_armed: AtomicBool,
    pub(crate) license_state_generation: AtomicU64,
    pub(crate) frontend_log_limiter: std::sync::Mutex<(std::time::Instant, u32)>,
    pub(crate) window_state: window_state::WindowStateTracker,
    pub(crate) settings: Arc<settings::SettingsStore>,
    pub(crate) config_refreshes: Arc<config_updates::RefreshCoordinator>,
    pub(crate) config_credentials: Arc<config_credentials::ConfigCredentialVault>,
    pub(crate) authorization: Arc<camellia_nexus_licensing::AuthorizationService>,
}

#[cfg(feature = "desktop")]
pub fn open_main_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        if window
            .unminimize()
            .and_then(|_| window.show())
            .and_then(|_| window.set_focus())
            .is_ok()
        {
            return Ok(());
        }
        if let Some(state) = app.try_state::<AppState>() {
            state.ui_ready.store(false, Ordering::Release);
        }
        let _ = window.destroy();
    }
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == "main")
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    let window = tauri::WebviewWindowBuilder::from_config(app, config)?.build()?;
    if let Some(state) = app.try_state::<AppState>()
        && let Err(error) = window_state::restore(&window, &state.data_dir, &state.window_state)
    {
        tracing::warn!(%error, "could not restore window state");
    }
    window.show()?;
    window.set_focus()?;
    Ok(())
}

#[cfg(feature = "desktop")]
pub(crate) async fn shutdown_and_exit(app: tauri::AppHandle, manager: Arc<ProgramManager>) {
    let report = manager.shutdown().await;
    if let Err(error) = privilege_broker::end_session().await {
        tracing::warn!(%error, "privilege broker session did not close cleanly");
    }
    let exit_code = if report.succeeded() { 0 } else { 1 };
    for (program_id, error) in &report.failures {
        tracing::error!(program = %program_id, %error, "program did not stop cleanly during application shutdown");
    }
    if report.timed_out {
        tracing::error!("application shutdown exceeded the 25-second safety window");
    }
    app.exit(exit_code);
}

#[cfg(feature = "desktop")]
fn init_logging(data_dir: &std::path::Path, level: settings::AppLogLevel) {
    use tracing_subscriber::EnvFilter;

    let logs_dir = data_dir.join("logs");
    let writer = app_logging::RotatingLogWriter::new(logs_dir, "camellia-nexus.log");
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level.as_filter()));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true)
        .with_writer(writer)
        .try_init();
}

#[cfg(feature = "desktop")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(feature = "desktop-e2e")]
    let builder = builder
        .plugin(tauri_plugin_wdio::init())
        .plugin(tauri_plugin_wdio_webdriver::init());
    #[cfg(not(feature = "desktop-e2e"))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
        if !has_autostart_argument(args.iter().map(String::as_str)) {
            let _ = open_main_window(app);
        }
    }));
    let builder = builder.plugin(
        tauri_plugin_autostart::Builder::new()
            .app_name("Camellia Nexus")
            .arg(AUTOSTART_ARGUMENT)
            .build(),
    );
    let builder = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init());
    let builder = builder
        .setup(|app| {
            let data_dir = application_data_directory(app)?;
            let settings = Arc::new(settings::SettingsStore::load(&data_dir));
            init_logging(&data_dir, settings.current().log_level);
            tracing::info!(
                path = %data_dir.join("logs").join("camellia-nexus.log").display(),
                "application logging initialized"
            );
            if let Some(error) = settings.load_issue() {
                tracing::error!(details = ?error.details, "application settings could not be loaded; safe defaults are active");
            }
            let config_refreshes = Arc::new(config_updates::RefreshCoordinator::default());
            let config_credentials = Arc::new(
                config_credentials::ConfigCredentialVault::new(&data_dir),
            );
            let authorization = licensing::initialize();
            let app_settings = settings.current();
            let store = Arc::new(
                FileStore::new(data_dir.clone()).map_err(Box::<dyn std::error::Error>::from)?,
            );
            let tool_runner: camellia_nexus_core::DynToolRunner =
                Arc::new(NativeToolRunner::default());
            let manager = ProgramManager::new(
                Arc::new(NativeProcessDriver::new(settings.clear_logs_on_start())),
                store.clone(),
                store,
                tool_runner.clone(),
            );
            let report = tauri::async_runtime::block_on(manager.initialize_without_auto_start())
                .map_err(Box::<dyn std::error::Error>::from)?;
            for invalid in &report.invalid {
                tracing::warn!(path = %invalid.path.display(), error = %invalid.error, "ignored invalid program workspace");
            }
            if let Err(error) = tauri::async_runtime::block_on(config_credentials.recover(&manager))
            {
                tracing::error!(%error, "configuration credential recovery is unavailable; credential-dependent updates remain disabled");
            }
            let license_runtime_impact = commands::license_runtime_impact(
                &authorization.state_at(licensing::unix_now()),
            );
            let license_active =
                license_runtime_impact == commands::LicenseRuntimeImpact::Active;
            tauri::async_runtime::block_on(
                manager.set_automatic_restarts_enabled(license_active),
            );
            if license_active {
                commands::schedule_program_auto_start(
                    manager.clone(),
                    std::time::Duration::from_millis(app_settings.program_startup_delay_ms),
                    "application_startup",
                );
            } else {
                tracing::info!("license is not active; skipping managed program autostart");
            }
            app.manage(AppState {
                manager: manager.clone(),
                tool_runner,
                invalid_programs: std::sync::Mutex::new(report.invalid),
                data_dir: data_dir.clone(),
                quitting: AtomicBool::new(false),
                ui_ready: AtomicBool::new(false),
                pending_ui_intent: std::sync::Mutex::new(None),
                license_authorization_callbacks: Arc::new(std::sync::Mutex::new(
                    std::collections::BTreeMap::new(),
                )),
                pending_license_authorizations: Arc::new(std::sync::Mutex::new(
                    license_session::PendingAuthorizationStore::default(),
                )),
                license_http_api: tokio::sync::OnceCell::new(),
                license_session_operation: tokio::sync::Mutex::new(()),
                runtime_authorization: RuntimeAuthorizationCoordinator::new(),
                license_auto_start_armed: AtomicBool::new(!license_active),
                license_state_generation: AtomicU64::new(0),
                frontend_log_limiter: std::sync::Mutex::new((std::time::Instant::now(), 0)),
                window_state: window_state::WindowStateTracker::default(),
                settings,
                config_refreshes: config_refreshes.clone(),
                config_credentials,
                authorization,
            });
            if let Some(window) = app.get_webview_window("main") {
                if let Some(state) = app.try_state::<AppState>()
                    && let Err(error) =
                        window_state::restore(&window, &data_dir, &state.window_state)
                {
                    tracing::warn!(%error, "could not restore window state");
                }
                if !started_at_login() {
                    window.show()?;
                    window.set_focus()?;
                }
            }
            tray::create(app.handle(), manager.clone())?;
            config_updates::spawn_scheduler(
                app.handle().clone(),
                manager.clone(),
                config_refreshes,
            );

            let app_handle = app.handle().clone();
            let mut events = manager.subscribe();
            tauri::async_runtime::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(event) => {
                            let _ = app_handle.emit("manager-event", &event);
                            let _ = tray::refresh(&app_handle, manager.clone()).await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let event = camellia_nexus_core::ManagerEvent::ProgramListChanged;
                            let _ = app_handle.emit("manager-event", &event);
                            let _ = tray::refresh(&app_handle, manager.clone()).await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            let license_monitor_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    if license_monitor_app
                        .try_state::<AppState>()
                        .is_some_and(|state| state.quitting.load(Ordering::Acquire))
                    {
                        break;
                    }
                    let _ = commands::synchronize_license_runtime(
                        &license_monitor_app,
                        "license_monitor",
                    )
                    .await;
                    tokio::time::sleep(commands::next_license_enforcement_delay(
                        &license_monitor_app,
                    ))
                    .await;
                }
            });
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut next_online_attempt = std::time::Instant::now();
                let mut consecutive_failures = 0_u32;
                loop {
                    if app_handle
                        .try_state::<AppState>()
                        .is_some_and(|state| state.quitting.load(Ordering::Acquire))
                    {
                        break;
                    }
                    if std::time::Instant::now() >= next_online_attempt {
                        match commands::maintain_license_online(&app_handle).await {
                            commands::LicenseMaintenanceOutcome::Idle => {
                                consecutive_failures = 0;
                                next_online_attempt = std::time::Instant::now()
                                    + randomized_delay(5 * 60, 30);
                            }
                            commands::LicenseMaintenanceOutcome::Succeeded => {
                                consecutive_failures = 0;
                                next_online_attempt = std::time::Instant::now()
                                    + randomized_delay(5 * 60, 30);
                            }
                            commands::LicenseMaintenanceOutcome::TransientFailure {
                                retry_after_seconds,
                            } => {
                                consecutive_failures =
                                    consecutive_failures.saturating_add(1).min(5);
                                let delay = 30_u64
                                    .saturating_mul(
                                        1_u64 << consecutive_failures.saturating_sub(1),
                                    )
                                    .min(15 * 60);
                                let local_delay = randomized_delay(delay, delay / 2);
                                let server_delay = std::time::Duration::from_secs(
                                    retry_after_seconds.unwrap_or_default(),
                                );
                                next_online_attempt = std::time::Instant::now()
                                    + local_delay.max(server_delay);
                            }
                        }
                    }
                    let delay = next_online_attempt
                        .saturating_duration_since(std::time::Instant::now())
                        .min(std::time::Duration::from_secs(30));
                    tokio::time::sleep(delay.max(std::time::Duration::from_millis(100))).await;
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_programs,
            commands::get_program_privilege_assessment,
            commands::log_frontend_event,
            commands::get_application_info,
            commands::get_entitlement_state,
            commands::get_local_license_device,
            commands::get_license_service_settings,
            commands::begin_license_authorization,
            commands::take_license_authorization_callback,
            commands::cancel_license_authorization,
            commands::complete_license_authorization,
            commands::refresh_license_entitlement,
            commands::reconnect_license_device,
            commands::get_license_devices,
            commands::get_license_billing_summary,
            commands::submit_license_payment_claim,
            commands::get_license_team_profile,
            commands::get_license_team_members,
            commands::create_license_team_invitation,
            commands::accept_license_team_invitation,
            commands::update_license_team_member,
            commands::create_license_team_device_enrollment,
            commands::create_license_team_member_device_enrollment,
            commands::accept_license_team_device_enrollment,
            commands::leave_license_team_workspace,
            commands::transfer_license_team_ownership,
            commands::get_license_workspace_configurations,
            commands::get_license_workspace_configuration,
            commands::create_license_workspace_configuration,
            commands::revise_license_workspace_configuration,
            commands::publish_license_workspace_configuration,
            commands::delete_license_workspace_configuration,
            commands::restore_license_workspace_configuration,
            commands::purge_license_workspace_configuration,
            commands::get_license_workspace_sync_feed,
            commands::get_license_workspace_checkpoint,
            commands::advance_license_workspace_checkpoint,
            commands::get_license_workspace_alert_rules,
            commands::create_license_workspace_alert_rule,
            commands::update_license_workspace_alert_rule,
            commands::delete_license_workspace_alert_rule,
            commands::get_license_workspace_alert_incidents,
            commands::acknowledge_license_workspace_alert_incident,
            commands::resolve_license_workspace_alert_incident,
            commands::get_license_workspace_audit_events,
            commands::get_license_workspace_audit_event_types,
            commands::export_license_workspace_audit_events,
            commands::get_license_workspace_webhook_endpoints,
            commands::create_license_workspace_webhook_endpoint,
            commands::update_license_workspace_webhook_endpoint,
            commands::rotate_license_workspace_webhook_endpoint,
            commands::delete_license_workspace_webhook_endpoint,
            commands::get_license_workspace_webhook_deliveries,
            commands::remove_license_device,
            commands::logout_license_session,
            commands::reset_license_device_identity,
            commands::get_program,
            commands::list_invalid_programs,
            commands::create_program,
            commands::update_program,
            commands::update_program_and_restart,
            commands::update_program_and_refresh_config,
            commands::remove_program,
            commands::start_program,
            commands::stop_program,
            commands::restart_program,
            commands::replace_package,
            commands::list_actions,
            commands::run_action,
            commands::load_config,
            commands::load_configuration_schema,
            commands::validate_config,
            commands::apply_config,
            commands::refresh_config_sources,
            commands::read_logs,
            commands::clear_logs,
            commands::open_working_directory,
            commands::open_data_directory,
            commands::open_app_log_directory,
            commands::open_documentation,
            commands::open_sing_box_dashboard,
            commands::open_mihomo_dashboard,
            commands::get_xray_dashboard_snapshot,
            commands::set_xray_balancer_target,
            commands::restart_xray_logger,
            commands::get_autostart,
            commands::set_autostart,
            commands::get_app_settings,
            commands::set_app_settings,
            commands::frontend_ready,
        ]);

    let app = builder
        .build(tauri::generate_context!())
        .expect("failed to build Camellia Nexus");
    app.run(|app, event| match event {
        tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_),
            ..
        } if label == "main" => {
            if let (Some(window), Some(state)) =
                (app.get_webview_window("main"), app.try_state::<AppState>())
            {
                window_state::remember_normal(&window, &state.window_state);
            }
        }
        tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::CloseRequested { api, .. },
            ..
        } if label == "main" => {
            if let (Some(window), Some(state)) =
                (app.get_webview_window("main"), app.try_state::<AppState>())
            {
                window_state::save(&window, &state.data_dir, &state.window_state);
                if !state.quitting.load(Ordering::Acquire) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        }
        tauri::RunEvent::ExitRequested { api, .. } => {
            let state = app.try_state::<AppState>();
            if let (Some(window), Some(state)) = (app.get_webview_window("main"), state.as_ref()) {
                window_state::save(&window, &state.data_dir, &state.window_state);
            }
            if let Some(state) = state
                && !state.quitting.swap(true, Ordering::AcqRel)
            {
                api.prevent_exit();
                let manager = state.manager.clone();
                let app = app.clone();
                tauri::async_runtime::spawn(shutdown_and_exit(app, manager));
            }
        }
        _ => {}
    });
}

#[cfg(feature = "desktop")]
fn application_data_directory(
    app: &tauri::App,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    #[cfg(feature = "desktop-e2e")]
    {
        let _ = app;
        let value = std::env::var_os("CAMELLIA_NEXUS_E2E_DATA_DIR")
            .ok_or("CAMELLIA_NEXUS_E2E_DATA_DIR is required by desktop-e2e builds")?;
        let path = std::path::PathBuf::from(value);
        if !path.is_absolute() {
            return Err("CAMELLIA_NEXUS_E2E_DATA_DIR must be absolute".into());
        }
        Ok(path)
    }
    #[cfg(all(not(feature = "desktop-e2e"), windows))]
    {
        let _ = app;
        return std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "LOCALAPPDATA is unavailable")
            })
            .map(|path| path.join("camellia-nexus"))
            .map_err(Into::into);
    }
    #[cfg(all(not(feature = "desktop-e2e"), not(windows)))]
    Ok(app.path().app_local_data_dir()?)
}

#[cfg(feature = "desktop")]
fn started_at_login() -> bool {
    has_autostart_argument(std::env::args_os())
}

#[cfg(feature = "desktop")]
fn randomized_delay(base_seconds: u64, spread_seconds: u64) -> std::time::Duration {
    let random = u64::from_le_bytes(
        uuid::Uuid::new_v4().as_bytes()[..8]
            .try_into()
            .expect("UUID prefix has a fixed length"),
    );
    let width = spread_seconds.saturating_mul(2).saturating_add(1);
    let seconds = base_seconds
        .saturating_sub(spread_seconds)
        .saturating_add(random % width.max(1));
    std::time::Duration::from_secs(seconds)
}

#[cfg(all(test, feature = "desktop"))]
mod startup_contract_tests {
    use super::{AUTOSTART_ARGUMENT, has_autostart_argument};

    #[test]
    fn recognizes_only_the_canonical_autostart_argument() {
        assert_eq!(AUTOSTART_ARGUMENT, "--autostart");
        assert!(has_autostart_argument(["--quiet", AUTOSTART_ARGUMENT]));
        assert!(!has_autostart_argument(["--quiet", "--startup-bridge"]));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        collections::BTreeMap,
        io::Write,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use camellia_nexus_core::{
        CamelliaNexusError, CreateProgramRequest, ErrorCode, ExecutableSpec, LaunchPlan,
        ManagedProcess, ManagerEvent, PrivilegePolicy, ProcessDriver, ProcessExit, ProgramId,
        ProgramManager, ProgramSpec, ProgramState, ProgramType, RestartPolicy, SCHEMA_VERSION,
    };

    use crate::{
        FileStore, NativeProcessDriver, NativeToolRunner, RuntimeAuthorizationCoordinator,
    };

    const TEST_PROCESS_EXIT: ProcessExit = ProcessExit {
        code: Some(143),
        success: false,
    };

    #[tokio::test]
    async fn sing_box_schema_loading_is_deduplicated_and_bound_to_the_binary() {
        let directory = tempfile::tempdir().expect("tempdir");
        let binary = directory.path().join("fake-sing-box");
        std::fs::write(
            &binary,
            r#"#!/bin/sh
counter="$(dirname "$0")/schema-count"
case "$1" in
  version)
    echo "sing-box version 1.14.0"
    ;;
  check)
    if [ "$2" = "--help" ]; then echo "-c --config -D --directory"; fi
    ;;
  format)
    if [ "$2" = "--help" ]; then echo "-w"; fi
    ;;
  schema)
    count=0
    if [ -f "$counter" ]; then count="$(cat "$counter")"; fi
    count=$((count + 1))
    echo "$count" > "$counter"
    echo '{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","additionalProperties":false}'
    ;;
esac
"#,
        )
        .expect("write fake sing-box");
        let mut permissions = std::fs::metadata(&binary)
            .expect("binary metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&binary, permissions).expect("binary permissions");

        let store =
            Arc::new(FileStore::new(directory.path().join("store")).expect("create file store"));
        let manager = ProgramManager::new(
            Arc::new(NativeProcessDriver::default()),
            store.clone(),
            store,
            Arc::new(NativeToolRunner::default()),
        );
        manager.initialize().await.expect("initialize");
        let id = ProgramId::parse("schema-cache-fixture").expect("program id");
        manager
            .create(CreateProgramRequest {
                spec: ProgramSpec {
                    schema_version: SCHEMA_VERSION,
                    id: id.clone(),
                    name: "Schema cache fixture".into(),
                    executable: ExecutableSpec::External {
                        path: binary.clone(),
                        metadata: None,
                    },
                    program_type: ProgramType::SingBox {
                        main_config: Some(PathBuf::from("config/config.json")),
                        extra_args: Vec::new(),
                    },
                    managed_config: None,
                    working_directory: directory.path().to_path_buf(),
                    environment: BTreeMap::new(),
                    auto_start: false,
                    restart_policy: RestartPolicy::Never,
                    privilege_policy: Default::default(),
                },
                package_source: None,
                initial_config: Some(r#"{"log":{"level":"info"}}"#.into()),
            })
            .await
            .expect("create sing-box fixture");

        let config = manager.load_config(&id).await.expect("load configuration");
        assert!(config.configuration_schema.is_some());
        let (first, concurrent) = tokio::join!(
            manager.load_configuration_schema(&id),
            manager.load_configuration_schema(&id),
        );
        let first = first.expect("first schema").expect("supported schema");
        let concurrent = concurrent
            .expect("concurrent schema")
            .expect("supported schema");
        assert_eq!(first, concurrent);
        assert_eq!(
            std::fs::read_to_string(directory.path().join("schema-count"))
                .expect("schema count")
                .trim(),
            "1"
        );
        assert_eq!(
            manager
                .load_configuration_schema(&id)
                .await
                .expect("cached schema")
                .expect("supported schema"),
            first
        );

        std::fs::OpenOptions::new()
            .append(true)
            .open(&binary)
            .expect("open binary")
            .write_all(b"\n")
            .expect("change binary identity");
        manager
            .load_configuration_schema(&id)
            .await
            .expect("schema after binary change")
            .expect("supported schema");
        assert_eq!(
            std::fs::read_to_string(directory.path().join("schema-count"))
                .expect("schema count")
                .trim(),
            "2"
        );
    }

    #[tokio::test]
    async fn authorization_transition_waits_for_an_in_flight_commit() {
        let coordinator = Arc::new(RuntimeAuthorizationCoordinator::new());
        let mutation = coordinator.mutation_permit().await;
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let completed = Arc::new(AtomicUsize::new(0));
        let task = {
            let coordinator = coordinator.clone();
            let barrier = barrier.clone();
            let completed = completed.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                let _transition = coordinator.transition_permit().await;
                completed.store(1, Ordering::Release);
            })
        };
        barrier.wait().await;
        tokio::task::yield_now().await;
        assert_eq!(completed.load(Ordering::Acquire), 0);
        drop(mutation);
        task.await.expect("transition task");
        assert_eq!(completed.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn protected_commit_waits_for_an_in_flight_authorization_transition() {
        let coordinator = Arc::new(RuntimeAuthorizationCoordinator::new());
        let transition = coordinator.transition_permit().await;
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let completed = Arc::new(AtomicUsize::new(0));
        let task = {
            let coordinator = coordinator.clone();
            let barrier = barrier.clone();
            let completed = completed.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                let _mutation = coordinator.mutation_permit().await;
                completed.store(1, Ordering::Release);
            })
        };
        barrier.wait().await;
        tokio::task::yield_now().await;
        assert_eq!(completed.load(Ordering::Acquire), 0);
        drop(transition);
        task.await.expect("mutation task");
        assert_eq!(completed.load(Ordering::Acquire), 1);
    }

    struct FailFirstStopDriver;

    #[async_trait::async_trait]
    impl ProcessDriver for FailFirstStopDriver {
        async fn spawn(
            &self,
            plan: LaunchPlan,
        ) -> camellia_nexus_core::Result<Box<dyn ManagedProcess>> {
            let inner = NativeProcessDriver::default().spawn(plan).await?;
            Ok(Box::new(FailFirstStopProcess {
                inner,
                fail_next_stop: true,
            }))
        }
    }

    struct FailFirstStopProcess {
        inner: Box<dyn ManagedProcess>,
        fail_next_stop: bool,
    }

    #[async_trait::async_trait]
    impl ManagedProcess for FailFirstStopProcess {
        fn pid(&self) -> u32 {
            self.inner.pid()
        }

        async fn wait(&mut self) -> camellia_nexus_core::Result<ProcessExit> {
            self.inner.wait().await
        }

        async fn stop(&mut self) -> camellia_nexus_core::Result<ProcessExit> {
            if std::mem::take(&mut self.fail_next_stop) {
                return Err(CamelliaNexusError::new(
                    ErrorCode::StopFailed,
                    "simulated transient stop failure",
                ));
            }
            self.inner.stop().await
        }
    }

    struct ExitOnFailedStopDriver {
        spawn_count: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl ProcessDriver for ExitOnFailedStopDriver {
        async fn spawn(
            &self,
            _plan: LaunchPlan,
        ) -> camellia_nexus_core::Result<Box<dyn ManagedProcess>> {
            self.spawn_count.fetch_add(1, Ordering::AcqRel);
            Ok(Box::new(ExitOnFailedStopProcess { exited: false }))
        }
    }

    struct ExitOnFailedStopProcess {
        exited: bool,
    }

    #[async_trait::async_trait]
    impl ManagedProcess for ExitOnFailedStopProcess {
        fn pid(&self) -> u32 {
            42
        }

        async fn wait(&mut self) -> camellia_nexus_core::Result<ProcessExit> {
            if self.exited {
                return Ok(TEST_PROCESS_EXIT);
            }
            std::future::pending().await
        }

        async fn stop(&mut self) -> camellia_nexus_core::Result<ProcessExit> {
            self.exited = true;
            Err(CamelliaNexusError::new(
                ErrorCode::StopFailed,
                "simulated error after the process exited",
            ))
        }
    }

    struct NeverStopDriver;

    #[async_trait::async_trait]
    impl ProcessDriver for NeverStopDriver {
        async fn spawn(
            &self,
            _plan: LaunchPlan,
        ) -> camellia_nexus_core::Result<Box<dyn ManagedProcess>> {
            Ok(Box::new(NeverStopProcess))
        }
    }

    struct RejectUnattendedElevationDriver;

    #[async_trait::async_trait]
    impl ProcessDriver for RejectUnattendedElevationDriver {
        async fn spawn(
            &self,
            plan: LaunchPlan,
        ) -> camellia_nexus_core::Result<Box<dyn ManagedProcess>> {
            if !plan.interactive {
                return Err(CamelliaNexusError::new(
                    ErrorCode::PrivilegeRequired,
                    "simulated interactive administrator authorization",
                ));
            }
            Ok(Box::new(NeverStopProcess))
        }
    }

    struct NeverStopProcess;

    #[async_trait::async_trait]
    impl ManagedProcess for NeverStopProcess {
        fn pid(&self) -> u32 {
            43
        }

        async fn wait(&mut self) -> camellia_nexus_core::Result<ProcessExit> {
            std::future::pending().await
        }

        async fn stop(&mut self) -> camellia_nexus_core::Result<ProcessExit> {
            Err(CamelliaNexusError::new(
                ErrorCode::StopFailed,
                "simulated persistent stop failure",
            ))
        }
    }

    struct BlockingStopDriver {
        stop_entered: Arc<tokio::sync::Notify>,
        release_stop: Arc<tokio::sync::Notify>,
    }

    #[async_trait::async_trait]
    impl ProcessDriver for BlockingStopDriver {
        async fn spawn(
            &self,
            _plan: LaunchPlan,
        ) -> camellia_nexus_core::Result<Box<dyn ManagedProcess>> {
            Ok(Box::new(BlockingStopProcess {
                exited: false,
                stop_entered: self.stop_entered.clone(),
                release_stop: self.release_stop.clone(),
            }))
        }
    }

    struct BlockingStopProcess {
        exited: bool,
        stop_entered: Arc<tokio::sync::Notify>,
        release_stop: Arc<tokio::sync::Notify>,
    }

    #[async_trait::async_trait]
    impl ManagedProcess for BlockingStopProcess {
        fn pid(&self) -> u32 {
            44
        }

        async fn wait(&mut self) -> camellia_nexus_core::Result<ProcessExit> {
            if self.exited {
                return Ok(TEST_PROCESS_EXIT);
            }
            std::future::pending().await
        }

        async fn stop(&mut self) -> camellia_nexus_core::Result<ProcessExit> {
            self.stop_entered.notify_one();
            self.release_stop.notified().await;
            self.exited = true;
            Ok(TEST_PROCESS_EXIT)
        }
    }

    async fn create_test_program(
        manager: &Arc<ProgramManager>,
        id: &str,
        executable: PathBuf,
        restart_policy: RestartPolicy,
    ) -> ProgramId {
        let id = ProgramId::parse(id).expect("id");
        manager
            .create(CreateProgramRequest {
                spec: ProgramSpec {
                    schema_version: SCHEMA_VERSION,
                    id: id.clone(),
                    name: id.as_str().into(),
                    executable: ExecutableSpec::External {
                        path: executable,
                        metadata: None,
                    },
                    program_type: ProgramType::Generic { args: Vec::new() },
                    managed_config: None,
                    working_directory: PathBuf::from("/bin"),
                    environment: BTreeMap::new(),
                    auto_start: false,
                    restart_policy,
                    privilege_policy: Default::default(),
                },
                package_source: None,
                initial_config: None,
            })
            .await
            .expect("create test program");
        id
    }

    async fn create_auto_start_program(
        manager: &Arc<ProgramManager>,
        id: &str,
        executable: PathBuf,
    ) -> ProgramId {
        let id = ProgramId::parse(id).expect("id");
        manager
            .create(CreateProgramRequest {
                spec: ProgramSpec {
                    schema_version: SCHEMA_VERSION,
                    id: id.clone(),
                    name: id.as_str().into(),
                    executable: ExecutableSpec::External {
                        path: executable,
                        metadata: None,
                    },
                    program_type: ProgramType::Generic {
                        args: vec!["-c".into(), "sleep 30".into()],
                    },
                    managed_config: None,
                    working_directory: PathBuf::from("/bin"),
                    environment: BTreeMap::new(),
                    auto_start: true,
                    restart_policy: RestartPolicy::Never,
                    privilege_policy: Default::default(),
                },
                package_source: None,
                initial_config: None,
            })
            .await
            .expect("create auto-start test program");
        id
    }

    #[tokio::test]
    async fn auto_start_reconciliation_is_idempotent() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(FileStore::new(directory.path().to_path_buf()).expect("store"));
        let manager = ProgramManager::new(
            Arc::new(NativeProcessDriver::default()),
            store.clone(),
            store,
            Arc::new(NativeToolRunner::default()),
        );
        manager
            .initialize_without_auto_start()
            .await
            .expect("initialize");
        let id = create_auto_start_program(&manager, "auto-start-once", "/bin/sh".into()).await;

        let first = manager
            .reconcile_auto_start_programs(std::time::Duration::ZERO)
            .await;
        let retry = manager
            .reconcile_auto_start_programs(std::time::Duration::ZERO)
            .await;

        assert_eq!(first.started, 1);
        assert_eq!(first.failed, 0);
        assert_eq!(retry.started, 0);
        assert_eq!(retry.already_active, 1);
        manager.stop(&id).await.expect("stop");
    }

    #[tokio::test]
    async fn elevated_auto_start_is_skipped_and_reported_for_manual_recovery() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(FileStore::new(directory.path().to_path_buf()).expect("store"));
        let manager = ProgramManager::new(
            Arc::new(RejectUnattendedElevationDriver),
            store.clone(),
            store,
            Arc::new(NativeToolRunner::default()),
        );
        manager
            .initialize_without_auto_start()
            .await
            .expect("initialize");
        let id =
            create_auto_start_program(&manager, "auto-start-needs-admin", "/bin/sh".into()).await;
        let (mut spec, _) = manager.get(&id).await.expect("program");
        spec.privilege_policy = PrivilegePolicy::Elevated;
        manager.update(spec).await.expect("update policy");
        let mut events = manager.subscribe();

        let report = manager
            .reconcile_auto_start_programs(std::time::Duration::ZERO)
            .await;
        assert_eq!(report.failed_program_ids, vec![id.clone()]);
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if let ManagerEvent::ProgramAutoStartPrivilegeRequired { ids } =
                    events.recv().await.expect("manager event")
                {
                    break ids;
                }
            }
        })
        .await
        .expect("privilege event");
        assert_eq!(event, vec![id]);
    }

    #[tokio::test]
    async fn manual_stop_cancels_a_pending_auto_start() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(FileStore::new(directory.path().to_path_buf()).expect("store"));
        let manager = ProgramManager::new(
            Arc::new(NativeProcessDriver::default()),
            store.clone(),
            store,
            Arc::new(NativeToolRunner::default()),
        );
        manager
            .initialize_without_auto_start()
            .await
            .expect("initialize");
        let id = create_auto_start_program(&manager, "auto-start-cancel", "/bin/sh".into()).await;
        let task = {
            let manager = manager.clone();
            tokio::spawn(async move {
                manager
                    .reconcile_auto_start_programs(std::time::Duration::from_millis(100))
                    .await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        manager.stop(&id).await.expect("manual stop");
        let report = task.await.expect("reconciliation task");

        assert_eq!(report.started, 0);
        assert_eq!(report.skipped, 1);
        assert!(!matches!(
            manager.get(&id).await.expect("program").1,
            ProgramState::Running { .. } | ProgramState::Starting
        ));
    }

    #[tokio::test]
    async fn manager_runs_a_generic_program_end_to_end() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(FileStore::new(directory.path().to_path_buf()).expect("store"));
        let manager = ProgramManager::new(
            Arc::new(NativeProcessDriver::default()),
            store.clone(),
            store,
            Arc::new(NativeToolRunner::default()),
        );
        manager.initialize().await.expect("initialize");
        let id = ProgramId::parse("managed-fixture").expect("id");
        manager
            .create(CreateProgramRequest {
                spec: ProgramSpec {
                    schema_version: SCHEMA_VERSION,
                    id: id.clone(),
                    name: "Managed fixture".into(),
                    executable: ExecutableSpec::External {
                        path: PathBuf::from("/bin/sh"),
                        metadata: None,
                    },
                    program_type: ProgramType::Generic {
                        args: vec!["-c".into(), "sleep 30".into()],
                    },
                    managed_config: None,
                    working_directory: PathBuf::from("."),
                    environment: BTreeMap::new(),
                    auto_start: false,
                    restart_policy: RestartPolicy::Never,
                    privilege_policy: Default::default(),
                },
                package_source: None,
                initial_config: None,
            })
            .await
            .expect("create");
        manager.start(&id).await.expect("start");
        manager.start(&id).await.expect("idempotent start");
        assert!(matches!(
            manager.get(&id).await.expect("get").1,
            ProgramState::Running { .. }
        ));
        manager.stop(&id).await.expect("stop");
        assert_eq!(manager.list().await.len(), 1);
        manager.remove(&id).await.expect("remove");
        assert!(manager.list().await.is_empty());
    }

    #[tokio::test]
    async fn failed_stop_keeps_process_managed_until_fail_closed_retry_succeeds() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(FileStore::new(directory.path().to_path_buf()).expect("store"));
        let manager = ProgramManager::new(
            Arc::new(FailFirstStopDriver),
            store.clone(),
            store,
            Arc::new(NativeToolRunner::default()),
        );
        manager.initialize().await.expect("initialize");
        let id = ProgramId::parse("stop-retry-fixture").expect("id");
        manager
            .create(CreateProgramRequest {
                spec: ProgramSpec {
                    schema_version: SCHEMA_VERSION,
                    id: id.clone(),
                    name: "Stop retry fixture".into(),
                    executable: ExecutableSpec::External {
                        path: PathBuf::from("/bin/sh"),
                        metadata: None,
                    },
                    program_type: ProgramType::Generic {
                        args: vec!["-c".into(), "sleep 30".into()],
                    },
                    managed_config: None,
                    working_directory: PathBuf::from("/bin"),
                    environment: BTreeMap::new(),
                    auto_start: false,
                    restart_policy: RestartPolicy::Never,
                    privilege_policy: Default::default(),
                },
                package_source: None,
                initial_config: None,
            })
            .await
            .expect("create");
        manager.start(&id).await.expect("start");

        let error = manager.stop(&id).await.expect_err("first stop must fail");
        assert_eq!(error.code, ErrorCode::StopFailed);
        assert!(matches!(
            manager.get(&id).await.expect("state after failed stop").1,
            ProgramState::StopFailed { .. }
        ));

        let report = manager.disable_automatic_restarts_and_stop_active().await;
        assert_eq!(report.stopped, 1);
        assert_eq!(report.failed, 0);
        assert_eq!(
            manager.get(&id).await.expect("state after retry").1,
            ProgramState::Stopped
        );
    }

    async fn assert_exit_after_failed_stop_does_not_restart(restart_policy: RestartPolicy) {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(FileStore::new(directory.path().to_path_buf()).expect("store"));
        let spawn_count = Arc::new(AtomicUsize::new(0));
        let manager = ProgramManager::new(
            Arc::new(ExitOnFailedStopDriver {
                spawn_count: spawn_count.clone(),
            }),
            store.clone(),
            store,
            Arc::new(NativeToolRunner::default()),
        );
        manager.initialize().await.expect("initialize");
        let id = create_test_program(
            &manager,
            "failed-stop-exit",
            PathBuf::from("/bin/sh"),
            restart_policy,
        )
        .await;
        manager.start(&id).await.expect("start");

        let error = manager
            .stop(&id)
            .await
            .expect_err("stop must report failure");
        assert_eq!(error.code, ErrorCode::StopFailed);
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if !matches!(
                    manager.get(&id).await.expect("state after failed stop").1,
                    ProgramState::StopFailed { .. }
                ) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("observe the eventual process exit");

        assert_eq!(
            manager.get(&id).await.expect("final state").1,
            ProgramState::Stopped
        );
        assert_eq!(spawn_count.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn failed_stop_suppresses_always_and_on_failure_restarts_after_late_exit() {
        assert_exit_after_failed_stop_does_not_restart(RestartPolicy::Always).await;
        assert_exit_after_failed_stop_does_not_restart(RestartPolicy::OnFailure).await;
    }

    #[tokio::test]
    async fn failed_restart_stop_phase_keeps_restart_suppressed_after_late_exit() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(FileStore::new(directory.path().to_path_buf()).expect("store"));
        let spawn_count = Arc::new(AtomicUsize::new(0));
        let manager = ProgramManager::new(
            Arc::new(ExitOnFailedStopDriver {
                spawn_count: spawn_count.clone(),
            }),
            store.clone(),
            store,
            Arc::new(NativeToolRunner::default()),
        );
        manager.initialize().await.expect("initialize");
        let id = create_test_program(
            &manager,
            "failed-restart-stop",
            PathBuf::from("/bin/sh"),
            RestartPolicy::Always,
        )
        .await;
        manager.start(&id).await.expect("start");

        let error = manager
            .restart(&id)
            .await
            .expect_err("restart must stop successfully before starting again");
        assert_eq!(error.code, ErrorCode::StopFailed);
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !matches!(
                manager
                    .get(&id)
                    .await
                    .expect("state after failed restart")
                    .1,
                ProgramState::Stopped
            ) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("observe late exit");
        assert_eq!(spawn_count.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn stop_report_reconciles_a_failed_command_with_the_final_stopped_state() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(FileStore::new(directory.path().to_path_buf()).expect("store"));
        let manager = ProgramManager::new(
            Arc::new(ExitOnFailedStopDriver {
                spawn_count: Arc::new(AtomicUsize::new(0)),
            }),
            store.clone(),
            store,
            Arc::new(NativeToolRunner::default()),
        );
        manager.initialize().await.expect("initialize");
        let id = create_test_program(
            &manager,
            "stop-report-final-state",
            PathBuf::from("/bin/sh"),
            RestartPolicy::Always,
        )
        .await;
        manager.start(&id).await.expect("start");

        let report = manager.disable_automatic_restarts_and_stop_active().await;

        assert_eq!(report.attempted, 1);
        assert_eq!(report.stopped, 1);
        assert_eq!(report.failed, 0);
        assert!(report.failed_program_ids.is_empty());
        assert_eq!(
            manager.get(&id).await.expect("final state").1,
            ProgramState::Stopped
        );
    }

    #[tokio::test]
    async fn stop_report_sorts_and_deduplicates_final_failures() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(FileStore::new(directory.path().join("store")).expect("store"));
        let manager = ProgramManager::new(
            Arc::new(NeverStopDriver),
            store.clone(),
            store,
            Arc::new(NativeToolRunner::default()),
        );
        manager.initialize().await.expect("initialize");
        let mut ids = Vec::new();
        for id in ["zeta-stop-failure", "alpha-stop-failure"] {
            let executable = directory.path().join(id);
            std::fs::copy("/bin/sh", &executable).expect("copy executable");
            let program_id =
                create_test_program(&manager, id, executable, RestartPolicy::Always).await;
            manager.start(&program_id).await.expect("start");
            ids.push(program_id);
        }
        ids.sort();

        let report = manager.disable_automatic_restarts_and_stop_active().await;

        assert_eq!(report.attempted, 2);
        assert_eq!(report.stopped, 0);
        assert_eq!(report.failed, 2);
        assert_eq!(report.failed_program_ids, ids);
    }

    #[tokio::test]
    async fn settings_restart_is_one_non_interleavable_controller_mutation() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(FileStore::new(directory.path().to_path_buf()).expect("store"));
        let stop_entered = Arc::new(tokio::sync::Notify::new());
        let release_stop = Arc::new(tokio::sync::Notify::new());
        let manager = ProgramManager::new(
            Arc::new(BlockingStopDriver {
                stop_entered: stop_entered.clone(),
                release_stop: release_stop.clone(),
            }),
            store.clone(),
            store,
            Arc::new(NativeToolRunner::default()),
        );
        manager.initialize().await.expect("initialize");
        let id = create_test_program(
            &manager,
            "atomic-settings-restart",
            PathBuf::from("/bin/sh"),
            RestartPolicy::Never,
        )
        .await;
        manager.start(&id).await.expect("start");
        let mut next = manager.get(&id).await.expect("settings").0;
        next.environment
            .insert("CAMELLIA_NEXUS_ATOMIC_RESTART".into(), "enabled".into());

        let update_manager = manager.clone();
        let update = tokio::spawn(async move { update_manager.update_and_restart(next).await });
        stop_entered.notified().await;
        let competing = manager
            .restart(&id)
            .await
            .expect_err("a second lifecycle mutation must not interleave");
        assert_eq!(competing.code, ErrorCode::ProgramBusy);
        release_stop.notify_one();
        update.await.expect("update task").expect("atomic update");

        let (saved, state) = manager.get(&id).await.expect("updated program");
        assert_eq!(
            saved
                .environment
                .get("CAMELLIA_NEXUS_ATOMIC_RESTART")
                .map(String::as_str),
            Some("enabled")
        );
        assert!(matches!(state, ProgramState::Running { .. }));

        let stop_manager = manager.clone();
        let stop_id = id.clone();
        let stop = tokio::spawn(async move { stop_manager.stop(&stop_id).await });
        stop_entered.notified().await;
        release_stop.notify_one();
        stop.await.expect("stop task").expect("stop");
    }

    #[tokio::test]
    async fn concurrent_remove_does_not_turn_a_completed_shutdown_into_a_stop_failure() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(FileStore::new(directory.path().to_path_buf()).expect("store"));
        let stop_entered = Arc::new(tokio::sync::Notify::new());
        let release_stop = Arc::new(tokio::sync::Notify::new());
        let manager = ProgramManager::new(
            Arc::new(BlockingStopDriver {
                stop_entered: stop_entered.clone(),
                release_stop: release_stop.clone(),
            }),
            store.clone(),
            store,
            Arc::new(NativeToolRunner::default()),
        );
        manager.initialize().await.expect("initialize");
        let id = create_test_program(
            &manager,
            "concurrent-remove",
            PathBuf::from("/bin/sh"),
            RestartPolicy::Always,
        )
        .await;
        manager.start(&id).await.expect("start");

        let remove_manager = manager.clone();
        let remove_id = id.clone();
        let remove = tokio::spawn(async move { remove_manager.remove(&remove_id).await });
        stop_entered.notified().await;
        let stop_manager = manager.clone();
        let stop_active = tokio::spawn(async move {
            stop_manager
                .disable_automatic_restarts_and_stop_active()
                .await
        });
        tokio::task::yield_now().await;
        release_stop.notify_one();

        remove.await.expect("remove task").expect("remove program");
        let report = stop_active.await.expect("stop task");
        assert_eq!(report.attempted, 1);
        assert_eq!(report.stopped, 1);
        assert_eq!(report.failed, 0);
        assert!(report.failed_program_ids.is_empty());
        assert!(manager.list().await.is_empty());
    }

    #[tokio::test]
    async fn stop_cancels_restart_backoff_and_explicit_start_restores_running_intent() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(FileStore::new(directory.path().to_path_buf()).expect("store"));
        let manager = ProgramManager::new(
            Arc::new(NativeProcessDriver::default()),
            store.clone(),
            store,
            Arc::new(NativeToolRunner::default()),
        );
        manager.initialize().await.expect("initialize");
        let id = ProgramId::parse("retry-fixture").expect("id");
        manager
            .create(CreateProgramRequest {
                spec: ProgramSpec {
                    schema_version: SCHEMA_VERSION,
                    id: id.clone(),
                    name: "Retry fixture".into(),
                    executable: ExecutableSpec::External {
                        path: PathBuf::from("/bin/sh"),
                        metadata: None,
                    },
                    program_type: ProgramType::Generic {
                        args: vec!["-c".into(), "exit 9".into()],
                    },
                    managed_config: None,
                    working_directory: PathBuf::from("/bin"),
                    environment: BTreeMap::new(),
                    auto_start: false,
                    restart_policy: RestartPolicy::OnFailure,
                    privilege_policy: Default::default(),
                },
                package_source: None,
                initial_config: None,
            })
            .await
            .expect("create");
        manager.start(&id).await.expect("start");

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if matches!(
                    manager.get(&id).await.expect("get state").1,
                    ProgramState::Backoff { .. }
                ) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("enter backoff");

        manager.stop(&id).await.expect("stop retrying");
        tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
        assert_eq!(
            manager.get(&id).await.expect("final state").1,
            ProgramState::Stopped
        );

        manager.start(&id).await.expect("explicitly start again");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if matches!(
                    manager.get(&id).await.expect("restarted state").1,
                    ProgramState::Backoff { .. }
                ) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("explicit start restores automatic restart eligibility");
        manager.stop(&id).await.expect("cancel restored backoff");
    }

    #[tokio::test]
    async fn exited_program_accepts_runtime_updates_and_returns_to_stopped() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(FileStore::new(directory.path().to_path_buf()).expect("store"));
        let manager = ProgramManager::new(
            Arc::new(NativeProcessDriver::default()),
            store.clone(),
            store,
            Arc::new(NativeToolRunner::default()),
        );
        manager.initialize().await.expect("initialize");
        let id = ProgramId::parse("exited-fixture").expect("id");
        manager
            .create(CreateProgramRequest {
                spec: ProgramSpec {
                    schema_version: SCHEMA_VERSION,
                    id: id.clone(),
                    name: "Exited fixture".into(),
                    executable: ExecutableSpec::External {
                        path: PathBuf::from("/bin/sh"),
                        metadata: None,
                    },
                    program_type: ProgramType::Generic {
                        args: vec!["-c".into(), "exit 0".into()],
                    },
                    managed_config: None,
                    working_directory: PathBuf::from("/bin"),
                    environment: BTreeMap::new(),
                    auto_start: false,
                    restart_policy: RestartPolicy::Never,
                    privilege_policy: Default::default(),
                },
                package_source: None,
                initial_config: None,
            })
            .await
            .expect("create");
        manager.start(&id).await.expect("start");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if matches!(
                    manager.get(&id).await.expect("get state").1,
                    ProgramState::Exited { .. }
                ) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("exit");

        let (mut spec, _) = manager.get(&id).await.expect("get spec");
        spec.program_type = ProgramType::Generic {
            args: vec!["-c".into(), "exit 7".into()],
        };
        manager.update(spec).await.expect("update exited program");
        let (spec, state) = manager.get(&id).await.expect("updated program");
        assert_eq!(state, ProgramState::Stopped);
        assert!(matches!(
            spec.program_type,
            ProgramType::Generic { args } if args.last().is_some_and(|arg| arg == "exit 7")
        ));
    }

    #[tokio::test]
    async fn same_external_executable_rejects_conflicting_profiles() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(FileStore::new(directory.path().to_path_buf()).expect("store"));
        let manager = ProgramManager::new(
            Arc::new(NativeProcessDriver::default()),
            store.clone(),
            store,
            Arc::new(NativeToolRunner::default()),
        );
        manager.initialize().await.expect("initialize");
        let spec = |id: &str, program_type: ProgramType| ProgramSpec {
            schema_version: SCHEMA_VERSION,
            id: ProgramId::parse(id).expect("id"),
            name: id.into(),
            executable: ExecutableSpec::External {
                path: PathBuf::from("/bin/sh"),
                metadata: None,
            },
            program_type,
            managed_config: None,
            working_directory: PathBuf::from("/bin"),
            environment: BTreeMap::new(),
            auto_start: false,
            restart_policy: RestartPolicy::Never,
            privilege_policy: Default::default(),
        };
        manager
            .create(CreateProgramRequest {
                spec: spec("generic-profile", ProgramType::Generic { args: Vec::new() }),
                package_source: None,
                initial_config: None,
            })
            .await
            .expect("create generic profile");
        let error = manager
            .create(CreateProgramRequest {
                spec: spec(
                    "xray-profile",
                    ProgramType::Xray {
                        main_config: None,
                        extra_args: Vec::new(),
                    },
                ),
                package_source: None,
                initial_config: None,
            })
            .await
            .expect_err("reject conflicting profile");
        assert_eq!(error.code, ErrorCode::InvalidSpec);

        let error = manager
            .create(CreateProgramRequest {
                spec: spec("second-generic", ProgramType::Generic { args: Vec::new() }),
                package_source: None,
                initial_config: None,
            })
            .await
            .expect_err("reject duplicate executable for the same profile type");
        assert_eq!(error.code, ErrorCode::InvalidSpec);
    }

    #[tokio::test]
    async fn initialization_quarantines_conflicting_external_profile() {
        let directory = tempfile::tempdir().expect("tempdir");
        let programs = directory.path().join("programs");
        let make_spec = |id: &str, program_type: ProgramType| ProgramSpec {
            schema_version: SCHEMA_VERSION,
            id: ProgramId::parse(id).expect("id"),
            name: id.into(),
            executable: ExecutableSpec::External {
                path: PathBuf::from("/bin/sh"),
                metadata: None,
            },
            program_type,
            managed_config: None,
            working_directory: PathBuf::from("/bin"),
            environment: BTreeMap::new(),
            auto_start: false,
            restart_policy: RestartPolicy::Never,
            privilege_policy: Default::default(),
        };
        let specs = [
            make_spec("a-generic", ProgramType::Generic { args: Vec::new() }),
            make_spec(
                "b-xray",
                ProgramType::Xray {
                    main_config: None,
                    extra_args: Vec::new(),
                },
            ),
        ];
        for spec in &specs {
            let workspace = programs.join(spec.id.as_str());
            std::fs::create_dir_all(&workspace).expect("workspace");
            std::fs::write(
                workspace.join("program.json"),
                serde_json::to_vec(spec).expect("serialize spec"),
            )
            .expect("write spec");
        }

        let store = Arc::new(FileStore::new(directory.path().to_path_buf()).expect("store"));
        let manager = ProgramManager::new(
            Arc::new(NativeProcessDriver::default()),
            store.clone(),
            store,
            Arc::new(NativeToolRunner::default()),
        );
        let report = manager.initialize().await.expect("initialize");

        assert_eq!(report.valid.len(), 1);
        assert_eq!(report.valid[0].spec.id.as_str(), "a-generic");
        assert_eq!(report.invalid.len(), 1);
        assert_eq!(manager.list().await.len(), 1);
    }

    #[tokio::test]
    async fn twenty_programs_start_and_stop_independently() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(FileStore::new(directory.path().to_path_buf()).expect("store"));
        let manager = ProgramManager::new(
            Arc::new(NativeProcessDriver::default()),
            store.clone(),
            store,
            Arc::new(NativeToolRunner::default()),
        );
        manager.initialize().await.expect("initialize");
        let mut ids = Vec::new();
        for index in 0..20 {
            let id = ProgramId::parse(format!("parallel-{index}")).expect("id");
            let executable = directory.path().join(format!("sh-{index}"));
            std::fs::copy("/bin/sh", &executable).expect("copy test executable");
            manager
                .create(CreateProgramRequest {
                    spec: ProgramSpec {
                        schema_version: SCHEMA_VERSION,
                        id: id.clone(),
                        name: format!("Parallel {index}"),
                        executable: ExecutableSpec::External {
                            path: executable,
                            metadata: None,
                        },
                        program_type: ProgramType::Generic {
                            args: vec!["-c".into(), "sleep 30".into()],
                        },
                        managed_config: None,
                        working_directory: directory.path().to_path_buf(),
                        environment: BTreeMap::new(),
                        auto_start: false,
                        restart_policy: RestartPolicy::Never,
                        privilege_policy: Default::default(),
                    },
                    package_source: None,
                    initial_config: None,
                })
                .await
                .expect("create");
            ids.push(id);
        }

        let mut tasks = tokio::task::JoinSet::new();
        for id in &ids {
            let id = id.clone();
            let manager = manager.clone();
            tasks.spawn(async move { manager.start(&id).await });
        }
        while let Some(result) = tasks.join_next().await {
            result.expect("start task").expect("start program");
        }
        assert_eq!(
            manager
                .list()
                .await
                .into_iter()
                .filter(|program| matches!(program.state, ProgramState::Running { .. }))
                .count(),
            20
        );

        for id in ids {
            let manager = manager.clone();
            tasks.spawn(async move { manager.stop(&id).await });
        }
        while let Some(result) = tasks.join_next().await {
            result.expect("stop task").expect("stop program");
        }
    }

    #[tokio::test]
    async fn failed_config_stabilization_restores_and_runs_the_backup() {
        let directory = tempfile::tempdir().expect("tempdir");
        let binary = directory.path().join("fake-xray");
        std::fs::write(
            &binary,
            r#"#!/bin/sh
if [ "$1" = "version" ]; then echo "Xray 1.0"; exit 0; fi
if [ "$1" = "help" ]; then echo "-c -format -test -dump"; exit 0; fi
test_mode=0
dump_mode=0
config=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -test) test_mode=1 ;;
    -dump) dump_mode=1 ;;
    -c) shift; config="$1" ;;
  esac
  shift
done
if [ "$test_mode" -eq 1 ]; then
  if [ "$dump_mode" -eq 1 ]; then cat "$config"; fi
  exit 0
fi
if grep -q 'runtimeFail' "$config"; then exit 1; fi
sleep 30
"#,
        )
        .expect("write fake xray");
        let mut permissions = std::fs::metadata(&binary).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&binary, permissions).expect("permissions");

        let store =
            Arc::new(FileStore::new(directory.path().join("store")).expect("create file store"));
        let manager = ProgramManager::new(
            Arc::new(NativeProcessDriver::default()),
            store.clone(),
            store,
            Arc::new(NativeToolRunner::default()),
        );
        manager.initialize().await.expect("initialize");
        let id = ProgramId::parse("rollback-fixture").expect("id");
        manager
            .create(CreateProgramRequest {
                spec: ProgramSpec {
                    schema_version: SCHEMA_VERSION,
                    id: id.clone(),
                    name: "Rollback fixture".into(),
                    executable: ExecutableSpec::External {
                        path: binary,
                        metadata: None,
                    },
                    program_type: ProgramType::Xray {
                        main_config: Some(PathBuf::from("config/config.json")),
                        extra_args: Vec::new(),
                    },
                    managed_config: None,
                    working_directory: PathBuf::from("."),
                    environment: BTreeMap::new(),
                    auto_start: false,
                    restart_policy: RestartPolicy::Never,
                    privilege_policy: Default::default(),
                },
                package_source: None,
                initial_config: Some(r#"{"stable":true}"#.into()),
            })
            .await
            .expect("create");
        manager.start(&id).await.expect("start old config");
        let (expected_spec, _) = manager.get(&id).await.expect("get spec");
        let document = manager.load_config(&id).await.expect("load config");
        let mut renamed_spec = expected_spec.clone();
        renamed_spec.name = "Renamed while preparing config".into();
        manager.update(renamed_spec).await.expect("rename program");
        let conflict = manager
            .apply_config(
                &id,
                &expected_spec,
                r#"{"stale":true}"#.into(),
                document.base_hash.clone(),
                true,
            )
            .await
            .expect_err("stale prepared config must not cross a spec update");
        assert_eq!(
            conflict.code,
            camellia_nexus_core::ErrorCode::ConfigConflict
        );
        let (expected_spec, _) = manager.get(&id).await.expect("get renamed spec");
        let error = manager
            .apply_config(
                &id,
                &expected_spec,
                r#"{"runtimeFail":true}"#.into(),
                document.base_hash,
                true,
            )
            .await
            .expect_err("new process must fail stabilization");
        assert_eq!(error.code, camellia_nexus_core::ErrorCode::ConfigInvalid);
        assert_eq!(
            manager
                .load_config(&id)
                .await
                .expect("load restored config")
                .content,
            r#"{"stable":true}"#
        );
        assert!(matches!(
            manager.get(&id).await.expect("get").1,
            ProgramState::Running { .. }
        ));

        let (before_transaction, _) = manager.get(&id).await.expect("transaction spec");
        let transaction_document = manager.load_config(&id).await.expect("transaction config");
        let mut rejected_spec = before_transaction.clone();
        rejected_spec.name = "Rejected transaction".into();
        let prepared_update = manager
            .prepare_update(rejected_spec)
            .await
            .expect("prepare settings");
        let prepared_config = manager
            .prepare_config(
                &id,
                prepared_update.expected_spec(),
                prepared_update.next_spec(),
                r#"{"runtimeFail":true}"#.into(),
                transaction_document.base_hash,
            )
            .await
            .expect("prepare configuration");
        let error = manager
            .commit_update_and_apply_config(prepared_update, prepared_config, true)
            .await
            .expect_err("failed stabilization must roll back settings and configuration");
        assert_eq!(error.code, ErrorCode::ConfigInvalid);
        assert_eq!(
            manager.get(&id).await.expect("rolled back spec").0,
            before_transaction
        );
        assert_eq!(
            manager
                .load_config(&id)
                .await
                .expect("rolled back config")
                .content,
            r#"{"stable":true}"#
        );
        assert!(matches!(
            manager.get(&id).await.expect("running after rollback").1,
            ProgramState::Running { .. }
        ));

        let (expected_spec, _) = manager.get(&id).await.expect("concurrency spec");
        let document = manager.load_config(&id).await.expect("concurrency config");
        let mut prepared_spec = expected_spec.clone();
        prepared_spec.name = "Prepared transaction".into();
        let prepared_update = manager
            .prepare_update(prepared_spec)
            .await
            .expect("prepare competing settings");
        let prepared_config = manager
            .prepare_config(
                &id,
                prepared_update.expected_spec(),
                prepared_update.next_spec(),
                r#"{"prepared":true}"#.into(),
                document.base_hash,
            )
            .await
            .expect("prepare competing config");
        let mut competing_spec = expected_spec;
        competing_spec.name = "Competing committed settings".into();
        manager
            .update(competing_spec.clone())
            .await
            .expect("commit competing settings");
        let conflict = manager
            .commit_update_and_apply_config(prepared_update, prepared_config, true)
            .await
            .expect_err("stale compound transaction must not overwrite competing settings");
        assert_eq!(conflict.code, ErrorCode::ConfigConflict);
        assert_eq!(
            manager.get(&id).await.expect("competing spec remains").0,
            competing_spec
        );
        assert_eq!(
            manager
                .load_config(&id)
                .await
                .expect("unchanged config")
                .content,
            r#"{"stable":true}"#
        );

        let expected_spec = manager.get(&id).await.expect("CAS spec").0;
        let document = manager.load_config(&id).await.expect("CAS config");
        let prepared = manager
            .prepare_config(
                &id,
                &expected_spec,
                &expected_spec,
                r#"{"prepared":true}"#.into(),
                document.base_hash,
            )
            .await
            .expect("prepare CAS config");
        let config_path = manager
            .workspace(&id)
            .await
            .expect("workspace")
            .join("config/config.json");
        std::fs::write(&config_path, r#"{"external":true}"#).expect("external config edit");
        let conflict = manager
            .apply_prepared_config(&id, &expected_spec, prepared, true)
            .await
            .expect_err("external edit must win the final CAS");
        assert_eq!(conflict.code, ErrorCode::ConfigConflict);
        assert_eq!(
            std::fs::read_to_string(&config_path).expect("external config remains"),
            r#"{"external":true}"#
        );
        assert!(matches!(
            manager
                .get(&id)
                .await
                .expect("running after CAS conflict")
                .1,
            ProgramState::Running { .. }
        ));
        manager.stop(&id).await.expect("stop restored process");
    }

    #[tokio::test]
    async fn prepared_package_never_overwrites_a_competing_program_update() {
        let directory = tempfile::tempdir().expect("tempdir");
        let first = directory.path().join("package-one");
        let second = directory.path().join("package-two");
        std::fs::create_dir_all(&first).expect("first package");
        std::fs::create_dir_all(&second).expect("second package");
        std::fs::write(first.join("tool"), b"old package").expect("old package");
        std::fs::write(second.join("tool"), b"new package").expect("new package");

        let store =
            Arc::new(FileStore::new(directory.path().join("store")).expect("create file store"));
        let manager = ProgramManager::new(
            Arc::new(NativeProcessDriver::default()),
            store.clone(),
            store,
            Arc::new(NativeToolRunner::default()),
        );
        manager.initialize().await.expect("initialize");
        let id = ProgramId::parse("prepared-package-cas").expect("id");
        manager
            .create(CreateProgramRequest {
                spec: ProgramSpec {
                    schema_version: SCHEMA_VERSION,
                    id: id.clone(),
                    name: "Managed package".into(),
                    executable: ExecutableSpec::Managed {
                        path: PathBuf::from("bin/tool"),
                        metadata: None,
                    },
                    program_type: ProgramType::Generic { args: Vec::new() },
                    managed_config: None,
                    working_directory: PathBuf::from("bin"),
                    environment: BTreeMap::new(),
                    auto_start: false,
                    restart_policy: RestartPolicy::Never,
                    privilege_policy: Default::default(),
                },
                package_source: Some(first),
                initial_config: None,
            })
            .await
            .expect("create managed program");

        let prepared = manager
            .prepare_package(&id, second.clone())
            .await
            .expect("prepare package");
        let mut competing = manager.get(&id).await.expect("current spec").0;
        competing.name = "Competing settings".into();
        manager
            .update(competing.clone())
            .await
            .expect("commit competing update");
        let conflict = manager
            .commit_package(prepared)
            .await
            .expect_err("stale package must not overwrite settings");
        assert_eq!(conflict.code, ErrorCode::ConfigConflict);
        assert_eq!(manager.get(&id).await.expect("saved spec").0, competing);
        let workspace = manager.workspace(&id).await.expect("workspace");
        assert_eq!(
            std::fs::read(workspace.join("bin/tool")).expect("active package"),
            b"old package"
        );
        assert!(!workspace.join("bin.new").exists());

        let prepared = manager
            .prepare_package(&id, second)
            .await
            .expect("prepare current package");
        manager
            .commit_package(prepared)
            .await
            .expect("commit package");
        assert_eq!(
            std::fs::read(workspace.join("bin/tool")).expect("replaced package"),
            b"new package"
        );
    }
}
