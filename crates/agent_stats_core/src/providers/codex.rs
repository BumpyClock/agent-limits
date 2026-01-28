use crate::config::ProviderSourceMode;
use crate::models::{
    CreditsSnapshot, ProviderErrorPayload, ProviderIdentitySnapshot, ProviderPayload, RateWindow,
    UsageSnapshot,
};
use crate::provider_metadata::ProviderId;

use super::{Provider, ProviderFetchContext};

use regex::Regex;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use time::OffsetDateTime;

pub struct CodexProvider;

impl Provider for CodexProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Codex
    }

    fn display_name(&self) -> &'static str {
        "Codex"
    }

    fn fetch(
        &self,
        context: &ProviderFetchContext,
    ) -> Result<ProviderPayload, ProviderErrorPayload> {
        let source_mode = context.source_mode();
        let strategies = resolve_strategies(&source_mode);

        let mut last_error: Option<ProviderErrorPayload> = None;

        for strategy in &strategies {
            match strategy {
                CodexStrategy::Oauth => match fetch_oauth(context) {
                    Ok(payload) => return Ok(payload),
                    Err(error) => {
                        log::warn!("Codex OAuth strategy failed: {}", error.message);
                        last_error = Some(error);
                    }
                },
                CodexStrategy::Cli => match fetch_cli(context) {
                    Ok(payload) => return Ok(payload),
                    Err(error) => {
                        log::warn!("Codex CLI strategy failed: {}", error.message);
                        last_error = Some(error);
                    }
                },
            }
        }

        Err(last_error.unwrap_or_else(|| ProviderErrorPayload {
            code: 1,
            message: format!(
                "No Codex fetch strategy available for source mode '{}'.",
                source_mode.as_str()
            ),
            kind: Some("provider".to_string()),
        }))
    }
}

enum CodexStrategy {
    Oauth,
    Cli,
}

fn resolve_strategies(source_mode: &ProviderSourceMode) -> Vec<CodexStrategy> {
    match source_mode {
        ProviderSourceMode::Oauth => vec![CodexStrategy::Oauth],
        ProviderSourceMode::Cli => vec![CodexStrategy::Cli],
        ProviderSourceMode::Auto => vec![CodexStrategy::Oauth, CodexStrategy::Cli],
        // Web and Api modes are not supported for Codex in this port.
        ProviderSourceMode::Web | ProviderSourceMode::Api => vec![],
    }
}

// ---------------------------------------------------------------------------
// CLI strategy
// ---------------------------------------------------------------------------

fn fetch_cli(context: &ProviderFetchContext) -> Result<ProviderPayload, ProviderErrorPayload> {
    let binary = resolve_codex_binary().ok_or_else(|| ProviderErrorPayload {
        code: 1,
        message:
            "Codex CLI not found. Install via `npm i -g @openai/codex` (or bun install) and restart."
                .to_string(),
        kind: Some("provider".to_string()),
    })?;

    let raw_output = run_codex_status(&binary).map_err(|message| ProviderErrorPayload {
        code: 1,
        message,
        kind: Some("provider".to_string()),
    })?;

    let snapshot = parse_cli_status(&raw_output).map_err(|message| ProviderErrorPayload {
        code: 1,
        message,
        kind: Some("provider".to_string()),
    })?;

    let now = context.now;

    let primary = snapshot.five_hour_percent_left.map(|percent_left| {
        let used_percent = (100 - percent_left).max(0).min(100) as f64;
        RateWindow {
            used_percent,
            window_minutes: Some(300),
            resets_at: None,
            reset_description: snapshot.five_hour_reset_description.clone(),
        }
    });

    let secondary = snapshot.weekly_percent_left.map(|percent_left| {
        let used_percent = (100 - percent_left).max(0).min(100) as f64;
        RateWindow {
            used_percent,
            window_minutes: Some(7 * 24 * 60),
            resets_at: None,
            reset_description: snapshot.weekly_reset_description.clone(),
        }
    });

    let usage = if primary.is_some() || secondary.is_some() {
        Some(UsageSnapshot {
            primary,
            secondary,
            tertiary: None,
            provider_cost: None,
            updated_at: now,
            identity: Some(ProviderIdentitySnapshot {
                provider_id: Some(ProviderId::Codex.to_string()),
                account_email: None,
                account_organization: None,
                login_method: None,
            }),
        })
    } else {
        None
    };

    let credits = snapshot.credits.map(|remaining| CreditsSnapshot {
        remaining,
        events: vec![],
        updated_at: now,
    });

    Ok(ProviderPayload {
        provider: ProviderId::Codex.to_string(),
        account: None,
        version: None,
        source: "codex-cli".to_string(),
        status: None,
        usage,
        credits,
        antigravity_plan_info: None,
        openai_dashboard: None,
        error: None,
    })
}

fn resolve_codex_binary() -> Option<String> {
    if let Ok(path) = std::env::var("CODEX_BINARY") {
        let path = path.trim().to_string();
        if !path.is_empty() {
            return Some(path);
        }
    }

    if let Ok(path) = which::which("codex") {
        return Some(path.to_string_lossy().to_string());
    }

    // Check common locations where npm/bun globals end up.
    let common_paths = [
        "/usr/local/bin/codex",
        "/opt/homebrew/bin/codex",
    ];
    for candidate in common_paths {
        if std::path::Path::new(candidate).is_file() {
            return Some(candidate.to_string());
        }
    }

    None
}

fn run_codex_status(binary: &str) -> Result<String, String> {
    // We invoke codex in read-only + untrusted mode and pipe /status via stdin.
    // The `-s read-only -a untrusted` flags prevent the CLI from executing any code.
    let result = Command::new(binary)
        .args(["-s", "read-only", "-a", "untrusted"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();

    let mut child = result.map_err(|error| format!("Failed to launch codex CLI: {error}"))?;

    if let Some(ref mut stdin) = child.stdin {
        use std::io::Write;
        // Send /status followed by newline, then close stdin.
        let _ = stdin.write_all(b"/status\n");
        let _ = stdin.flush();
    }
    // Drop stdin to signal EOF.
    drop(child.stdin.take());

    let output = child
        .wait_with_output()
        .map_err(|error| format!("Failed to read codex CLI output: {error}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    // Combine stdout + stderr for parsing (status output may appear on either).
    let combined = if stderr.is_empty() {
        stdout
    } else {
        format!("{stdout}\n{stderr}")
    };

    if combined.trim().is_empty() {
        return Err("Codex CLI returned no output.".to_string());
    }

    Ok(combined)
}

struct CliStatusSnapshot {
    credits: Option<f64>,
    five_hour_percent_left: Option<i32>,
    weekly_percent_left: Option<i32>,
    five_hour_reset_description: Option<String>,
    weekly_reset_description: Option<String>,
}

fn parse_cli_status(raw: &str) -> Result<CliStatusSnapshot, String> {
    let clean = strip_ansi_codes(raw);

    if clean.is_empty() {
        return Err("Codex status probe timed out.".to_string());
    }

    let lower = clean.to_lowercase();
    if lower.contains("data not available yet") {
        return Err("Codex status data not available yet.".to_string());
    }
    if lower.contains("update available") && lower.contains("codex") {
        return Err(
            "Codex CLI update needed. Run `bun install -g @openai/codex` to continue.".to_string(),
        );
    }

    let credits = parse_credits(&clean);
    let (five_hour_percent_left, five_hour_reset_description) = parse_limit_line(&clean, "5h limit");
    let (weekly_percent_left, weekly_reset_description) = parse_limit_line(&clean, "Weekly limit");

    if credits.is_none() && five_hour_percent_left.is_none() && weekly_percent_left.is_none() {
        let preview: String = clean.chars().take(400).collect();
        return Err(format!("Could not parse Codex status: {preview}"));
    }

    Ok(CliStatusSnapshot {
        credits,
        five_hour_percent_left,
        weekly_percent_left,
        five_hour_reset_description,
        weekly_reset_description,
    })
}

fn strip_ansi_codes(text: &str) -> String {
    // CSI sequences: ESC [ ... ending in 0x40–0x7E
    match Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]") {
        Ok(re) => re.replace_all(text, "").to_string(),
        Err(_) => text.to_string(),
    }
}

fn parse_credits(text: &str) -> Option<f64> {
    let re = Regex::new(r"(?i)Credits:\s*([0-9][0-9.,]*)").ok()?;
    let captures = re.captures(text)?;
    let raw = captures.get(1)?.as_str();
    parse_number_string(raw)
}

fn parse_limit_line(text: &str, pattern_prefix: &str) -> (Option<i32>, Option<String>) {
    let escaped = regex::escape(pattern_prefix);
    let line_pattern = format!(r"(?i){escaped}[^\n]*");
    let line_re = match Regex::new(&line_pattern) {
        Ok(re) => re,
        Err(_) => return (None, None),
    };

    let line = match line_re.find(text) {
        Some(matched) => matched.as_str(),
        None => return (None, None),
    };

    let percent_left = parse_percent_left(line);
    let reset_description = parse_reset_string(line);

    (percent_left, reset_description)
}

fn parse_percent_left(line: &str) -> Option<i32> {
    let re = Regex::new(r"(\d{1,3})%\s+left").ok()?;
    let captures = re.captures(line)?;
    let value: i32 = captures.get(1)?.as_str().parse().ok()?;
    Some(value)
}

fn parse_reset_string(line: &str) -> Option<String> {
    let re = Regex::new(r"(?i)resets?\s+(.+)").ok()?;
    let captures = re.captures(line)?;
    let value = captures.get(1)?.as_str().trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn parse_number_string(raw: &str) -> Option<f64> {
    let mut text = raw
        .replace('\u{00A0}', "")
        .replace('\u{202F}', "")
        .replace(' ', "");

    let has_comma = text.contains(',');
    let has_dot = text.contains('.');

    if has_comma && has_dot {
        let last_comma = text.rfind(',')?;
        let last_dot = text.rfind('.')?;
        if last_comma > last_dot {
            text = text.replace('.', "");
            text = text.replace(',', ".");
        } else {
            text = text.replace(',', "");
        }
    } else if has_comma {
        // Check if it looks like thousands separators (e.g. "1,000").
        let thousands_re = Regex::new(r"^\d{1,3}(,\d{3})+$").ok()?;
        if thousands_re.is_match(&text) {
            text = text.replace(',', "");
        } else {
            text = text.replace(',', ".");
        }
    } else if has_dot {
        let thousands_re = Regex::new(r"^\d{1,3}(\.\d{3})+$").ok()?;
        if thousands_re.is_match(&text) {
            text = text.replace('.', "");
        }
    }

    text.parse::<f64>().ok()
}

// ---------------------------------------------------------------------------
// OAuth strategy
// ---------------------------------------------------------------------------

fn fetch_oauth(context: &ProviderFetchContext) -> Result<ProviderPayload, ProviderErrorPayload> {
    let mut credentials = load_oauth_credentials().map_err(|message| ProviderErrorPayload {
        code: 1,
        message,
        kind: Some("provider".to_string()),
    })?;

    if credentials.needs_refresh && !credentials.refresh_token.is_empty() {
        match refresh_token(&credentials) {
            Ok(refreshed) => {
                if let Err(error) = save_oauth_credentials(&refreshed) {
                    log::warn!("Failed to save refreshed Codex credentials: {error}");
                }
                credentials = refreshed;
            }
            Err(error) => {
                log::warn!("Codex token refresh failed: {error}");
                // Continue with the existing (possibly stale) access token; the API call
                // will fail with 401 if it's truly expired, and we'll surface that error.
            }
        }
    }

    let usage_response = fetch_usage_api(&credentials).map_err(|message| ProviderErrorPayload {
        code: 1,
        message,
        kind: Some("provider".to_string()),
    })?;

    let now = context.now;
    let usage = map_oauth_usage(&usage_response, &credentials, now);
    let credits = map_oauth_credits(&usage_response, now);

    Ok(ProviderPayload {
        provider: ProviderId::Codex.to_string(),
        account: resolve_account_email(&credentials),
        version: None,
        source: "oauth".to_string(),
        status: None,
        usage: Some(usage),
        credits,
        antigravity_plan_info: None,
        openai_dashboard: None,
        error: None,
    })
}

struct OAuthCredentials {
    access_token: String,
    refresh_token: String,
    id_token: Option<String>,
    account_id: Option<String>,
    needs_refresh: bool,
}

fn codex_home_dir() -> PathBuf {
    if let Ok(codex_home) = std::env::var("CODEX_HOME") {
        let trimmed = codex_home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex")
}

fn load_oauth_credentials() -> Result<OAuthCredentials, String> {
    let auth_path = codex_home_dir().join("auth.json");
    if !auth_path.exists() {
        return Err("Codex auth.json not found. Run `codex` to log in.".to_string());
    }

    let contents =
        fs::read_to_string(&auth_path).map_err(|error| format!("Failed to read auth.json: {error}"))?;

    let json: serde_json::Value = serde_json::from_str(&contents)
        .map_err(|error| format!("Failed to parse auth.json: {error}"))?;

    // Legacy: OPENAI_API_KEY as a top-level field.
    if let Some(api_key) = json
        .get("OPENAI_API_KEY")
        .and_then(|value| value.as_str())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return Ok(OAuthCredentials {
            access_token: api_key,
            refresh_token: String::new(),
            id_token: None,
            account_id: None,
            needs_refresh: false,
        });
    }

    let tokens = json
        .get("tokens")
        .ok_or("Codex auth.json exists but contains no tokens.")?;

    let access_token = tokens
        .get("access_token")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty())
        .ok_or("Codex auth.json: missing access_token.")?;

    let refresh_token = tokens
        .get("refresh_token")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .unwrap_or_default();

    let id_token = tokens
        .get("id_token")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());

    let account_id = tokens
        .get("account_id")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());

    let needs_refresh = compute_needs_refresh(&json);

    Ok(OAuthCredentials {
        access_token,
        refresh_token,
        id_token,
        account_id,
        needs_refresh,
    })
}

fn compute_needs_refresh(json: &serde_json::Value) -> bool {
    let last_refresh_str = match json.get("last_refresh").and_then(|value| value.as_str()) {
        Some(value) => value,
        None => return true,
    };

    let parsed = time::OffsetDateTime::parse(
        last_refresh_str,
        &time::format_description::well_known::Rfc3339,
    );

    match parsed {
        Ok(last_refresh) => {
            let eight_days_seconds: i64 = 8 * 24 * 60 * 60;
            let elapsed = OffsetDateTime::now_utc() - last_refresh;
            elapsed.whole_seconds() > eight_days_seconds
        }
        Err(_) => true,
    }
}

fn save_oauth_credentials(credentials: &OAuthCredentials) -> Result<(), String> {
    let auth_path = codex_home_dir().join("auth.json");

    let mut json: serde_json::Value = if auth_path.exists() {
        let contents = fs::read_to_string(&auth_path)
            .map_err(|error| format!("Failed to read auth.json for save: {error}"))?;
        serde_json::from_str(&contents)
            .map_err(|error| format!("Failed to parse existing auth.json: {error}"))?
    } else {
        serde_json::json!({})
    };

    let mut tokens = serde_json::Map::new();
    tokens.insert(
        "access_token".to_string(),
        serde_json::Value::String(credentials.access_token.clone()),
    );
    tokens.insert(
        "refresh_token".to_string(),
        serde_json::Value::String(credentials.refresh_token.clone()),
    );
    if let Some(ref id_token) = credentials.id_token {
        tokens.insert(
            "id_token".to_string(),
            serde_json::Value::String(id_token.clone()),
        );
    }
    if let Some(ref account_id) = credentials.account_id {
        tokens.insert(
            "account_id".to_string(),
            serde_json::Value::String(account_id.clone()),
        );
    }

    json["tokens"] = serde_json::Value::Object(tokens);

    let now = OffsetDateTime::now_utc();
    let now_str = now
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| format!("Failed to format timestamp: {error}"))?;
    json["last_refresh"] = serde_json::Value::String(now_str);

    let parent = auth_path
        .parent()
        .ok_or("Cannot determine auth.json parent directory")?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Failed to create directory {}: {error}", parent.display()))?;

    let serialized = serde_json::to_string_pretty(&json)
        .map_err(|error| format!("Failed to serialize credentials: {error}"))?;

    fs::write(&auth_path, serialized)
        .map_err(|error| format!("Failed to write auth.json: {error}"))?;

    Ok(())
}

const REFRESH_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
const OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

fn refresh_token(credentials: &OAuthCredentials) -> Result<OAuthCredentials, String> {
    if credentials.refresh_token.is_empty() {
        return Ok(OAuthCredentials {
            access_token: credentials.access_token.clone(),
            refresh_token: credentials.refresh_token.clone(),
            id_token: credentials.id_token.clone(),
            account_id: credentials.account_id.clone(),
            needs_refresh: false,
        });
    }

    let body = serde_json::json!({
        "client_id": OAUTH_CLIENT_ID,
        "grant_type": "refresh_token",
        "refresh_token": credentials.refresh_token,
        "scope": "openid profile email",
    });

    let response = ureq::post(REFRESH_ENDPOINT)
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(30))
        .send_json(body);

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(401, response)) => {
            let body = response
                .into_string()
                .unwrap_or_else(|_| String::new());
            let error_code = extract_refresh_error_code(&body);
            let message = match error_code.as_deref() {
                Some("refresh_token_expired") => {
                    "Refresh token expired. Please run `codex` to log in again."
                }
                Some("refresh_token_reused") => {
                    "Refresh token was already used. Please run `codex` to log in again."
                }
                Some("refresh_token_invalidated") => {
                    "Refresh token was revoked. Please run `codex` to log in again."
                }
                _ => "Refresh token expired. Please run `codex` to log in again.",
            };
            return Err(message.to_string());
        }
        Err(ureq::Error::Status(code, response)) => {
            let body = response
                .into_string()
                .unwrap_or_else(|_| String::new());
            return Err(format!("Token refresh failed (HTTP {code}): {body}"));
        }
        Err(error) => {
            return Err(format!("Network error during token refresh: {error}"));
        }
    };

    let json: serde_json::Value = response
        .into_json()
        .map_err(|error| format!("Invalid refresh response JSON: {error}"))?;

    let new_access_token = json
        .get("access_token")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .unwrap_or_else(|| credentials.access_token.clone());

    let new_refresh_token = json
        .get("refresh_token")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .unwrap_or_else(|| credentials.refresh_token.clone());

    let new_id_token = json
        .get("id_token")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .or_else(|| credentials.id_token.clone());

    Ok(OAuthCredentials {
        access_token: new_access_token,
        refresh_token: new_refresh_token,
        id_token: new_id_token,
        account_id: credentials.account_id.clone(),
        needs_refresh: false,
    })
}

fn extract_refresh_error_code(body: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;

    // { "error": { "code": "..." } }
    if let Some(error_obj) = json.get("error") {
        if let Some(code) = error_obj.get("code").and_then(|value| value.as_str()) {
            return Some(code.to_string());
        }
        // { "error": "..." }
        if let Some(error_str) = error_obj.as_str() {
            return Some(error_str.to_string());
        }
    }
    // { "code": "..." }
    json.get("code")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
}

// ---------------------------------------------------------------------------
// OAuth usage API
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CodexUsageResponse {
    plan_type: Option<String>,
    rate_limit: Option<RateLimitDetails>,
    credits: Option<CreditDetails>,
}

#[derive(Debug, Deserialize)]
struct RateLimitDetails {
    primary_window: Option<WindowSnapshot>,
    secondary_window: Option<WindowSnapshot>,
}

#[derive(Debug, Deserialize)]
struct WindowSnapshot {
    used_percent: i32,
    reset_at: i64,
    limit_window_seconds: i64,
}

#[derive(Debug, Deserialize)]
struct CreditDetails {
    #[serde(default)]
    #[allow(dead_code)]
    has_credits: bool,
    #[serde(default)]
    #[allow(dead_code)]
    unlimited: bool,
    balance: Option<CreditBalance>,
}

// The balance field can be a number or a string.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum CreditBalance {
    Number(f64),
    Text(String),
}

impl CreditBalance {
    fn as_f64(&self) -> Option<f64> {
        match self {
            CreditBalance::Number(value) => Some(*value),
            CreditBalance::Text(text) => text.parse::<f64>().ok(),
        }
    }
}

const DEFAULT_CHATGPT_BASE_URL: &str = "https://chatgpt.com/backend-api";

fn resolve_usage_url() -> String {
    let base = resolve_chatgpt_base_url();
    let normalized = normalize_chatgpt_base_url(&base);

    if normalized.contains("/backend-api") {
        format!("{normalized}/wham/usage")
    } else {
        format!("{normalized}/api/codex/usage")
    }
}

fn resolve_chatgpt_base_url() -> String {
    if let Some(contents) = load_codex_config_toml() {
        if let Some(url) = parse_chatgpt_base_url_from_toml(&contents) {
            return url;
        }
    }
    DEFAULT_CHATGPT_BASE_URL.to_string()
}

fn normalize_chatgpt_base_url(value: &str) -> String {
    let mut trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        trimmed = DEFAULT_CHATGPT_BASE_URL.to_string();
    }
    while trimmed.ends_with('/') {
        trimmed.pop();
    }
    if (trimmed.starts_with("https://chatgpt.com")
        || trimmed.starts_with("https://chat.openai.com"))
        && !trimmed.contains("/backend-api")
    {
        trimmed.push_str("/backend-api");
    }
    trimmed
}

fn load_codex_config_toml() -> Option<String> {
    let config_path = codex_home_dir().join("config.toml");
    fs::read_to_string(config_path).ok()
}

fn parse_chatgpt_base_url_from_toml(contents: &str) -> Option<String> {
    let table: toml::Table = contents.parse().ok()?;
    let value = table.get("chatgpt_base_url")?.as_str()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn fetch_usage_api(credentials: &OAuthCredentials) -> Result<CodexUsageResponse, String> {
    let url = resolve_usage_url();

    let mut request = ureq::get(&url)
        .set(
            "Authorization",
            &format!("Bearer {}", credentials.access_token),
        )
        .set("User-Agent", "AgentStats")
        .set("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(30));

    if let Some(ref account_id) = credentials.account_id {
        if !account_id.is_empty() {
            request = request.set("ChatGPT-Account-Id", account_id);
        }
    }

    let response = match request.call() {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response
                .into_string()
                .unwrap_or_else(|_| "<no body>".to_string());
            if code == 401 || code == 403 {
                return Err(
                    "Codex OAuth token expired or invalid. Run `codex` to re-authenticate."
                        .to_string(),
                );
            }
            return Err(format!("Codex API error (HTTP {code}): {body}"));
        }
        Err(error) => return Err(format!("Network error fetching Codex usage: {error}")),
    };

    let usage: CodexUsageResponse = response
        .into_json()
        .map_err(|error| format!("Invalid response from Codex usage API: {error}"))?;

    Ok(usage)
}

fn map_oauth_usage(
    response: &CodexUsageResponse,
    credentials: &OAuthCredentials,
    now: OffsetDateTime,
) -> UsageSnapshot {
    let primary = response
        .rate_limit
        .as_ref()
        .and_then(|rate_limit| rate_limit.primary_window.as_ref())
        .map(|window| make_rate_window(window));

    let secondary = response
        .rate_limit
        .as_ref()
        .and_then(|rate_limit| rate_limit.secondary_window.as_ref())
        .map(|window| make_rate_window(window));

    let account_email = resolve_account_email(credentials);
    let plan = resolve_plan(response, credentials);

    let identity = ProviderIdentitySnapshot {
        provider_id: Some(ProviderId::Codex.to_string()),
        account_email,
        account_organization: None,
        login_method: plan,
    };

    UsageSnapshot {
        primary: primary.or(Some(RateWindow {
            used_percent: 0.0,
            window_minutes: None,
            resets_at: None,
            reset_description: None,
        })),
        secondary,
        tertiary: None,
        provider_cost: None,
        updated_at: now,
        identity: Some(identity),
    }
}

fn map_oauth_credits(response: &CodexUsageResponse, now: OffsetDateTime) -> Option<CreditsSnapshot> {
    let credit_details = response.credits.as_ref()?;
    let balance = credit_details.balance.as_ref()?.as_f64()?;
    Some(CreditsSnapshot {
        remaining: balance,
        events: vec![],
        updated_at: now,
    })
}

fn make_rate_window(window: &WindowSnapshot) -> RateWindow {
    let reset_date = OffsetDateTime::from_unix_timestamp(window.reset_at).ok();
    let reset_description = reset_date.map(|date| format_reset_description(date));
    RateWindow {
        used_percent: window.used_percent as f64,
        window_minutes: Some(window.limit_window_seconds / 60),
        resets_at: reset_date,
        reset_description,
    }
}

fn format_reset_description(reset_at: OffsetDateTime) -> String {
    let now = OffsetDateTime::now_utc();
    let remaining = reset_at - now;
    let total_seconds = remaining.whole_seconds();

    if total_seconds <= 0 {
        return "resets soon".to_string();
    }

    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;

    if hours > 0 {
        format!("resets in {hours}h {minutes}m")
    } else if minutes > 0 {
        format!("resets in {minutes}m")
    } else {
        "resets in <1m".to_string()
    }
}

// ---------------------------------------------------------------------------
// JWT helpers for extracting email/plan from id_token
// ---------------------------------------------------------------------------

fn resolve_account_email(credentials: &OAuthCredentials) -> Option<String> {
    let id_token = credentials.id_token.as_ref()?;
    let payload = parse_jwt_payload(id_token)?;

    // Try top-level "email" first, then nested profile.
    let email = payload
        .get("email")
        .and_then(|value| value.as_str())
        .or_else(|| {
            payload
                .get("https://api.openai.com/profile")
                .and_then(|value| value.get("email"))
                .and_then(|value| value.as_str())
        })?;

    let trimmed = email.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn resolve_plan(response: &CodexUsageResponse, credentials: &OAuthCredentials) -> Option<String> {
    if let Some(ref plan) = response.plan_type {
        if !plan.is_empty() {
            return Some(plan.clone());
        }
    }

    let id_token = credentials.id_token.as_ref()?;
    let payload = parse_jwt_payload(id_token)?;

    let plan = payload
        .get("https://api.openai.com/auth")
        .and_then(|value| value.get("chatgpt_plan_type"))
        .and_then(|value| value.as_str())
        .or_else(|| {
            payload
                .get("chatgpt_plan_type")
                .and_then(|value| value.as_str())
        })?;

    let trimmed = plan.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_jwt_payload(token: &str) -> Option<serde_json::Value> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }

    let payload_b64 = parts.get(1)?;
    let decoded = base64_url_decode(payload_b64)?;
    serde_json::from_slice(&decoded).ok()
}

fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    // JWT uses base64url encoding (no padding, - instead of +, _ instead of /).
    let mut b64 = input.replace('-', "+").replace('_', "/");

    // Add padding if needed.
    let padding = (4 - b64.len() % 4) % 4;
    for _ in 0..padding {
        b64.push('=');
    }

    // Use a simple base64 decoder. We avoid adding an extra crate dependency by
    // implementing a minimal decoder.
    decode_base64(&b64)
}

fn decode_base64(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut lookup = [255u8; 256];
    for (index, &byte) in TABLE.iter().enumerate() {
        lookup[byte as usize] = index as u8;
    }

    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len() * 3 / 4);

    let mut index = 0;
    while index < bytes.len() {
        let remaining = bytes.len() - index;
        if remaining < 4 {
            break;
        }

        let a = lookup[bytes[index] as usize];
        let b = lookup[bytes[index + 1] as usize];
        let c = if bytes[index + 2] == b'=' {
            0
        } else {
            lookup[bytes[index + 2] as usize]
        };
        let d = if bytes[index + 3] == b'=' {
            0
        } else {
            lookup[bytes[index + 3] as usize]
        };

        if a == 255 || b == 255 {
            return None;
        }

        output.push((a << 2) | (b >> 4));
        if bytes[index + 2] != b'=' {
            if c == 255 {
                return None;
            }
            output.push(((b & 0x0F) << 4) | (c >> 2));
        }
        if bytes[index + 3] != b'=' {
            if d == 255 {
                return None;
            }
            output.push(((c & 0x03) << 6) | d);
        }

        index += 4;
    }

    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi_codes() {
        let input = "\x1b[32mHello\x1b[0m World";
        assert_eq!(strip_ansi_codes(input), "Hello World");
    }

    #[test]
    fn test_strip_ansi_codes_no_ansi() {
        let input = "Hello World";
        assert_eq!(strip_ansi_codes(input), "Hello World");
    }

    #[test]
    fn test_parse_credits() {
        let text = "Credits: 150.00\nSome other text";
        assert_eq!(parse_credits(text), Some(150.0));
    }

    #[test]
    fn test_parse_credits_with_commas() {
        let text = "Credits: 1,500.00";
        assert_eq!(parse_credits(text), Some(1500.0));
    }

    #[test]
    fn test_parse_credits_missing() {
        let text = "No credit info here";
        assert_eq!(parse_credits(text), None);
    }

    #[test]
    fn test_parse_percent_left() {
        assert_eq!(parse_percent_left("5h limit - 85% left"), Some(85));
    }

    #[test]
    fn test_parse_percent_left_zero() {
        assert_eq!(parse_percent_left("Weekly limit - 0% left"), Some(0));
    }

    #[test]
    fn test_parse_reset_string() {
        let line = "5h limit - 85% left resets in 3h 42m";
        assert_eq!(
            parse_reset_string(line),
            Some("in 3h 42m".to_string())
        );
    }

    #[test]
    fn test_parse_limit_line() {
        let text = "Credits: 50.00\n5h limit - 72% left reset in 2h 15m\nWeekly limit - 90% left\n";
        let (five_pct, five_reset) = parse_limit_line(text, "5h limit");
        assert_eq!(five_pct, Some(72));
        assert_eq!(five_reset, Some("in 2h 15m".to_string()));

        let (weekly_pct, weekly_reset) = parse_limit_line(text, "Weekly limit");
        assert_eq!(weekly_pct, Some(90));
        assert_eq!(weekly_reset, None);
    }

    #[test]
    fn test_parse_cli_status_full() {
        let text = "Credits: 200.00\n5h limit - 65% left resets in 4h 30m\nWeekly limit - 80% left resets in 3d\n";
        let snapshot = parse_cli_status(text).expect("should parse");
        assert_eq!(snapshot.credits, Some(200.0));
        assert_eq!(snapshot.five_hour_percent_left, Some(65));
        assert_eq!(snapshot.weekly_percent_left, Some(80));
    }

    #[test]
    fn test_parse_cli_status_partial() {
        let text = "Credits: 50.00\n";
        let snapshot = parse_cli_status(text).expect("should parse");
        assert_eq!(snapshot.credits, Some(50.0));
        assert_eq!(snapshot.five_hour_percent_left, None);
        assert_eq!(snapshot.weekly_percent_left, None);
    }

    #[test]
    fn test_parse_cli_status_empty() {
        let result = parse_cli_status("");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_cli_status_update_required() {
        let text = "Update available for codex\nPlease update to continue";
        let result = parse_cli_status(text);
        assert!(result.is_err());
        assert!(result
            .err()
            .expect("should be error")
            .contains("update needed"));
    }

    #[test]
    fn test_parse_number_string_simple() {
        assert_eq!(parse_number_string("123.45"), Some(123.45));
    }

    #[test]
    fn test_parse_number_string_thousands_comma() {
        assert_eq!(parse_number_string("1,234"), Some(1234.0));
    }

    #[test]
    fn test_parse_number_string_european() {
        assert_eq!(parse_number_string("1.234,56"), Some(1234.56));
    }

    #[test]
    fn test_base64_url_decode() {
        let encoded = "SGVsbG8gV29ybGQ"; // "Hello World"
        let decoded = base64_url_decode(encoded).expect("should decode");
        assert_eq!(String::from_utf8(decoded).expect("valid utf8"), "Hello World");
    }

    #[test]
    fn test_parse_jwt_payload() {
        // A minimal JWT with payload {"email":"test@example.com"}
        let payload_json = r#"{"email":"test@example.com"}"#;
        let payload_b64 = simple_base64_encode(payload_json.as_bytes());
        let token = format!("header.{payload_b64}.signature");
        let parsed = parse_jwt_payload(&token).expect("should parse");
        assert_eq!(
            parsed.get("email").and_then(|value| value.as_str()),
            Some("test@example.com")
        );
    }

    #[test]
    fn test_normalize_chatgpt_base_url() {
        assert_eq!(
            normalize_chatgpt_base_url("https://chatgpt.com"),
            "https://chatgpt.com/backend-api"
        );
        assert_eq!(
            normalize_chatgpt_base_url("https://chatgpt.com/backend-api/"),
            "https://chatgpt.com/backend-api"
        );
        assert_eq!(
            normalize_chatgpt_base_url("https://custom.example.com/api"),
            "https://custom.example.com/api"
        );
    }

    fn simple_base64_encode(data: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut result = String::new();
        let mut index = 0;
        while index < data.len() {
            let a = data[index];
            let b = if index + 1 < data.len() { data[index + 1] } else { 0 };
            let c = if index + 2 < data.len() { data[index + 2] } else { 0 };

            result.push(TABLE[(a >> 2) as usize] as char);
            result.push(TABLE[((a & 0x03) << 4 | b >> 4) as usize] as char);

            if index + 1 < data.len() {
                result.push(TABLE[((b & 0x0F) << 2 | c >> 6) as usize] as char);
            }
            if index + 2 < data.len() {
                result.push(TABLE[(c & 0x3F) as usize] as char);
            }

            index += 3;
        }
        result
    }

    #[test]
    fn test_format_reset_description() {
        let future = OffsetDateTime::now_utc() + time::Duration::hours(2) + time::Duration::minutes(30);
        let description = format_reset_description(future);
        assert!(description.contains("resets in"));
        assert!(description.contains("2h"));
    }

    #[test]
    fn test_format_reset_description_past() {
        let past = OffsetDateTime::now_utc() - time::Duration::hours(1);
        assert_eq!(format_reset_description(past), "resets soon");
    }
}
