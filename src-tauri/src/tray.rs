use std::sync::{Arc, atomic::Ordering};

use camellia_nexus_core::{ProgramId, ProgramManager, ProgramState};
use camellia_nexus_licensing::{ProtectedOperation, RestrictedOperation};
use tauri::{
    AppHandle, Emitter, Manager,
    menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

use crate::{
    APP_AUTHOR, APP_LICENSE, AppState, commands, open_main_window, settings::AppLanguage,
    shutdown_and_exit,
};

const TRAY_ID: &str = "camellia-nexus-main";
const TRAY_PROGRAM_LIMIT: usize = 20;
const TRAY_ICON: tauri::image::Image<'_> = tauri::include_image!("./icons/tray-icon.png");

pub fn create(app: &AppHandle, manager: Arc<ProgramManager>) -> tauri::Result<()> {
    let menu = tauri::async_runtime::block_on(build_menu(app, manager))?;
    let chinese = uses_chinese(app);
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(TRAY_ICON)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip(if chinese {
            "Camellia Nexus · 已就绪"
        } else {
            "Camellia Nexus · Ready"
        })
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(handle_menu)
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                let _ = open_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

pub async fn refresh(app: &AppHandle, manager: Arc<ProgramManager>) -> tauri::Result<()> {
    let menu = build_menu(app, manager).await?;
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        tray.set_menu(Some(menu))?;
        tray.set_tooltip(Some(if uses_chinese(app) {
            "Camellia Nexus · 已就绪"
        } else {
            "Camellia Nexus · Ready"
        }))?;
    }
    Ok(())
}

async fn build_menu(
    app: &AppHandle,
    manager: Arc<ProgramManager>,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let chinese = uses_chinese(app);
    let programs = manager.list().await;
    let can_activate = license_can_activate(app);
    let running = programs
        .iter()
        .filter(|program| matches!(program.state, ProgramState::Running { .. }))
        .count();
    let summary = MenuItemBuilder::with_id(
        "summary",
        if chinese {
            format!("{} 个程序 · {running} 个运行中", programs.len())
        } else {
            format!("{} programs · {running} running", programs.len())
        },
    )
    .enabled(false)
    .build(app)?;
    let mut builder = MenuBuilder::new(app)
        .text("open", tr(chinese, "Open dashboard", "打开主界面"))
        .item(&summary)
        .text("add", tr(chinese, "Add program…", "添加程序…"))
        .separator();

    if programs.is_empty() {
        let empty =
            MenuItemBuilder::with_id("empty", tr(chinese, "No programs configured", "暂无程序"))
                .enabled(false)
                .build(app)?;
        builder = builder.item(&empty);
    } else {
        let start_all = MenuItemBuilder::with_id(
            "start-all",
            tr(chinese, "Start all available", "启动全部可用程序"),
        )
        .enabled(can_activate && programs.iter().any(|program| can_start(&program.state)))
        .build(app)?;
        let stop_all = MenuItemBuilder::with_id(
            "stop-all",
            tr(chinese, "Stop all active", "停止全部活动程序"),
        )
        .enabled(programs.iter().any(|program| can_stop(&program.state)))
        .build(app)?;
        builder = builder.item(&start_all).item(&stop_all).separator();
    }

    let hidden_programs = programs.len().saturating_sub(TRAY_PROGRAM_LIMIT);
    for program in programs.into_iter().take(TRAY_PROGRAM_LIMIT) {
        let status = state_label(&program.state, chinese);
        let mut submenu = SubmenuBuilder::with_id(
            app,
            format!("program:{}", program.id),
            format!("{} · {status}", compact_menu_text(&program.name, 30)),
        )
        .text(
            format!("show:{}", program.id),
            tr(chinese, "Open details", "打开详情"),
        );
        if can_activate && can_start(&program.state) {
            submenu = submenu.text(
                format!("start:{}", program.id),
                tr(chinese, "Start", "启动"),
            );
        }
        if can_stop(&program.state) {
            let stop_label = if matches!(&program.state, ProgramState::Backoff { .. }) {
                tr(chinese, "Stop retries", "停止重试")
            } else {
                tr(chinese, "Stop", "停止")
            };
            let restart_label = if matches!(&program.state, ProgramState::Backoff { .. }) {
                tr(chinese, "Retry now", "立即重试")
            } else {
                tr(chinese, "Restart", "重启")
            };
            submenu = submenu.text(format!("stop:{}", program.id), stop_label);
            if can_activate {
                submenu = submenu.text(format!("restart:{}", program.id), restart_label);
            }
        }
        builder = builder.item(&submenu.build()?);
    }
    if hidden_programs > 0 {
        let overflow = MenuItemBuilder::with_id(
            "program-overflow",
            if chinese {
                format!("另有 {hidden_programs} 个程序位于主界面")
            } else {
                format!("{hidden_programs} more in dashboard")
            },
        )
        .enabled(false)
        .build(app)?;
        builder = builder.item(&overflow);
    }

    let copyright =
        MenuItemBuilder::with_id("copyright", format!("© 2026 {APP_AUTHOR} · {APP_LICENSE}"))
            .enabled(false)
            .build(app)?;
    builder
        .separator()
        .text(
            "open-data",
            tr(chinese, "Open data directory", "打开数据目录"),
        )
        .text(
            "about",
            tr(chinese, "About Camellia Nexus", "关于 Camellia Nexus"),
        )
        .item(&copyright)
        .separator()
        .text(
            "quit",
            tr(chinese, "Quit Camellia Nexus", "退出 Camellia Nexus"),
        )
        .build()
}

fn compact_menu_text(value: &str, max_chars: usize) -> String {
    let mut characters = value.chars();
    let prefix: String = characters.by_ref().take(max_chars).collect();
    if characters.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn handle_menu(app: &AppHandle, event: tauri::menu::MenuEvent) {
    let menu_id = event.id().as_ref().to_owned();
    if menu_id == "open" {
        if let Err(error) = open_main_window(app) {
            tracing::error!(%error, "tray could not open the main window");
        }
        return;
    }
    if menu_id == "add" {
        open_create_program(app);
        return;
    }
    if menu_id == "about" {
        open_about(app);
        return;
    }

    let state = app.state::<AppState>();
    if menu_id == "open-data" {
        if let Err(error) = commands::open_external(&state.data_dir) {
            tracing::warn!(%error, "tray could not open the data directory");
        }
        return;
    }
    if menu_id == "quit" {
        if state.shutdown.request() {
            let manager = state.manager.clone();
            let app = app.clone();
            tauri::async_runtime::spawn(shutdown_and_exit(app, manager));
        }
        return;
    }
    if matches!(menu_id.as_str(), "start-all" | "stop-all") {
        run_bulk_action(app.clone(), menu_id == "start-all");
        return;
    }

    let Some((action, id)) = menu_id.split_once(':') else {
        return;
    };
    let Ok(id) = ProgramId::parse(id) else {
        return;
    };
    if action == "show" {
        open_program(app, id);
        return;
    }

    let manager = state.manager.clone();
    let app = app.clone();
    let action = action.to_owned();
    tauri::async_runtime::spawn(async move {
        match action.as_str() {
            "start" => {
                let state = app.state::<AppState>();
                let Some(_license_operation) = acquire_tray_activation(&state, "tray_start").await
                else {
                    return;
                };
                if let Err(error) = manager.start(&id).await {
                    tracing::warn!(program = %id, %error, "tray start action failed");
                }
            }
            "stop" => {
                if let Err(error) = manager.stop(&id).await {
                    tracing::warn!(program = %id, %error, "tray stop action failed");
                }
            }
            "restart" => {
                let state = app.state::<AppState>();
                let Some(_license_operation) =
                    acquire_tray_activation(&state, "tray_restart").await
                else {
                    return;
                };
                if let Err(error) = manager.restart(&id).await {
                    tracing::warn!(program = %id, %error, "tray restart action failed");
                }
            }
            _ => {}
        }
    });
}

fn run_bulk_action(app: AppHandle, start: bool) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let _license_operation = if start {
            let Some(operation) = acquire_tray_activation(&state, "tray_start_all").await else {
                return;
            };
            Some(operation)
        } else {
            None
        };
        let manager = state.manager.clone();
        let programs = manager.list().await;
        let mut tasks = tokio::task::JoinSet::new();
        for program in programs {
            if (start && can_start(&program.state)) || (!start && can_stop(&program.state)) {
                let manager = manager.clone();
                tasks.spawn(async move {
                    if start {
                        manager.start(&program.id).await
                    } else {
                        manager.stop(&program.id).await
                    }
                });
            }
        }
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => tracing::warn!(%error, "tray bulk action item failed"),
                Err(error) => tracing::error!(%error, "tray bulk action task failed"),
            }
        }
    });
}

fn license_can_activate(app: &AppHandle) -> bool {
    app.try_state::<AppState>().is_some_and(|state| {
        state
            .authorization
            .authorize(
                RestrictedOperation::Protected(ProtectedOperation::Activate),
                crate::licensing::unix_now(),
            )
            .is_ok()
    })
}

async fn acquire_tray_activation<'a>(
    state: &'a AppState,
    operation: &'static str,
) -> Option<crate::commands::RuntimeMutationPermit<'a>> {
    let operation_guard =
        crate::commands::authorize_runtime_protected(state, ProtectedOperation::Activate).await;
    if operation_guard.is_err() {
        tracing::warn!(operation, "tray activation action denied by license state");
        return None;
    }
    operation_guard.ok()
}

fn open_create_program(app: &AppHandle) {
    dispatch_ui_intent(app, commands::UiIntent::CreateProgram);
}

fn open_program(app: &AppHandle, id: ProgramId) {
    dispatch_ui_intent(
        app,
        commands::UiIntent::SelectProgram {
            program_id: id.as_str().to_owned(),
        },
    );
}

fn open_about(app: &AppHandle) {
    dispatch_ui_intent(app, commands::UiIntent::About);
}

fn dispatch_ui_intent(app: &AppHandle, intent: commands::UiIntent) {
    if open_main_window(app).is_err() {
        return;
    }
    let state = app.state::<AppState>();
    let mut pending = state
        .pending_ui_intent
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !state.ui_ready.load(Ordering::Acquire) {
        *pending = Some(intent);
        return;
    }
    drop(pending);
    let emitted = match &intent {
        commands::UiIntent::CreateProgram => app.emit("open-create-program", ()),
        commands::UiIntent::SelectProgram { program_id } => app.emit("select-program", program_id),
        commands::UiIntent::About => app.emit("open-about", ()),
    };
    if emitted.is_err() {
        state.ui_ready.store(false, Ordering::Release);
        *state
            .pending_ui_intent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(intent);
    }
}

fn can_start(state: &ProgramState) -> bool {
    matches!(
        state,
        ProgramState::Stopped | ProgramState::Exited { .. } | ProgramState::Error { .. }
    )
}

fn can_stop(state: &ProgramState) -> bool {
    !matches!(state, ProgramState::Stopped)
}

fn uses_chinese(app: &AppHandle) -> bool {
    app.try_state::<AppState>().is_some_and(|state| {
        matches!(
            state.settings.current().language,
            Some(AppLanguage::Chinese)
        )
    })
}

fn tr(chinese: bool, english: &'static str, chinese_text: &'static str) -> &'static str {
    if chinese { chinese_text } else { english }
}

fn state_label(state: &ProgramState, chinese: bool) -> &'static str {
    match state {
        ProgramState::Stopped => tr(chinese, "Stopped", "已停止"),
        ProgramState::Starting => tr(chinese, "Starting", "启动中"),
        ProgramState::Running { .. } => tr(chinese, "Running", "运行中"),
        ProgramState::Stopping => tr(chinese, "Stopping", "停止中"),
        ProgramState::Exited { .. } => tr(chinese, "Exited", "已退出"),
        ProgramState::Backoff { .. } => tr(chinese, "Backoff", "等待重试"),
        ProgramState::StopFailed { .. } => tr(chinese, "Stop failed", "停止失败"),
        ProgramState::Error { .. } => tr(chinese, "Error", "错误"),
    }
}
