use crate::config::ProviderSourceMode;
use crate::models::{ProviderErrorPayload, ProviderIdentitySnapshot, ProviderPayload, RateWindow, UsageSnapshot};
use crate::provider_metadata::ProviderId;
use super::{Provider, ProviderFetchContext};
use serde::Deserialize;
use time::OffsetDateTime;

pub struct ZaiProvider;

impl Provider for ZaiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Zai
    }

    fn display_name(&self) -> &'static str {
        "z.ai"
    }

    fn fetch(&self, context: &ProviderFetchContext) -> Result<ProviderPayload, ProviderErrorPayload> {
        let source_mode = context.source_mode();
        match source_mode {
            ProviderSourceMode::Auto | ProviderSourceMode::Api => {}
            _ => {
                return Err(ProviderErrorPayload {
                    code: 1,
                    message: "z.ai only supports API-based access.".to_string(),
                    kind: Some("provider".to_string()),
                });
            }
        }

        let api_key = context
            .provider_config
            .api_key
            .clone()
            .or_else(|| std::env::var("Z_AI_API_KEY").ok())
            .unwrap_or_default();

        let api_key = clean_string(&api_key);

        if api_key.is_empty() {
            return Err(ProviderErrorPayload {
                code: 1,
                message: "z.ai API key missing. Set apiKey in config or Z_AI_API_KEY env var."
                    .to_string(),
                kind: Some("provider".to_string()),
            });
        }

        let region = context
            .provider_config
            .region
            .as_deref()
            .and_then(ZaiRegion::from_str)
            .unwrap_or(ZaiRegion::Global);

        let quota_url = resolve_quota_url(region);

        let snapshot = fetch_usage(&api_key, &quota_url, context.now).map_err(|error| {
            ProviderErrorPayload {
                code: 1,
                message: error,
                kind: Some("provider".to_string()),
            }
        })?;

        let usage_snapshot = snapshot.to_usage_snapshot();

        Ok(ProviderPayload {
            provider: ProviderId::Zai.to_string(),
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

#[derive(Debug, Clone, Copy)]
enum ZaiRegion {
    Global,
    BigModelCN,
}

impl ZaiRegion {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "global" => Some(ZaiRegion::Global),
            "bigmodel-cn" => Some(ZaiRegion::BigModelCN),
            _ => None,
        }
    }

    fn base_url(self) -> &'static str {
        match self {
            ZaiRegion::Global => "https://api.z.ai",
            ZaiRegion::BigModelCN => "https://open.bigmodel.cn",
        }
    }
}

const QUOTA_API_PATH: &str = "api/monitor/usage/quota/limit";

fn resolve_quota_url(region: ZaiRegion) -> String {
    if let Some(full_url) = env_cleaned("Z_AI_QUOTA_URL") {
        return ensure_url_scheme(&full_url);
    }

    if let Some(host) = env_cleaned("Z_AI_API_HOST") {
        let base = ensure_url_scheme(&host);
        let base = base.trim_end_matches('/');
        return format!("{base}/{QUOTA_API_PATH}");
    }

    let base = region.base_url();
    format!("{base}/{QUOTA_API_PATH}")
}

fn ensure_url_scheme(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        format!("https://{value}")
    }
}

fn env_cleaned(key: &str) -> Option<String> {
    let value = std::env::var(key).ok()?;
    let cleaned = clean_string(&value);
    if cleaned.is_empty() { None } else { Some(cleaned) }
}

fn clean_string(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut value = trimmed.to_string();
    if (value.starts_with('"') && value.ends_with('"'))
        || (value.starts_with('\'') && value.ends_with('\''))
    {
        value.remove(0);
        value.pop();
    }

    value.trim().to_string()
}

// --- z.ai API response types ---

#[derive(Debug, Deserialize)]
struct ZaiQuotaResponse {
    code: i32,
    #[serde(default)]
    msg: String,
    data: Option<ZaiQuotaData>,
    #[serde(default)]
    success: bool,
}

impl ZaiQuotaResponse {
    fn is_success(&self) -> bool {
        self.success && self.code == 200
    }
}

#[derive(Debug, Deserialize)]
struct ZaiQuotaData {
    #[serde(default)]
    limits: Vec<ZaiLimitRaw>,
    #[serde(rename = "planName")]
    plan_name: Option<String>,
    plan: Option<String>,
    #[serde(rename = "plan_type")]
    plan_type: Option<String>,
    #[serde(rename = "packageName")]
    package_name: Option<String>,
}

impl ZaiQuotaData {
    fn resolved_plan_name(&self) -> Option<String> {
        [&self.plan_name, &self.plan, &self.plan_type, &self.package_name]
            .iter()
            .filter_map(|field| field.as_ref())
            .map(|value| value.trim().to_string())
            .find(|value| !value.is_empty())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ZaiLimitRaw {
    #[serde(rename = "type")]
    limit_type: String,
    #[serde(default)]
    unit: i32,
    #[serde(default)]
    number: i64,
    #[serde(default)]
    usage: i64,
    #[serde(default)]
    current_value: i64,
    #[serde(default)]
    remaining: i64,
    #[serde(default)]
    percentage: f64,
    next_reset_time: Option<i64>,
}

// --- Limit domain types ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ZaiLimitType {
    TokensLimit,
    TimeLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ZaiLimitUnit {
    Unknown,
    Days,
    Hours,
    Minutes,
}

impl ZaiLimitUnit {
    fn from_raw(value: i32) -> Self {
        match value {
            1 => ZaiLimitUnit::Days,
            3 => ZaiLimitUnit::Hours,
            5 => ZaiLimitUnit::Minutes,
            _ => ZaiLimitUnit::Unknown,
        }
    }

    fn label(self) -> Option<&'static str> {
        match self {
            ZaiLimitUnit::Minutes => Some("minute"),
            ZaiLimitUnit::Hours => Some("hour"),
            ZaiLimitUnit::Days => Some("day"),
            ZaiLimitUnit::Unknown => None,
        }
    }
}

#[derive(Debug)]
struct ZaiLimitEntry {
    limit_type: ZaiLimitType,
    unit: ZaiLimitUnit,
    number: i64,
    usage: i64,
    remaining: i64,
    current_value: i64,
    percentage: f64,
    next_reset_time: Option<OffsetDateTime>,
}

impl ZaiLimitEntry {
    fn from_raw(raw: &ZaiLimitRaw) -> Option<Self> {
        let limit_type = match raw.limit_type.as_str() {
            "TOKENS_LIMIT" => ZaiLimitType::TokensLimit,
            "TIME_LIMIT" => ZaiLimitType::TimeLimit,
            _ => return None,
        };

        let unit = ZaiLimitUnit::from_raw(raw.unit);

        let next_reset_time = raw.next_reset_time.and_then(|epoch_ms| {
            let seconds = epoch_ms / 1000;
            OffsetDateTime::from_unix_timestamp(seconds).ok()
        });

        Some(ZaiLimitEntry {
            limit_type,
            unit,
            number: raw.number,
            usage: raw.usage,
            remaining: raw.remaining,
            current_value: raw.current_value,
            percentage: raw.percentage,
            next_reset_time,
        })
    }

    fn used_percent(&self) -> f64 {
        if let Some(computed) = self.computed_used_percent() {
            return computed;
        }
        self.percentage.clamp(0.0, 100.0)
    }

    fn computed_used_percent(&self) -> Option<f64> {
        if self.usage <= 0 {
            return None;
        }
        let limit = self.usage.max(0);
        if limit == 0 {
            return None;
        }

        let used_from_remaining = limit - self.remaining;
        let used = used_from_remaining.max(self.current_value).clamp(0, limit);
        let percent = (used as f64 / limit as f64) * 100.0;
        Some(percent.clamp(0.0, 100.0))
    }

    fn window_minutes(&self) -> Option<i64> {
        if self.number <= 0 {
            return None;
        }
        match self.unit {
            ZaiLimitUnit::Minutes => Some(self.number),
            ZaiLimitUnit::Hours => Some(self.number * 60),
            ZaiLimitUnit::Days => Some(self.number * 24 * 60),
            ZaiLimitUnit::Unknown => None,
        }
    }

    fn window_description(&self) -> Option<String> {
        if self.number <= 0 {
            return None;
        }
        let unit_label = self.unit.label()?;
        let suffix = if self.number == 1 {
            unit_label.to_string()
        } else {
            format!("{unit_label}s")
        };
        Some(format!("{} {suffix}", self.number))
    }

    fn window_label(&self) -> Option<String> {
        self.window_description().map(|description| format!("{description} window"))
    }

    fn reset_description(&self) -> Option<String> {
        if let Some(label) = self.window_label() {
            return Some(label);
        }
        if self.limit_type == ZaiLimitType::TimeLimit {
            return Some("Monthly".to_string());
        }
        None
    }

    fn to_rate_window(&self) -> RateWindow {
        RateWindow {
            used_percent: self.used_percent(),
            window_minutes: if self.limit_type == ZaiLimitType::TokensLimit {
                self.window_minutes()
            } else {
                None
            },
            resets_at: self.next_reset_time,
            reset_description: self.reset_description(),
        }
    }
}

// --- Snapshot ---

struct ZaiUsageSnapshot {
    token_limit: Option<ZaiLimitEntry>,
    time_limit: Option<ZaiLimitEntry>,
    plan_name: Option<String>,
    updated_at: OffsetDateTime,
}

impl ZaiUsageSnapshot {
    fn to_usage_snapshot(&self) -> UsageSnapshot {
        let primary_entry = self.token_limit.as_ref().or(self.time_limit.as_ref());
        let secondary_entry = if self.token_limit.is_some() && self.time_limit.is_some() {
            self.time_limit.as_ref()
        } else {
            None
        };

        let primary = primary_entry.map(|entry| entry.to_rate_window());
        let secondary = secondary_entry.map(|entry| entry.to_rate_window());

        let login_method = self
            .plan_name
            .as_ref()
            .map(|plan| plan.trim().to_string())
            .filter(|plan| !plan.is_empty());

        let identity = ProviderIdentitySnapshot {
            provider_id: Some(ProviderId::Zai.to_string()),
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

// --- Fetching ---

fn fetch_usage(api_key: &str, quota_url: &str, now: OffsetDateTime) -> Result<ZaiUsageSnapshot, String> {
    let response = ureq::get(quota_url)
        .set("authorization", &format!("Bearer {api_key}"))
        .set("accept", "application/json")
        .call();

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response
                .into_string()
                .unwrap_or_else(|_| "<no body>".to_string());
            if code == 401 || code == 403 {
                return Err("Invalid z.ai API credentials".to_string());
            }
            return Err(format!("z.ai API error (HTTP {code}): {body}"));
        }
        Err(error) => return Err(format!("z.ai network error: {error}")),
    };

    let body: serde_json::Value = response
        .into_json()
        .map_err(|error| format!("Failed to parse z.ai JSON response: {error}"))?;

    parse_usage_snapshot(body, now)
}

fn parse_usage_snapshot(value: serde_json::Value, now: OffsetDateTime) -> Result<ZaiUsageSnapshot, String> {
    let api_response: ZaiQuotaResponse = serde_json::from_value(value)
        .map_err(|error| format!("Failed to decode z.ai response: {error}"))?;

    if !api_response.is_success() {
        return Err(format!("z.ai API error: {}", api_response.msg));
    }

    let data = api_response
        .data
        .ok_or_else(|| "z.ai response missing data field".to_string())?;

    let plan_name = data.resolved_plan_name();

    let mut token_limit: Option<ZaiLimitEntry> = None;
    let mut time_limit: Option<ZaiLimitEntry> = None;

    for raw_limit in &data.limits {
        if let Some(entry) = ZaiLimitEntry::from_raw(raw_limit) {
            match entry.limit_type {
                ZaiLimitType::TokensLimit => token_limit = Some(entry),
                ZaiLimitType::TimeLimit => time_limit = Some(entry),
            }
        }
    }

    if token_limit.is_none() && time_limit.is_none() {
        return Err("z.ai response contains no valid limit entries".to_string());
    }

    Ok(ZaiUsageSnapshot {
        token_limit,
        time_limit,
        plan_name,
        updated_at: now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_response() -> serde_json::Value {
        serde_json::json!({
            "code": 200,
            "msg": "success",
            "success": true,
            "data": {
                "limits": [
                    {
                        "type": "TOKENS_LIMIT",
                        "unit": 3,
                        "number": 3,
                        "usage": 1000,
                        "currentValue": 250,
                        "remaining": 750,
                        "percentage": 25,
                        "usageDetails": [
                            {"modelCode": "glm-4", "usage": 250}
                        ],
                        "nextResetTime": 1706486400000_i64
                    },
                    {
                        "type": "TIME_LIMIT",
                        "unit": 1,
                        "number": 30,
                        "usage": 100,
                        "currentValue": 10,
                        "remaining": 90,
                        "percentage": 10,
                        "usageDetails": [],
                        "nextResetTime": null
                    }
                ],
                "planName": "Pro Plan"
            }
        })
    }

    #[test]
    fn test_parse_basic_response() {
        let now = OffsetDateTime::now_utc();
        let snapshot = parse_usage_snapshot(sample_response(), now).expect("should parse");

        assert!(snapshot.token_limit.is_some());
        assert!(snapshot.time_limit.is_some());
        assert_eq!(snapshot.plan_name.as_deref(), Some("Pro Plan"));
    }

    #[test]
    fn test_token_limit_fields() {
        let now = OffsetDateTime::now_utc();
        let snapshot = parse_usage_snapshot(sample_response(), now).expect("should parse");
        let token = snapshot.token_limit.as_ref().expect("token limit present");

        assert_eq!(token.limit_type, ZaiLimitType::TokensLimit);
        assert_eq!(token.unit, ZaiLimitUnit::Hours);
        assert_eq!(token.number, 3);
        assert_eq!(token.window_minutes(), Some(180));
        assert!(token.next_reset_time.is_some());
    }

    #[test]
    fn test_time_limit_fields() {
        let now = OffsetDateTime::now_utc();
        let snapshot = parse_usage_snapshot(sample_response(), now).expect("should parse");
        let time = snapshot.time_limit.as_ref().expect("time limit present");

        assert_eq!(time.limit_type, ZaiLimitType::TimeLimit);
        assert_eq!(time.unit, ZaiLimitUnit::Days);
        assert_eq!(time.number, 30);
        assert_eq!(time.window_minutes(), Some(30 * 24 * 60));
    }

    #[test]
    fn test_usage_snapshot_mapping() {
        let now = OffsetDateTime::now_utc();
        let snapshot = parse_usage_snapshot(sample_response(), now).expect("should parse");
        let usage = snapshot.to_usage_snapshot();

        assert!(usage.primary.is_some());
        assert!(usage.secondary.is_some());
        assert!(usage.tertiary.is_none());

        let primary = usage.primary.as_ref().expect("primary");
        assert_eq!(primary.window_minutes, Some(180));
        assert!(primary.resets_at.is_some());

        let secondary = usage.secondary.as_ref().expect("secondary");
        assert!(secondary.window_minutes.is_none());

        let identity = usage.identity.as_ref().expect("identity");
        assert_eq!(identity.login_method.as_deref(), Some("Pro Plan"));
    }

    #[test]
    fn test_tokens_only_response() {
        let value = serde_json::json!({
            "code": 200, "msg": "ok", "success": true,
            "data": {
                "limits": [{
                    "type": "TOKENS_LIMIT",
                    "unit": 5, "number": 60,
                    "usage": 500, "currentValue": 100, "remaining": 400,
                    "percentage": 20
                }]
            }
        });
        let snapshot = parse_usage_snapshot(value, OffsetDateTime::now_utc()).expect("parse");
        let usage = snapshot.to_usage_snapshot();

        assert!(usage.primary.is_some());
        assert!(usage.secondary.is_none());
    }

    #[test]
    fn test_time_only_response() {
        let value = serde_json::json!({
            "code": 200, "msg": "ok", "success": true,
            "data": {
                "limits": [{
                    "type": "TIME_LIMIT",
                    "unit": 1, "number": 30,
                    "usage": 50, "currentValue": 5, "remaining": 45,
                    "percentage": 10
                }]
            }
        });
        let snapshot = parse_usage_snapshot(value, OffsetDateTime::now_utc()).expect("parse");
        let usage = snapshot.to_usage_snapshot();

        assert!(usage.primary.is_some());
        assert!(usage.secondary.is_none());
    }

    #[test]
    fn test_api_error_response() {
        let value = serde_json::json!({
            "code": 500, "msg": "internal error", "success": false,
            "data": null
        });
        let result = parse_usage_snapshot(value, OffsetDateTime::now_utc());
        assert!(result.is_err());
        assert!(result.err().expect("error").contains("internal error"));
    }

    #[test]
    fn test_empty_limits() {
        let value = serde_json::json!({
            "code": 200, "msg": "ok", "success": true,
            "data": { "limits": [] }
        });
        let result = parse_usage_snapshot(value, OffsetDateTime::now_utc());
        assert!(result.is_err());
    }

    #[test]
    fn test_unknown_limit_type_skipped() {
        let value = serde_json::json!({
            "code": 200, "msg": "ok", "success": true,
            "data": {
                "limits": [{
                    "type": "UNKNOWN_LIMIT",
                    "unit": 1, "number": 1,
                    "usage": 10, "currentValue": 1, "remaining": 9,
                    "percentage": 10
                }]
            }
        });
        let result = parse_usage_snapshot(value, OffsetDateTime::now_utc());
        assert!(result.is_err());
    }

    #[test]
    fn test_plan_name_fallback_keys() {
        let value = serde_json::json!({
            "code": 200, "msg": "ok", "success": true,
            "data": {
                "limits": [{
                    "type": "TOKENS_LIMIT",
                    "unit": 3, "number": 1,
                    "usage": 100, "currentValue": 10, "remaining": 90,
                    "percentage": 10
                }],
                "plan": "Business"
            }
        });
        let snapshot = parse_usage_snapshot(value, OffsetDateTime::now_utc()).expect("parse");
        assert_eq!(snapshot.plan_name.as_deref(), Some("Business"));
    }

    #[test]
    fn test_computed_used_percent() {
        let entry = ZaiLimitEntry {
            limit_type: ZaiLimitType::TokensLimit,
            unit: ZaiLimitUnit::Hours,
            number: 3,
            usage: 1000,
            current_value: 300,
            remaining: 700,
            percentage: 25.0,
            next_reset_time: None,
        };
        let used = entry.used_percent();
        assert!((used - 30.0).abs() < 0.01);
    }

    #[test]
    fn test_percentage_fallback_when_no_usage() {
        let entry = ZaiLimitEntry {
            limit_type: ZaiLimitType::TokensLimit,
            unit: ZaiLimitUnit::Hours,
            number: 3,
            usage: 0,
            current_value: 0,
            remaining: 0,
            percentage: 42.0,
            next_reset_time: None,
        };
        assert!((entry.used_percent() - 42.0).abs() < 0.01);
    }

    #[test]
    fn test_window_description() {
        let entry = ZaiLimitEntry {
            limit_type: ZaiLimitType::TokensLimit,
            unit: ZaiLimitUnit::Hours,
            number: 3,
            usage: 0,
            current_value: 0,
            remaining: 0,
            percentage: 0.0,
            next_reset_time: None,
        };
        assert_eq!(entry.window_description(), Some("3 hours".to_string()));
        assert_eq!(entry.window_label(), Some("3 hours window".to_string()));
    }

    #[test]
    fn test_window_description_singular() {
        let entry = ZaiLimitEntry {
            limit_type: ZaiLimitType::TokensLimit,
            unit: ZaiLimitUnit::Days,
            number: 1,
            usage: 0,
            current_value: 0,
            remaining: 0,
            percentage: 0.0,
            next_reset_time: None,
        };
        assert_eq!(entry.window_description(), Some("1 day".to_string()));
    }

    #[test]
    fn test_clean_string() {
        assert_eq!(clean_string("  hello  "), "hello");
        assert_eq!(clean_string("\"quoted\""), "quoted");
        assert_eq!(clean_string("'single'"), "single");
        assert_eq!(clean_string("  "), "");
        assert_eq!(clean_string("plain"), "plain");
    }

    #[test]
    fn test_resolve_quota_url_default() {
        unsafe {
            std::env::remove_var("Z_AI_QUOTA_URL");
            std::env::remove_var("Z_AI_API_HOST");
        }
        let url = resolve_quota_url(ZaiRegion::Global);
        assert_eq!(url, "https://api.z.ai/api/monitor/usage/quota/limit");
    }

    #[test]
    fn test_resolve_quota_url_bigmodel() {
        unsafe {
            std::env::remove_var("Z_AI_QUOTA_URL");
            std::env::remove_var("Z_AI_API_HOST");
        }
        let url = resolve_quota_url(ZaiRegion::BigModelCN);
        assert_eq!(url, "https://open.bigmodel.cn/api/monitor/usage/quota/limit");
    }

    #[test]
    fn test_reset_description_time_limit_monthly() {
        let entry = ZaiLimitEntry {
            limit_type: ZaiLimitType::TimeLimit,
            unit: ZaiLimitUnit::Unknown,
            number: 0,
            usage: 50,
            current_value: 5,
            remaining: 45,
            percentage: 10.0,
            next_reset_time: None,
        };
        assert_eq!(entry.reset_description(), Some("Monthly".to_string()));
    }

    #[test]
    fn test_rate_window_no_window_minutes_for_time_limit() {
        let entry = ZaiLimitEntry {
            limit_type: ZaiLimitType::TimeLimit,
            unit: ZaiLimitUnit::Days,
            number: 30,
            usage: 50,
            current_value: 5,
            remaining: 45,
            percentage: 10.0,
            next_reset_time: None,
        };
        let rate_window = entry.to_rate_window();
        assert!(rate_window.window_minutes.is_none());
    }

    #[test]
    fn test_ensure_url_scheme() {
        assert_eq!(ensure_url_scheme("example.com"), "https://example.com");
        assert_eq!(ensure_url_scheme("https://example.com"), "https://example.com");
        assert_eq!(ensure_url_scheme("http://example.com"), "http://example.com");
    }
}
