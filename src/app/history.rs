use agent_stats_core::models::{ProviderPayload, UsageSnapshot};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use time::{Date, OffsetDateTime, Time};

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

#[derive(Debug, Clone)]
pub(crate) struct HistorySummary {
    pub(crate) range_start: Date,
    pub(crate) range_end: Date,
    pub(crate) range_days: u32,
    pub(crate) providers: HashMap<String, ProviderHistorySummary>,
}

#[derive(Debug, Clone)]
pub(crate) struct ProviderHistorySummary {
    pub(crate) provider: String,
    pub(crate) daily: Vec<DailyUsageAggregate>,
    pub(crate) current: PeriodStats,
    pub(crate) previous: Option<PeriodStats>,
}

#[derive(Debug, Clone)]
pub(crate) struct DailyUsageAggregate {
    pub(crate) date: Date,
    pub(crate) average: f64,
    pub(crate) samples: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PeriodStats {
    pub(crate) total: f64,
    pub(crate) average: f64,
    pub(crate) days_with_data: usize,
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

pub(crate) fn load_history_summary(
    range_days: u32,
    now: OffsetDateTime,
) -> Result<HistorySummary> {
    let range_days = range_days.max(1);
    let path = history_path();
    let store = load_store(&path)?;

    let today = now.date();
    let range_start = today - time::Duration::days((range_days - 1) as i64);
    let range_end = today;

    let previous_end = range_start - time::Duration::days(1);
    let previous_start = previous_end - time::Duration::days((range_days - 1) as i64);

    let mut providers: HashMap<String, ProviderHistoryAccumulator> = HashMap::new();

    for entry in store.snapshots.iter() {
        let date = entry.captured_at.date();
        if date < previous_start || date > range_end {
            continue;
        }

        let Some(value) = usage_value(&entry.payload) else {
            continue;
        };

        let accumulator = providers
            .entry(entry.provider.clone())
            .or_insert_with(ProviderHistoryAccumulator::default);

        if date >= range_start {
            accumulator.add_current(date, value);
        } else {
            accumulator.add_previous(date, value);
        }
    }

    let mut summaries = HashMap::new();
    for (provider, accumulator) in providers {
        let daily = build_daily_aggregates(&accumulator.current_days, range_start, range_end);
        let current = compute_period_stats(&accumulator.current_days, range_start, range_end);
        let previous = compute_period_stats(
            &accumulator.previous_days,
            previous_start,
            previous_end,
        );
        let summary = ProviderHistorySummary {
            provider: provider.clone(),
            daily,
            current,
            previous: if previous.days_with_data > 0 {
                Some(previous)
            } else {
                None
            },
        };
        summaries.insert(provider, summary);
    }

    Ok(HistorySummary {
        range_start,
        range_end,
        range_days,
        providers: summaries,
    })
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

#[derive(Default)]
struct ProviderHistoryAccumulator {
    current_days: HashMap<Date, DayAggregate>,
    previous_days: HashMap<Date, DayAggregate>,
}

impl ProviderHistoryAccumulator {
    fn add_current(&mut self, date: Date, value: f64) {
        add_day_value(&mut self.current_days, date, value);
    }

    fn add_previous(&mut self, date: Date, value: f64) {
        add_day_value(&mut self.previous_days, date, value);
    }
}

#[derive(Default, Clone, Copy)]
struct DayAggregate {
    sum: f64,
    count: usize,
}

fn add_day_value(map: &mut HashMap<Date, DayAggregate>, date: Date, value: f64) {
    let entry = map.entry(date).or_insert_with(DayAggregate::default);
    entry.sum += value;
    entry.count = entry.count.saturating_add(1);
}

fn build_daily_aggregates(
    days: &HashMap<Date, DayAggregate>,
    range_start: Date,
    range_end: Date,
) -> Vec<DailyUsageAggregate> {
    let mut entries = Vec::new();
    let mut cursor = OffsetDateTime::new_utc(range_start, Time::MIDNIGHT);
    let end_cursor = OffsetDateTime::new_utc(range_end, Time::MIDNIGHT);

    while cursor <= end_cursor {
        let date = cursor.date();
        if let Some(aggregate) = days.get(&date) {
            if aggregate.count > 0 {
                entries.push(DailyUsageAggregate {
                    date,
                    average: aggregate.sum / aggregate.count as f64,
                    samples: aggregate.count,
                });
            }
        }
        cursor += time::Duration::days(1);
    }

    entries
}

fn compute_period_stats(
    days: &HashMap<Date, DayAggregate>,
    range_start: Date,
    range_end: Date,
) -> PeriodStats {
    let mut total = 0.0;
    let mut days_with_data = 0usize;

    let mut cursor = OffsetDateTime::new_utc(range_start, Time::MIDNIGHT);
    let end_cursor = OffsetDateTime::new_utc(range_end, Time::MIDNIGHT);

    while cursor <= end_cursor {
        let date = cursor.date();
        if let Some(aggregate) = days.get(&date) {
            if aggregate.count > 0 {
                total += aggregate.sum / aggregate.count as f64;
                days_with_data += 1;
            }
        }
        cursor += time::Duration::days(1);
    }

    let average = if days_with_data > 0 {
        total / days_with_data as f64
    } else {
        0.0
    };

    PeriodStats {
        total,
        average,
        days_with_data,
    }
}

fn usage_value(payload: &ProviderPayload) -> Option<f64> {
    let usage = payload.usage.as_ref()?;
    usage_window_percent(usage).or_else(|| usage_cost_percent(usage))
}

fn usage_window_percent(snapshot: &UsageSnapshot) -> Option<f64> {
    snapshot
        .primary
        .as_ref()
        .or(snapshot.secondary.as_ref())
        .or(snapshot.tertiary.as_ref())
        .map(|window| window.used_percent)
}

fn usage_cost_percent(snapshot: &UsageSnapshot) -> Option<f64> {
    let cost = snapshot.provider_cost.as_ref()?;
    if cost.limit <= 0.0 {
        return None;
    }
    Some((cost.used / cost.limit) * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_stats_core::models::{RateWindow, UsageSnapshot};
    use std::env;
    use std::sync::{Mutex, OnceLock};
    use tempfile::tempdir;
    use time::{Date, Month};

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

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let lock = LOCK.get_or_init(|| Mutex::new(()));
        match lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[test]
    fn snapshots_are_trimmed_to_the_retention_window() -> Result<()> {
        let _lock = env_lock();
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

    #[test]
    fn history_summary_aggregates_daily_usage_and_period_stats() -> Result<()> {
        let _lock = env_lock();
        let dir = tempdir()?;
        let path = dir.path().join("history.json");
        let _guard = EnvGuard::set(HISTORY_ENV, path.to_string_lossy().as_ref());

        let now = OffsetDateTime::new_utc(
            Date::from_calendar_date(2025, Month::January, 10)?,
            Time::MIDNIGHT,
        );
        let current_day_one = now - time::Duration::days(2);
        let current_day_two = now - time::Duration::days(1);
        let previous_day = now - time::Duration::days(5);

        let store = HistoryStore {
            version: 1,
            snapshots: vec![
                HistoryEntry {
                    provider: "codex".to_string(),
                    captured_at: current_day_one,
                    payload: sample_payload_with_usage("codex", 20.0, current_day_one),
                },
                HistoryEntry {
                    provider: "codex".to_string(),
                    captured_at: current_day_one + time::Duration::hours(2),
                    payload: sample_payload_with_usage("codex", 40.0, current_day_one),
                },
                HistoryEntry {
                    provider: "codex".to_string(),
                    captured_at: current_day_two,
                    payload: sample_payload_with_usage("codex", 50.0, current_day_two),
                },
                HistoryEntry {
                    provider: "codex".to_string(),
                    captured_at: previous_day,
                    payload: sample_payload_with_usage("codex", 10.0, previous_day),
                },
            ],
        };
        write_store(&path, &store)?;

        let summary = load_history_summary(3, now)?;
        let provider = summary
            .providers
            .get("codex")
            .context("codex summary missing")?;

        assert_eq!(provider.daily.len(), 2);
        assert_eq!(provider.current.days_with_data, 2);
        assert!((provider.current.total - 80.0).abs() < 0.01);
        assert!((provider.current.average - 40.0).abs() < 0.01);

        let previous = provider.previous.context("previous stats missing")?;
        assert_eq!(previous.days_with_data, 1);
        assert!((previous.average - 10.0).abs() < 0.01);
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

    fn sample_payload_with_usage(
        provider: &str,
        used_percent: f64,
        now: OffsetDateTime,
    ) -> ProviderPayload {
        ProviderPayload {
            provider: provider.to_string(),
            account: None,
            version: None,
            source: "test".to_string(),
            status: None,
            usage: Some(UsageSnapshot {
                primary: Some(RateWindow {
                    used_percent,
                    window_minutes: Some(60),
                    resets_at: Some(now + time::Duration::hours(1)),
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
