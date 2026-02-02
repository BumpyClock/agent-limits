use time::{Duration as TimeDuration, OffsetDateTime};

use agent_stats_core::models::{
    CreditsSnapshot, ProviderCostSnapshot, ProviderErrorPayload, ProviderPayload, RateWindow,
    UsageSnapshot,
};

use crate::settings::AppSettings;

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub(crate) fn build_account_line(
    payload: Option<&ProviderPayload>,
    usage: Option<&UsageSnapshot>,
) -> Option<String> {
    let identity = usage.and_then(|usage| usage.identity.as_ref());

    let email = identity
        .and_then(|identity| identity.account_email.as_ref())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    let account = payload
        .and_then(|payload| payload.account.as_ref())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    let organization = identity
        .and_then(|identity| identity.account_organization.as_ref())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    let login_method = identity
        .and_then(|identity| identity.login_method.as_ref())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());

    let mut parts = Vec::new();
    if let Some(email) = email {
        parts.push(email.to_string());
    } else if let Some(account) = account {
        parts.push(account.to_string());
    }

    if let Some(organization) = organization {
        if parts.iter().all(|value| value != organization) {
            parts.push(organization.to_string());
        }
    }

    if let Some(method) = login_method {
        parts.push(method.to_string());
    }

    if parts.is_empty() {
        None
    } else {
        Some(format!("Account: {}", trim_menu_text(&parts.join(" | "))))
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub(crate) fn format_updated_line(
    usage: Option<&UsageSnapshot>,
    error: Option<&ProviderErrorPayload>,
    refreshing: bool,
    now: OffsetDateTime,
) -> String {
    if refreshing {
        return "Updating...".to_string();
    }

    if error.is_some() {
        return "Last refresh failed".to_string();
    }

    if let Some(usage) = usage {
        return format_updated_string(usage.updated_at, now);
    }

    "No data yet".to_string()
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub(crate) fn format_rate_line(
    label: &str,
    window: &RateWindow,
    settings: &AppSettings,
    now: OffsetDateTime,
) -> String {
    let percent = if settings.show_used {
        window.used_percent
    } else {
        window.remaining_percent()
    };
    let percent_label = if settings.show_used { "used" } else { "left" };
    let show_reset = window.resets_at.is_some()
        || window
            .reset_description
            .as_ref()
            .is_some_and(|description| !description.trim().is_empty());
    if show_reset {
        let reset_text = format_reset_with_now(
            window.resets_at,
            settings.show_absolute_reset,
            window.reset_description.as_deref(),
            now,
        );
        format!(
            "{label}: {percent:.0}% {percent_label} | {}",
            trim_menu_text(&reset_text)
        )
    } else {
        format!("{label}: {percent:.0}% {percent_label}")
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub(crate) fn format_credits_line(credits: &CreditsSnapshot) -> String {
    format!("Credits: {:.2} left", credits.remaining)
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub(crate) fn format_cost_line(cost: &ProviderCostSnapshot) -> String {
    let currency = currency_symbol(&cost.currency_code);
    if cost.limit > 0.0 {
        format!(
            "Cost: {}{:.2} of {}{:.2}",
            currency,
            cost.used,
            currency,
            cost.limit
        )
    } else {
        format!("Cost: {}{:.2}", currency, cost.used)
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub(crate) fn trim_menu_text(text: &str) -> String {
    let max = 72usize;
    if text.chars().count() <= max {
        return text.to_string();
    }

    let trimmed: String = text.chars().take(max.saturating_sub(3)).collect();
    format!("{trimmed}...")
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn format_updated_string(updated_at: OffsetDateTime, now: OffsetDateTime) -> String {
    let diff = now - updated_at;
    let seconds = diff.whole_seconds().abs();
    if seconds < 60 {
        return "Updated just now".to_string();
    }

    if seconds < 60 * 60 {
        let minutes = (seconds / 60).max(1);
        return format!("Updated {minutes}m ago");
    }

    if seconds < 60 * 60 * 24 {
        let hours = (seconds / 3600).max(1);
        return format!("Updated {hours}h ago");
    }

    let format =
        time::format_description::parse("[hour repr:12]:[minute padding:zero] [period]").ok();
    if let Some(format) = format {
        if let Ok(text) = updated_at.format(&format) {
            return format!("Updated {text}");
        }
    }

    "Updated".to_string()
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn format_reset_with_now(
    resets_at: Option<OffsetDateTime>,
    show_absolute: bool,
    reset_description: Option<&str>,
    now: OffsetDateTime,
) -> String {
    if let Some(description) = reset_description {
        let trimmed = description.trim();
        if trimmed.is_empty() {
            return "Reset time unavailable".to_string();
        }

        if trimmed.to_lowercase().starts_with("resets") {
            return trimmed.to_string();
        }

        return format!("Resets {trimmed}");
    }

    let Some(resets_at) = resets_at else {
        return "Reset time unavailable".to_string();
    };

    if show_absolute {
        let format =
            time::format_description::parse("[hour repr:12]:[minute padding:zero] [period]").ok();
        if let Some(format) = format {
            if let Ok(text) = resets_at.format(&format) {
                return format!("Resets at {text}");
            }
        }
    }

    let diff = resets_at - now;
    if diff.is_negative() {
        return "Resetting soon".to_string();
    }

    let hours = diff.whole_hours();
    let minutes = (diff - TimeDuration::hours(hours)).whole_minutes();
    if hours > 0 {
        format!("Resets in {hours}h {minutes}m")
    } else {
        format!("Resets in {minutes}m")
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn currency_symbol(code: &str) -> &str {
    match code.trim().to_uppercase().as_str() {
        "USD" => "$",
        "EUR" => "EUR ",
        "GBP" => "GBP ",
        _ => "",
    }
}
