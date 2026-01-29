use crate::config::{ProviderConfig, ProviderSourceMode};
use crate::models::{ProviderErrorPayload, ProviderIdentitySnapshot, ProviderPayload, RateWindow, UsageSnapshot};
use crate::provider_metadata::ProviderId;
use crate::token_storage::{SecureTokenStorage, accounts};
use super::{Provider, ProviderFetchContext};
use serde_json::Value;
use time::OffsetDateTime;

const CREDITS_URL: &str = "https://kimi-k2.ai/api/user/credits";

pub struct KimiK2Provider;

impl Provider for KimiK2Provider {
    fn id(&self) -> ProviderId {
        ProviderId::KimiK2
    }

    fn display_name(&self) -> &'static str {
        "Kimi K2"
    }

    fn fetch(
        &self,
        context: &ProviderFetchContext,
    ) -> Result<ProviderPayload, ProviderErrorPayload> {
        let source_mode = context.source_mode();
        match source_mode {
            ProviderSourceMode::Auto | ProviderSourceMode::Api => {}
            _ => {
                return Err(provider_error("Kimi K2 supports Auto/Api source modes only."));
            }
        }

        let api_key = resolve_token(&context.provider_config)?;

        let response = ureq::get(CREDITS_URL)
            .set("Authorization", &format!("Bearer {api_key}"))
            .set("Accept", "application/json")
            .call();

        let response = match response {
            Ok(response) => response,
            Err(ureq::Error::Status(code, response)) => {
                let body = response.into_string().unwrap_or_default();
                let snippet = body.chars().take(200).collect::<String>();
                return Err(provider_error(&format!(
                    "Kimi K2 API error (HTTP {code}): {snippet}"
                )));
            }
            Err(error) => {
                return Err(provider_error(&format!("Kimi K2 network error: {error}")));
            }
        };

        let header_remaining = response
            .header("x-credits-remaining")
            .and_then(|value| value.parse::<f64>().ok());

        let body = response
            .into_string()
            .map_err(|error| provider_error(&format!("Kimi K2 response invalid: {error}")))?;

        let value: Value = serde_json::from_str(&body)
            .map_err(|error| provider_error(&format!("Failed to parse Kimi K2 response: {error}")))?;

        let summary = parse_summary(&value, header_remaining, context.now)?;
        let usage_snapshot = summary.to_usage_snapshot();

        Ok(ProviderPayload {
            provider: ProviderId::KimiK2.to_string(),
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

struct KimiK2Summary {
    consumed: f64,
    remaining: f64,
    updated_at: OffsetDateTime,
}

impl KimiK2Summary {
    fn to_usage_snapshot(&self) -> UsageSnapshot {
        let total = (self.consumed + self.remaining).max(0.0);
        let used_percent = if total > 0.0 {
            (self.consumed / total * 100.0).max(0.0).min(100.0)
        } else {
            0.0
        };
        let used_text = format!("{:.0}", self.consumed.max(0.0));
        let total_text = format!("{:.0}", total);

        let primary = RateWindow {
            used_percent,
            window_minutes: None,
            resets_at: None,
            reset_description: if total > 0.0 {
                Some(format!("Credits: {used_text}/{total_text}"))
            } else {
                None
            },
        };

        let identity = ProviderIdentitySnapshot {
            provider_id: Some(ProviderId::KimiK2.to_string()),
            account_email: None,
            account_organization: None,
            login_method: None,
        };

        UsageSnapshot {
            primary: Some(primary),
            secondary: None,
            tertiary: None,
            provider_cost: None,
            updated_at: self.updated_at,
            identity: Some(identity),
        }
    }
}

fn parse_summary(
    value: &Value,
    header_remaining: Option<f64>,
    now: OffsetDateTime,
) -> Result<KimiK2Summary, ProviderErrorPayload> {
    let contexts = build_contexts(value);
    let consumed = double_value_for_paths(&contexts, &CONSUMED_PATHS).unwrap_or(0.0);
    let remaining = double_value_for_paths(&contexts, &REMAINING_PATHS)
        .or(header_remaining)
        .unwrap_or(0.0);
    let updated_at = date_value_for_paths(&contexts, &TIMESTAMP_PATHS).unwrap_or(now);

    Ok(KimiK2Summary {
        consumed,
        remaining: remaining.max(0.0),
        updated_at,
    })
}

const CONSUMED_PATHS: &[&[&str]] = &[
    &["total_credits_consumed"],
    &["totalCreditsConsumed"],
    &["total_credits_used"],
    &["totalCreditsUsed"],
    &["credits_consumed"],
    &["creditsConsumed"],
    &["consumedCredits"],
    &["usedCredits"],
    &["total"],
    &["usage", "total"],
    &["usage", "consumed"],
];

const REMAINING_PATHS: &[&[&str]] = &[
    &["credits_remaining"],
    &["creditsRemaining"],
    &["remaining_credits"],
    &["remainingCredits"],
    &["available_credits"],
    &["availableCredits"],
    &["credits_left"],
    &["creditsLeft"],
    &["usage", "credits_remaining"],
    &["usage", "remaining"],
];

const TIMESTAMP_PATHS: &[&[&str]] = &[
    &["updated_at"],
    &["updatedAt"],
    &["timestamp"],
    &["time"],
    &["last_update"],
    &["lastUpdated"],
];

fn build_contexts(value: &Value) -> Vec<&Value> {
    let mut contexts = Vec::new();
    contexts.push(value);

    if let Some(data) = value.get("data") {
        contexts.push(data);
        if let Some(data_usage) = data.get("usage") {
            contexts.push(data_usage);
        }
        if let Some(data_credits) = data.get("credits") {
            contexts.push(data_credits);
        }
    }

    if let Some(result) = value.get("result") {
        contexts.push(result);
        if let Some(result_usage) = result.get("usage") {
            contexts.push(result_usage);
        }
        if let Some(result_credits) = result.get("credits") {
            contexts.push(result_credits);
        }
    }

    if let Some(usage) = value.get("usage") {
        contexts.push(usage);
    }

    if let Some(credits) = value.get("credits") {
        contexts.push(credits);
    }

    contexts
}

fn double_value_for_paths(contexts: &[&Value], paths: &[&[&str]]) -> Option<f64> {
    for path in paths {
        if let Some(raw) = value_for_path(contexts, path) {
            if let Some(value) = double_from_value(raw) {
                return Some(value);
            }
        }
    }
    None
}

fn date_value_for_paths(contexts: &[&Value], paths: &[&[&str]]) -> Option<OffsetDateTime> {
    for path in paths {
        if let Some(raw) = value_for_path(contexts, path) {
            if let Some(value) = date_from_value(raw) {
                return Some(value);
            }
        }
    }
    None
}

fn value_for_path<'a>(contexts: &[&'a Value], path: &[&str]) -> Option<&'a Value> {
    for context in contexts {
        let mut cursor = *context;
        let mut valid = true;
        for key in path {
            match cursor.get(*key) {
                Some(next) => cursor = next,
                None => {
                    valid = false;
                    break;
                }
            }
        }
        if valid {
            return Some(cursor);
        }
    }
    None
}

fn double_from_value(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(raw) => raw.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn date_from_value(value: &Value) -> Option<OffsetDateTime> {
    match value {
        Value::Number(number) => number.as_f64().and_then(date_from_numeric),
        Value::String(raw) => {
            let trimmed = raw.trim();
            if let Ok(number) = trimmed.parse::<f64>() {
                return date_from_numeric(number);
            }
            OffsetDateTime::parse(trimmed, &time::format_description::well_known::Rfc3339).ok()
        }
        _ => None,
    }
}

fn date_from_numeric(value: f64) -> Option<OffsetDateTime> {
    if value <= 0.0 {
        return None;
    }
    if value > 1_000_000_000_000.0 {
        return OffsetDateTime::from_unix_timestamp((value / 1000.0) as i64).ok();
    }
    OffsetDateTime::from_unix_timestamp(value as i64).ok()
}

fn provider_error(message: &str) -> ProviderErrorPayload {
    ProviderErrorPayload {
        code: 1,
        message: message.to_string(),
        kind: Some("provider".to_string()),
    }
}

fn resolve_token(config: &ProviderConfig) -> Result<String, ProviderErrorPayload> {
    // Priority 1: Environment variable
    if let Ok(env_token) = std::env::var("KIMIK2_API_KEY") {
        let token = env_token.trim();
        if !token.is_empty() {
            return Ok(token.to_string());
        }
    }

    // Priority 2: Secure storage
    let storage = SecureTokenStorage::default();
    if storage.is_available() {
        if let Ok(stored_token) = storage.retrieve(accounts::KIMIK2) {
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

    Err(provider_error("Missing Kimi K2 API key. Set in config, KIMIK2_API_KEY env var, or store in secure storage."))
}
