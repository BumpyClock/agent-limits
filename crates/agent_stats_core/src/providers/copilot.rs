use crate::config::ProviderSourceMode;
use crate::models::{
    ProviderErrorPayload, ProviderIdentitySnapshot, ProviderPayload, RateWindow, UsageSnapshot,
};
use crate::provider_metadata::ProviderId;

use super::{Provider, ProviderFetchContext};
use serde::Deserialize;
use time::OffsetDateTime;

const COPILOT_API_URL: &str = "https://api.github.com/copilot_internal/user";

// Device flow constants – used by the public device flow API below.
const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const VSCODE_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";

pub struct CopilotProvider;

impl Provider for CopilotProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Copilot
    }

    fn display_name(&self) -> &'static str {
        "Copilot"
    }

    fn fetch(
        &self,
        context: &ProviderFetchContext,
    ) -> Result<ProviderPayload, ProviderErrorPayload> {
        let source_mode = context.source_mode();
        match source_mode {
            ProviderSourceMode::Auto | ProviderSourceMode::Api => {}
            _ => {
                return Err(provider_error(
                    "Copilot only supports API access (Auto or Api mode).",
                ));
            }
        }

        let token = resolve_token(&context.provider_config.api_key)?;

        let usage_response = fetch_copilot_usage(&token).map_err(|error| ProviderErrorPayload {
            code: 1,
            message: error,
            kind: Some("provider".to_string()),
        })?;

        let usage_snapshot = build_usage_snapshot(&usage_response, context.now);

        Ok(ProviderPayload {
            provider: ProviderId::Copilot.to_string(),
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

fn resolve_token(config_api_key: &Option<String>) -> Result<String, ProviderErrorPayload> {
    let token = config_api_key
        .clone()
        .or_else(|| std::env::var("COPILOT_API_TOKEN").ok())
        .unwrap_or_default();

    if token.trim().is_empty() {
        return Err(provider_error(
            "Copilot token missing. Set in config apiKey or COPILOT_API_TOKEN env var.",
        ));
    }

    Ok(token)
}

// ---------------------------------------------------------------------------
// Usage API
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CopilotQuotaSnapshot {
    #[serde(default)]
    percent_remaining: f64,
    #[allow(dead_code)]
    #[serde(default)]
    entitlement: Option<f64>,
    #[allow(dead_code)]
    #[serde(default)]
    remaining: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct CopilotQuotaSnapshots {
    premium_interactions: Option<CopilotQuotaSnapshot>,
    chat: Option<CopilotQuotaSnapshot>,
}

#[derive(Debug, Deserialize)]
struct CopilotUsageResponse {
    quota_snapshots: CopilotQuotaSnapshots,
    #[serde(default)]
    copilot_plan: String,
    #[serde(default)]
    quota_reset_date: String,
}

fn fetch_copilot_usage(token: &str) -> Result<CopilotUsageResponse, String> {
    let response = ureq::get(COPILOT_API_URL)
        .set("Authorization", &format!("token {token}"))
        .set("Accept", "application/json")
        .set("Editor-Version", "vscode/1.96.2")
        .set("Editor-Plugin-Version", "copilot-chat/0.26.7")
        .set("User-Agent", "GitHubCopilotChat/0.26.7")
        .set("X-Github-Api-Version", "2025-04-01")
        .call();

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response
                .into_string()
                .unwrap_or_else(|_| "<no body>".to_string());
            if code == 401 || code == 403 {
                return Err(format!(
                    "Invalid or expired Copilot credentials (HTTP {code}). \
                     Re-authenticate via device flow or update your token."
                ));
            }
            return Err(format!("Copilot API error (HTTP {code}): {body}"));
        }
        Err(error) => return Err(format!("Copilot network error: {error}")),
    };

    response
        .into_json()
        .map_err(|error| format!("Failed to parse Copilot API response: {error}"))
}

// ---------------------------------------------------------------------------
// Snapshot mapping
// ---------------------------------------------------------------------------

fn make_rate_window(
    snapshot: &CopilotQuotaSnapshot,
    reset_date: Option<OffsetDateTime>,
) -> RateWindow {
    let used_percent = (100.0 - snapshot.percent_remaining).clamp(0.0, 100.0);
    let reset_description = reset_date.as_ref().map(format_reset_date);

    RateWindow {
        used_percent,
        window_minutes: None,
        resets_at: reset_date,
        reset_description,
    }
}

fn build_usage_snapshot(response: &CopilotUsageResponse, now: OffsetDateTime) -> UsageSnapshot {
    let reset_date = parse_reset_date(&response.quota_reset_date);

    let primary = response
        .quota_snapshots
        .premium_interactions
        .as_ref()
        .map(|snapshot| make_rate_window(snapshot, reset_date))
        .or_else(|| {
            Some(RateWindow {
                used_percent: 0.0,
                window_minutes: None,
                resets_at: reset_date,
                reset_description: reset_date.as_ref().map(format_reset_date),
            })
        });

    let secondary = response
        .quota_snapshots
        .chat
        .as_ref()
        .map(|snapshot| make_rate_window(snapshot, None));

    let plan_label = if response.copilot_plan.trim().is_empty() {
        None
    } else {
        Some(capitalize_first(&response.copilot_plan))
    };

    let identity = ProviderIdentitySnapshot {
        provider_id: Some(ProviderId::Copilot.to_string()),
        account_email: None,
        account_organization: None,
        login_method: plan_label,
    };

    UsageSnapshot {
        primary,
        secondary,
        tertiary: None,
        provider_cost: None,
        updated_at: now,
        identity: Some(identity),
    }
}

// ---------------------------------------------------------------------------
// Device flow (GitHub OAuth)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Deserialize)]
struct AccessTokenResponse {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct DeviceFlowErrorResponse {
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug)]
pub enum DeviceFlowPollResult {
    Token(String),
    Pending,
    SlowDown,
}

pub fn request_device_code() -> Result<DeviceCodeResponse, ProviderErrorPayload> {
    let body = format!("client_id={VSCODE_CLIENT_ID}&scope=read:user");

    let response = ureq::post(DEVICE_CODE_URL)
        .set("Accept", "application/json")
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_string(&body);

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response
                .into_string()
                .unwrap_or_else(|_| "<no body>".to_string());
            return Err(provider_error(&format!(
                "Failed to request device code (HTTP {code}): {body}"
            )));
        }
        Err(error) => {
            return Err(provider_error(&format!(
                "Device code request failed: {error}"
            )));
        }
    };

    response.into_json().map_err(|error| {
        provider_error(&format!(
            "Failed to parse device code response: {error}"
        ))
    })
}

pub fn poll_for_token(device_code: &str) -> Result<DeviceFlowPollResult, ProviderErrorPayload> {
    let body = format!(
        "client_id={VSCODE_CLIENT_ID}&device_code={device_code}&grant_type=urn:ietf:params:oauth:grant-type:device_code"
    );

    let response = ureq::post(ACCESS_TOKEN_URL)
        .set("Accept", "application/json")
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_string(&body);

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response
                .into_string()
                .unwrap_or_else(|_| "<no body>".to_string());
            return Err(provider_error(&format!(
                "Token poll failed (HTTP {code}): {body}"
            )));
        }
        Err(error) => {
            return Err(provider_error(&format!("Token poll failed: {error}")));
        }
    };

    let body_string = response
        .into_string()
        .map_err(|error| provider_error(&format!("Failed to read token poll response: {error}")))?;

    if let Ok(error_response) = serde_json::from_str::<DeviceFlowErrorResponse>(&body_string) {
        if let Some(error_code) = &error_response.error {
            match error_code.as_str() {
                "authorization_pending" => return Ok(DeviceFlowPollResult::Pending),
                "slow_down" => return Ok(DeviceFlowPollResult::SlowDown),
                "expired_token" => {
                    return Err(provider_error(
                        "Device flow expired. Please restart the login process.",
                    ));
                }
                "access_denied" => {
                    return Err(provider_error("User denied the device flow authorization."));
                }
                _ => {
                    let description = error_response
                        .error_description
                        .as_deref()
                        .unwrap_or("unknown error");
                    return Err(provider_error(&format!(
                        "Device flow error ({error_code}): {description}"
                    )));
                }
            }
        }
    }

    let token_response: AccessTokenResponse =
        serde_json::from_str(&body_string).map_err(|error| {
            provider_error(&format!(
                "Failed to parse access token response: {error}"
            ))
        })?;

    Ok(DeviceFlowPollResult::Token(token_response.access_token))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn capitalize_first(value: &str) -> String {
    let trimmed = value.trim();
    let mut chars = trimmed.chars();
    match chars.next() {
        Some(first) => {
            let upper: String = first.to_uppercase().collect();
            format!("{upper}{}", chars.as_str())
        }
        None => String::new(),
    }
}

fn parse_reset_date(value: &str) -> Option<OffsetDateTime> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Try RFC 3339 first
    if let Ok(date) = OffsetDateTime::parse(trimmed, &time::format_description::well_known::Rfc3339)
    {
        return Some(date);
    }
    // Try date-only format (YYYY-MM-DD) as midnight UTC
    if let Ok(date) = time::Date::parse(
        trimmed,
        &time::format_description::well_known::Iso8601::DATE,
    ) {
        return OffsetDateTime::new_utc(date, time::Time::MIDNIGHT).into();
    }
    None
}

fn format_reset_date(date: &OffsetDateTime) -> String {
    let format = time::format_description::parse(
        "[month repr:short] [day] at [hour repr:12]:[minute][period case:lower]",
    );
    match format {
        Ok(ref format) => date.format(format).unwrap_or_else(|_| date.to_string()),
        Err(_) => date.to_string(),
    }
}

fn provider_error(message: &str) -> ProviderErrorPayload {
    ProviderErrorPayload {
        code: 1,
        message: message.to_string(),
        kind: Some("provider".to_string()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capitalize_first() {
        assert_eq!(capitalize_first("business"), "Business");
        assert_eq!(capitalize_first("individual"), "Individual");
        assert_eq!(capitalize_first(""), "");
        assert_eq!(capitalize_first("  pro  "), "Pro");
    }

    #[test]
    fn test_parse_reset_date_rfc3339() {
        let result = parse_reset_date("2026-02-01T00:00:00Z");
        assert!(result.is_some());
        let date = result.expect("should parse");
        assert_eq!(date.month(), time::Month::February);
        assert_eq!(date.day(), 1);
    }

    #[test]
    fn test_parse_reset_date_date_only() {
        let result = parse_reset_date("2026-02-01");
        assert!(result.is_some());
        let date = result.expect("should parse");
        assert_eq!(date.month(), time::Month::February);
        assert_eq!(date.day(), 1);
    }

    #[test]
    fn test_parse_reset_date_empty() {
        assert!(parse_reset_date("").is_none());
        assert!(parse_reset_date("   ").is_none());
    }

    #[test]
    fn test_parse_reset_date_invalid() {
        assert!(parse_reset_date("not-a-date").is_none());
    }

    #[test]
    fn test_make_rate_window_100_percent_remaining() {
        let snapshot = CopilotQuotaSnapshot {
            percent_remaining: 100.0,
            entitlement: Some(300.0),
            remaining: Some(300.0),
        };
        let window = make_rate_window(&snapshot, None);
        assert!((window.used_percent - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_make_rate_window_0_percent_remaining() {
        let snapshot = CopilotQuotaSnapshot {
            percent_remaining: 0.0,
            entitlement: Some(300.0),
            remaining: Some(0.0),
        };
        let window = make_rate_window(&snapshot, None);
        assert!((window.used_percent - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_make_rate_window_with_reset_date() {
        let snapshot = CopilotQuotaSnapshot {
            percent_remaining: 75.0,
            entitlement: None,
            remaining: None,
        };
        let reset = OffsetDateTime::from_unix_timestamp(1738368000).ok();
        let window = make_rate_window(&snapshot, reset);
        assert!((window.used_percent - 25.0).abs() < f64::EPSILON);
        assert!(window.resets_at.is_some());
        assert!(window.reset_description.is_some());
    }

    #[test]
    fn test_build_usage_snapshot_full() {
        let response = CopilotUsageResponse {
            quota_snapshots: CopilotQuotaSnapshots {
                premium_interactions: Some(CopilotQuotaSnapshot {
                    percent_remaining: 80.0,
                    entitlement: Some(300.0),
                    remaining: Some(240.0),
                }),
                chat: Some(CopilotQuotaSnapshot {
                    percent_remaining: 50.0,
                    entitlement: Some(1000.0),
                    remaining: Some(500.0),
                }),
            },
            copilot_plan: "business".to_string(),
            quota_reset_date: "2026-02-01T00:00:00Z".to_string(),
        };

        let now = OffsetDateTime::now_utc();
        let snapshot = build_usage_snapshot(&response, now);

        let primary = snapshot.primary.as_ref().expect("primary should exist");
        assert!((primary.used_percent - 20.0).abs() < f64::EPSILON);
        assert!(primary.resets_at.is_some());

        let secondary = snapshot.secondary.as_ref().expect("secondary should exist");
        assert!((secondary.used_percent - 50.0).abs() < f64::EPSILON);

        let identity = snapshot.identity.as_ref().expect("identity should exist");
        assert_eq!(identity.login_method.as_deref(), Some("Business"));
    }

    #[test]
    fn test_build_usage_snapshot_no_premium() {
        let response = CopilotUsageResponse {
            quota_snapshots: CopilotQuotaSnapshots {
                premium_interactions: None,
                chat: None,
            },
            copilot_plan: String::new(),
            quota_reset_date: String::new(),
        };

        let now = OffsetDateTime::now_utc();
        let snapshot = build_usage_snapshot(&response, now);

        let primary = snapshot.primary.as_ref().expect("fallback primary should exist");
        assert!((primary.used_percent - 0.0).abs() < f64::EPSILON);
        assert!(snapshot.secondary.is_none());
        assert_eq!(
            snapshot
                .identity
                .as_ref()
                .and_then(|identity| identity.login_method.as_deref()),
            None
        );
    }

    #[test]
    fn test_build_usage_snapshot_clamping() {
        let response = CopilotUsageResponse {
            quota_snapshots: CopilotQuotaSnapshots {
                premium_interactions: Some(CopilotQuotaSnapshot {
                    percent_remaining: -10.0,
                    entitlement: None,
                    remaining: None,
                }),
                chat: Some(CopilotQuotaSnapshot {
                    percent_remaining: 200.0,
                    entitlement: None,
                    remaining: None,
                }),
            },
            copilot_plan: "individual".to_string(),
            quota_reset_date: String::new(),
        };

        let now = OffsetDateTime::now_utc();
        let snapshot = build_usage_snapshot(&response, now);

        let primary = snapshot.primary.as_ref().expect("primary should exist");
        assert!((primary.used_percent - 100.0).abs() < f64::EPSILON);

        let secondary = snapshot.secondary.as_ref().expect("secondary should exist");
        assert!((secondary.used_percent - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_resolve_token_from_config() {
        let api_key = Some("test-token-123".to_string());
        let result = resolve_token(&api_key);
        assert!(result.is_ok());
        assert_eq!(result.expect("should resolve"), "test-token-123");
    }

    #[test]
    fn test_resolve_token_empty_returns_error() {
        let api_key = Some("   ".to_string());
        let result = resolve_token(&api_key);
        assert!(result.is_err());
        let error = result.expect_err("should fail");
        assert!(error.message.contains("token missing"));
    }

    #[test]
    fn test_resolve_token_none_without_env_returns_error() {
        // This test depends on COPILOT_API_TOKEN not being set in the test env.
        // If it is set, the test will pass the token resolution but that's acceptable.
        let api_key: Option<String> = None;
        let result = resolve_token(&api_key);
        if std::env::var("COPILOT_API_TOKEN").is_err() {
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_format_reset_date() {
        let date = OffsetDateTime::from_unix_timestamp(1738368000);
        if let Ok(date) = date {
            let formatted = format_reset_date(&date);
            assert!(!formatted.is_empty());
        }
    }

    #[test]
    fn test_deserialize_usage_response() {
        let json = r#"{
            "quota_snapshots": {
                "premium_interactions": {
                    "percent_remaining": 85.5,
                    "entitlement": 300,
                    "remaining": 256,
                    "quota_id": "premium"
                },
                "chat": {
                    "percent_remaining": 60.0,
                    "entitlement": 1000,
                    "remaining": 600,
                    "quota_id": "chat"
                }
            },
            "copilot_plan": "business",
            "assigned_date": "2025-01-01",
            "quota_reset_date": "2026-02-01T00:00:00Z"
        }"#;

        let response: CopilotUsageResponse =
            serde_json::from_str(json).expect("should deserialize");
        assert_eq!(response.copilot_plan, "business");
        assert_eq!(response.quota_reset_date, "2026-02-01T00:00:00Z");

        let premium = response
            .quota_snapshots
            .premium_interactions
            .as_ref()
            .expect("premium_interactions should exist");
        assert!((premium.percent_remaining - 85.5).abs() < f64::EPSILON);
        assert_eq!(premium.entitlement, Some(300.0));
        assert_eq!(premium.remaining, Some(256.0));

        let chat = response
            .quota_snapshots
            .chat
            .as_ref()
            .expect("chat should exist");
        assert!((chat.percent_remaining - 60.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_deserialize_usage_response_minimal() {
        let json = r#"{
            "quota_snapshots": {}
        }"#;

        let response: CopilotUsageResponse =
            serde_json::from_str(json).expect("should deserialize minimal");
        assert!(response.quota_snapshots.premium_interactions.is_none());
        assert!(response.quota_snapshots.chat.is_none());
        assert!(response.copilot_plan.is_empty());
        assert!(response.quota_reset_date.is_empty());
    }

    #[test]
    fn test_device_flow_error_response_parsing() {
        let json = r#"{"error": "authorization_pending", "error_description": "waiting for user"}"#;
        let response: DeviceFlowErrorResponse =
            serde_json::from_str(json).expect("should deserialize");
        assert_eq!(response.error.as_deref(), Some("authorization_pending"));
        assert_eq!(
            response.error_description.as_deref(),
            Some("waiting for user")
        );
    }
}
