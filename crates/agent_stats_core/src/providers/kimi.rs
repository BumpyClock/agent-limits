use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use crate::config::ProviderSourceMode;
use crate::models::{ProviderErrorPayload, ProviderIdentitySnapshot, ProviderPayload, RateWindow, UsageSnapshot};
use crate::provider_metadata::ProviderId;
use crate::token_storage::{SecureTokenStorage, accounts};
use super::{Provider, ProviderFetchContext};
use regex::Regex;
use serde::Deserialize;
use time::OffsetDateTime;

const USAGE_URL: &str =
    "https://www.kimi.com/apiv2/kimi.gateway.billing.v1.BillingService/GetUsages";
const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";

pub struct KimiProvider;

impl Provider for KimiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Kimi
    }

    fn display_name(&self) -> &'static str {
        "Kimi"
    }

    fn fetch(
        &self,
        context: &ProviderFetchContext,
    ) -> Result<ProviderPayload, ProviderErrorPayload> {
        let source_mode = context.source_mode();
        match source_mode {
            ProviderSourceMode::Auto | ProviderSourceMode::Web | ProviderSourceMode::Api => {}
            _ => {
                return Err(provider_error(
                    "Kimi supports Auto/Web/Api source modes only.",
                ));
            }
        }

        let token = resolve_token(context).ok_or_else(|| {
            provider_error(
                "Kimi auth token is missing. Please add your JWT token from the Kimi console.",
            )
        })?;

        let session_info = decode_session_info(&token);
        let timezone = std::env::var("TZ").unwrap_or_else(|_| "UTC".to_string());

        let mut request = ureq::post(USAGE_URL)
            .set("Content-Type", "application/json")
            .set("Authorization", &format!("Bearer {token}"))
            .set("Cookie", &format!("kimi-auth={token}"))
            .set("Origin", "https://www.kimi.com")
            .set("Referer", "https://www.kimi.com/code/console")
            .set("Accept", "*/*")
            .set("Accept-Language", "en-US,en;q=0.9")
            .set("User-Agent", USER_AGENT)
            .set("connect-protocol-version", "1")
            .set("x-language", "en-US")
            .set("x-msh-platform", "web")
            .set("r-timezone", &timezone);

        if let Some(session_info) = session_info {
            if let Some(device_id) = session_info.device_id {
                request = request.set("x-msh-device-id", &device_id);
            }
            if let Some(session_id) = session_info.session_id {
                request = request.set("x-msh-session-id", &session_id);
            }
            if let Some(traffic_id) = session_info.traffic_id {
                request = request.set("x-traffic-id", &traffic_id);
            }
        }

        let response = request.send_json(serde_json::json!({"scope": ["FEATURE_CODING"]}));
        let response = match response {
            Ok(response) => response,
            Err(ureq::Error::Status(code, response)) => {
                let body = response.into_string().unwrap_or_default();
                if code == 401 || code == 403 {
                    return Err(provider_error(
                        "Kimi auth token is invalid or expired. Please refresh your token.",
                    ));
                }
                if code == 400 {
                    return Err(provider_error("Invalid request: Bad request"));
                }
                return Err(provider_error(&format!(
                    "Kimi API error (HTTP {code}): {}",
                    body.chars().take(200).collect::<String>()
                )));
            }
            Err(error) => {
                return Err(provider_error(&format!("Kimi network error: {error}")));
            }
        };

        let body = response
            .into_string()
            .map_err(|error| provider_error(&format!("Kimi response invalid: {error}")))?;

        let usage_response: KimiUsageResponse = serde_json::from_str(&body)
            .map_err(|error| provider_error(&format!("Failed to parse Kimi usage: {error}")))?;

        let coding_usage = usage_response
            .usages
            .into_iter()
            .find(|usage| usage.scope == "FEATURE_CODING")
            .ok_or_else(|| {
                provider_error("FEATURE_CODING scope not found in Kimi response.")
            })?;

        let usage_snapshot = build_usage_snapshot(coding_usage, context.now);

        Ok(ProviderPayload {
            provider: ProviderId::Kimi.to_string(),
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

#[derive(Debug, Deserialize)]
struct KimiUsageResponse {
    usages: Vec<KimiUsage>,
}

#[derive(Debug, Deserialize)]
struct KimiUsage {
    scope: String,
    detail: KimiUsageDetail,
    limits: Option<Vec<KimiRateLimit>>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct KimiUsageDetail {
    limit: String,
    used: Option<String>,
    remaining: Option<String>,
    reset_time: String,
}

#[derive(Debug, Deserialize)]
struct KimiRateLimit {
    detail: KimiUsageDetail,
}

fn build_usage_snapshot(usage: KimiUsage, now: OffsetDateTime) -> UsageSnapshot {
    let weekly_window = build_rate_window(&usage.detail, None, |used, limit| {
        format!("{used}/{limit} requests")
    });

    let rate_window = usage
        .limits
        .as_ref()
        .and_then(|limits| limits.first())
        .map(|limit| build_rate_window(&limit.detail, Some(300), |used, limit| {
            format!("Rate: {used}/{limit} per 5 hours")
        }));

    let identity = ProviderIdentitySnapshot {
        provider_id: Some(ProviderId::Kimi.to_string()),
        account_email: None,
        account_organization: None,
        login_method: None,
    };

    UsageSnapshot {
        primary: Some(weekly_window),
        secondary: rate_window,
        tertiary: None,
        provider_cost: None,
        updated_at: now,
        identity: Some(identity),
    }
}

fn build_rate_window(
    detail: &KimiUsageDetail,
    window_minutes: Option<i64>,
    description: impl Fn(i64, i64) -> String,
) -> RateWindow {
    let limit = parse_i64(&detail.limit).unwrap_or(0);
    let remaining = detail.remaining.as_ref().and_then(|value| parse_i64(value));
    let used = detail
        .used
        .as_ref()
        .and_then(|value| parse_i64(value))
        .or_else(|| remaining.map(|remaining| (limit - remaining).max(0)))
        .unwrap_or(0);

    let used_percent = if limit > 0 {
        (used as f64 / limit as f64) * 100.0
    } else {
        0.0
    };

    let resets_at = parse_rfc3339(&detail.reset_time);

    RateWindow {
        used_percent: used_percent.max(0.0).min(100.0),
        window_minutes,
        resets_at,
        reset_description: Some(description(used, limit)),
    }
}

fn parse_i64(raw: &str) -> Option<i64> {
    raw.trim().parse::<i64>().ok()
}

fn parse_rfc3339(raw: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339).ok()
}

struct SessionInfo {
    device_id: Option<String>,
    session_id: Option<String>,
    traffic_id: Option<String>,
}

fn decode_session_info(token: &str) -> Option<SessionInfo> {
    let mut parts = token.split('.');
    let _ = parts.next()?;
    let payload = parts.next()?;

    let payload_data = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&payload_data).ok()?;

    Some(SessionInfo {
        device_id: json.get("device_id").and_then(|value| value.as_str()).map(|value| value.to_string()),
        session_id: json.get("ssid").and_then(|value| value.as_str()).map(|value| value.to_string()),
        traffic_id: json.get("sub").and_then(|value| value.as_str()).map(|value| value.to_string()),
    })
}

fn resolve_token(context: &ProviderFetchContext) -> Option<String> {
    // Priority 1: Config api_key (existing behavior)
    if let Some(api_key) = context.provider_config.api_key.as_ref() {
        if let Some(cleaned) = cleaned(api_key) {
            return Some(cleaned);
        }
    }

    // Priority 2: Cookie header from config (existing behavior)
    if let Some(cookie_header) = context.provider_config.cookie_header.as_ref() {
        if let Some(token) = override_from(cookie_header) {
            return Some(token);
        }
    }

    // Priority 3: Secure storage
    let storage = SecureTokenStorage::default();
    if storage.is_available() {
        if let Ok(stored_token) = storage.retrieve(accounts::KIMI) {
            if let Some(token) = override_from(&stored_token) {
                return Some(token);
            }
        }
    }

    // Priority 4: Environment variables
    if let Ok(env_token) = std::env::var("KIMI_AUTH_TOKEN") {
        if let Some(token) = override_from(&env_token) {
            return Some(token);
        }
    }
    if let Ok(env_token) = std::env::var("kimi_auth_token") {
        if let Some(token) = override_from(&env_token) {
            return Some(token);
        }
    }
    if let Ok(env_token) = std::env::var("KIMI_MANUAL_COOKIE") {
        if let Some(token) = override_from(&env_token) {
            return Some(token);
        }
    }

    None
}

fn override_from(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    if let Some(token) = extract_kimi_auth_token(raw) {
        return Some(token);
    }

    if let Some(header) = extract_cookie_header(raw) {
        if let Some(token) = extract_kimi_auth_token(&header) {
            return Some(token);
        }
    }

    if looks_like_jwt(raw) {
        return Some(raw.to_string());
    }

    None
}

fn extract_kimi_auth_token(raw: &str) -> Option<String> {
    let patterns = [
        r"(?i)kimi-auth=([A-Za-z0-9._\-+=/]+)",
        r"(?i)kimi-auth:\s*([A-Za-z0-9._\-+=/]+)",
    ];

    for pattern in patterns {
        if let Ok(regex) = Regex::new(pattern) {
            if let Some(capture) = regex.captures(raw).and_then(|captures| captures.get(1)) {
                let token = capture.as_str().trim();
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
        }
    }

    None
}

fn extract_cookie_header(raw: &str) -> Option<String> {
    let patterns = [
        "(?i)-H\\s*'Cookie:\\s*([^']+)'",
        "(?i)-H\\s*\\\"Cookie:\\s*([^\\\"]+)\\\"",
        "(?i)\\bcookie:\\s*'([^']+)'",
        "(?i)\\bcookie:\\s*\\\"([^\\\"]+)\\\"",
        "(?i)\\bcookie:\\s*([^\\r\\n]+)",
    ];

    for pattern in patterns {
        if let Ok(regex) = Regex::new(pattern) {
            if let Some(capture) = regex.captures(raw).and_then(|captures| captures.get(1)) {
                let header = capture.as_str().trim();
                if !header.is_empty() {
                    return Some(header.to_string());
                }
            }
        }
    }

    None
}

fn looks_like_jwt(raw: &str) -> bool {
    raw.starts_with("eyJ") && raw.split('.').count() == 3
}

fn cleaned(raw: &str) -> Option<String> {
    let mut value = raw.trim().to_string();
    if value.is_empty() {
        return None;
    }

    if (value.starts_with('"') && value.ends_with('"'))
        || (value.starts_with('\'') && value.ends_with('\''))
    {
        value = value.trim_matches(&['"', '\''][..]).to_string();
    }

    let value = value.trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn provider_error(message: &str) -> ProviderErrorPayload {
    ProviderErrorPayload {
        code: 1,
        message: message.to_string(),
        kind: Some("provider".to_string()),
    }
}
