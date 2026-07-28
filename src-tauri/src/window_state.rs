use std::{io::Read, path::Path, sync::Mutex};

use serde::{Deserialize, Serialize};
use tauri::{PhysicalPosition, PhysicalSize};

const FILE_NAME: &str = "window-state.json";
const STATE_MAX_BYTES: u64 = 64 * 1024;
const STATE_VERSION: u8 = 1;
const MIN_LOGICAL_WIDTH: f64 = 680.0;
const MIN_LOGICAL_HEIGHT: f64 = 480.0;

#[derive(Default)]
pub struct WindowStateTracker {
    normal: Mutex<Option<NormalWindowState>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SavedWindowState {
    version: u8,
    normal: NormalWindowState,
    maximized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NormalWindowState {
    x: i32,
    y: i32,
    logical_width: f64,
    logical_height: f64,
    monitor_name: Option<String>,
    monitor_x: i32,
    monitor_y: i32,
}

pub fn restore(
    window: &tauri::WebviewWindow,
    data_dir: &Path,
    tracker: &WindowStateTracker,
) -> tauri::Result<()> {
    let saved = load(data_dir);
    let fallback = capture_normal(window)?;
    let desired = saved
        .as_ref()
        .map(|state| state.normal.clone())
        .unwrap_or(fallback);
    let monitors = window.available_monitors()?;
    let center_x = desired.x as i64;
    let center_y = desired.y as i64;
    let monitor = desired
        .monitor_name
        .as_ref()
        .and_then(|name| {
            monitors.iter().find(|monitor| {
                let area = monitor.work_area();
                monitor.name() == Some(name)
                    && area.position.x == desired.monitor_x
                    && area.position.y == desired.monitor_y
            })
        })
        .cloned()
        .or_else(|| {
            desired
                .monitor_name
                .as_ref()
                .and_then(|name| monitors.iter().find(|monitor| monitor.name() == Some(name)))
                .cloned()
        })
        .or_else(|| {
            monitors
                .iter()
                .find(|monitor| {
                    let area = monitor.work_area();
                    center_x >= area.position.x as i64
                        && center_x < area.position.x as i64 + area.size.width as i64
                        && center_y >= area.position.y as i64
                        && center_y < area.position.y as i64 + area.size.height as i64
                })
                .cloned()
        })
        .or(window.current_monitor()?)
        .or(window.primary_monitor()?);

    let mut restored = desired;
    if let Some(monitor) = monitor {
        let area = monitor.work_area();
        let scale = monitor.scale_factor();
        let current_inner = window.inner_size()?;
        let current_outer = window.outer_size()?;
        let frame_width = current_outer.width.saturating_sub(current_inner.width);
        let frame_height = current_outer.height.saturating_sub(current_inner.height);
        let max_inner_width = area.size.width.saturating_sub(frame_width).max(1);
        let max_inner_height = area.size.height.saturating_sub(frame_height).max(1);
        let min_inner_width = logical_to_physical(MIN_LOGICAL_WIDTH, scale).min(max_inner_width);
        let min_inner_height = logical_to_physical(MIN_LOGICAL_HEIGHT, scale).min(max_inner_height);
        let width = logical_to_physical(restored.logical_width, scale)
            .clamp(min_inner_width, max_inner_width);
        let height = logical_to_physical(restored.logical_height, scale)
            .clamp(min_inner_height, max_inner_height);
        window.set_size(PhysicalSize::new(width, height))?;

        let outer_width = width.saturating_add(frame_width).min(area.size.width);
        let outer_height = height.saturating_add(frame_height).min(area.size.height);
        let requested_x = area.position.x as i64 + (restored.x as i64 - restored.monitor_x as i64);
        let requested_y = area.position.y as i64 + (restored.y as i64 - restored.monitor_y as i64);
        let max_x = area.position.x as i64 + area.size.width as i64 - outer_width as i64;
        let max_y = area.position.y as i64 + area.size.height as i64 - outer_height as i64;
        let x = requested_x.clamp(area.position.x as i64, max_x) as i32;
        let y = requested_y.clamp(area.position.y as i64, max_y) as i32;
        window.set_position(PhysicalPosition::new(x, y))?;

        restored.x = x;
        restored.y = y;
        restored.logical_width = width as f64 / scale;
        restored.logical_height = height as f64 / scale;
        restored.monitor_name = monitor.name().cloned();
        restored.monitor_x = area.position.x;
        restored.monitor_y = area.position.y;
    }
    set_tracked(tracker, restored);
    if saved.is_some_and(|state| state.maximized) {
        window.maximize()?;
    }
    Ok(())
}

pub fn remember_normal(window: &tauri::WebviewWindow, tracker: &WindowStateTracker) {
    if window.is_minimized().unwrap_or(false)
        || window.is_maximized().unwrap_or(false)
        || looks_maximized(window)
    {
        return;
    }
    if let Ok(state) = capture_normal(window) {
        set_tracked(tracker, state);
    }
}

pub fn save(window: &tauri::WebviewWindow, data_dir: &Path, tracker: &WindowStateTracker) {
    if !window.is_minimized().unwrap_or(false) && !window.is_maximized().unwrap_or(false) {
        remember_normal(window, tracker);
    }
    let normal = tracker
        .normal
        .lock()
        .ok()
        .and_then(|state| state.clone())
        .or_else(|| load(data_dir).map(|state| state.normal))
        .or_else(|| capture_normal(window).ok());
    let Some(normal) = normal else {
        return;
    };
    let state = SavedWindowState {
        version: STATE_VERSION,
        normal,
        maximized: window.is_maximized().unwrap_or(false),
    };
    let Ok(bytes) = serde_json::to_vec(&state) else {
        return;
    };
    let path = data_dir.join(FILE_NAME);
    if let Err(error) = crate::storage::write_bytes_atomic(&path, &bytes) {
        tracing::warn!(%error, "could not save window state");
    }
}

fn load(data_dir: &Path) -> Option<SavedWindowState> {
    let file = std::fs::File::open(data_dir.join(FILE_NAME)).ok()?;
    let mut bytes = Vec::with_capacity(file.metadata().ok()?.len().min(STATE_MAX_BYTES) as usize);
    file.take(STATE_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > STATE_MAX_BYTES {
        return None;
    }
    serde_json::from_slice::<SavedWindowState>(&bytes)
        .ok()
        .filter(|state| state.version == STATE_VERSION && valid_normal(&state.normal))
}

fn capture_normal(window: &tauri::WebviewWindow) -> tauri::Result<NormalWindowState> {
    let position = window.outer_position()?;
    let inner = window.inner_size()?;
    let scale = window.scale_factor()?;
    let monitor = window.current_monitor()?;
    let area = monitor.as_ref().map(tauri::window::Monitor::work_area);
    Ok(NormalWindowState {
        x: position.x,
        y: position.y,
        logical_width: inner.width as f64 / scale,
        logical_height: inner.height as f64 / scale,
        monitor_name: monitor.as_ref().and_then(|value| value.name().cloned()),
        monitor_x: area.map_or(0, |value| value.position.x),
        monitor_y: area.map_or(0, |value| value.position.y),
    })
}

fn looks_maximized(window: &tauri::WebviewWindow) -> bool {
    let Ok(Some(monitor)) = window.current_monitor() else {
        return false;
    };
    let Ok(position) = window.outer_position() else {
        return false;
    };
    let Ok(size) = window.outer_size() else {
        return false;
    };
    let area = monitor.work_area();
    (position.x - area.position.x).abs() <= 8
        && (position.y - area.position.y).abs() <= 8
        && size.width.abs_diff(area.size.width) <= 16
        && size.height.abs_diff(area.size.height) <= 16
}

fn logical_to_physical(value: f64, scale: f64) -> u32 {
    (value.max(1.0) * scale).round().clamp(1.0, u32::MAX as f64) as u32
}

fn valid_normal(state: &NormalWindowState) -> bool {
    state.logical_width.is_finite()
        && state.logical_height.is_finite()
        && state.logical_width > 0.0
        && state.logical_height > 0.0
}

fn set_tracked(tracker: &WindowStateTracker, state: NormalWindowState) {
    if let Ok(mut current) = tracker.normal.lock() {
        *current = Some(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_conversion_is_stable() {
        assert_eq!(logical_to_physical(800.0, 1.25), 1000);
        assert_eq!(logical_to_physical(800.0, 1.0), 800);
    }

    #[test]
    fn rejects_invalid_normal_geometry() {
        let invalid = NormalWindowState {
            x: 0,
            y: 0,
            logical_width: f64::NAN,
            logical_height: 500.0,
            monitor_name: None,
            monitor_x: 0,
            monitor_y: 0,
        };
        assert!(!valid_normal(&invalid));
    }

    #[test]
    fn rejects_obsolete_window_state_shapes() {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            directory.path().join(FILE_NAME),
            br#"{"x":0,"y":0,"width":1200,"height":800,"maximized":false}"#,
        )
        .expect("write obsolete state");

        assert!(load(directory.path()).is_none());
    }
}
