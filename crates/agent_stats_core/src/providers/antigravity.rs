use crate::models::{
    AntigravityPlanInfoSummary, ProviderErrorPayload, ProviderIdentitySnapshot, ProviderPayload,
    RateWindow, UsageSnapshot,
};
use crate::provider_metadata::ProviderId;

use super::{Provider, ProviderFetchContext};
use regex::Regex;
use serde::Deserialize;
use time::OffsetDateTime;

pub struct AntigravityProvider;

impl Provider for AntigravityProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Antigravity
    }

    fn display_name(&self) -> &'static str {
        "Antigravity"
    }

    fn fetch(
        &self,
        _context: &ProviderFetchContext,
    ) -> Result<ProviderPayload, ProviderErrorPayload> {
        let process_info = detect_process_info()?;
        let ports = listening_ports(process_info.pid)?;
        let connect_port = find_working_port(&ports, &process_info.csrf_token)?;

        let request_context = RequestContext {
            https_port: connect_port,
            http_port: process_info.extension_port,
            csrf_token: process_info.csrf_token,
        };

        let snapshot = fetch_quota(&request_context)?;
        let plan_info = fetch_plan_info(&request_context).ok().flatten();

        let account_email = snapshot
            .identity
            .as_ref()
            .and_then(|identity| identity.account_email.clone());

        Ok(ProviderPayload {
            provider: ProviderId::Antigravity.to_string(),
            account: account_email,
            version: Some("running".to_string()),
            source: "local".to_string(),
            status: None,
            usage: Some(snapshot),
            credits: None,
            antigravity_plan_info: plan_info,
            openai_dashboard: None,
            error: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Process detection
// ---------------------------------------------------------------------------

const PROCESS_NAME: &str = "language_server_macos";

struct ProcessInfo {
    pid: u32,
    extension_port: Option<u16>,
    csrf_token: String,
}

fn detect_process_info() -> Result<ProcessInfo, ProviderErrorPayload> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-ax", "-o", "pid=,command="])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|error| {
            provider_error(&format!("Failed to run ps for Antigravity detection: {error}"))
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut saw_antigravity = false;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Some((pid, command)) = parse_process_line(trimmed) else {
            continue;
        };

        let lower = command.to_lowercase();
        if !lower.contains(PROCESS_NAME) {
            continue;
        }
        if !is_antigravity_command_line(&lower) {
            continue;
        }

        saw_antigravity = true;

        let Some(token) = extract_flag("--csrf_token", command) else {
            continue;
        };
        let port = extract_flag("--extension_server_port", command)
            .and_then(|value| value.parse::<u16>().ok());

        return Ok(ProcessInfo {
            pid,
            extension_port: port,
            csrf_token: token.to_string(),
        });
    }

    if saw_antigravity {
        Err(provider_error(
            "Antigravity CSRF token not found. Restart Antigravity and retry.",
        ))
    } else {
        Err(provider_error(
            "Antigravity language server not detected. Launch Antigravity and retry.",
        ))
    }
}

fn parse_process_line(line: &str) -> Option<(u32, &str)> {
    let mut parts = line.splitn(2, |character: char| character.is_whitespace());
    let pid_str = parts.next()?.trim();
    let command = parts.next()?.trim();
    let pid = pid_str.parse::<u32>().ok()?;
    if command.is_empty() {
        return None;
    }
    Some((pid, command))
}

fn is_antigravity_command_line(lower_command: &str) -> bool {
    if lower_command.contains("--app_data_dir") && lower_command.contains("antigravity") {
        return true;
    }
    if lower_command.contains("/antigravity/") || lower_command.contains("\\antigravity\\") {
        return true;
    }
    false
}

fn extract_flag<'a>(flag: &str, command: &'a str) -> Option<&'a str> {
    let escaped = regex::escape(flag);
    let pattern = format!(r"{}[=\s]+(\S+)", escaped);
    let regex = Regex::new(&pattern).ok()?;
    let captures = regex.captures(command)?;
    captures.get(1).map(|matched| matched.as_str())
}

// ---------------------------------------------------------------------------
// Port detection
// ---------------------------------------------------------------------------

fn listening_ports(pid: u32) -> Result<Vec<u16>, ProviderErrorPayload> {
    let lsof_path = ["/usr/sbin/lsof", "/usr/bin/lsof"]
        .iter()
        .find(|path| std::path::Path::new(path).exists())
        .ok_or_else(|| {
            provider_error(
                "Antigravity port detection needs lsof. Install it, then retry.",
            )
        })?;

    let output = std::process::Command::new(lsof_path)
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-a", "-p", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|error| {
            provider_error(&format!("Failed to run lsof for Antigravity: {error}"))
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let ports = parse_listening_ports(&stdout);

    if ports.is_empty() {
        return Err(provider_error(
            "Antigravity is running but not exposing ports yet. Try again in a few seconds.",
        ));
    }

    Ok(ports)
}

fn parse_listening_ports(output: &str) -> Vec<u16> {
    let regex = match Regex::new(r":(\d+)\s+\(LISTEN\)") {
        Ok(regex) => regex,
        Err(_) => return Vec::new(),
    };
    let mut ports: std::collections::BTreeSet<u16> = std::collections::BTreeSet::new();
    for captures in regex.captures_iter(output) {
        if let Some(matched) = captures.get(1) {
            if let Ok(port) = matched.as_str().parse::<u16>() {
                ports.insert(port);
            }
        }
    }
    ports.into_iter().collect()
}

fn find_working_port(
    ports: &[u16],
    csrf_token: &str,
) -> Result<u16, ProviderErrorPayload> {
    let unleash_path =
        "/exa.language_server_pb.LanguageServerService/GetUnleashData";
    let body = serde_json::json!({
        "context": {
            "properties": {
                "devMode": "false",
                "extensionVersion": "unknown",
                "hasAnthropicModelAccess": "true",
                "ide": "antigravity",
                "ideVersion": "unknown",
                "installationId": "codexbar",
                "language": "UNSPECIFIED",
                "os": "macos",
                "requestedModelId": "MODEL_UNSPECIFIED"
            }
        }
    });
    let body_string = body.to_string();

    for &port in ports {
        let url = format!("https://127.0.0.1:{port}{unleash_path}");
        let result = ureq::post(&url)
            .set("Content-Type", "application/json")
            .set("Connect-Protocol-Version", "1")
            .set("X-Codeium-Csrf-Token", csrf_token)
            .send_string(&body_string);

        match result {
            Ok(_) => return Ok(port),
            Err(_) => {
                log::debug!("Antigravity port probe failed on port {port}");
                continue;
            }
        }
    }

    Err(provider_error(
        "Antigravity port detection failed: no working API port found",
    ))
}

// ---------------------------------------------------------------------------
// HTTP requests
// ---------------------------------------------------------------------------

struct RequestContext {
    https_port: u16,
    http_port: Option<u16>,
    csrf_token: String,
}

const GET_USER_STATUS_PATH: &str =
    "/exa.language_server_pb.LanguageServerService/GetUserStatus";
const GET_COMMAND_MODEL_CONFIGS_PATH: &str =
    "/exa.language_server_pb.LanguageServerService/GetCommandModelConfigs";

fn default_request_body() -> String {
    serde_json::json!({
        "metadata": {
            "ideName": "antigravity",
            "extensionName": "antigravity",
            "ideVersion": "unknown",
            "locale": "en"
        }
    })
    .to_string()
}

fn make_request(
    path: &str,
    body: &str,
    context: &RequestContext,
) -> Result<String, ProviderErrorPayload> {
    // Try HTTPS first
    let https_url = format!("https://127.0.0.1:{}{}", context.https_port, path);
    match send_request(&https_url, body, &context.csrf_token) {
        Ok(response) => return Ok(response),
        Err(https_error) => {
            // Try HTTP fallback on extension port if available and different
            if let Some(http_port) = context.http_port {
                if http_port != context.https_port {
                    let http_url = format!("http://127.0.0.1:{http_port}{path}");
                    return send_request(&http_url, body, &context.csrf_token);
                }
            }
            Err(https_error)
        }
    }
}

fn send_request(
    url: &str,
    body: &str,
    csrf_token: &str,
) -> Result<String, ProviderErrorPayload> {
    let response = ureq::post(url)
        .set("Content-Type", "application/json")
        .set("Connect-Protocol-Version", "1")
        .set("X-Codeium-Csrf-Token", csrf_token)
        .send_string(body);

    match response {
        Ok(response) => response.into_string().map_err(|error| {
            provider_error(&format!(
                "Antigravity API error: failed to read response body: {error}"
            ))
        }),
        Err(ureq::Error::Status(401, _)) | Err(ureq::Error::Status(403, _)) => Err(
            provider_error("Antigravity session expired. Restart Antigravity and retry."),
        ),
        Err(ureq::Error::Status(code, response)) => {
            let body_text = response.into_string().unwrap_or_default();
            let preview = truncate_body(&body_text, 300);
            Err(provider_error(&format!(
                "Antigravity API error: HTTP {code}: {preview}"
            )))
        }
        Err(error) => Err(provider_error(&format!(
            "Antigravity API error: {error}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Quota fetch + parsing
// ---------------------------------------------------------------------------

fn fetch_quota(context: &RequestContext) -> Result<UsageSnapshot, ProviderErrorPayload> {
    let body = default_request_body();

    // Primary: GetUserStatus
    match make_request(GET_USER_STATUS_PATH, &body, context) {
        Ok(response_text) => match parse_user_status_response(&response_text) {
            Ok(snapshot) => return Ok(snapshot.to_usage_snapshot()?),
            Err(error) => {
                log::debug!("Antigravity GetUserStatus parse failed: {}", error.message);
            }
        },
        Err(error) => {
            log::debug!("Antigravity GetUserStatus request failed: {}", error.message);
        }
    }

    // Fallback: GetCommandModelConfigs
    let response_text = make_request(GET_COMMAND_MODEL_CONFIGS_PATH, &body, context)?;
    let snapshot = parse_command_model_response(&response_text)?;
    snapshot.to_usage_snapshot()
}

fn fetch_plan_info(
    context: &RequestContext,
) -> Result<Option<AntigravityPlanInfoSummary>, ProviderErrorPayload> {
    let body = default_request_body();
    let response_text = make_request(GET_USER_STATUS_PATH, &body, context)?;
    let response: UserStatusResponse = serde_json::from_str(&response_text).map_err(|error| {
        provider_error(&format!(
            "Could not parse Antigravity user status: {error}"
        ))
    })?;

    if let Some(ref code) = response.code {
        if let Some(error_message) = invalid_code(code) {
            return Err(provider_error(&format!(
                "Antigravity API error: {error_message}"
            )));
        }
    }

    let user_status = match response.user_status {
        Some(status) => status,
        None => return Ok(None),
    };

    let plan_info = match user_status
        .plan_status
        .and_then(|status| status.plan_info)
    {
        Some(info) => info,
        None => return Ok(None),
    };

    Ok(Some(AntigravityPlanInfoSummary {
        plan_name: plan_info.plan_name,
        plan_display_name: plan_info.plan_display_name,
        display_name: plan_info.display_name,
        product_name: plan_info.product_name,
        plan_short_name: plan_info.plan_short_name,
    }))
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserStatusResponse {
    code: Option<CodeValue>,
    #[allow(dead_code)]
    message: Option<String>,
    user_status: Option<UserStatus>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommandModelConfigResponse {
    code: Option<CodeValue>,
    #[allow(dead_code)]
    message: Option<String>,
    client_model_configs: Option<Vec<ModelConfig>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserStatus {
    email: Option<String>,
    plan_status: Option<PlanStatus>,
    cascade_model_config_data: Option<ModelConfigData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanStatus {
    plan_info: Option<PlanInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanInfo {
    plan_name: Option<String>,
    plan_display_name: Option<String>,
    display_name: Option<String>,
    product_name: Option<String>,
    plan_short_name: Option<String>,
}

impl PlanInfo {
    fn preferred_name(&self) -> Option<&str> {
        let candidates = [
            self.plan_display_name.as_deref(),
            self.display_name.as_deref(),
            self.product_name.as_deref(),
            self.plan_name.as_deref(),
            self.plan_short_name.as_deref(),
        ];
        for candidate in candidates {
            if let Some(value) = candidate {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed);
                }
            }
        }
        None
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelConfigData {
    client_model_configs: Option<Vec<ModelConfig>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelConfig {
    label: String,
    #[allow(dead_code)]
    model_or_alias: ModelAlias,
    quota_info: Option<QuotaInfo>,
}

#[derive(Debug, Deserialize)]
struct ModelAlias {
    #[allow(dead_code)]
    model: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaInfo {
    remaining_fraction: Option<f64>,
    reset_time: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum CodeValue {
    Int(i64),
    Str(String),
}

impl CodeValue {
    fn is_ok(&self) -> bool {
        match self {
            CodeValue::Int(value) => *value == 0,
            CodeValue::Str(value) => {
                let lower = value.to_lowercase();
                lower == "ok" || lower == "success" || value == "0"
            }
        }
    }

    fn raw_value(&self) -> String {
        match self {
            CodeValue::Int(value) => value.to_string(),
            CodeValue::Str(value) => value.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Snapshot types + mapping
// ---------------------------------------------------------------------------

struct ModelQuota {
    label: String,
    remaining_fraction: Option<f64>,
    reset_time: Option<OffsetDateTime>,
}

impl ModelQuota {
    fn remaining_percent(&self) -> f64 {
        match self.remaining_fraction {
            Some(fraction) => (fraction * 100.0).clamp(0.0, 100.0),
            None => 0.0,
        }
    }
}

struct StatusSnapshot {
    model_quotas: Vec<ModelQuota>,
    account_email: Option<String>,
    account_plan: Option<String>,
}

impl StatusSnapshot {
    fn to_usage_snapshot(&self) -> Result<UsageSnapshot, ProviderErrorPayload> {
        let ordered = select_models(&self.model_quotas);
        let primary_quota = ordered.first().ok_or_else(|| {
            provider_error("Could not parse Antigravity quota: No quota models available")
        })?;

        let primary = rate_window_from_quota(primary_quota);
        let secondary = ordered.get(1).map(|quota| rate_window_from_quota(quota));
        let tertiary = ordered.get(2).map(|quota| rate_window_from_quota(quota));

        let identity = ProviderIdentitySnapshot {
            provider_id: Some(ProviderId::Antigravity.to_string()),
            account_email: self.account_email.clone(),
            account_organization: None,
            login_method: self.account_plan.clone(),
        };

        Ok(UsageSnapshot {
            primary: Some(primary),
            secondary,
            tertiary,
            provider_cost: None,
            updated_at: OffsetDateTime::now_utc(),
            identity: Some(identity),
        })
    }
}

fn rate_window_from_quota(quota: &ModelQuota) -> RateWindow {
    let remaining = quota.remaining_percent();
    let used_percent = (100.0 - remaining).clamp(0.0, 100.0);

    RateWindow {
        used_percent,
        window_minutes: None,
        resets_at: quota.reset_time,
        reset_description: quota.reset_time.as_ref().and_then(|date| {
            let format = time::format_description::parse(
                "[month repr:short] [day] at [hour repr:12]:[minute][period case:lower]",
            )
            .ok()?;
            date.format(&format).ok()
        }),
    }
}

// ---------------------------------------------------------------------------
// Model selection
// ---------------------------------------------------------------------------

fn select_models(models: &[ModelQuota]) -> Vec<&ModelQuota> {
    let mut ordered: Vec<&ModelQuota> = Vec::new();

    // 1) Claude (label contains "claude" but not "thinking")
    if let Some(claude) = models.iter().find(|model| is_claude_without_thinking(&model.label)) {
        ordered.push(claude);
    }

    // 2) Gemini Pro Low (label contains "pro" and "low")
    if let Some(pro) = models.iter().find(|model| {
        is_gemini_pro_low(&model.label)
            && !ordered.iter().any(|existing| existing.label == model.label)
    }) {
        ordered.push(pro);
    }

    // 3) Gemini Flash (label contains "gemini" and "flash")
    if let Some(flash) = models.iter().find(|model| {
        is_gemini_flash(&model.label)
            && !ordered.iter().any(|existing| existing.label == model.label)
    }) {
        ordered.push(flash);
    }

    // Fallback: sort by lowest remaining percent
    if ordered.is_empty() {
        let mut sorted: Vec<&ModelQuota> = models.iter().collect();
        sorted.sort_by(|a, b| {
            a.remaining_percent()
                .partial_cmp(&b.remaining_percent())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        return sorted;
    }

    ordered
}

fn is_claude_without_thinking(label: &str) -> bool {
    let lower = label.to_lowercase();
    lower.contains("claude") && !lower.contains("thinking")
}

fn is_gemini_pro_low(label: &str) -> bool {
    let lower = label.to_lowercase();
    lower.contains("pro") && lower.contains("low")
}

fn is_gemini_flash(label: &str) -> bool {
    let lower = label.to_lowercase();
    lower.contains("gemini") && lower.contains("flash")
}

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

fn parse_user_status_response(text: &str) -> Result<StatusSnapshot, ProviderErrorPayload> {
    let response: UserStatusResponse = serde_json::from_str(text).map_err(|error| {
        provider_error(&format!(
            "Could not parse Antigravity user status: {error}"
        ))
    })?;

    if let Some(ref code) = response.code {
        if let Some(error_message) = invalid_code(code) {
            return Err(provider_error(&format!(
                "Antigravity API error: {error_message}"
            )));
        }
    }

    let user_status = response.user_status.ok_or_else(|| {
        provider_error("Could not parse Antigravity quota: Missing userStatus")
    })?;

    let model_configs = user_status
        .cascade_model_config_data
        .and_then(|data| data.client_model_configs)
        .unwrap_or_default();

    let models: Vec<ModelQuota> = model_configs
        .into_iter()
        .filter_map(quota_from_config)
        .collect();

    let plan_name = user_status
        .plan_status
        .and_then(|status| status.plan_info)
        .and_then(|info| info.preferred_name().map(|name| name.to_string()));

    Ok(StatusSnapshot {
        model_quotas: models,
        account_email: user_status.email,
        account_plan: plan_name,
    })
}

fn parse_command_model_response(text: &str) -> Result<StatusSnapshot, ProviderErrorPayload> {
    let response: CommandModelConfigResponse =
        serde_json::from_str(text).map_err(|error| {
            provider_error(&format!(
                "Could not parse Antigravity model configs: {error}"
            ))
        })?;

    if let Some(ref code) = response.code {
        if let Some(error_message) = invalid_code(code) {
            return Err(provider_error(&format!(
                "Antigravity API error: {error_message}"
            )));
        }
    }

    let model_configs = response.client_model_configs.unwrap_or_default();
    let models: Vec<ModelQuota> = model_configs
        .into_iter()
        .filter_map(quota_from_config)
        .collect();

    Ok(StatusSnapshot {
        model_quotas: models,
        account_email: None,
        account_plan: None,
    })
}

fn quota_from_config(config: ModelConfig) -> Option<ModelQuota> {
    let quota = config.quota_info?;
    let reset_time = quota.reset_time.as_deref().and_then(parse_reset_time);
    Some(ModelQuota {
        label: config.label,
        remaining_fraction: quota.remaining_fraction,
        reset_time,
    })
}

fn invalid_code(code: &CodeValue) -> Option<String> {
    if code.is_ok() {
        None
    } else {
        Some(code.raw_value())
    }
}

fn parse_reset_time(value: &str) -> Option<OffsetDateTime> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    // ISO-8601 preferred
    if let Ok(date) =
        OffsetDateTime::parse(trimmed, &time::format_description::well_known::Rfc3339)
    {
        return Some(date);
    }
    // Numeric epoch seconds as fallback
    if let Ok(seconds) = trimmed.parse::<f64>() {
        return OffsetDateTime::from_unix_timestamp(seconds as i64).ok();
    }
    None
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn provider_error(message: &str) -> ProviderErrorPayload {
    ProviderErrorPayload {
        code: 1,
        message: message.to_string(),
        kind: Some("provider".to_string()),
    }
}

fn truncate_body(body: &str, max_length: usize) -> String {
    let cleaned = body
        .replace('\n', " ")
        .replace('\r', "")
        .trim()
        .to_string();
    if cleaned.len() > max_length {
        format!("{}…", &cleaned[..max_length])
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_claude_without_thinking() {
        assert!(is_claude_without_thinking("Claude 3.5 Sonnet"));
        assert!(!is_claude_without_thinking("Claude 3.5 Sonnet Thinking"));
        assert!(!is_claude_without_thinking("Gemini Flash"));
    }

    #[test]
    fn test_is_gemini_pro_low() {
        assert!(is_gemini_pro_low("Gemini Pro Low"));
        assert!(!is_gemini_pro_low("Gemini Pro"));
        assert!(!is_gemini_pro_low("Gemini Flash"));
    }

    #[test]
    fn test_is_gemini_flash() {
        assert!(is_gemini_flash("Gemini 2.0 Flash"));
        assert!(!is_gemini_flash("Gemini Pro Low"));
        assert!(!is_gemini_flash("Claude 3.5 Sonnet"));
    }

    #[test]
    fn test_parse_reset_time_iso() {
        let result = parse_reset_time("2026-01-28T15:00:00Z");
        assert!(result.is_some());
    }

    #[test]
    fn test_parse_reset_time_epoch() {
        let result = parse_reset_time("1738076400");
        assert!(result.is_some());
    }

    #[test]
    fn test_parse_reset_time_empty() {
        assert!(parse_reset_time("").is_none());
        assert!(parse_reset_time("  ").is_none());
    }

    #[test]
    fn test_parse_reset_time_garbage() {
        assert!(parse_reset_time("not-a-date").is_none());
    }

    #[test]
    fn test_code_value_ok() {
        let int_zero = CodeValue::Int(0);
        assert!(int_zero.is_ok());

        let int_nonzero = CodeValue::Int(5);
        assert!(!int_nonzero.is_ok());

        let str_ok = CodeValue::Str("OK".to_string());
        assert!(str_ok.is_ok());

        let str_success = CodeValue::Str("success".to_string());
        assert!(str_success.is_ok());

        let str_zero = CodeValue::Str("0".to_string());
        assert!(str_zero.is_ok());

        let str_error = CodeValue::Str("INTERNAL".to_string());
        assert!(!str_error.is_ok());
    }

    #[test]
    fn test_select_models_preferred_order() {
        let models = vec![
            ModelQuota {
                label: "Gemini 2.0 Flash".to_string(),
                remaining_fraction: Some(0.9),
                reset_time: None,
            },
            ModelQuota {
                label: "Gemini Pro Low".to_string(),
                remaining_fraction: Some(0.5),
                reset_time: None,
            },
            ModelQuota {
                label: "Claude 3.5 Sonnet".to_string(),
                remaining_fraction: Some(0.7),
                reset_time: None,
            },
        ];

        let selected = select_models(&models);
        assert_eq!(selected.len(), 3);
        assert_eq!(selected[0].label, "Claude 3.5 Sonnet");
        assert_eq!(selected[1].label, "Gemini Pro Low");
        assert_eq!(selected[2].label, "Gemini 2.0 Flash");
    }

    #[test]
    fn test_select_models_fallback_sorted() {
        let models = vec![
            ModelQuota {
                label: "Model A".to_string(),
                remaining_fraction: Some(0.8),
                reset_time: None,
            },
            ModelQuota {
                label: "Model B".to_string(),
                remaining_fraction: Some(0.2),
                reset_time: None,
            },
        ];

        let selected = select_models(&models);
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].label, "Model B");
        assert_eq!(selected[1].label, "Model A");
    }

    #[test]
    fn test_parse_user_status_response_valid() {
        let json = r#"{
            "code": 0,
            "userStatus": {
                "email": "user@example.com",
                "planStatus": {
                    "planInfo": {
                        "planName": "Pro",
                        "planDisplayName": "Antigravity Pro",
                        "displayName": null,
                        "productName": null,
                        "planShortName": "pro"
                    }
                },
                "cascadeModelConfigData": {
                    "clientModelConfigs": [
                        {
                            "label": "Claude 3.5 Sonnet",
                            "modelOrAlias": { "model": "claude-3-5-sonnet" },
                            "quotaInfo": {
                                "remainingFraction": 0.75,
                                "resetTime": "2026-01-28T15:00:00Z"
                            }
                        },
                        {
                            "label": "Gemini Pro Low",
                            "modelOrAlias": { "model": "gemini-pro-low" },
                            "quotaInfo": {
                                "remainingFraction": 0.5,
                                "resetTime": null
                            }
                        }
                    ]
                }
            }
        }"#;

        let snapshot = parse_user_status_response(json);
        assert!(snapshot.is_ok());
        let snapshot = snapshot.expect("parse should succeed");
        assert_eq!(snapshot.account_email.as_deref(), Some("user@example.com"));
        assert_eq!(snapshot.account_plan.as_deref(), Some("Antigravity Pro"));
        assert_eq!(snapshot.model_quotas.len(), 2);

        let usage = snapshot.to_usage_snapshot();
        assert!(usage.is_ok());
        let usage = usage.expect("usage mapping should succeed");
        let primary = usage.primary.as_ref().expect("primary should exist");
        assert!((primary.used_percent - 25.0).abs() < 0.01);
    }

    #[test]
    fn test_parse_user_status_response_error_code() {
        let json = r#"{"code": "INTERNAL", "message": "something broke"}"#;
        let result = parse_user_status_response(json);
        assert!(result.is_err());
        let error = result.err().expect("should be error");
        assert!(error.message.contains("INTERNAL"));
    }

    #[test]
    fn test_parse_command_model_response_valid() {
        let json = r#"{
            "code": 0,
            "clientModelConfigs": [
                {
                    "label": "Claude 3.5 Sonnet",
                    "modelOrAlias": { "model": "claude-3-5-sonnet" },
                    "quotaInfo": {
                        "remainingFraction": 0.6
                    }
                }
            ]
        }"#;

        let snapshot = parse_command_model_response(json);
        assert!(snapshot.is_ok());
        let snapshot = snapshot.expect("parse should succeed");
        assert_eq!(snapshot.model_quotas.len(), 1);
        assert!(snapshot.account_email.is_none());
    }

    #[test]
    fn test_rate_window_from_quota() {
        let quota = ModelQuota {
            label: "Test".to_string(),
            remaining_fraction: Some(0.75),
            reset_time: None,
        };
        let window = rate_window_from_quota(&quota);
        assert!((window.used_percent - 25.0).abs() < 0.01);
        assert!(window.reset_description.is_none());
    }

    #[test]
    fn test_rate_window_from_quota_with_reset() {
        let reset = OffsetDateTime::parse(
            "2026-01-28T15:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .ok();
        let quota = ModelQuota {
            label: "Test".to_string(),
            remaining_fraction: Some(0.5),
            reset_time: reset,
        };
        let window = rate_window_from_quota(&quota);
        assert!((window.used_percent - 50.0).abs() < 0.01);
        assert!(window.reset_description.is_some());
    }

    #[test]
    fn test_parse_listening_ports() {
        let output = "COMMAND   PID USER   FD   TYPE  DEVICE SIZE/OFF NODE NAME\n\
                       language_ 123 user   8u   IPv4  0x1234  0t0  TCP 127.0.0.1:42100 (LISTEN)\n\
                       language_ 123 user   9u   IPv4  0x5678  0t0  TCP 127.0.0.1:42101 (LISTEN)\n";
        let ports = parse_listening_ports(output);
        assert_eq!(ports, vec![42100, 42101]);
    }

    #[test]
    fn test_parse_listening_ports_empty() {
        let output = "COMMAND   PID USER   FD   TYPE  DEVICE SIZE/OFF NODE NAME\n";
        let ports = parse_listening_ports(output);
        assert!(ports.is_empty());
    }

    #[test]
    fn test_parse_process_line() {
        let result = parse_process_line("12345 /path/to/language_server_macos --csrf_token abc123");
        assert!(result.is_some());
        let (pid, command) = result.expect("should parse");
        assert_eq!(pid, 12345);
        assert!(command.contains("language_server_macos"));
    }

    #[test]
    fn test_parse_process_line_no_pid() {
        assert!(parse_process_line("notanumber some command").is_none());
    }

    #[test]
    fn test_is_antigravity_command_line() {
        assert!(is_antigravity_command_line(
            "/app/antigravity/language_server_macos --csrf_token abc"
        ));
        assert!(is_antigravity_command_line(
            "language_server_macos --app_data_dir /data/antigravity"
        ));
        assert!(!is_antigravity_command_line(
            "language_server_macos --app_data_dir /data/other"
        ));
    }

    #[test]
    fn test_extract_flag() {
        let command = "/bin/language_server_macos --csrf_token abc123 --extension_server_port 9000";
        assert_eq!(extract_flag("--csrf_token", command), Some("abc123"));
        assert_eq!(
            extract_flag("--extension_server_port", command),
            Some("9000")
        );
        assert_eq!(extract_flag("--nonexistent", command), None);
    }

    #[test]
    fn test_extract_flag_equals_syntax() {
        let command = "language_server_macos --csrf_token=abc123";
        assert_eq!(extract_flag("--csrf_token", command), Some("abc123"));
    }

    #[test]
    fn test_invalid_code() {
        assert!(invalid_code(&CodeValue::Int(0)).is_none());
        assert!(invalid_code(&CodeValue::Str("OK".to_string())).is_none());
        assert!(invalid_code(&CodeValue::Int(5)).is_some());
        assert!(invalid_code(&CodeValue::Str("INTERNAL".to_string())).is_some());
    }

    #[test]
    fn test_snapshot_to_usage_no_models() {
        let snapshot = StatusSnapshot {
            model_quotas: vec![],
            account_email: None,
            account_plan: None,
        };
        let result = snapshot.to_usage_snapshot();
        assert!(result.is_err());
    }

    #[test]
    fn test_truncate_body() {
        assert_eq!(truncate_body("short", 100), "short");
        assert_eq!(truncate_body("hello world", 5), "hello…");
        assert_eq!(truncate_body("line1\nline2", 20), "line1 line2");
    }

    #[test]
    fn test_plan_info_preferred_name() {
        let info = PlanInfo {
            plan_name: Some("basic".to_string()),
            plan_display_name: Some("Pro Plan".to_string()),
            display_name: None,
            product_name: None,
            plan_short_name: None,
        };
        assert_eq!(info.preferred_name(), Some("Pro Plan"));

        let info_empty_display = PlanInfo {
            plan_name: Some("basic".to_string()),
            plan_display_name: Some("  ".to_string()),
            display_name: None,
            product_name: Some("Product X".to_string()),
            plan_short_name: None,
        };
        assert_eq!(info_empty_display.preferred_name(), Some("Product X"));

        let all_none = PlanInfo {
            plan_name: None,
            plan_display_name: None,
            display_name: None,
            product_name: None,
            plan_short_name: None,
        };
        assert_eq!(all_none.preferred_name(), None);
    }
}
