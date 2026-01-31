use agent_stats_core::models::ProviderPayload;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

const HISTORY_ENV: &str = "AGENT_STATS_HISTORY";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryStore {
    version: u32,
    snapshots: Vec<HistoryEntry>,
}

impl Default for HistoryStore {
    fn default() -> Self {
        Self {
            version: 1,
            snapshots: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryEntry {
    provider: String,
    #[serde(with = "time::serde::rfc3339")]
    captured_at: OffsetDateTime,
    payload: ProviderPayload,
}

pub(crate) fn snapshot_usage(payloads: &[ProviderPayload], retention_days: u32) -> Result<()> {
    let path = history_path();
    let mut store = match load_store(&path) {
        Ok(store) => store,
        Err(error) => {
            log::error!("Failed to read history: {error}");
            HistoryStore::default()
        }
    };

    let now = OffsetDateTime::now_utc();
    let cutoff = now - time::Duration::days(retention_days as i64);

    let before_count = store.snapshots.len();
    store.snapshots.retain(|entry| entry.captured_at >= cutoff);
    let mut changed = store.snapshots.len() != before_count;

    let new_entries: Vec<HistoryEntry> = payloads
        .iter()
        .filter(|payload| has_usage_data(payload))
        .map(|payload| HistoryEntry {
            provider: payload.provider.clone(),
            captured_at: now,
            payload: payload.clone(),
        })
        .collect();

    if !new_entries.is_empty() {
        store.snapshots.extend(new_entries);
        changed = true;
    }

    if !changed {
        return Ok(());
    }

    write_store(&path, &store)?;
    Ok(())
}

fn has_usage_data(payload: &ProviderPayload) -> bool {
    payload.usage.is_some() || payload.credits.is_some() || payload.openai_dashboard.is_some()
}

fn history_path() -> PathBuf {
    if let Ok(path) = std::env::var(HISTORY_ENV) {
        return PathBuf::from(path);
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".agent-stats").join("history.json")
}

fn load_store(path: &Path) -> Result<HistoryStore> {
    if !path.exists() {
        return Ok(HistoryStore::default());
    }
    let contents = fs::read_to_string(path)
        .with_context(|| format!("Failed to read history at {}", path.display()))?;
    let store = serde_json::from_str(&contents)
        .with_context(|| "Failed to parse history JSON")?;
    Ok(store)
}

fn write_store(path: &Path, store: &HistoryStore) -> Result<()> {
    ensure_parent_dir(path)?;
    let contents = serde_json::to_string_pretty(store)?;
    fs::write(path, contents)
        .with_context(|| format!("Failed to write history to {}", path.display()))?;
    set_private_permissions(path)?;
    Ok(())
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create history directory {}", parent.display()))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use agent_stats_core::models::{RateWindow, UsageSnapshot};
    use std::env;
    use tempfile::tempdir;

    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = env::var(key).ok();
            unsafe {
                env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => unsafe {
                    env::set_var(self.key, value);
                },
                None => unsafe {
                    env::remove_var(self.key);
                },
            }
        }
    }

    #[test]
    fn snapshots_are_trimmed_to_the_retention_window() -> Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("history.json");
        let _guard = EnvGuard::set(HISTORY_ENV, path.to_string_lossy().as_ref());

        let now = OffsetDateTime::now_utc();
        let old_entry = HistoryEntry {
            provider: "codex".to_string(),
            captured_at: now - time::Duration::days(10),
            payload: sample_payload("codex", now - time::Duration::days(10)),
        };
        let recent_entry = HistoryEntry {
            provider: "claude".to_string(),
            captured_at: now - time::Duration::days(2),
            payload: sample_payload("claude", now - time::Duration::days(2)),
        };
        let store = HistoryStore {
            version: 1,
            snapshots: vec![old_entry, recent_entry],
        };
        write_store(&path, &store)?;

        let new_payload = sample_payload("cursor", now);
        snapshot_usage(&[new_payload], 7)?;

        let updated = load_store(&path)?;
        let providers: Vec<&str> = updated
            .snapshots
            .iter()
            .map(|entry| entry.provider.as_str())
            .collect();
        assert_eq!(updated.snapshots.len(), 2);
        assert!(providers.contains(&"claude"));
        assert!(providers.contains(&"cursor"));
        Ok(())
    }

    fn sample_payload(provider: &str, now: OffsetDateTime) -> ProviderPayload {
        ProviderPayload {
            provider: provider.to_string(),
            account: None,
            version: None,
            source: "test".to_string(),
            status: None,
            usage: Some(UsageSnapshot {
                primary: Some(RateWindow {
                    used_percent: 10.0,
                    window_minutes: Some(60),
                    resets_at: Some(now),
                    reset_description: None,
                }),
                secondary: None,
                tertiary: None,
                provider_cost: None,
                updated_at: now,
                identity: None,
            }),
            credits: None,
            antigravity_plan_info: None,
            openai_dashboard: None,
            error: None,
        }
    }
}
