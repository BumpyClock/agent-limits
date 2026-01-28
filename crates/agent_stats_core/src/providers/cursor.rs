use crate::config::ProviderSourceMode;
use crate::models::{
    ProviderCostSnapshot, ProviderErrorPayload, ProviderIdentitySnapshot, ProviderPayload, RateWindow,
    UsageSnapshot,
};
use crate::provider_metadata::ProviderId;
use super::{Provider, ProviderFetchContext};
use serde::Deserialize;
use time::OffsetDateTime;
use time::macros::format_description;

const USAGE_SUMMARY_ENDPOINT: &str = "https://cursor.com/api/usage-summary";
const USER_INFO_ENDPOINT: &str = "https://cursor.com/api/auth/me";
const REQUEST_USAGE_ENDPOINT: &str = "https://cursor.com/api/usage";

pub struct CursorProvider;

impl Provider for CursorProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Cursor
    }

    fn display_name(&self) -> &'static str {
        "Cursor"
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
                    "Cursor supports Auto/Web source modes only.",
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
                "Cursor cookie header missing. Paste a Cookie header from cursor.com.",
            ));
        }

        let summary = fetch_usage_summary(&cookie_header)?;
        let user_info = fetch_user_info(&cookie_header).ok();

        let request_usage = user_info
            .as_ref()
            .and_then(|info| info.sub.clone())
            .and_then(|user_id| fetch_request_usage(&cookie_header, &user_id).ok());

        let snapshot = build_snapshot(&summary, user_info.as_ref(), request_usage.as_ref(), context.now);
        let account = user_info.as_ref().and_then(|info| info.email.clone());

        Ok(ProviderPayload {
            provider: ProviderId::Cursor.to_string(),
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
struct CursorUsageSummary {
    billing_cycle_end: Option<String>,
    membership_type: Option<String>,
    individual_usage: Option<CursorIndividualUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorIndividualUsage {
    plan: Option<CursorPlanUsage>,
    on_demand: Option<CursorOnDemandUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorPlanUsage {
    used: Option<i64>,
    limit: Option<i64>,
    breakdown: Option<CursorPlanBreakdown>,
    total_percent_used: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorPlanBreakdown {
    total: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorOnDemandUsage {
    used: Option<i64>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorUserInfo {
    email: Option<String>,
    sub: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CursorUsageResponse {
    #[serde(rename = "gpt4")]
    gpt4: Option<CursorModelUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorModelUsage {
    num_requests_total: Option<i64>,
    num_requests: Option<i64>,
    max_request_usage: Option<i64>,
}

fn fetch_usage_summary(cookie_header: &str) -> Result<CursorUsageSummary, ProviderErrorPayload> {
    let response = ureq::get(USAGE_SUMMARY_ENDPOINT)
        .set("Accept", "application/json")
        .set("Cookie", cookie_header)
        .call();

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            return Err(status_error(
                code,
                &body,
                "Cursor API error",
            ));
        }
        Err(error) => {
            return Err(provider_error(&format!(
                "Cursor network error: {error}"
            )));
        }
    };

    if response.status() == 401 || response.status() == 403 {
        return Err(provider_error(
            "Not logged in to Cursor. Provide a valid Cookie header from cursor.com.",
        ));
    }

    let body = response
        .into_string()
        .map_err(|error| provider_error(&format!("Cursor response invalid: {error}")))?;

    serde_json::from_str(&body)
        .map_err(|error| provider_error(&format!("Failed to parse Cursor usage: {error}")))
}

fn fetch_user_info(cookie_header: &str) -> Result<CursorUserInfo, ProviderErrorPayload> {
    let response = ureq::get(USER_INFO_ENDPOINT)
        .set("Accept", "application/json")
        .set("Cookie", cookie_header)
        .call();

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            return Err(status_error(
                code,
                &body,
                "Cursor user info error",
            ));
        }
        Err(error) => {
            return Err(provider_error(&format!(
                "Cursor user info network error: {error}"
            )));
        }
    };

    if response.status() != 200 {
        return Err(provider_error("Failed to fetch Cursor user info."));
    }

    let body = response
        .into_string()
        .map_err(|error| provider_error(&format!("Cursor user info invalid: {error}")))?;

    serde_json::from_str(&body)
        .map_err(|error| provider_error(&format!("Failed to parse Cursor user info: {error}")))
}

fn fetch_request_usage(
    cookie_header: &str,
    user_id: &str,
) -> Result<CursorUsageResponse, ProviderErrorPayload> {
    let url = format!("{REQUEST_USAGE_ENDPOINT}?user={user_id}");
    let response = ureq::get(&url)
        .set("Accept", "application/json")
        .set("Cookie", cookie_header)
        .call();

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            return Err(status_error(
                code,
                &body,
                "Cursor request usage error",
            ));
        }
        Err(error) => {
            return Err(provider_error(&format!(
                "Cursor request usage network error: {error}"
            )));
        }
    };

    if response.status() != 200 {
        return Err(provider_error("Failed to fetch Cursor request usage."));
    }

    let body = response
        .into_string()
        .map_err(|error| provider_error(&format!("Cursor request usage invalid: {error}")))?;

    serde_json::from_str(&body)
        .map_err(|error| provider_error(&format!("Failed to parse Cursor request usage: {error}")))
}

fn build_snapshot(
    summary: &CursorUsageSummary,
    user_info: Option<&CursorUserInfo>,
    request_usage: Option<&CursorUsageResponse>,
    now: OffsetDateTime,
) -> UsageSnapshot {
    let plan_used_raw = summary
        .individual_usage
        .as_ref()
        .and_then(|usage| usage.plan.as_ref())
        .and_then(|plan| plan.used)
        .unwrap_or(0) as f64;

    let plan_limit_raw = summary
        .individual_usage
        .as_ref()
        .and_then(|usage| usage.plan.as_ref())
        .and_then(|plan| plan.breakdown.as_ref().and_then(|breakdown| breakdown.total).or(plan.limit))
        .unwrap_or(0) as f64;

    let plan_percent = if plan_limit_raw > 0.0 {
        (plan_used_raw / plan_limit_raw) * 100.0
    } else {
        summary
            .individual_usage
            .as_ref()
            .and_then(|usage| usage.plan.as_ref())
            .and_then(|plan| plan.total_percent_used)
            .map(|value| if value <= 1.0 { value * 100.0 } else { value })
            .unwrap_or(0.0)
    };

    let on_demand_used = summary
        .individual_usage
        .as_ref()
        .and_then(|usage| usage.on_demand.as_ref())
        .and_then(|on_demand| on_demand.used)
        .unwrap_or(0) as f64
        / 100.0;

    let on_demand_limit = summary
        .individual_usage
        .as_ref()
        .and_then(|usage| usage.on_demand.as_ref())
        .and_then(|on_demand| on_demand.limit)
        .map(|value| value as f64 / 100.0);

    let resets_at = summary
        .billing_cycle_end
        .as_ref()
        .and_then(|raw| parse_timestamp(raw));

    let reset_description = resets_at.and_then(format_reset_description);

    let request_percent = request_usage.and_then(|usage| {
        let gpt4 = usage.gpt4.as_ref()?;
        let limit = gpt4.max_request_usage?;
        let used = gpt4.num_requests_total.or(gpt4.num_requests)?;
        if limit > 0 {
            Some((used as f64 / limit as f64) * 100.0)
        } else {
            None
        }
    });

    let primary = RateWindow {
        used_percent: clamp_percent(request_percent.unwrap_or(plan_percent)),
        window_minutes: None,
        resets_at,
        reset_description: reset_description.clone(),
    };

    let secondary = on_demand_limit.and_then(|limit| {
        if limit > 0.0 {
            Some(RateWindow {
                used_percent: clamp_percent((on_demand_used / limit) * 100.0),
                window_minutes: None,
                resets_at,
                reset_description: reset_description.clone(),
            })
        } else {
            None
        }
    });

    let provider_cost = if on_demand_used > 0.0 {
        Some(ProviderCostSnapshot {
            used: on_demand_used,
            limit: on_demand_limit.unwrap_or(0.0),
            currency_code: "USD".to_string(),
            period: Some("monthly".to_string()),
            resets_at,
            updated_at: now,
        })
    } else {
        None
    };

    let login_method = summary
        .membership_type
        .as_ref()
        .map(|value| format_membership_type(value));

    let identity = ProviderIdentitySnapshot {
        provider_id: Some(ProviderId::Cursor.to_string()),
        account_email: user_info.and_then(|info| info.email.clone()),
        account_organization: None,
        login_method,
    };

    UsageSnapshot {
        primary: Some(primary),
        secondary,
        tertiary: None,
        provider_cost,
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

fn format_membership_type(raw: &str) -> String {
    match raw.trim().to_lowercase().as_str() {
        "enterprise" => "Cursor Enterprise".to_string(),
        "pro" => "Cursor Pro".to_string(),
        "hobby" => "Cursor Hobby".to_string(),
        "team" => "Cursor Team".to_string(),
        other => {
            let mut chars = other.chars();
            let Some(first) = chars.next() else {
                return "Cursor".to_string();
            };
            let mut label = String::from("Cursor ");
            label.push_str(&first.to_uppercase().to_string());
            label.push_str(chars.as_str());
            label
        }
    }
}

fn clamp_percent(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0).min(100.0)
    } else {
        0.0
    }
}

fn status_error(code: u16, body: &str, prefix: &str) -> ProviderErrorPayload {
    if code == 401 || code == 403 {
        return provider_error(
            "Not logged in to Cursor. Provide a valid Cookie header from cursor.com.",
        );
    }

    let snippet = body.trim();
    let snippet = if snippet.is_empty() {
        String::new()
    } else {
        format!(": {}", snippet.chars().take(200).collect::<String>())
    };
    provider_error(&format!("{prefix} (HTTP {code}){snippet}"))
}

fn provider_error(message: &str) -> ProviderErrorPayload {
    ProviderErrorPayload {
        code: 1,
        message: message.to_string(),
        kind: Some("provider".to_string()),
    }
}
