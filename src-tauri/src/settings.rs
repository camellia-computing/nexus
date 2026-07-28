use std::{
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use camellia_nexus_core::{CamelliaNexusError, Result};
use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "settings.json";
const SETTINGS_MAX_BYTES: u64 = 64 * 1024;
const SETTINGS_VERSION: u8 = 1;
pub const DEFAULT_STARTUP_DELAY_MS: u64 = 750;
const STARTUP_DELAY_OPTIONS_MS: [u64; 3] = [0, DEFAULT_STARTUP_DELAY_MS, 2_000];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LogRetention {
    #[default]
    Preserve,
    ClearOnStart,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppLogLevel {
    Error,
    #[default]
    Warn,
    Info,
    Debug,
    Trace,
}

impl AppLogLevel {
    pub const fn as_filter(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppLanguage {
    #[serde(rename = "en")]
    English,
    #[serde(rename = "zh-CN")]
    Chinese,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub version: u8,
    pub log_retention: LogRetention,
    #[serde(default)]
    pub log_level: AppLogLevel,
    pub program_startup_delay_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<AppLanguage>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            log_retention: LogRetention::Preserve,
            log_level: AppLogLevel::Warn,
            program_startup_delay_ms: DEFAULT_STARTUP_DELAY_MS,
            language: None,
        }
    }
}

impl AppSettings {
    fn validate(&self) -> Result<()> {
        if self.version != SETTINGS_VERSION {
            return Err(CamelliaNexusError::invalid_spec(
                "Unsupported application settings version",
            ));
        }
        if !STARTUP_DELAY_OPTIONS_MS.contains(&self.program_startup_delay_ms) {
            return Err(CamelliaNexusError::invalid_spec(
                "Program startup spacing must be 0, 750, or 2000 milliseconds",
            ));
        }
        Ok(())
    }
}

pub struct SettingsStore {
    path: PathBuf,
    current: Mutex<AppSettings>,
    load_issue: Mutex<Option<CamelliaNexusError>>,
    clear_logs_on_start: Arc<AtomicBool>,
}

impl SettingsStore {
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join(FILE_NAME);
        let (current, load_issue) = load_settings(&path);
        let clear_logs_on_start = Arc::new(AtomicBool::new(matches!(
            current.log_retention,
            LogRetention::ClearOnStart
        )));
        Self {
            path,
            current: Mutex::new(current),
            load_issue: Mutex::new(load_issue),
            clear_logs_on_start,
        }
    }

    pub fn current(&self) -> AppSettings {
        self.current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn current_result(&self) -> Result<AppSettings> {
        if let Some(error) = self
            .load_issue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return Err(error);
        }
        Ok(self.current())
    }

    pub fn load_issue(&self) -> Option<CamelliaNexusError> {
        self.load_issue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn clear_logs_on_start(&self) -> Arc<AtomicBool> {
        self.clear_logs_on_start.clone()
    }

    pub fn update(&self, settings: AppSettings) -> Result<()> {
        settings.validate()?;
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        write_atomic(&self.path, &settings)?;
        self.clear_logs_on_start.store(
            matches!(settings.log_retention, LogRetention::ClearOnStart),
            Ordering::Release,
        );
        *current = settings;
        *self
            .load_issue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        Ok(())
    }
}

fn load_settings(path: &Path) -> (AppSettings, Option<CamelliaNexusError>) {
    let bytes = match read_settings(path) {
        Ok(None) => return (AppSettings::default(), None),
        Ok(Some(bytes)) => bytes,
        Err(error) => {
            return (
                AppSettings::default(),
                Some(settings_load_error(error.to_string())),
            );
        }
    };
    let settings = match serde_json::from_slice::<AppSettings>(&bytes) {
        Ok(settings) => settings,
        Err(error) => return recover_invalid_settings(path, error.to_string()),
    };
    match settings.validate() {
        Ok(()) => (settings, None),
        Err(error) => recover_invalid_settings(path, error.to_string()),
    }
}

fn read_settings(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::with_capacity(SETTINGS_MAX_BYTES as usize);
    file.take(SETTINGS_MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > SETTINGS_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("application settings exceed {SETTINGS_MAX_BYTES} bytes"),
        ));
    }
    Ok(Some(bytes))
}

fn recover_invalid_settings(
    path: &Path,
    reason: String,
) -> (AppSettings, Option<CamelliaNexusError>) {
    let details = match quarantine_invalid_settings(path) {
        Ok(quarantine) => format!(
            "Invalid application settings were moved to {}. {reason}",
            quarantine.display()
        ),
        Err(error) => format!(
            "Application settings are invalid and could not be quarantined: {error}. {reason}"
        ),
    };
    (AppSettings::default(), Some(settings_load_error(details)))
}

fn quarantine_invalid_settings(path: &Path) -> io::Result<PathBuf> {
    for index in 0..=1000 {
        let name = if index == 0 {
            "settings.invalid.json".to_owned()
        } else {
            format!("settings.invalid.{index}.json")
        };
        let quarantine = path.with_file_name(name);
        if !quarantine.exists() {
            std::fs::rename(path, &quarantine)?;
            return Ok(quarantine);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "too many quarantined application settings files",
    ))
}

fn settings_load_error(details: String) -> CamelliaNexusError {
    CamelliaNexusError::storage(details)
}

fn write_atomic(path: &Path, settings: &AppSettings) -> Result<()> {
    crate::storage::write_bytes_atomic(path, &serde_json::to_vec_pretty(settings)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_runtime_preferences() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = SettingsStore::load(directory.path());
        let settings = AppSettings {
            version: SETTINGS_VERSION,
            log_retention: LogRetention::ClearOnStart,
            log_level: AppLogLevel::Debug,
            program_startup_delay_ms: 2_000,
            language: Some(AppLanguage::Chinese),
        };
        store.update(settings.clone()).expect("save settings");

        let reloaded = SettingsStore::load(directory.path());
        assert_eq!(reloaded.current(), settings);
        assert!(reloaded.clear_logs_on_start().load(Ordering::Acquire));
    }

    #[test]
    fn missing_log_level_uses_warn() {
        let settings: AppSettings = serde_json::from_str(
            r#"{"version":1,"logRetention":"preserve","programStartupDelayMs":750}"#,
        )
        .expect("settings");
        assert_eq!(settings.log_level, AppLogLevel::Warn);
        assert_eq!(settings.log_level.as_filter(), "warn");
        settings.validate().expect("valid settings");
    }

    #[test]
    fn invalid_file_is_quarantined_and_reported_until_settings_are_saved() {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(directory.path().join(FILE_NAME), br#"{"version":99}"#)
            .expect("write invalid settings");
        let store = SettingsStore::load(directory.path());
        assert_eq!(store.current(), AppSettings::default());
        assert!(store.current_result().is_err());
        assert!(store.load_issue().is_some());
        assert!(!directory.path().join(FILE_NAME).exists());
        assert!(directory.path().join("settings.invalid.json").exists());

        let settings = AppSettings::default();
        store.update(settings.clone()).expect("save defaults");
        assert_eq!(
            store.current_result().expect("recovered settings"),
            settings
        );
        assert!(store.load_issue().is_none());
    }

    #[test]
    fn oversized_file_is_reported_without_destroying_evidence() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join(FILE_NAME);
        std::fs::write(&path, vec![b' '; SETTINGS_MAX_BYTES as usize + 1])
            .expect("write oversized settings");
        let store = SettingsStore::load(directory.path());
        assert!(store.current_result().is_err());
        assert!(path.exists());
    }
}
