use crate::config::ProviderSourceMode;
use crate::models::{ProviderErrorPayload, ProviderIdentitySnapshot, ProviderPayload, RateWindow, UsageSnapshot};
use crate::provider_metadata::ProviderId;
use super::{Provider, ProviderFetchContext};
use serde::Deserialize;
use time::OffsetDateTime;
use time::macros::format_description;

const BASE_URL: &str = "https://app.factory.ai";
const AUTH_ENDPOINT: &str = "https://app.factory.ai/api/app/auth/me";
const USAGE_ENDPOINT: &str = "https://app.factory.ai/api/organization/subscription/usage";

pub struct FactoryProvider;

impl Provider for FactoryProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Factory
    }

    fn display_name(&self) -> &'static str {
        "Factory"
    }

    fn fetch(
        &self,
        context: &ProviderFetchContext,
    ) -> Result<ProviderPayload, ProviderErrorPayload> {
        let source_mode = context.source_mode();
        match source_mode {
            ProviderSourceMode::Auto | ProviderSourceMode::Web => {}
            _ => {
                return Err(provider_error(
                    "Factory supports Auto/Web source modes only.",
                ));
            }
        }

        let cookie_header = context
            .provider_config
            .cookie_header
            .clone()
            .unwrap_or_default();

        if cookie_header.trim().is_empty() {
            return Err(provider_error(
                "Factory cookie header missing. Paste a Cookie header from app.factory.ai.",
            ));
        }

        let bearer_token = extract_cookie_value(&cookie_header, "access-token");
        let auth = fetch_auth(&cookie_header, bearer_token.as_deref())?;
        let usage = fetch_usage(&cookie_header, bearer_token.as_deref(), None)?;

        let snapshot = build_snapshot(&auth, &usage, context.now);
        let account = None;

        Ok(ProviderPayload {
            provider: ProviderId::Factory.to_string(),
            account,
            version: None,
            source: "web".to_string(),
            status: None,
            usage: Some(snapshot),
            credits: None,
            antigravity_plan_info: None,
            openai_dashboard: None,
            error: None,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FactoryAuthResponse {
    organization: Option<FactoryOrganization>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FactoryOrganization {
    name: Option<String>,
    subscription: Option<FactorySubscription>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FactorySubscription {
    factory_tier: Option<String>,
    orb_subscription: Option<FactoryOrbSubscription>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FactoryOrbSubscription {
    plan: Option<FactoryPlan>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FactoryPlan {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FactoryUsageResponse {
    usage: Option<FactoryUsageData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FactoryUsageData {
    end_date: Option<i64>,
    standard: Option<FactoryTokenUsage>,
    premium: Option<FactoryTokenUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FactoryTokenUsage {
    user_tokens: Option<i64>,
    total_allowance: Option<i64>,
    used_ratio: Option<f64>,
}

fn fetch_auth(
    cookie_header: &str,
    bearer_token: Option<&str>,
) -> Result<FactoryAuthResponse, ProviderErrorPayload> {
    let mut request = ureq::get(AUTH_ENDPOINT)
        .set("Accept", "application/json")
        .set("Content-Type", "application/json")
        .set("Origin", BASE_URL)
        .set("Referer", "https://app.factory.ai/")
        .set("x-factory-client", "web-app");

    if !cookie_header.trim().is_empty() {
        request = request.set("Cookie", cookie_header);
    }
    if let Some(token) = bearer_token {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }

    let response = request.call();

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            return Err(status_error(code, &body));
        }
        Err(error) => {
            return Err(provider_error(&format!("Factory network error: {error}")));
        }
    };

    let body = response
        .into_string()
        .map_err(|error| provider_error(&format!("Factory auth invalid: {error}")))?;

    serde_json::from_str(&body)
        .map_err(|error| provider_error(&format!("Failed to parse Factory auth: {error}")))
}

fn fetch_usage(
    cookie_header: &str,
    bearer_token: Option<&str>,
    user_id: Option<&str>,
) -> Result<FactoryUsageResponse, ProviderErrorPayload> {
    let mut request = ureq::post(USAGE_ENDPOINT)
        .set("Accept", "application/json")
        .set("Content-Type", "application/json")
        .set("Origin", BASE_URL)
        .set("Referer", "https://app.factory.ai/")
        .set("x-factory-client", "web-app");

    if !cookie_header.trim().is_empty() {
        request = request.set("Cookie", cookie_header);
    }
    if let Some(token) = bearer_token {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }

    let mut body = serde_json::Map::new();
    body.insert("useCache".to_string(), serde_json::Value::Bool(true));
    if let Some(user_id) = user_id {
        body.insert("userId".to_string(), serde_json::Value::String(user_id.to_string()));
    }

    let response = request.send_json(serde_json::Value::Object(body));

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            return Err(status_error(code, &body));
        }
        Err(error) => {
            return Err(provider_error(&format!("Factory usage network error: {error}")));
        }
    };

    let body = response
        .into_string()
        .map_err(|error| provider_error(&format!("Factory usage invalid: {error}")))?;

    serde_json::from_str(&body)
        .map_err(|error| provider_error(&format!("Failed to parse Factory usage: {error}")))
}

fn build_snapshot(
    auth: &FactoryAuthResponse,
    usage: &FactoryUsageResponse,
    now: OffsetDateTime,
) -> UsageSnapshot {
    let usage_data = usage.usage.as_ref();
    let standard = usage_data.and_then(|data| data.standard.as_ref());
    let premium = usage_data.and_then(|data| data.premium.as_ref());

    let standard_user_tokens = standard.and_then(|value| value.user_tokens).unwrap_or(0);
    let standard_allowance = standard.and_then(|value| value.total_allowance).unwrap_or(0);
    let standard_ratio = standard.and_then(|value| value.used_ratio);

    let premium_user_tokens = premium.and_then(|value| value.user_tokens).unwrap_or(0);
    let premium_allowance = premium.and_then(|value| value.total_allowance).unwrap_or(0);
    let premium_ratio = premium.and_then(|value| value.used_ratio);

    let period_end = usage_data
        .and_then(|data| data.end_date)
        .and_then(parse_epoch_timestamp);

    let reset_description = period_end.and_then(format_reset_description);

    let primary = RateWindow {
        used_percent: calculate_usage_percent(standard_user_tokens, standard_allowance, standard_ratio),
        window_minutes: None,
        resets_at: period_end,
        reset_description: reset_description.clone(),
    };

    let secondary = RateWindow {
        used_percent: calculate_usage_percent(premium_user_tokens, premium_allowance, premium_ratio),
        window_minutes: None,
        resets_at: period_end,
        reset_description,
    };

    let tier = auth
        .organization
        .as_ref()
        .and_then(|org| org.subscription.as_ref())
        .and_then(|sub| sub.factory_tier.clone());
    let plan = auth
        .organization
        .as_ref()
        .and_then(|org| org.subscription.as_ref())
        .and_then(|sub| sub.orb_subscription.as_ref())
        .and_then(|orb| orb.plan.as_ref())
        .and_then(|plan| plan.name.clone());

    let login_method = format_login_method(tier.as_deref(), plan.as_deref());

    let identity = ProviderIdentitySnapshot {
        provider_id: Some(ProviderId::Factory.to_string()),
        account_email: None,
        account_organization: auth
            .organization
            .as_ref()
            .and_then(|org| org.name.clone()),
        login_method,
    };

    UsageSnapshot {
        primary: Some(primary),
        secondary: Some(secondary),
        tertiary: None,
        provider_cost: None,
        updated_at: now,
        identity: Some(identity),
    }
}

fn calculate_usage_percent(used: i64, allowance: i64, api_ratio: Option<f64>) -> f64 {
    let unlimited_threshold: i64 = 1_000_000_000_000;

    if let Some(ratio) = api_ratio {
        if let Some(percent) = percent_from_api_ratio(ratio, allowance, unlimited_threshold) {
            return percent;
        }
    }

    if allowance > unlimited_threshold {
        let reference_tokens: f64 = 100_000_000.0;
        return (used as f64 / reference_tokens * 100.0).min(100.0).max(0.0);
    }

    if allowance > 0 {
        return (used as f64 / allowance as f64 * 100.0).min(100.0).max(0.0);
    }

    0.0
}

fn percent_from_api_ratio(
    ratio: f64,
    allowance: i64,
    unlimited_threshold: i64,
) -> Option<f64> {
    if !ratio.is_finite() {
        return None;
    }

    if (-0.001..=1.001).contains(&ratio) {
        return Some((ratio * 100.0).min(100.0).max(0.0));
    }

    let allowance_is_reliable = allowance > 0 && allowance <= unlimited_threshold;
    if !allowance_is_reliable && (-0.1..=100.1).contains(&ratio) {
        return Some(ratio.min(100.0).max(0.0));
    }

    None
}

fn format_login_method(tier: Option<&str>, plan: Option<&str>) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(tier) = tier {
        let trimmed = tier.trim();
        if !trimmed.is_empty() {
            let mut chars = trimmed.chars();
            let Some(first) = chars.next() else {
                return None;
            };
            let mut label = String::from("Factory ");
            label.push_str(&first.to_uppercase().to_string());
            label.push_str(chars.as_str());
            parts.push(label);
        }
    }
    if let Some(plan) = plan {
        let trimmed = plan.trim();
        if !trimmed.is_empty() && !trimmed.to_lowercase().contains("factory") {
            parts.push(trimmed.to_string());
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" - "))
    }
}

fn parse_epoch_timestamp(raw: i64) -> Option<OffsetDateTime> {
    let timestamp = if raw > 1_000_000_000_000 { raw / 1000 } else { raw };
    OffsetDateTime::from_unix_timestamp(timestamp).ok()
}

fn format_reset_description(date: OffsetDateTime) -> Option<String> {
    let format = format_description!("[month repr:short] [day] at [hour repr:12]:[minute][period case:upper]");
    date.format(&format)
        .ok()
        .map(|value| format!("Resets {value}"))
}

fn extract_cookie_value(header: &str, name: &str) -> Option<String> {
    for part in header.split(';') {
        let mut iter = part.trim().splitn(2, '=');
        let key = iter.next()?.trim();
        let value = iter.next()?.trim();
        if key == name && !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

fn status_error(code: u16, body: &str) -> ProviderErrorPayload {
    if code == 401 || code == 403 {
        return provider_error("Not logged in to Factory. Please log in to app.factory.ai.");
    }
    let snippet = body.trim();
    let snippet = if snippet.is_empty() {
        String::new()
    } else {
        format!(": {}", snippet.chars().take(200).collect::<String>())
    };
    provider_error(&format!("Factory API error (HTTP {code}){snippet}"))
}

fn provider_error(message: &str) -> ProviderErrorPayload {
    ProviderErrorPayload {
        code: 1,
        message: message.to_string(),
        kind: Some("provider".to_string()),
    }
}
