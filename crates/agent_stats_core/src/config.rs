use crate::provider_metadata::{ProviderId, provider_metadata};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderSourceMode {
    Auto,
    Web,
    Cli,
    Oauth,
    Api,
}

impl ProviderSourceMode {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "auto" => Some(ProviderSourceMode::Auto),
            "web" => Some(ProviderSourceMode::Web),
            "cli" => Some(ProviderSourceMode::Cli),
            "oauth" => Some(ProviderSourceMode::Oauth),
            "api" => Some(ProviderSourceMode::Api),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderSourceMode::Auto => "auto",
            ProviderSourceMode::Web => "web",
            ProviderSourceMode::Cli => "cli",
            ProviderSourceMode::Oauth => "oauth",
            ProviderSourceMode::Api => "api",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderCookieSource {
    Auto,
    Manual,
    Off,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenAccountData {
    pub version: u32,
    pub active_index: usize,
    pub accounts: Vec<TokenAccountEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenAccountEntry {
    pub id: String,
    pub label: String,
    pub token: String,
    pub added_at: i64,
    pub last_used: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    pub id: ProviderId,
    pub enabled: Option<bool>,
    pub source: Option<ProviderSourceMode>,
    pub cookie_source: Option<ProviderCookieSource>,
    pub cookie_header: Option<String>,
    pub api_key: Option<String>,
    pub region: Option<String>,
    pub workspace_id: Option<String>,
    pub alert_threshold_percent: Option<f64>,
    pub token_accounts: Option<TokenAccountData>,
}

impl ProviderConfig {
    pub fn is_enabled(&self, default_enabled: bool) -> bool {
        self.enabled.unwrap_or(default_enabled)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub version: u32,
    pub providers: Vec<ProviderConfig>,
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum ConfigError {
    #[error("config file not found")]
    NotFound,
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        let path = config_path()?;
        if !path.exists() {
            return Err(ConfigError::NotFound.into());
        }
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config at {}", path.display()))?;
        let mut config: AppConfig = serde_json::from_str(&contents)
            .with_context(|| "Failed to parse config JSON")?;
        config.normalize();
        Ok(config)
    }

    pub fn load_or_default() -> Result<Self> {
        match Self::load() {
            Ok(config) => Ok(config),
            Err(error) => {
                if error.downcast_ref::<ConfigError>() == Some(&ConfigError::NotFound) {
                    let config = Self::default_config();
                    config.save()?;
                    Ok(config)
                } else {
                    Err(error)
                }
            }
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path()?;
        ensure_parent_dir(&path)?;
        let contents = serde_json::to_string_pretty(self)?;
        fs::write(&path, contents)
            .with_context(|| format!("Failed to write config at {}", path.display()))?;
        set_private_permissions(&path)?;
        Ok(())
    }

    pub fn provider_map(&self) -> HashMap<ProviderId, ProviderConfig> {
        self.providers
            .iter()
            .cloned()
            .map(|provider| (provider.id, provider))
            .collect()
    }

    pub fn normalize(&mut self) {
        let mut seen = HashSet::new();
        let mut ordered = Vec::new();

        for provider in self.providers.drain(..) {
            if seen.insert(provider.id) {
                ordered.push(provider);
            }
        }

        for metadata in provider_metadata() {
            if !seen.contains(&metadata.id) {
                ordered.push(ProviderConfig {
                    id: metadata.id,
                    enabled: None,
                    source: None,
                    cookie_source: None,
                    cookie_header: None,
                    api_key: None,
                    region: None,
                    workspace_id: None,
                    alert_threshold_percent: None,
                    token_accounts: None,
                });
            }
        }

        self.providers = ordered;
    }

    pub fn default_config() -> Self {
        let providers = provider_metadata()
            .iter()
            .map(|metadata| ProviderConfig {
                id: metadata.id,
                enabled: Some(metadata.default_enabled),
                source: None,
                cookie_source: None,
                cookie_header: None,
                api_key: None,
                region: None,
                workspace_id: None,
                alert_threshold_percent: None,
                token_accounts: None,
            })
            .collect();

        AppConfig { version: 1, providers }
    }
}

pub fn config_path() -> Result<PathBuf> {
    if let Ok(override_path) = std::env::var("AGENT_STATS_CONFIG") {
        return Ok(PathBuf::from(override_path));
    }

    let home = dirs::home_dir().context("Could not resolve home directory")?;
    Ok(home.join(".agent-status").join("config.json"))
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory {}", parent.display()))?;
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
