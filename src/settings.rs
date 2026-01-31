use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const DEFAULT_ALERT_THRESHOLD_PERCENT: f64 = 10.0;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RefreshCadence {
    Manual,
    OneMinute,
    TwoMinutes,
    FiveMinutes,
    FifteenMinutes,
    ThirtyMinutes,
}

impl RefreshCadence {
    pub fn duration(&self) -> Option<Duration> {
        match self {
            RefreshCadence::Manual => None,
            RefreshCadence::OneMinute => Some(Duration::from_secs(60)),
            RefreshCadence::TwoMinutes => Some(Duration::from_secs(120)),
            RefreshCadence::FiveMinutes => Some(Duration::from_secs(300)),
            RefreshCadence::FifteenMinutes => Some(Duration::from_secs(900)),
            RefreshCadence::ThirtyMinutes => Some(Duration::from_secs(1800)),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            RefreshCadence::Manual => "Manual",
            RefreshCadence::OneMinute => "1m",
            RefreshCadence::TwoMinutes => "2m",
            RefreshCadence::FiveMinutes => "5m",
            RefreshCadence::FifteenMinutes => "15m",
            RefreshCadence::ThirtyMinutes => "30m",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub refresh_cadence: RefreshCadence,
    pub show_used: bool,
    pub show_absolute_reset: bool,
    pub include_status: bool,
    #[serde(default = "default_alert_threshold_percent")]
    pub alert_threshold_percent: f64,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            refresh_cadence: RefreshCadence::FiveMinutes,
            show_used: false,
            show_absolute_reset: false,
            include_status: true,
            alert_threshold_percent: DEFAULT_ALERT_THRESHOLD_PERCENT,
        }
    }
}

impl AppSettings {
    pub fn load() -> Self {
        let path = settings_path();
        if let Ok(contents) = fs::read_to_string(&path) {
            if let Ok(settings) = serde_json::from_str(&contents) {
                return settings;
            }
        }
        Self::default()
    }

    pub fn save(&self) -> Result<()> {
        let path = settings_path();
        ensure_parent_dir(&path)?;
        let contents = serde_json::to_string_pretty(self)?;
        fs::write(&path, contents)
            .with_context(|| format!("Failed to write settings to {}", path.display()))?;
        set_private_permissions(&path)?;
        Ok(())
    }
}

fn default_alert_threshold_percent() -> f64 {
    DEFAULT_ALERT_THRESHOLD_PERCENT
}

fn settings_path() -> PathBuf {
    if let Ok(path) = std::env::var("AGENT_STATS_SETTINGS") {
        return PathBuf::from(path);
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".agent-stats").join("settings.json")
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create settings directory {}", parent.display()))?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let permissions = fs::Permissions::from_mode(0o600);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("Failed to set permissions for {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> Result<()> {
    Ok(())
}
