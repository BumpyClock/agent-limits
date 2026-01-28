use crate::config::ProviderSourceMode;
use crate::models::{ProviderErrorPayload, ProviderIdentitySnapshot, ProviderPayload, RateWindow, UsageSnapshot};
use crate::provider_metadata::ProviderId;
use super::{Provider, ProviderFetchContext};
use serde::Deserialize;
use time::OffsetDateTime;
use time::macros::format_description;

const CREDITS_ENDPOINT: &str = "https://app.augmentcode.com/api/credits";
const SUBSCRIPTION_ENDPOINT: &str = "https://app.augmentcode.com/api/subscription";

pub struct AugmentProvider;

impl Provider for AugmentProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Augment
    }

    fn display_name(&self) -> &'static str {
        "Augment"
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
                    "Augment supports Auto/Web source modes only.",
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
                "Augment cookie header missing. Paste a Cookie header from app.augmentcode.com.",
            ));
        }

        let credits = fetch_credits(&cookie_header)?;
        let subscription = fetch_subscription(&cookie_header).ok();
        let snapshot = build_snapshot(&credits, subscription.as_ref(), context.now);

        Ok(ProviderPayload {
            provider: ProviderId::Augment.to_string(),
            account: subscription.as_ref().and_then(|sub| sub.email.clone()),
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
struct AugmentCreditsResponse {
    usage_units_remaining: Option<f64>,
    usage_units_consumed_this_billing_cycle: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AugmentSubscriptionResponse {
    plan_name: Option<String>,
    billing_period_end: Option<String>,
    email: Option<String>,
    organization: Option<String>,
}

fn fetch_credits(cookie_header: &str) -> Result<AugmentCreditsResponse, ProviderErrorPayload> {
    let response = ureq::get(CREDITS_ENDPOINT)
        .set("Accept", "application/json")
        .set("Cookie", cookie_header)
        .call();

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            return Err(status_error(code, &body));
        }
        Err(error) => {
            return Err(provider_error(&format!(
                "Augment network error: {error}"
            )));
        }
    };

    let body = response
        .into_string()
        .map_err(|error| provider_error(&format!("Augment response invalid: {error}")))?;

    serde_json::from_str(&body)
        .map_err(|error| provider_error(&format!("Failed to parse Augment credits: {error}")))
}

fn fetch_subscription(
    cookie_header: &str,
) -> Result<AugmentSubscriptionResponse, ProviderErrorPayload> {
    let response = ureq::get(SUBSCRIPTION_ENDPOINT)
        .set("Accept", "application/json")
        .set("Cookie", cookie_header)
        .call();

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            return Err(status_error(code, &body));
        }
        Err(error) => {
            return Err(provider_error(&format!(
                "Augment subscription network error: {error}"
            )));
        }
    };

    if response.status() != 200 {
        return Err(provider_error("Failed to fetch Augment subscription."));
    }

    let body = response
        .into_string()
        .map_err(|error| provider_error(&format!("Augment subscription invalid: {error}")))?;

    serde_json::from_str(&body)
        .map_err(|error| provider_error(&format!("Failed to parse Augment subscription: {error}")))
}

fn build_snapshot(
    credits: &AugmentCreditsResponse,
    subscription: Option<&AugmentSubscriptionResponse>,
    now: OffsetDateTime,
) -> UsageSnapshot {
    let remaining = credits.usage_units_remaining;
    let used = credits.usage_units_consumed_this_billing_cycle;
    let limit = match (remaining, used) {
        (Some(remaining), Some(used)) => Some(remaining + used),
        _ => None,
    };

    let percent_used = match (used, limit, remaining) {
        (Some(used), Some(limit), _) if limit > 0.0 => (used / limit) * 100.0,
        (_, Some(limit), Some(remaining)) if limit > 0.0 => ((limit - remaining) / limit) * 100.0,
        _ => 0.0,
    };

    let resets_at = subscription
        .and_then(|sub| sub.billing_period_end.as_ref())
        .and_then(|raw| parse_timestamp(raw));

    let reset_description = resets_at.and_then(format_reset_description);

    let primary = RateWindow {
        used_percent: clamp_percent(percent_used),
        window_minutes: None,
        resets_at,
        reset_description,
    };

    let identity = ProviderIdentitySnapshot {
        provider_id: Some(ProviderId::Augment.to_string()),
        account_email: subscription.and_then(|sub| sub.email.clone()),
        account_organization: subscription.and_then(|sub| sub.organization.clone()),
        login_method: subscription.and_then(|sub| sub.plan_name.clone()),
    };

    UsageSnapshot {
        primary: Some(primary),
        secondary: None,
        tertiary: None,
        provider_cost: None,
        updated_at: now,
        identity: Some(identity),
    }
}

fn parse_timestamp(raw: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339).ok()
}

fn format_reset_description(date: OffsetDateTime) -> Option<String> {
    let format = format_description!("[month repr:short] [day] at [hour repr:12]:[minute][period case:upper]");
    date.format(&format)
        .ok()
        .map(|value| format!("Resets {value}"))
}

fn status_error(code: u16, body: &str) -> ProviderErrorPayload {
    if code == 401 {
        return provider_error("Augment session expired. Please log in again.");
    }
    if code == 403 {
        return provider_error("Not logged in to Augment. Please log in to app.augmentcode.com.");
    }

    let snippet = body.trim();
    let snippet = if snippet.is_empty() {
        String::new()
    } else {
        format!(": {}", snippet.chars().take(200).collect::<String>())
    };
    provider_error(&format!("Augment API error (HTTP {code}){snippet}"))
}

fn clamp_percent(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0).min(100.0)
    } else {
        0.0
    }
}

fn provider_error(message: &str) -> ProviderErrorPayload {
    ProviderErrorPayload {
        code: 1,
        message: message.to_string(),
        kind: Some("provider".to_string()),
    }
}
