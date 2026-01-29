use crate::config::ProviderSourceMode;
use crate::models::{ProviderIdentitySnapshot, ProviderPayload, RateWindow, UsageSnapshot};
use crate::provider_metadata::ProviderId;
use crate::token_storage::{SecureTokenStorage, accounts};
use super::{Provider, ProviderFetchContext};
use serde_json::Value;
use time::OffsetDateTime;

pub struct SyntheticProvider;

impl Provider for SyntheticProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Synthetic
    }

    fn display_name(&self) -> &'static str {
        "Synthetic"
    }

    fn fetch(&self, context: &ProviderFetchContext) -> Result<ProviderPayload, crate::models::ProviderErrorPayload> {
        let source_mode = context.source_mode();
        match source_mode {
            ProviderSourceMode::Auto | ProviderSourceMode::Api => {}
            _ => {
                return Err(crate::models::ProviderErrorPayload {
                    code: 1,
                    message: "Synthetic supports API-only access.".to_string(),
                    kind: Some("provider".to_string()),
                });
            }
        }

        let api_key = resolve_token(&context.provider_config)?;

        let snapshot = fetch_usage(&api_key, context.now).map_err(|error| {
            crate::models::ProviderErrorPayload {
                code: 1,
                message: error,
                kind: Some("provider".to_string()),
            }
        })?;

        let usage_snapshot = snapshot.to_usage_snapshot();

        Ok(ProviderPayload {
            provider: ProviderId::Synthetic.to_string(),
            account: None,
            version: None,
            source: "api".to_string(),
            status: None,
            usage: Some(usage_snapshot),
            credits: None,
            antigravity_plan_info: None,
            openai_dashboard: None,
            error: None,
        })
    }
}

struct SyntheticQuotaEntry {
    used_percent: f64,
    window_minutes: Option<i64>,
    resets_at: Option<OffsetDateTime>,
    reset_description: Option<String>,
}

struct SyntheticUsageSnapshot {
    quotas: Vec<SyntheticQuotaEntry>,
    plan_name: Option<String>,
    updated_at: OffsetDateTime,
}

impl SyntheticUsageSnapshot {
    fn to_usage_snapshot(&self) -> UsageSnapshot {
        let primary = self.quotas.first().map(rate_window_for);
        let secondary = self.quotas.iter().nth(1).map(rate_window_for);

        let login_method = self
            .plan_name
            .as_ref()
            .map(|plan| plan.trim().to_string())
            .filter(|plan| !plan.is_empty());

        let identity = ProviderIdentitySnapshot {
            provider_id: Some(ProviderId::Synthetic.to_string()),
            account_email: None,
            account_organization: None,
            login_method,
        };

        UsageSnapshot {
            primary,
            secondary,
            tertiary: None,
            provider_cost: None,
            updated_at: self.updated_at,
            identity: Some(identity),
        }
    }
}

fn rate_window_for(quota: &SyntheticQuotaEntry) -> RateWindow {
    RateWindow {
        used_percent: quota.used_percent,
        window_minutes: quota.window_minutes,
        resets_at: quota.resets_at,
        reset_description: quota.reset_description.clone(),
    }
}

fn resolve_token(config: &ProviderConfig) -> Result<String, crate::models::ProviderErrorPayload> {
    // Priority 1: Environment variable
    if let Ok(env_token) = std::env::var("SYNTHETIC_API_KEY") {
        let token = env_token.trim();
        if !token.is_empty() {
            return Ok(token.to_string());
        }
    }

    // Priority 2: Secure storage
    let storage = SecureTokenStorage::default();
    if storage.is_available() {
        if let Ok(stored_token) = storage.retrieve(accounts::SYNTHETIC) {
            let token = stored_token.trim();
            if !token.is_empty() {
                return Ok(token.to_string());
            }
        }
    }

    // Priority 3: Config file (fallback)
    if let Some(config_token) = &config.api_key {
        let token = config_token.trim();
        if !token.is_empty() {
            return Ok(token.to_string());
        }
    }

    Err(crate::models::ProviderErrorPayload {
        code: 1,
        message: "Synthetic API key missing. Set in config apiKey, SYNTHETIC_API_KEY env var, or store in secure storage.".to_string(),
        kind: Some("provider".to_string()),
    })
}

fn fetch_usage(api_key: &str, now: OffsetDateTime) -> Result<SyntheticUsageSnapshot, String> {
    let response = ureq::get("https://api.synthetic.new/v2/quotas")
        .set("Authorization", &format!("Bearer {api_key}"))
        .set("Accept", "application/json")
        .call();

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response
                .into_string()
                .unwrap_or_else(|_| "<no body>".to_string());
            if code == 401 || code == 403 {
                return Err("Invalid Synthetic API credentials".to_string());
            }
            return Err(format!("Synthetic API error (HTTP {code}): {body}"));
        }
        Err(error) => return Err(format!("Synthetic network error: {error}")),
    };

    let value: Value = response
        .into_json()
        .map_err(|error| format!("Failed to parse Synthetic JSON: {error}"))?;

    parse_snapshot(value, now)
}

fn parse_snapshot(value: Value, now: OffsetDateTime) -> Result<SyntheticUsageSnapshot, String> {
    let root = match value {
        Value::Object(map) => Value::Object(map),
        Value::Array(array) => {
            let mut map = serde_json::Map::new();
            map.insert("quotas".to_string(), Value::Array(array));
            Value::Object(map)
        }
        _ => Value::Object(serde_json::Map::new()),
    };

    let plan_name = plan_name_from(&root);
    let quota_objects = quota_objects_from(&root);
    let mut quotas = Vec::new();
    for quota in quota_objects {
        if let Some(entry) = parse_quota(&quota) {
            quotas.push(entry);
        }
    }

    if quotas.is_empty() {
        return Err("Missing Synthetic quota data".to_string());
    }

    Ok(SyntheticUsageSnapshot {
        quotas,
        plan_name,
        updated_at: now,
    })
}

fn plan_name_from(root: &Value) -> Option<String> {
    first_string(root, &[
        "plan",
        "planName",
        "plan_name",
        "subscription",
        "subscriptionPlan",
        "tier",
        "package",
        "packageName",
    ])
    .or_else(|| {
        root.get("data")
            .and_then(|data| first_string(data, &[
                "plan",
                "planName",
                "plan_name",
                "subscription",
                "subscriptionPlan",
                "tier",
                "package",
                "packageName",
            ]))
    })
}

fn quota_objects_from(root: &Value) -> Vec<Value> {
    let data = root.get("data");
    let candidates = [
        root.get("quotas"),
        root.get("quota"),
        root.get("limits"),
        root.get("usage"),
        root.get("entries"),
        root.get("subscription"),
        root.get("data"),
        data.and_then(|data| data.get("quotas")),
        data.and_then(|data| data.get("quota")),
        data.and_then(|data| data.get("limits")),
        data.and_then(|data| data.get("usage")),
        data.and_then(|data| data.get("entries")),
        data.and_then(|data| data.get("subscription")),
    ];

    for candidate in candidates {
        if let Some(array) = candidate.and_then(|value| value.as_array()) {
            let dicts: Vec<Value> = array
                .iter()
                .filter(|value| value.is_object())
                .cloned()
                .collect();
            if !dicts.is_empty() {
                return dicts;
            }
        }
        if let Some(dict) = candidate {
            if is_quota_payload(dict) {
                return vec![dict.clone()];
            }
        }
    }

    Vec::new()
}

fn parse_quota(payload: &Value) -> Option<SyntheticQuotaEntry> {
    let _label = first_string(payload, &[
        "name",
        "label",
        "type",
        "period",
        "scope",
        "title",
        "id",
    ]);

    let percent_used = normalized_percent(first_f64(payload, &[
        "percentUsed",
        "usedPercent",
        "usagePercent",
        "usage_percent",
        "used_percent",
        "percent_used",
        "percent",
    ]));

    let percent_remaining = normalized_percent(first_f64(payload, &[
        "percentRemaining",
        "remainingPercent",
        "remaining_percent",
        "percent_remaining",
    ]));

    let mut used_percent = percent_used;
    if used_percent.is_none() {
        if let Some(remaining) = percent_remaining {
            used_percent = Some(100.0 - remaining);
        }
    }

    if used_percent.is_none() {
        let mut limit = first_f64(payload, &["limit", "quota", "max", "total", "capacity", "allowance"]);
        let mut used = first_f64(payload, &[
            "used",
            "usage",
            "requests",
            "requestCount",
            "request_count",
            "consumed",
            "spent",
        ]);
        let remaining = first_f64(payload, &["remaining", "left", "available", "balance"]);

        if limit.is_none() {
            if let (Some(used_value), Some(remaining_value)) = (used, remaining) {
                limit = Some(used_value + remaining_value);
            }
        }
        if used.is_none() {
            if let (Some(limit_value), Some(remaining_value)) = (limit, remaining) {
                used = Some(limit_value - remaining_value);
            }
        }
        if let (Some(limit_value), Some(used_value)) = (limit, used) {
            if limit_value > 0.0 {
                used_percent = Some((used_value / limit_value) * 100.0);
            }
        }
    }

    let used_percent = used_percent?;
    let used_percent = used_percent.clamp(0.0, 100.0);

    let window_minutes = window_minutes(payload);
    let resets_at = first_date(payload, &[
        "resetAt",
        "reset_at",
        "resetsAt",
        "resets_at",
        "renewAt",
        "renew_at",
        "renewsAt",
        "renews_at",
        "periodEnd",
        "period_end",
        "expiresAt",
        "expires_at",
        "endAt",
        "end_at",
    ]);

    let reset_description = if resets_at.is_none() {
        window_description(window_minutes)
    } else {
        None
    };

    Some(SyntheticQuotaEntry {
        used_percent,
        window_minutes,
        resets_at,
        reset_description,
    })
}

fn is_quota_payload(payload: &Value) -> bool {
    let keys: [&[&str]; 5] = [
        &["limit", "quota", "max", "total", "capacity", "allowance"],
        &["used", "usage", "requests", "requestCount", "request_count", "consumed", "spent"],
        &["remaining", "left", "available", "balance"],
        &[
            "percentUsed",
            "usedPercent",
            "usagePercent",
            "usage_percent",
            "used_percent",
            "percent_used",
            "percent",
        ],
        &["percentRemaining", "remainingPercent", "remaining_percent", "percent_remaining"],
    ];

    keys.iter()
        .any(|set| first_f64(payload, set).is_some())
}

fn window_minutes(payload: &Value) -> Option<i64> {
    if let Some(minutes) = first_i64(payload, &["windowMinutes", "window_minutes", "periodMinutes", "period_minutes"]) {
        return Some(minutes);
    }
    if let Some(hours) = first_f64(payload, &["windowHours", "window_hours", "periodHours", "period_hours"]) {
        return Some((hours * 60.0).round() as i64);
    }
    if let Some(days) = first_f64(payload, &["windowDays", "window_days", "periodDays", "period_days"]) {
        return Some((days * 24.0 * 60.0).round() as i64);
    }
    if let Some(seconds) = first_f64(payload, &["windowSeconds", "window_seconds", "periodSeconds", "period_seconds"]) {
        return Some((seconds / 60.0).round() as i64);
    }
    None
}

fn window_description(minutes: Option<i64>) -> Option<String> {
    let minutes = minutes?;
    if minutes <= 0 {
        return None;
    }
    let day_minutes = 24 * 60;
    if minutes % day_minutes == 0 {
        let days = minutes / day_minutes;
        return Some(format!("{days} day{} window", if days == 1 { "" } else { "s" }));
    }
    if minutes % 60 == 0 {
        let hours = minutes / 60;
        return Some(format!("{hours} hour{} window", if hours == 1 { "" } else { "s" }));
    }
    Some(format!("{minutes} minute{} window", if minutes == 1 { "" } else { "s" }))
}

fn normalized_percent(value: Option<f64>) -> Option<f64> {
    let value = value?;
    if value <= 1.0 {
        return Some(value * 100.0);
    }
    Some(value)
}

fn first_string(payload: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = payload.get(*key) {
            if let Some(value) = string_value(value) {
                return Some(value);
            }
        }
    }
    None
}

fn first_f64(payload: &Value, keys: &[&str]) -> Option<f64> {
    for key in keys {
        if let Some(value) = payload.get(*key) {
            if let Some(value) = f64_value(value) {
                return Some(value);
            }
        }
    }
    None
}

fn first_i64(payload: &Value, keys: &[&str]) -> Option<i64> {
    for key in keys {
        if let Some(value) = payload.get(*key) {
            if let Some(value) = i64_value(value) {
                return Some(value);
            }
        }
    }
    None
}

fn first_date(payload: &Value, keys: &[&str]) -> Option<OffsetDateTime> {
    for key in keys {
        if let Some(value) = payload.get(*key) {
            if let Some(date) = date_value(value) {
                return Some(date);
            }
        }
    }
    None
}

fn string_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        _ => None,
    }
}

fn f64_value(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn i64_value(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64().or_else(|| number.as_f64().map(|n| n.round() as i64)),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn date_value(value: &Value) -> Option<OffsetDateTime> {
    if let Some(number) = f64_value(value) {
        if number > 1_000_000_000_000.0 {
            return OffsetDateTime::from_unix_timestamp((number / 1000.0) as i64).ok();
        }
        if number > 1_000_000_000.0 {
            return OffsetDateTime::from_unix_timestamp(number as i64).ok();
        }
    }
    if let Value::String(text) = value {
        let trimmed = text.trim();
        if let Ok(number) = trimmed.parse::<f64>() {
            return date_value(&Value::Number(serde_json::Number::from_f64(number)?));
        }
        if let Ok(parsed) = OffsetDateTime::parse(trimmed, &time::format_description::well_known::Rfc3339) {
            return Some(parsed);
        }
    }
    None
}
