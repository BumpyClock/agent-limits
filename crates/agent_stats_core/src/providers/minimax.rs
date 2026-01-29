use crate::config::{ProviderConfig, ProviderSourceMode};
use crate::models::{ProviderErrorPayload, ProviderIdentitySnapshot, ProviderPayload, RateWindow, UsageSnapshot};
use crate::provider_metadata::ProviderId;
use crate::token_storage::{SecureTokenStorage, accounts};
use super::{Provider, ProviderFetchContext};
use regex::Regex;
use serde_json::Value;
use time::{Duration, OffsetDateTime};

const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";
const API_REMAINS_URL: &str = "https://api.minimaxi.com/v1/api/openplatform/coding_plan/remains";

pub struct MiniMaxProvider;

impl Provider for MiniMaxProvider {
    fn id(&self) -> ProviderId {
        ProviderId::MiniMax
    }

    fn display_name(&self) -> &'static str {
        "MiniMax"
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
                    "MiniMax supports Auto/Web/Api source modes only.",
                ));
            }
        }

        let region = MiniMaxRegion::from_config(context.provider_config.region.as_deref());
        let now = context.now;

        let cookie_raw = context.provider_config.cookie_header.clone().unwrap_or_default();

        if matches!(source_mode, ProviderSourceMode::Api)
            || (matches!(source_mode, ProviderSourceMode::Auto) && cookie_raw.trim().is_empty())
        {
            let api_token = resolve_api_token(&context.provider_config)?;
            let snapshot = fetch_api_usage(&api_token, now)?;
            return Ok(ProviderPayload {
                provider: ProviderId::MiniMax.to_string(),
                account: None,
                version: None,
                source: "api".to_string(),
                status: None,
                usage: Some(snapshot.to_usage_snapshot()),
                credits: None,
                antigravity_plan_info: None,
                openai_dashboard: None,
                error: None,
            });
        }

        let cookie_override = cookie_override(&cookie_raw)
            .ok_or_else(|| provider_error("MiniMax session not found. Provide a Cookie header."))?;
        let group_id = context
            .provider_config
            .workspace_id
            .clone()
            .or_else(|| cookie_override.group_id.clone());

        let snapshot = fetch_cookie_usage(&cookie_override, group_id.as_deref(), region, now)?;

        Ok(ProviderPayload {
            provider: ProviderId::MiniMax.to_string(),
            account: None,
            version: None,
            source: "web".to_string(),
            status: None,
            usage: Some(snapshot.to_usage_snapshot()),
            credits: None,
            antigravity_plan_info: None,
            openai_dashboard: None,
            error: None,
        })
    }
}

#[derive(Clone)]
struct MiniMaxSnapshot {
    plan_name: Option<String>,
    available_prompts: Option<i64>,
    window_minutes: Option<i64>,
    used_percent: Option<f64>,
    resets_at: Option<OffsetDateTime>,
    updated_at: OffsetDateTime,
}

impl MiniMaxSnapshot {
    fn to_usage_snapshot(&self) -> UsageSnapshot {
        let used_percent = self.used_percent.unwrap_or(0.0).max(0.0).min(100.0);
        let reset_description = self.limit_description();
        let primary = RateWindow {
            used_percent,
            window_minutes: self.window_minutes,
            resets_at: self.resets_at,
            reset_description,
        };

        let login_method = self
            .plan_name
            .as_ref()
            .and_then(|value| {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            });

        let identity = ProviderIdentitySnapshot {
            provider_id: Some(ProviderId::MiniMax.to_string()),
            account_email: None,
            account_organization: None,
            login_method,
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

    fn limit_description(&self) -> Option<String> {
        let available = self.available_prompts?;
        if available <= 0 {
            return self.window_description();
        }
        if let Some(window) = self.window_description() {
            return Some(format!("{available} prompts / {window}"));
        }
        Some(format!("{available} prompts"))
    }

    fn window_description(&self) -> Option<String> {
        let minutes = self.window_minutes?;
        if minutes <= 0 {
            return None;
        }
        if minutes % (24 * 60) == 0 {
            let days = minutes / (24 * 60);
            return Some(format!("{days} {}", if days == 1 { "day" } else { "days" }));
        }
        if minutes % 60 == 0 {
            let hours = minutes / 60;
            return Some(format!("{hours} {}", if hours == 1 { "hour" } else { "hours" }));
        }
        Some(format!("{minutes} {}", if minutes == 1 { "minute" } else { "minutes" }))
    }
}

struct CookieOverride {
    cookie_header: String,
    authorization_token: Option<String>,
    group_id: Option<String>,
}

#[derive(Clone, Copy)]
enum MiniMaxRegion {
    Global,
    China,
}

impl MiniMaxRegion {
    fn from_config(raw: Option<&str>) -> Self {
        match raw.map(|value| value.trim().to_lowercase()) {
            Some(value) if value == "cn" || value.contains("china") => MiniMaxRegion::China,
            _ => MiniMaxRegion::Global,
        }
    }

    fn base_url(self) -> &'static str {
        match self {
            MiniMaxRegion::Global => "https://platform.minimax.io",
            MiniMaxRegion::China => "https://platform.minimaxi.com",
        }
    }

    fn coding_plan_url(self) -> String {
        format!("{}/user-center/payment/coding-plan?cycle_type=3", self.base_url())
    }

    fn coding_plan_referer(self) -> String {
        format!("{}/user-center/payment/coding-plan", self.base_url())
    }

    fn remains_url(self) -> String {
        format!("{}/v1/api/openplatform/coding_plan/remains", self.base_url())
    }
}

fn fetch_api_usage(api_token: &str, now: OffsetDateTime) -> Result<MiniMaxSnapshot, ProviderErrorPayload> {
    let response = ureq::get(API_REMAINS_URL)
        .set("Authorization", &format!("Bearer {api_token}"))
        .set("Accept", "application/json")
        .set("Content-Type", "application/json")
        .set("MM-API-Source", "CodexBar")
        .call();

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            if code == 401 || code == 403 {
                return Err(provider_error("MiniMax credentials are invalid or expired."));
            }
            return Err(provider_error(&format!("MiniMax API error (HTTP {code}): {body}")));
        }
        Err(error) => {
            return Err(provider_error(&format!("MiniMax network error: {error}")));
        }
    };

    let body = response
        .into_string()
        .map_err(|error| provider_error(&format!("MiniMax response invalid: {error}")))?;
    let value: Value = serde_json::from_str(&body)
        .map_err(|error| provider_error(&format!("Failed to parse MiniMax response: {error}")))?;

    parse_payload(&value, now)
}

fn fetch_cookie_usage(
    cookie: &CookieOverride,
    group_id: Option<&str>,
    region: MiniMaxRegion,
    now: OffsetDateTime,
) -> Result<MiniMaxSnapshot, ProviderErrorPayload> {
    let url = region.coding_plan_url();
    let mut request = ureq::get(&url)
        .set("Cookie", &cookie.cookie_header)
        .set("accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .set("user-agent", USER_AGENT)
        .set("accept-language", "en-US,en;q=0.9")
        .set("origin", region.base_url())
        .set("referer", &region.coding_plan_referer());

    if let Some(token) = cookie.authorization_token.as_ref() {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }

    let response = request.call();
    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            if code == 401 || code == 403 {
                return Err(provider_error("MiniMax credentials are invalid or expired."));
            }
            return Err(provider_error(&format!("MiniMax API error (HTTP {code}): {body}")));
        }
        Err(error) => {
            return Err(provider_error(&format!("MiniMax network error: {error}")));
        }
    };

    let content_type = response.header("Content-Type").unwrap_or("").to_lowercase();
    let body = response
        .into_string()
        .map_err(|error| provider_error(&format!("MiniMax response invalid: {error}")))?;

    if content_type.contains("application/json") {
        let value: Value = serde_json::from_str(&body)
            .map_err(|error| provider_error(&format!("Failed to parse MiniMax JSON: {error}")))?;
        return parse_payload(&value, now);
    }

    if let Some(snapshot) = parse_html(&body, now) {
        return Ok(snapshot);
    }

    let remains_snapshot = fetch_remains(cookie, group_id, region, now)?;
    Ok(remains_snapshot)
}

fn fetch_remains(
    cookie: &CookieOverride,
    group_id: Option<&str>,
    region: MiniMaxRegion,
    now: OffsetDateTime,
) -> Result<MiniMaxSnapshot, ProviderErrorPayload> {
    let mut url = region.remains_url();
    if let Some(group_id) = group_id {
        if !group_id.trim().is_empty() {
            let separator = if url.contains('?') { "&" } else { "?" };
            url.push_str(separator);
            url.push_str("GroupId=");
            url.push_str(group_id);
        }
    }

    let mut request = ureq::get(&url)
        .set("Cookie", &cookie.cookie_header)
        .set("accept", "application/json, text/plain, */*")
        .set("x-requested-with", "XMLHttpRequest")
        .set("user-agent", USER_AGENT)
        .set("accept-language", "en-US,en;q=0.9")
        .set("origin", region.base_url())
        .set("referer", &region.coding_plan_referer());

    if let Some(token) = cookie.authorization_token.as_ref() {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }

    let response = request.call();
    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            if code == 401 || code == 403 {
                return Err(provider_error("MiniMax credentials are invalid or expired."));
            }
            return Err(provider_error(&format!("MiniMax API error (HTTP {code}): {body}")));
        }
        Err(error) => {
            return Err(provider_error(&format!("MiniMax network error: {error}")));
        }
    };

    let content_type = response.header("Content-Type").unwrap_or("").to_lowercase();
    let body = response
        .into_string()
        .map_err(|error| provider_error(&format!("MiniMax response invalid: {error}")))?;

    if content_type.contains("application/json") {
        let value: Value = serde_json::from_str(&body)
            .map_err(|error| provider_error(&format!("Failed to parse MiniMax JSON: {error}")))?;
        return parse_payload(&value, now);
    }

    parse_html(&body, now).ok_or_else(|| provider_error("Missing MiniMax coding plan data."))
}

fn parse_payload(value: &Value, now: OffsetDateTime) -> Result<MiniMaxSnapshot, ProviderErrorPayload> {
    let payload = find_coding_plan_payload(value).ok_or_else(|| {
        provider_error("Missing MiniMax coding plan data.")
    })?;

    let data = payload.get("data").unwrap_or(payload);

    if let Some(base_resp) = data.get("base_resp").or_else(|| data.get("baseResp")) {
        if let Some(status_code) = value_for_key(base_resp, &[
            "status_code",
            "statusCode",
        ])
        .and_then(int_from_value)
        {
            if status_code != 0 {
                let message = value_for_key(base_resp, &["status_msg", "statusMessage"])
                    .and_then(string_from_value)
                    .unwrap_or_else(|| format!("status_code {status_code}"));
                let lower = message.to_lowercase();
                if status_code == 1004
                    || lower.contains("cookie")
                    || lower.contains("log in")
                    || lower.contains("login")
                {
                    return Err(provider_error("MiniMax credentials are invalid or expired."));
                }
                return Err(provider_error(&message));
            }
        }
    }

    let model_remains = value_for_key(data, &["model_remains", "modelRemains"])
        .and_then(|value| value.as_array())
        .and_then(|array| array.first())
        .ok_or_else(|| provider_error("Missing MiniMax coding plan data."))?;

    let total = value_for_key(model_remains, &[
        "current_interval_total_count",
        "currentIntervalTotalCount",
    ])
    .and_then(int_from_value);

    let remaining = value_for_key(model_remains, &[
        "current_interval_usage_count",
        "currentIntervalUsageCount",
    ])
    .and_then(int_from_value);

    let used_percent = used_percent(total, remaining);

    let start_time = value_for_key(model_remains, &["start_time", "startTime"])
        .and_then(int_from_value)
        .and_then(epoch_to_datetime);

    let end_time = value_for_key(model_remains, &["end_time", "endTime"])
        .and_then(int_from_value)
        .and_then(epoch_to_datetime);

    let window_minutes = window_minutes(start_time, end_time);

    let remains_time = value_for_key(model_remains, &["remains_time", "remainsTime"])
        .and_then(int_from_value);

    let resets_at = resets_at(end_time, remains_time, now);

    let plan_name = parse_plan_name(data);

    Ok(MiniMaxSnapshot {
        plan_name,
        available_prompts: total,
        window_minutes,
        used_percent,
        resets_at,
        updated_at: now,
    })
}

fn parse_html(html: &str, now: OffsetDateTime) -> Option<MiniMaxSnapshot> {
    if let Some(next_data) = extract_next_data(html) {
        if let Ok(value) = serde_json::from_str::<Value>(&next_data) {
            if let Ok(snapshot) = parse_payload(&value, now) {
                return Some(snapshot);
            }
        }
    }

    let text = strip_html(html);
    let plan_name = parse_plan_name_text(html, &text);
    let available = parse_available_usage(&text);
    let used_percent = parse_used_percent(&text);
    let resets_at = parse_resets_at(&text, now);

    if plan_name.is_none() && available.is_none() && used_percent.is_none() {
        return None;
    }

    Some(MiniMaxSnapshot {
        plan_name,
        available_prompts: available.map(|value| value.0),
        window_minutes: available.map(|value| value.1),
        used_percent,
        resets_at,
        updated_at: now,
    })
}

fn extract_next_data(html: &str) -> Option<String> {
    let id_index = html.find("id=\"__NEXT_DATA__\"")?;
    let after_id = &html[id_index..];
    let open_tag = after_id.find('>')?;
    let content_start = id_index + open_tag + 1;
    let close_tag = html[content_start..].find("</script>")?;
    let content_end = content_start + close_tag;
    let raw = html[content_start..content_end].trim();
    if raw.is_empty() {
        None
    } else {
        Some(raw.to_string())
    }
}

fn find_coding_plan_payload(value: &Value) -> Option<&Value> {
    if let Value::Object(map) = value {
        if map.contains_key("model_remains") || map.contains_key("modelRemains") {
            return Some(value);
        }
        for child in map.values() {
            if let Some(found) = find_coding_plan_payload(child) {
                return Some(found);
            }
        }
    } else if let Value::Array(array) = value {
        for child in array {
            if let Some(found) = find_coding_plan_payload(child) {
                return Some(found);
            }
        }
    }
    None
}

fn parse_plan_name(data: &Value) -> Option<String> {
    let keys = [
        "current_subscribe_title",
        "currentSubscribeTitle",
        "plan_name",
        "planName",
        "combo_title",
        "comboTitle",
        "current_plan_title",
        "currentPlanTitle",
    ];

    for key in keys {
        if let Some(value) = data.get(key).and_then(|value| value.as_str()) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }

    if let Some(combo_card) = data.get("current_combo_card").or_else(|| data.get("currentComboCard")) {
        if let Some(title) = combo_card.get("title").and_then(|value| value.as_str()) {
            let trimmed = title.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }

    None
}

fn parse_plan_name_text(html: &str, text: &str) -> Option<String> {
    let candidates = [
        extract_first(r#"(?i)"planName"\s*:\s*"([^"]+)""#, html),
        extract_first(r#"(?i)"plan"\s*:\s*"([^"]+)""#, html),
        extract_first(r#"(?i)"packageName"\s*:\s*"([^"]+)""#, html),
        extract_first(r#"(?i)Coding\s*Plan\s*([A-Za-z0-9][A-Za-z0-9\s._-]{0,32})"#, text),
    ];

    for candidate in candidates.into_iter().flatten() {
        let cleaned = candidate.trim().to_string();
        if !cleaned.is_empty() {
            return Some(cleaned);
        }
    }

    None
}

fn parse_available_usage(text: &str) -> Option<(i64, i64)> {
    let pattern = r"(?i)available\s+usage[:\s]*([0-9][0-9,]*)\s*prompts?\s*/\s*([0-9]+(?:\.[0-9]+)?)\s*(hours?|hrs?|h|minutes?|mins?|m|days?|d)";
    let captures = Regex::new(pattern).ok()?.captures(text)?;
    let prompts_raw = captures.get(1)?.as_str();
    let duration_raw = captures.get(2)?.as_str();
    let unit_raw = captures.get(3)?.as_str();

    let prompts = prompts_raw.replace(',', "").parse::<i64>().ok()?;
    if prompts <= 0 {
        return None;
    }

    let duration = duration_raw.parse::<f64>().ok()?;
    let window_minutes = minutes_from(duration, unit_raw);
    if window_minutes <= 0 {
        return None;
    }
    Some((prompts, window_minutes))
}

fn parse_used_percent(text: &str) -> Option<f64> {
    for pattern in [
        r"(?i)([0-9]{1,3}(?:\.[0-9]+)?)\s*%\s*used",
        r"(?i)used\s*([0-9]{1,3}(?:\.[0-9]+)?)\s*%",
    ] {
        if let Some(raw) = extract_first(pattern, text) {
            if let Ok(value) = raw.parse::<f64>() {
                if (0.0..=100.0).contains(&value) {
                    return Some(value);
                }
            }
        }
    }
    None
}

fn parse_resets_at(text: &str, now: OffsetDateTime) -> Option<OffsetDateTime> {
    if let Some(captures) = Regex::new(
        r"(?i)resets?\s+in\s+([0-9]+)\s*(seconds?|secs?|s|minutes?|mins?|m|hours?|hrs?|h|days?|d)",
    )
    .ok()
    .and_then(|regex| regex.captures(text))
    {
        let value = captures.get(1)?.as_str().parse::<f64>().ok()?;
        let unit = captures.get(2)?.as_str();
        let seconds = seconds_from(value, unit);
        return Some(now + Duration::seconds_f64(seconds));
    }

    if let Some(captures) = Regex::new(
        r"(?i)resets?\s+at\s+([0-9]{1,2}:[0-9]{2})(?:\s*\(([^)]+)\))?",
    )
    .ok()
    .and_then(|regex| regex.captures(text))
    {
        let time_text = captures.get(1)?.as_str();
        let tz_hint = captures.get(2).map(|value| value.as_str());
        return date_for_time(time_text, tz_hint, now);
    }

    None
}

fn date_for_time(time_text: &str, tz_hint: Option<&str>, now: OffsetDateTime) -> Option<OffsetDateTime> {
    let format = time::format_description::parse("[hour]:[minute]").ok()?;
    let time = time::Time::parse(time_text, &format).ok()?;
    let _ = tz_hint;
    let date = now.date();
    let mut candidate = OffsetDateTime::new_utc(date, time);
    if candidate < now {
        candidate = candidate + Duration::days(1);
    }
    Some(candidate)
}

fn strip_html(html: &str) -> String {
    let mut text = Regex::new("<[^>]+>").map(|regex| regex.replace_all(html, " ").to_string()).unwrap_or_else(|_| html.to_string());
    text = text.replace("&nbsp;", " ");
    text = text.replace("&amp;", "&");
    text = text.replace("&lt;", "<");
    text = text.replace("&gt;", ">");
    text = Regex::new(r"\s+").map(|regex| regex.replace_all(&text, " ").to_string()).unwrap_or(text);
    text.trim().to_string()
}

fn extract_first(pattern: &str, text: &str) -> Option<String> {
    let regex = Regex::new(pattern).ok()?;
    let captures = regex.captures(text)?;
    let value = captures.get(1)?.as_str().trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn used_percent(total: Option<i64>, remaining: Option<i64>) -> Option<f64> {
    let total = total?;
    let remaining = remaining?;
    if total <= 0 {
        return None;
    }
    let used = (total - remaining).max(0) as f64;
    Some((used / total as f64 * 100.0).min(100.0).max(0.0))
}

fn window_minutes(start: Option<OffsetDateTime>, end: Option<OffsetDateTime>) -> Option<i64> {
    let start = start?;
    let end = end?;
    let minutes = ((end - start).whole_minutes()).max(0);
    if minutes > 0 { Some(minutes) } else { None }
}

fn resets_at(end: Option<OffsetDateTime>, remains: Option<i64>, now: OffsetDateTime) -> Option<OffsetDateTime> {
    if let Some(end) = end {
        if end > now {
            return Some(end);
        }
    }
    let remains = remains?;
    if remains <= 0 {
        return None;
    }
    let seconds = if remains > 1_000_000 { remains as f64 / 1000.0 } else { remains as f64 };
    Some(now + Duration::seconds_f64(seconds))
}

fn epoch_to_datetime(raw: i64) -> Option<OffsetDateTime> {
    if raw <= 0 {
        return None;
    }
    let timestamp = if raw > 1_000_000_000_000 { raw / 1000 } else { raw };
    OffsetDateTime::from_unix_timestamp(timestamp).ok()
}

fn minutes_from(value: f64, unit: &str) -> i64 {
    let lower = unit.to_lowercase();
    if lower.starts_with('d') { return (value * 24.0 * 60.0).round() as i64; }
    if lower.starts_with('h') { return (value * 60.0).round() as i64; }
    if lower.starts_with('m') { return value.round() as i64; }
    if lower.starts_with('s') { return ((value / 60.0).round() as i64).max(1); }
    0
}

fn seconds_from(value: f64, unit: &str) -> f64 {
    let lower = unit.to_lowercase();
    if lower.starts_with('d') { return value * 24.0 * 60.0 * 60.0; }
    if lower.starts_with('h') { return value * 60.0 * 60.0; }
    if lower.starts_with('m') { return value * 60.0; }
    value
}

fn value_for_key<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    for key in keys {
        if let Some(found) = value.get(*key) {
            return Some(found);
        }
    }
    None
}

fn int_from_value(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64().or_else(|| number.as_f64().map(|value| value as i64)),
        Value::String(raw) => raw.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn string_from_value(value: &Value) -> Option<String> {
    match value {
        Value::String(raw) => Some(raw.to_string()),
        _ => None,
    }
}

fn cookie_override(raw: &str) -> Option<CookieOverride> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    let header = extract_cookie_header(raw).unwrap_or_else(|| raw.to_string());
    let header = strip_cookie_prefix(&header);
    let header = strip_wrapping_quotes(&header);
    if header.trim().is_empty() {
        return None;
    }

    let authorization_token = extract_first(r"(?i)\bauthorization:\s*bearer\s+([A-Za-z0-9._\-+=/]+)", raw);
    let group_id = extract_first(r"(?i)\bgroup[_]?id=([0-9]{4,})", raw);

    Some(CookieOverride {
        cookie_header: header,
        authorization_token,
        group_id,
    })
}

fn extract_cookie_header(raw: &str) -> Option<String> {
    let patterns = [
        "(?i)-H\\s*'Cookie:\\s*([^']+)'",
        "(?i)-H\\s*\\\"Cookie:\\s*([^\\\"]+)\\\"",
        "(?i)\\bcookie:\\s*'([^']+)'",
        "(?i)\\bcookie:\\s*\\\"([^\\\"]+)\\\"",
        "(?i)\\bcookie:\\s*([^\\r\\n]+)",
        "(?i)(?:--cookie|-b)\\s*'([^']+)'",
        "(?i)(?:--cookie|-b)\\s*\\\"([^\\\"]+)\\\"",
        "(?i)(?:--cookie|-b)\\s*([^\\s]+)",
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

fn strip_cookie_prefix(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.to_lowercase().starts_with("cookie:") {
        let value = &trimmed["cookie:".len()..];
        value.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

fn strip_wrapping_quotes(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() >= 2 {
        let first = trimmed.chars().next().unwrap_or(' ');
        let last = trimmed.chars().last().unwrap_or(' ');
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            return trimmed[1..trimmed.len() - 1].to_string();
        }
    }
    trimmed.to_string()
}

fn provider_error(message: &str) -> ProviderErrorPayload {
    ProviderErrorPayload {
        code: 1,
        message: message.to_string(),
        kind: Some("provider".to_string()),
    }
}

fn resolve_api_token(config: &ProviderConfig) -> Result<String, ProviderErrorPayload> {
    // Priority 1: Environment variable
    if let Ok(env_token) = std::env::var("MINIMAX_API_TOKEN") {
        let token = env_token.trim();
        if !token.is_empty() {
            return Ok(token.to_string());
        }
    }

    // Priority 2: Secure storage
    let storage = SecureTokenStorage::default();
    if storage.is_available() {
        if let Ok(stored_token) = storage.retrieve(accounts::MINIMAX) {
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

    Err(provider_error("MiniMax API token missing. Set in config, MINIMAX_API_TOKEN env var, or store in secure storage."))
}
