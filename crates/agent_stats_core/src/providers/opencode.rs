use crate::config::ProviderSourceMode;
use crate::models::{ProviderErrorPayload, ProviderIdentitySnapshot, ProviderPayload, RateWindow, UsageSnapshot};
use crate::provider_metadata::ProviderId;
use super::{Provider, ProviderFetchContext};
use regex::Regex;
use serde_json::Value;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const BASE_URL: &str = "https://opencode.ai";
const SERVER_URL: &str = "https://opencode.ai/_server";
const WORKSPACES_SERVER_ID: &str =
    "def39973159c7f0483d8793a822b8dbb10d067e12c65455fcb4608459ba0234f";
const SUBSCRIPTION_SERVER_ID: &str =
    "7abeebee372f304e050aaaf92be863f4a86490e382f8c79db68fd94040d691b4";

const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";

pub struct OpenCodeProvider;

impl Provider for OpenCodeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::OpenCode
    }

    fn display_name(&self) -> &'static str {
        "OpenCode"
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
                    "OpenCode supports Auto/Web source modes only.",
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
                "OpenCode cookie header missing. Paste a Cookie header from opencode.ai.",
            ));
        }

        let workspace_id = if let Some(raw) = context.provider_config.workspace_id.as_ref() {
            normalize_workspace_id(raw)
        } else {
            None
        };

        let workspace_id = if let Some(workspace_id) = workspace_id {
            workspace_id
        } else {
            fetch_workspace_id(&cookie_header)?
        };

        let subscription_text = fetch_subscription_text(&cookie_header, &workspace_id)?;
        let usage = parse_subscription(&subscription_text, context.now)?;

        Ok(ProviderPayload {
            provider: ProviderId::OpenCode.to_string(),
            account: None,
            version: None,
            source: "web".to_string(),
            status: None,
            usage: Some(usage),
            credits: None,
            antigravity_plan_info: None,
            openai_dashboard: None,
            error: None,
        })
    }
}

fn fetch_workspace_id(cookie_header: &str) -> Result<String, ProviderErrorPayload> {
    let text = fetch_server_text(
        ServerRequest {
            server_id: WORKSPACES_SERVER_ID,
            args: None,
            method: "GET",
            referer: BASE_URL,
        },
        cookie_header,
    )?;

    if looks_signed_out(&text) {
        return Err(provider_error("OpenCode session cookie is invalid or expired."));
    }

    let mut ids = parse_workspace_ids(&text);
    if ids.is_empty() {
        ids = parse_workspace_ids_from_json(&text);
    }

    if ids.is_empty() {
        let fallback = fetch_server_text(
            ServerRequest {
                server_id: WORKSPACES_SERVER_ID,
                args: Some(vec![]),
                method: "POST",
                referer: BASE_URL,
            },
            cookie_header,
        )?;
        if looks_signed_out(&fallback) {
            return Err(provider_error("OpenCode session cookie is invalid or expired."));
        }
        ids = parse_workspace_ids(&fallback);
        if ids.is_empty() {
            ids = parse_workspace_ids_from_json(&fallback);
        }
    }

    ids.into_iter()
        .next()
        .ok_or_else(|| provider_error("Missing OpenCode workspace id."))
}

fn fetch_subscription_text(
    cookie_header: &str,
    workspace_id: &str,
) -> Result<String, ProviderErrorPayload> {
    let referer = format!("{BASE_URL}/workspace/{workspace_id}/billing");
    let text = fetch_server_text(
        ServerRequest {
            server_id: SUBSCRIPTION_SERVER_ID,
            args: Some(vec![Value::String(workspace_id.to_string())]),
            method: "POST",
            referer: &referer,
        },
        cookie_header,
    )?;

    if looks_signed_out(&text) {
        return Err(provider_error("OpenCode session cookie is invalid or expired."));
    }

    Ok(text)
}

fn parse_subscription(text: &str, now: OffsetDateTime) -> Result<UsageSnapshot, ProviderErrorPayload> {
    if let Some(snapshot) = parse_subscription_json(text, now) {
        return Ok(snapshot.to_usage_snapshot(now));
    }

    let rolling_percent = extract_double(
        r"rollingUsage[^}]*?usagePercent\s*:\s*([0-9]+(?:\.[0-9]+)?)",
        text,
    )
    .ok_or_else(|| provider_error("Missing OpenCode rolling usage percent."))?;
    let rolling_reset = extract_int(
        r"rollingUsage[^}]*?resetInSec\s*:\s*([0-9]+)",
        text,
    )
    .ok_or_else(|| provider_error("Missing OpenCode rolling reset."))?;

    let weekly_percent = extract_double(
        r"weeklyUsage[^}]*?usagePercent\s*:\s*([0-9]+(?:\.[0-9]+)?)",
        text,
    )
    .ok_or_else(|| provider_error("Missing OpenCode weekly usage percent."))?;
    let weekly_reset = extract_int(
        r"weeklyUsage[^}]*?resetInSec\s*:\s*([0-9]+)",
        text,
    )
    .ok_or_else(|| provider_error("Missing OpenCode weekly reset."))?;

    let snapshot = OpenCodeUsageSnapshot {
        rolling_usage_percent: rolling_percent,
        weekly_usage_percent: weekly_percent,
        rolling_reset_in_sec: Some(rolling_reset as i64),
        weekly_reset_in_sec: Some(weekly_reset as i64),
        rolling_reset_at: None,
        weekly_reset_at: None,
    };

    Ok(snapshot.to_usage_snapshot(now))
}

#[derive(Debug, Clone)]
struct OpenCodeUsageSnapshot {
    rolling_usage_percent: f64,
    weekly_usage_percent: f64,
    rolling_reset_in_sec: Option<i64>,
    weekly_reset_in_sec: Option<i64>,
    rolling_reset_at: Option<OffsetDateTime>,
    weekly_reset_at: Option<OffsetDateTime>,
}

impl OpenCodeUsageSnapshot {
    fn to_usage_snapshot(&self, now: OffsetDateTime) -> UsageSnapshot {
        let primary = RateWindow {
            used_percent: clamp_percent(self.rolling_usage_percent),
            window_minutes: None,
            resets_at: self
                .rolling_reset_at
                .or_else(|| self.rolling_reset_in_sec.map(|sec| now + Duration::seconds(sec))),
            reset_description: None,
        };

        let secondary = RateWindow {
            used_percent: clamp_percent(self.weekly_usage_percent),
            window_minutes: None,
            resets_at: self
                .weekly_reset_at
                .or_else(|| self.weekly_reset_in_sec.map(|sec| now + Duration::seconds(sec))),
            reset_description: None,
        };

        let identity = ProviderIdentitySnapshot {
            provider_id: Some(ProviderId::OpenCode.to_string()),
            account_email: None,
            account_organization: None,
            login_method: Some("OpenCode".to_string()),
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
}

struct ServerRequest<'a> {
    server_id: &'a str,
    args: Option<Vec<Value>>,
    method: &'a str,
    referer: &'a str,
}

fn fetch_server_text(
    request: ServerRequest<'_>,
    cookie_header: &str,
) -> Result<String, ProviderErrorPayload> {
    let url = server_request_url(request.server_id, request.args.as_ref(), request.method);

    let mut builder = if request.method.eq_ignore_ascii_case("GET") {
        ureq::get(&url)
    } else {
        ureq::post(SERVER_URL)
    };

    builder = builder
        .set("Cookie", cookie_header)
        .set("X-Server-Id", request.server_id)
        .set("X-Server-Instance", &format!("server-fn:{}", Uuid::new_v4()))
        .set("User-Agent", USER_AGENT)
        .set("Origin", BASE_URL)
        .set("Referer", request.referer)
        .set("Accept", "text/javascript, application/json;q=0.9, */*;q=0.8");

    if !request.method.eq_ignore_ascii_case("GET") {
        if let Some(args) = request.args {
            builder = builder.set("Content-Type", "application/json");
            let response = builder.send_json(Value::Array(args));
            return handle_server_response(response);
        }
    }

    handle_server_response(builder.call())
}

fn handle_server_response(
    response: Result<ureq::Response, ureq::Error>,
) -> Result<String, ProviderErrorPayload> {
    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            if code == 401 || code == 403 || looks_signed_out(&body) {
                return Err(provider_error("OpenCode session cookie is invalid or expired."));
            }
            let snippet = body.trim();
            let snippet = if snippet.is_empty() {
                String::new()
            } else {
                format!(": {}", snippet.chars().take(200).collect::<String>())
            };
            return Err(provider_error(&format!(
                "OpenCode API error (HTTP {code}){snippet}"
            )));
        }
        Err(error) => {
            return Err(provider_error(&format!(
                "OpenCode network error: {error}"
            )));
        }
    };

    response
        .into_string()
        .map_err(|error| provider_error(&format!("OpenCode response invalid: {error}")))
}

fn server_request_url(server_id: &str, _args: Option<&Vec<Value>>, method: &str) -> String {
    if !method.eq_ignore_ascii_case("GET") {
        return SERVER_URL.to_string();
    }

    format!("{SERVER_URL}?id={server_id}")
}

fn parse_workspace_ids(text: &str) -> Vec<String> {
    let Some(regex) = Regex::new("id\\s*:\\s*\\\"(wrk_[^\\\"]+)\\\"").ok() else {
        return Vec::new();
    };
    regex
        .captures_iter(text)
        .filter_map(|captures| captures.get(1).map(|value| value.as_str().to_string()))
        .collect()
}

fn parse_workspace_ids_from_json(text: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    collect_workspace_ids(&value, &mut ids);
    ids
}

fn collect_workspace_ids(value: &Value, ids: &mut Vec<String>) {
    match value {
        Value::String(value) => {
            if value.starts_with("wrk_") && !ids.contains(value) {
                ids.push(value.clone());
            }
        }
        Value::Array(values) => {
            for entry in values {
                collect_workspace_ids(entry, ids);
            }
        }
        Value::Object(map) => {
            for value in map.values() {
                collect_workspace_ids(value, ids);
            }
        }
        _ => {}
    }
}

fn parse_subscription_json(text: &str, now: OffsetDateTime) -> Option<OpenCodeUsageSnapshot> {
    let value: Value = serde_json::from_str(text).ok()?;
    parse_usage_json(&value, now)
}

fn parse_usage_json(value: &Value, now: OffsetDateTime) -> Option<OpenCodeUsageSnapshot> {
    if let Some(snapshot) = parse_usage_dictionary(value) {
        return Some(snapshot);
    }

    if let Value::Object(map) = value {
        for key in ["data", "result", "usage", "billing", "payload"] {
            if let Some(nested) = map.get(key) {
                if let Some(snapshot) = parse_usage_dictionary(nested) {
                    return Some(snapshot);
                }
            }
        }
        if let Some(snapshot) = parse_usage_nested(map, 0) {
            return Some(snapshot);
        }
    }

    parse_usage_from_candidates(value, now)
}

fn parse_usage_dictionary(value: &Value) -> Option<OpenCodeUsageSnapshot> {
    let Value::Object(map) = value else { return None };
    if let Some(usage) = map.get("usage") {
        if let Some(snapshot) = parse_usage_dictionary(usage) {
            return Some(snapshot);
        }
    }

    let rolling = find_window(map, &["rollingUsage", "rolling", "rolling_usage", "rollingWindow", "rolling_window"]);
    let weekly = find_window(map, &["weeklyUsage", "weekly", "weekly_usage", "weeklyWindow", "weekly_window"]);

    match (rolling, weekly) {
        (Some(rolling), Some(weekly)) => build_snapshot(&rolling, &weekly),
        _ => None,
    }
}

#[derive(Clone)]
struct WindowCandidate {
    id: usize,
    percent: f64,
    reset_in_sec: i64,
    path_lower: String,
}

fn parse_usage_from_candidates(value: &Value, now: OffsetDateTime) -> Option<OpenCodeUsageSnapshot> {
    let mut candidates = Vec::new();
    collect_window_candidates(value, now, &mut Vec::new(), &mut candidates);

    if candidates.is_empty() {
        return None;
    }

    let rolling_candidates: Vec<_> = candidates
        .iter()
        .filter(|candidate| {
            candidate.path_lower.contains("rolling")
                || candidate.path_lower.contains("hour")
                || candidate.path_lower.contains("5h")
                || candidate.path_lower.contains("5-hour")
        })
        .cloned()
        .collect();

    let weekly_candidates: Vec<_> = candidates
        .iter()
        .filter(|candidate| {
            candidate.path_lower.contains("weekly") || candidate.path_lower.contains("week")
        })
        .cloned()
        .collect();

    let rolling = pick_candidate(&rolling_candidates, &candidates, true, None)?;
    let weekly = pick_candidate(&weekly_candidates, &candidates, false, Some(rolling.id))?;

    Some(OpenCodeUsageSnapshot {
        rolling_usage_percent: rolling.percent,
        weekly_usage_percent: weekly.percent,
        rolling_reset_in_sec: Some(rolling.reset_in_sec),
        weekly_reset_in_sec: Some(weekly.reset_in_sec),
        rolling_reset_at: None,
        weekly_reset_at: None,
    })
}

fn collect_window_candidates(
    value: &Value,
    now: OffsetDateTime,
    path: &mut Vec<String>,
    out: &mut Vec<WindowCandidate>,
) {
    match value {
        Value::Object(map) => {
            if let Some(window) = parse_window(map, now) {
                let path_lower = path.join(".").to_lowercase();
                let id = out.len();
                out.push(WindowCandidate {
                    id,
                    percent: window.0,
                    reset_in_sec: window.1,
                    path_lower,
                });
            }
            for (key, value) in map {
                path.push(key.clone());
                collect_window_candidates(value, now, path, out);
                path.pop();
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                path.push(format!("[{index}]"));
                collect_window_candidates(value, now, path, out);
                path.pop();
            }
        }
        _ => {}
    }
}

fn pick_candidate(
    preferred: &[WindowCandidate],
    fallback: &[WindowCandidate],
    pick_shorter: bool,
    excluding: Option<usize>,
) -> Option<WindowCandidate> {
    let preferred_filtered: Vec<_> = preferred
        .iter()
        .filter(|candidate| Some(candidate.id) != excluding)
        .cloned()
        .collect();
    if let Some(candidate) = pick_candidate_from(&preferred_filtered, pick_shorter) {
        return Some(candidate);
    }

    let fallback_filtered: Vec<_> = fallback
        .iter()
        .filter(|candidate| Some(candidate.id) != excluding)
        .cloned()
        .collect();
    pick_candidate_from(&fallback_filtered, pick_shorter)
}

fn pick_candidate_from(candidates: &[WindowCandidate], pick_shorter: bool) -> Option<WindowCandidate> {
    let mut best: Option<WindowCandidate> = None;
    for candidate in candidates {
        let replace = match &best {
            None => true,
            Some(current) => {
                if pick_shorter {
                    if candidate.reset_in_sec == current.reset_in_sec {
                        candidate.percent > current.percent
                    } else {
                        candidate.reset_in_sec < current.reset_in_sec
                    }
                } else if candidate.reset_in_sec == current.reset_in_sec {
                    candidate.percent > current.percent
                } else {
                    candidate.reset_in_sec > current.reset_in_sec
                }
            }
        };
        if replace {
            best = Some(candidate.clone());
        }
    }
    best
}

fn parse_window(map: &serde_json::Map<String, Value>, now: OffsetDateTime) -> Option<(f64, i64)> {
    let mut percent = value_from_keys(map, &[
        "usagePercent",
        "usedPercent",
        "percentUsed",
        "percent",
        "usage_percent",
        "used_percent",
        "utilization",
        "utilizationPercent",
        "utilization_percent",
        "usage",
    ])
    .and_then(|value| number_from_value(value));

    if percent.is_none() {
        let used = value_from_keys(map, &["used", "usage", "consumed", "count", "usedTokens"])
            .and_then(|value| number_from_value(value));
        let limit = value_from_keys(map, &["limit", "total", "quota", "max", "cap", "tokenLimit"])
            .and_then(|value| number_from_value(value));
        if let (Some(used), Some(limit)) = (used, limit) {
            if limit > 0.0 {
                percent = Some((used / limit) * 100.0);
            }
        }
    }

    let mut resolved_percent = percent?;
    if resolved_percent <= 1.0 && resolved_percent >= 0.0 {
        resolved_percent *= 100.0;
    }
    resolved_percent = clamp_percent(resolved_percent);

    let reset_in_sec = value_from_keys(map, &[
        "resetInSec",
        "resetInSeconds",
        "resetSeconds",
        "reset_sec",
        "reset_in_sec",
        "resetsInSec",
        "resetsInSeconds",
        "resetIn",
        "resetSec",
    ])
    .and_then(|value| integer_from_value(value))
    .or_else(|| {
        let reset_at = value_from_keys(map, &[
            "resetAt",
            "resetsAt",
            "reset_at",
            "resets_at",
            "nextReset",
            "next_reset",
            "renewAt",
            "renew_at",
        ])?;
        parse_date_value(reset_at)
            .map(|date| (date - now).whole_seconds().max(0))
    })
    .unwrap_or(0);

    Some((resolved_percent, reset_in_sec))
}

fn value_from_keys<'a>(map: &'a serde_json::Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    for key in keys {
        if let Some(value) = map.get(*key) {
            return Some(value);
        }
    }
    None
}

fn number_from_value(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(raw) => raw.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn integer_from_value(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64().or_else(|| number.as_f64().map(|value| value as i64)),
        Value::String(raw) => raw.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn parse_date_value(value: &Value) -> Option<OffsetDateTime> {
    match value {
        Value::Number(number) => {
            let Some(raw) = number.as_f64() else { return None };
            if raw > 1_000_000_000_000.0 {
                return OffsetDateTime::from_unix_timestamp((raw / 1000.0) as i64).ok();
            }
            if raw > 1_000_000_000.0 {
                return OffsetDateTime::from_unix_timestamp(raw as i64).ok();
            }
            None
        }
        Value::String(raw) => {
            let trimmed = raw.trim();
            if let Ok(number) = trimmed.parse::<f64>() {
                let value = Value::Number(serde_json::Number::from_f64(number)?);
                return parse_date_value(&value);
            }
            OffsetDateTime::parse(trimmed, &time::format_description::well_known::Rfc3339).ok()
        }
        _ => None,
    }
}

fn parse_usage_nested(map: &serde_json::Map<String, Value>, depth: usize) -> Option<OpenCodeUsageSnapshot> {
    if depth > 3 {
        return None;
    }

    let mut rolling = None;
    let mut weekly = None;

    for (key, value) in map {
        if let Value::Object(_) = value {
            let lower = key.to_lowercase();
            if lower.contains("rolling") {
                rolling = Some(value.clone());
            } else if lower.contains("weekly") || lower.contains("week") {
                weekly = Some(value.clone());
            }
        }
    }

    if let (Some(rolling), Some(weekly)) = (rolling, weekly) {
        if let Some(snapshot) = build_snapshot(&rolling, &weekly) {
            return Some(snapshot);
        }
    }

    for value in map.values() {
        if let Value::Object(nested) = value {
            if let Some(snapshot) = parse_usage_nested(nested, depth + 1) {
                return Some(snapshot);
            }
        }
    }

    None
}

fn find_window(map: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<Value> {
    for key in keys {
        if let Some(value) = map.get(*key) {
            if value.is_object() {
                return Some(value.clone());
            }
        }
    }
    None
}

fn build_snapshot(rolling: &Value, weekly: &Value) -> Option<OpenCodeUsageSnapshot> {
    let rolling_percent = extract_percent(rolling)?;
    let weekly_percent = extract_percent(weekly)?;
    let rolling_reset_in_sec = extract_reset_seconds(rolling);
    let weekly_reset_in_sec = extract_reset_seconds(weekly);
    let rolling_reset_at = extract_reset_at(rolling);
    let weekly_reset_at = extract_reset_at(weekly);

    Some(OpenCodeUsageSnapshot {
        rolling_usage_percent: rolling_percent,
        weekly_usage_percent: weekly_percent,
        rolling_reset_in_sec,
        weekly_reset_in_sec,
        rolling_reset_at,
        weekly_reset_at,
    })
}

fn extract_percent(value: &Value) -> Option<f64> {
    let Value::Object(map) = value else { return None };
    let keys = [
        "usagePercent",
        "usedPercent",
        "percentUsed",
        "percent",
        "usage_percent",
        "used_percent",
        "utilization",
        "utilizationPercent",
        "utilization_percent",
        "usage",
    ];
    for key in keys {
        if let Some(value) = map.get(key) {
            if let Some(percent) = value.as_f64().or_else(|| value.as_i64().map(|value| value as f64)) {
                return Some(if percent <= 1.0 { percent * 100.0 } else { percent });
            }
        }
    }
    None
}

fn extract_reset_seconds(value: &Value) -> Option<i64> {
    let Value::Object(map) = value else { return None };
    let keys = [
        "resetInSec",
        "resetInSeconds",
        "resetSeconds",
        "reset_sec",
        "reset_in_sec",
        "resetsInSec",
        "resetsInSeconds",
        "resetIn",
        "resetSec",
    ];
    for key in keys {
        if let Some(value) = map.get(key) {
            if let Some(seconds) = value.as_i64().or_else(|| value.as_f64().map(|value| value as i64)) {
                return Some(seconds);
            }
        }
    }
    None
}

fn extract_reset_at(value: &Value) -> Option<OffsetDateTime> {
    let Value::Object(map) = value else { return None };
    let keys = [
        "resetAt",
        "resetsAt",
        "reset_at",
        "resets_at",
        "nextReset",
        "next_reset",
        "renewAt",
        "renew_at",
    ];
    for key in keys {
        if let Some(value) = map.get(key) {
            if let Some(raw) = value.as_str() {
                if let Ok(parsed) = OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339) {
                    return Some(parsed);
                }
            }
        }
    }
    None
}

fn extract_double(pattern: &str, text: &str) -> Option<f64> {
    let regex = Regex::new(pattern).ok()?;
    let captures = regex.captures(text)?;
    captures
        .get(1)
        .and_then(|value| value.as_str().parse::<f64>().ok())
}

fn extract_int(pattern: &str, text: &str) -> Option<i64> {
    let regex = Regex::new(pattern).ok()?;
    let captures = regex.captures(text)?;
    captures
        .get(1)
        .and_then(|value| value.as_str().parse::<i64>().ok())
}

fn looks_signed_out(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("sign in") || lower.contains("log in") || lower.contains("/login")
}

fn normalize_workspace_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.starts_with("wrk_") && trimmed.len() > 4 {
        return Some(trimmed.to_string());
    }
    if let Some(index) = trimmed.find("/workspace/") {
        let tail = &trimmed[index + "/workspace/".len()..];
        let candidate = tail.split('/').next().unwrap_or("");
        if candidate.starts_with("wrk_") && candidate.len() > 4 {
            return Some(candidate.to_string());
        }
    }
    if let Some(match_str) = Regex::new(r"wrk_[A-Za-z0-9]+")
        .ok()
        .and_then(|regex| regex.find(trimmed).map(|match_| match_.as_str().to_string()))
    {
        return Some(match_str);
    }
    None
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
