use agent_stats_core::models::{RateWindow, UsageSnapshot};
use agent_stats_core::provider_metadata::ProviderId;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Mutex;
use time::OffsetDateTime;

use super::notifications;
use super::types::ProviderState;
use crate::settings::AppSettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum AlertWindowKind {
    Primary,
    Secondary,
    Tertiary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct AlertKey {
    provider_id: ProviderId,
    window_kind: AlertWindowKind,
}

#[derive(Default)]
struct AlertTracker {
    notified_windows: HashMap<AlertKey, String>,
}

impl AlertTracker {
    fn should_notify(&mut self, key: AlertKey, window_id: &str) -> bool {
        match self.notified_windows.get(&key) {
            Some(existing) if existing == window_id => false,
            _ => {
                self.notified_windows
                    .insert(key, window_id.to_string());
                true
            }
        }
    }
}

static ALERT_TRACKER: Lazy<Mutex<AlertTracker>> = Lazy::new(|| Mutex::new(AlertTracker::default()));

pub(crate) fn handle_limit_alerts(providers: &[ProviderState], settings: &AppSettings) {
    let mut tracker = match ALERT_TRACKER.lock() {
        Ok(guard) => guard,
        Err(error) => {
            log::error!("Alert tracker lock poisoned: {error}");
            error.into_inner()
        }
    };

    let alerts = collect_limit_alerts_with_tracker(providers, settings, &mut tracker);
    for alert in alerts {
        notifications::post_limit_notification(&alert.title, &alert.body);
    }
}

fn collect_limit_alerts_with_tracker(
    providers: &[ProviderState],
    settings: &AppSettings,
    tracker: &mut AlertTracker,
) -> Vec<LimitAlert> {
    let mut alerts = Vec::new();
    for provider in providers {
        if !provider
            .config
            .enabled
            .unwrap_or(provider.metadata.default_enabled)
        {
            continue;
        }
        let payload = provider.payload.as_ref();
        let snapshot = payload.and_then(|payload| payload.usage.as_ref());
        let Some(snapshot) = snapshot else {
            continue;
        };
        let threshold_percent = effective_threshold(provider, settings);
        if !threshold_percent.is_finite() {
            continue;
        }

        add_window_alerts(
            provider,
            snapshot,
            AlertWindowKind::Primary,
            snapshot.primary.as_ref(),
            threshold_percent,
            tracker,
            &mut alerts,
        );
        add_window_alerts(
            provider,
            snapshot,
            AlertWindowKind::Secondary,
            snapshot.secondary.as_ref(),
            threshold_percent,
            tracker,
            &mut alerts,
        );
        add_window_alerts(
            provider,
            snapshot,
            AlertWindowKind::Tertiary,
            snapshot.tertiary.as_ref(),
            threshold_percent,
            tracker,
            &mut alerts,
        );
    }
    alerts
}

fn add_window_alerts(
    provider: &ProviderState,
    snapshot: &UsageSnapshot,
    window_kind: AlertWindowKind,
    window: Option<&RateWindow>,
    threshold_percent: f64,
    tracker: &mut AlertTracker,
    alerts: &mut Vec<LimitAlert>,
) {
    let Some(window) = window else {
        return;
    };
    let remaining_percent = window.remaining_percent();
    if !remaining_percent.is_finite() {
        return;
    }
    if remaining_percent > threshold_percent {
        return;
    }

    let window_id = window_identity(window, snapshot.updated_at);
    let key = AlertKey {
        provider_id: provider.config.id,
        window_kind,
    };
    if !tracker.should_notify(key, &window_id) {
        return;
    }

    let window_label = window_label(provider.config.id, window_kind);
    let title = format!("{} {window_label} limit low", provider.metadata.display_name);
    let body = format!(
        "{remaining_percent:.0}% remaining (threshold {threshold_percent:.0}%)"
    );
    alerts.push(LimitAlert { title, body });
}

fn effective_threshold(provider: &ProviderState, settings: &AppSettings) -> f64 {
    let default_threshold = AppSettings::default().alert_threshold_percent;
    let settings_threshold = sanitize_threshold(settings.alert_threshold_percent, default_threshold);
    let provider_threshold = provider
        .config
        .alert_threshold_percent
        .unwrap_or(settings_threshold);
    sanitize_threshold(provider_threshold, settings_threshold)
}

fn sanitize_threshold(value: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0).min(100.0)
    } else {
        fallback
    }
}

fn window_identity(window: &RateWindow, updated_at: OffsetDateTime) -> String {
    if let Some(resets_at) = window.resets_at {
        return format!("reset:{}", resets_at.unix_timestamp());
    }
    if let Some(window_minutes) = window.window_minutes {
        if window_minutes > 0 {
            let window_seconds = window_minutes.saturating_mul(60);
            let bucket = updated_at.unix_timestamp() / window_seconds;
            return format!("bucket:{window_minutes}:{bucket}");
        }
    }
    "unknown".to_string()
}

fn window_label(provider_id: ProviderId, window_kind: AlertWindowKind) -> &'static str {
    match window_kind {
        AlertWindowKind::Primary => "Session",
        AlertWindowKind::Secondary => "Weekly",
        AlertWindowKind::Tertiary => match provider_id {
            ProviderId::Claude => "Sonnet",
            _ => "Tertiary",
        },
    }
}

struct LimitAlert {
    title: String,
    body: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_stats_core::models::{ProviderPayload, UsageSnapshot};
    use agent_stats_core::provider_metadata::{ProviderMetadata, provider_metadata};

    fn test_provider_state(
        provider_id: ProviderId,
        remaining_percent: f64,
        resets_at: Option<OffsetDateTime>,
        window_minutes: Option<i64>,
    ) -> ProviderState {
        let used_percent = (100.0 - remaining_percent).max(0.0).min(100.0);
        let window = RateWindow {
            used_percent,
            window_minutes,
            resets_at,
            reset_description: None,
        };
        let updated_at = OffsetDateTime::from_unix_timestamp(1_700_000_000)
            .unwrap_or_else(|_| OffsetDateTime::UNIX_EPOCH);
        let usage = UsageSnapshot {
            primary: Some(window),
            secondary: None,
            tertiary: None,
            provider_cost: None,
            updated_at,
            identity: None,
        };
        let payload = ProviderPayload {
            provider: provider_id.to_string(),
            account: None,
            version: None,
            source: "test".to_string(),
            status: None,
            usage: Some(usage),
            credits: None,
            antigravity_plan_info: None,
            openai_dashboard: None,
            error: None,
        };
        let metadata = provider_metadata()
            .iter()
            .find(|metadata| metadata.id == provider_id)
            .cloned()
            .unwrap_or_else(|| ProviderMetadata {
                id: provider_id,
                display_name: provider_id.display_name(),
                default_enabled: true,
            });
        ProviderState {
            config: agent_stats_core::config::ProviderConfig {
                id: provider_id,
                enabled: Some(true),
                source: None,
                cookie_source: None,
                cookie_header: None,
                api_key: None,
                region: None,
                workspace_id: None,
                alert_threshold_percent: None,
                token_accounts: None,
            },
            metadata,
            payload: Some(payload),
        }
    }

    #[test]
    fn sends_alert_once_per_window() {
        let updated_at = OffsetDateTime::from_unix_timestamp(1_700_000_000)
            .unwrap_or_else(|_| OffsetDateTime::UNIX_EPOCH);
        let resets_at = OffsetDateTime::from_unix_timestamp(1_700_100_000)
            .unwrap_or_else(|_| updated_at);
        let provider = test_provider_state(ProviderId::Codex, 5.0, Some(resets_at), Some(300));
        let settings = AppSettings {
            refresh_cadence: crate::settings::RefreshCadence::Manual,
            show_used: false,
            show_absolute_reset: false,
            include_status: false,
            alert_threshold_percent: 10.0,
            history_retention_days: 30,
        };
        let mut tracker = AlertTracker::default();

        let first = collect_limit_alerts_with_tracker(&[provider.clone()], &settings, &mut tracker);
        assert_eq!(first.len(), 1);

        let second = collect_limit_alerts_with_tracker(&[provider], &settings, &mut tracker);
        assert_eq!(second.len(), 0);
    }

    #[test]
    fn window_identity_uses_reset_timestamp() {
        let updated_at = OffsetDateTime::from_unix_timestamp(1_700_000_000)
            .unwrap_or_else(|_| OffsetDateTime::UNIX_EPOCH);
        let resets_at = OffsetDateTime::from_unix_timestamp(1_700_100_000)
            .unwrap_or_else(|_| updated_at);
        let window = RateWindow {
            used_percent: 50.0,
            window_minutes: Some(60),
            resets_at: Some(resets_at),
            reset_description: None,
        };

        let identity = window_identity(&window, updated_at);
        assert!(identity.starts_with("reset:"));
    }

    #[test]
    fn window_identity_uses_bucket_when_no_reset() {
        let updated_at = OffsetDateTime::from_unix_timestamp(1_700_000_000)
            .unwrap_or_else(|_| OffsetDateTime::UNIX_EPOCH);
        let window = RateWindow {
            used_percent: 50.0,
            window_minutes: Some(60),
            resets_at: None,
            reset_description: None,
        };

        let identity = window_identity(&window, updated_at);
        assert!(identity.starts_with("bucket:"));
    }
}
