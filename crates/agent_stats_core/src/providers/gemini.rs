use crate::config::ProviderSourceMode;
use crate::models::{
    ProviderErrorPayload, ProviderIdentitySnapshot, ProviderPayload, RateWindow, UsageSnapshot,
};
use crate::provider_metadata::ProviderId;

use super::{Provider, ProviderFetchContext};

use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use time::OffsetDateTime;

const QUOTA_ENDPOINT: &str = "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota";
const LOAD_CODE_ASSIST_ENDPOINT: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
const PROJECTS_ENDPOINT: &str = "https://cloudresourcemanager.googleapis.com/v1/projects";
const TOKEN_REFRESH_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const CREDENTIALS_PATH: &str = ".gemini/oauth_creds.json";
const SETTINGS_PATH: &str = ".gemini/settings.json";

const REQUEST_TIMEOUT_MS: u64 = 10_000;

pub struct GeminiProvider;

impl Provider for GeminiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Gemini
    }

    fn display_name(&self) -> &'static str {
        "Gemini"
    }

    fn fetch(
        &self,
        context: &ProviderFetchContext,
    ) -> Result<ProviderPayload, ProviderErrorPayload> {
        let source_mode = context.source_mode();
        match source_mode {
            ProviderSourceMode::Auto | ProviderSourceMode::Api | ProviderSourceMode::Oauth => {}
            _ => {
                return Err(provider_error(
                    "Gemini only supports Auto/API/OAuth source modes.",
                ));
            }
        }

        let home =
            dirs::home_dir().ok_or_else(|| provider_error("Could not resolve home directory."))?;

        let auth_type = read_auth_type(&home);
        match auth_type {
            AuthType::ApiKey => {
                return Err(provider_error(
                    "Gemini API key auth not supported. Use Google account (OAuth) instead.",
                ));
            }
            AuthType::VertexAI => {
                return Err(provider_error(
                    "Gemini Vertex AI auth not supported. Use Google account (OAuth) instead.",
                ));
            }
            AuthType::OAuthPersonal | AuthType::Unknown => {}
        }

        let credentials = load_credentials(&home)?;

        let access_token = match &credentials.access_token {
            Some(token) if !token.is_empty() => token.clone(),
            _ => {
                return Err(provider_error(
                    "Not logged in to Gemini. Run 'gemini' in Terminal to authenticate.",
                ));
            }
        };

        let access_token = maybe_refresh_token(access_token, &credentials, &home)?;

        let claims = extract_claims_from_id_token(credentials.id_token.as_deref());

        let code_assist_status = load_code_assist_status(&access_token).unwrap_or_default();

        let project_id = code_assist_status
            .project_id
            .clone()
            .or_else(|| discover_gemini_project_id(&access_token).ok().flatten());

        let model_quotas = fetch_quota(&access_token, project_id.as_deref())?;

        if model_quotas.is_empty() {
            return Err(provider_error("No quota data returned from Gemini API."));
        }

        let plan = determine_plan(
            code_assist_status.tier.as_ref(),
            claims.hosted_domain.as_deref(),
        );

        let usage = build_usage_snapshot(
            &model_quotas,
            claims.email.as_deref(),
            plan.as_deref(),
            context.now,
        );

        Ok(ProviderPayload {
            provider: ProviderId::Gemini.to_string(),
            account: claims.email.clone(),
            version: None,
            source: "api".to_string(),
            status: None,
            usage: Some(usage),
            credits: None,
            antigravity_plan_info: None,
            openai_dashboard: None,
            error: None,
        })
    }
}

// --- Auth type detection ---

#[derive(Debug, PartialEq, Eq)]
enum AuthType {
    OAuthPersonal,
    ApiKey,
    VertexAI,
    Unknown,
}

fn read_auth_type(home: &Path) -> AuthType {
    let settings_path = home.join(SETTINGS_PATH);
    let content = match fs::read_to_string(&settings_path) {
        Ok(content) => content,
        Err(_) => return AuthType::Unknown,
    };

    let json: Value = match serde_json::from_str(&content) {
        Ok(json) => json,
        Err(_) => return AuthType::Unknown,
    };

    let selected_type = json
        .get("security")
        .and_then(|security| security.get("auth"))
        .and_then(|auth| auth.get("selectedType"))
        .and_then(|value| value.as_str());

    match selected_type {
        Some("oauth-personal") => AuthType::OAuthPersonal,
        Some("api-key") => AuthType::ApiKey,
        Some("vertex-ai") => AuthType::VertexAI,
        _ => AuthType::Unknown,
    }
}

// --- OAuth credentials ---

struct OAuthCredentials {
    access_token: Option<String>,
    id_token: Option<String>,
    refresh_token: Option<String>,
    expiry_date_ms: Option<f64>,
}

fn load_credentials(home: &Path) -> Result<OAuthCredentials, ProviderErrorPayload> {
    let creds_path = home.join(CREDENTIALS_PATH);

    if !creds_path.exists() {
        return Err(provider_error(
            "Not logged in to Gemini. Run 'gemini' in Terminal to authenticate.",
        ));
    }

    let content = fs::read_to_string(&creds_path)
        .map_err(|error| provider_error(&format!("Failed to read Gemini credentials: {error}")))?;

    let json: Value = serde_json::from_str(&content)
        .map_err(|error| provider_error(&format!("Invalid Gemini credentials file: {error}")))?;

    Ok(OAuthCredentials {
        access_token: json
            .get("access_token")
            .and_then(|value| value.as_str())
            .map(String::from),
        id_token: json
            .get("id_token")
            .and_then(|value| value.as_str())
            .map(String::from),
        refresh_token: json
            .get("refresh_token")
            .and_then(|value| value.as_str())
            .map(String::from),
        expiry_date_ms: json.get("expiry_date").and_then(|value| value.as_f64()),
    })
}

// --- Token refresh ---

fn maybe_refresh_token(
    access_token: String,
    credentials: &OAuthCredentials,
    home: &Path,
) -> Result<String, ProviderErrorPayload> {
    let is_expired = match credentials.expiry_date_ms {
        Some(expiry_ms) => {
            let expiry_secs = (expiry_ms / 1000.0) as i64;
            let now_secs = OffsetDateTime::now_utc().unix_timestamp();
            now_secs >= expiry_secs
        }
        None => false,
    };

    if !is_expired {
        return Ok(access_token);
    }

    let refresh_token = credentials.refresh_token.as_deref().ok_or_else(|| {
        provider_error(
            "Gemini token expired and no refresh token available. Re-authenticate with 'gemini'.",
        )
    })?;

    let client_creds = extract_oauth_client_credentials().ok_or_else(|| {
        provider_error("Could not find Gemini CLI OAuth configuration for token refresh.")
    })?;

    let body = format!(
        "client_id={}&client_secret={}&refresh_token={}&grant_type=refresh_token",
        client_creds.client_id, client_creds.client_secret, refresh_token
    );

    let response = ureq::post(TOKEN_REFRESH_ENDPOINT)
        .timeout(std::time::Duration::from_millis(REQUEST_TIMEOUT_MS))
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_string(&body);

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, _)) => {
            if code == 401 || code == 400 {
                return Err(provider_error(
                    "Gemini token refresh failed (credentials expired). Re-authenticate with 'gemini'.",
                ));
            }
            return Err(provider_error(&format!(
                "Gemini token refresh failed (HTTP {code})."
            )));
        }
        Err(error) => {
            return Err(provider_error(&format!(
                "Gemini token refresh network error: {error}"
            )));
        }
    };

    let json: Value = response.into_json().map_err(|error| {
        provider_error(&format!(
            "Failed to parse Gemini token refresh response: {error}"
        ))
    })?;

    let new_access_token = json
        .get("access_token")
        .and_then(|value| value.as_str())
        .ok_or_else(|| provider_error("Gemini token refresh response missing access_token."))?
        .to_string();

    update_stored_credentials(&json, home).log_err_quietly();

    Ok(new_access_token)
}

struct OAuthClientCredentials {
    client_id: String,
    client_secret: String,
}

fn extract_oauth_client_credentials() -> Option<OAuthClientCredentials> {
    let gemini_path = which::which("gemini").ok()?;

    let real_path = fs::canonicalize(&gemini_path).unwrap_or(gemini_path);
    let bin_dir = real_path.parent()?;
    let base_dir = bin_dir.parent()?;

    let oauth_subpath =
        "node_modules/@google/gemini-cli/node_modules/@google/gemini-cli-core/dist/src/code_assist/oauth2.js";
    let nix_share_subpath =
        "share/gemini-cli/node_modules/@google/gemini-cli-core/dist/src/code_assist/oauth2.js";
    let oauth_file = "dist/src/code_assist/oauth2.js";

    let possible_paths = [
        base_dir.join("libexec").join("lib").join(oauth_subpath),
        base_dir.join("lib").join(oauth_subpath),
        base_dir.join(nix_share_subpath),
        base_dir.join("..").join("gemini-cli-core").join(oauth_file),
        base_dir
            .join("node_modules")
            .join("@google")
            .join("gemini-cli-core")
            .join(oauth_file),
    ];

    for path in &possible_paths {
        if let Ok(content) = fs::read_to_string(path) {
            if let Some(creds) = parse_oauth_credentials_from_source(&content) {
                return Some(creds);
            }
        }
    }

    None
}

fn parse_oauth_credentials_from_source(content: &str) -> Option<OAuthClientCredentials> {
    let client_id = extract_js_string_constant(content, "OAUTH_CLIENT_ID")?;
    let client_secret = extract_js_string_constant(content, "OAUTH_CLIENT_SECRET")?;
    Some(OAuthClientCredentials {
        client_id,
        client_secret,
    })
}

fn extract_js_string_constant(content: &str, name: &str) -> Option<String> {
    let pos = content.find(name)?;
    let after_name = &content[pos + name.len()..];

    let equals_pos = after_name.find('=')?;
    let after_equals = after_name[equals_pos + 1..].trim_start();

    let (quote_char, after_quote) = if after_equals.starts_with('\'') {
        ('\'', &after_equals[1..])
    } else if after_equals.starts_with('"') {
        ('"', &after_equals[1..])
    } else {
        return None;
    };

    let end_pos = after_quote.find(quote_char)?;
    let value = &after_quote[..end_pos];

    if value.is_empty() {
        return None;
    }

    Some(value.to_string())
}

fn update_stored_credentials(refresh_response: &Value, home: &Path) -> Result<(), String> {
    let creds_path = home.join(CREDENTIALS_PATH);
    let existing_content = fs::read_to_string(&creds_path)
        .map_err(|error| format!("Failed to read credentials for update: {error}"))?;

    let mut json: Value = serde_json::from_str(&existing_content)
        .map_err(|error| format!("Failed to parse existing credentials: {error}"))?;

    if let Some(access_token) = refresh_response.get("access_token") {
        json["access_token"] = access_token.clone();
    }
    if let Some(expires_in) = refresh_response
        .get("expires_in")
        .and_then(|value| value.as_f64())
    {
        let now_ms = OffsetDateTime::now_utc().unix_timestamp() as f64 * 1000.0;
        let new_expiry = now_ms + (expires_in * 1000.0);
        json["expiry_date"] = serde_json::json!(new_expiry);
    }
    if let Some(id_token) = refresh_response.get("id_token") {
        json["id_token"] = id_token.clone();
    }

    let updated = serde_json::to_string_pretty(&json)
        .map_err(|error| format!("Failed to serialize updated credentials: {error}"))?;

    fs::write(&creds_path, updated)
        .map_err(|error| format!("Failed to write updated credentials: {error}"))?;

    Ok(())
}

// --- JWT claims extraction ---

struct TokenClaims {
    email: Option<String>,
    hosted_domain: Option<String>,
}

fn extract_claims_from_id_token(id_token: Option<&str>) -> TokenClaims {
    let empty = TokenClaims {
        email: None,
        hosted_domain: None,
    };

    let token = match id_token {
        Some(token) if !token.is_empty() => token,
        _ => return empty,
    };

    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return empty;
    }

    let payload = parts[1];
    let decoded = match base64_url_decode(payload) {
        Some(bytes) => bytes,
        None => return empty,
    };

    let json: Value = match serde_json::from_slice(&decoded) {
        Ok(json) => json,
        Err(_) => return empty,
    };

    TokenClaims {
        email: json
            .get("email")
            .and_then(|value| value.as_str())
            .map(String::from),
        hosted_domain: json
            .get("hd")
            .and_then(|value| value.as_str())
            .map(String::from),
    }
}

fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    let mut standardized = input.replace('-', "+").replace('_', "/");

    let remainder = standardized.len() % 4;
    if remainder > 0 {
        for _ in 0..(4 - remainder) {
            standardized.push('=');
        }
    }

    // Use a simple base64 decoder (no external crate dependency)
    base64_decode_standard(&standardized)
}

fn base64_decode_standard(input: &str) -> Option<Vec<u8>> {
    fn decode_char(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut i = 0;

    while i + 3 < bytes.len() {
        let a = decode_char(bytes[i]);
        let b = decode_char(bytes[i + 1]);
        let c_val = decode_char(bytes[i + 2]);
        let d = decode_char(bytes[i + 3]);

        match (a, b) {
            (Some(a_val), Some(b_val)) => {
                output.push((a_val << 2) | (b_val >> 4));
                if let Some(c_decoded) = c_val {
                    output.push((b_val << 4) | (c_decoded >> 2));
                    if let Some(d_decoded) = d {
                        output.push((c_decoded << 6) | d_decoded);
                    }
                }
            }
            _ => return None,
        }

        i += 4;
    }

    Some(output)
}

// --- Code Assist status (tier + project discovery) ---

#[derive(Default)]
struct CodeAssistStatus {
    tier: Option<GeminiTier>,
    project_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GeminiTier {
    Free,
    Standard,
    Legacy,
}

fn load_code_assist_status(access_token: &str) -> Result<CodeAssistStatus, ProviderErrorPayload> {
    let body = r#"{"metadata":{"ideType":"GEMINI_CLI","pluginType":"GEMINI"}}"#;

    let response = ureq::post(LOAD_CODE_ASSIST_ENDPOINT)
        .timeout(std::time::Duration::from_millis(REQUEST_TIMEOUT_MS))
        .set("Authorization", &format!("Bearer {access_token}"))
        .set("Content-Type", "application/json")
        .send_string(body);

    let response = match response {
        Ok(response) => response,
        Err(_) => return Ok(CodeAssistStatus::default()),
    };

    let json: Value = match response.into_json() {
        Ok(json) => json,
        Err(_) => return Ok(CodeAssistStatus::default()),
    };

    let project_id = extract_project_id_from_code_assist(&json);

    let tier = json
        .get("currentTier")
        .and_then(|tier| tier.get("id"))
        .and_then(|id| id.as_str())
        .and_then(|id| match id {
            "free-tier" => Some(GeminiTier::Free),
            "standard-tier" => Some(GeminiTier::Standard),
            "legacy-tier" => Some(GeminiTier::Legacy),
            _ => None,
        });

    Ok(CodeAssistStatus { tier, project_id })
}

fn extract_project_id_from_code_assist(json: &Value) -> Option<String> {
    let project_value = json.get("cloudaicompanionProject")?;

    let raw = if let Some(project_str) = project_value.as_str() {
        project_str.to_string()
    } else if let Some(project_obj) = project_value.as_object() {
        project_obj
            .get("id")
            .or_else(|| project_obj.get("projectId"))
            .and_then(|value| value.as_str())
            .map(String::from)?
    } else {
        return None;
    };

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

// --- Project discovery fallback ---

fn discover_gemini_project_id(access_token: &str) -> Result<Option<String>, ProviderErrorPayload> {
    let response = ureq::get(PROJECTS_ENDPOINT)
        .timeout(std::time::Duration::from_millis(REQUEST_TIMEOUT_MS))
        .set("Authorization", &format!("Bearer {access_token}"))
        .call();

    let response = match response {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };

    let json: Value = match response.into_json() {
        Ok(json) => json,
        Err(_) => return Ok(None),
    };

    let projects = match json.get("projects").and_then(|value| value.as_array()) {
        Some(projects) => projects,
        None => return Ok(None),
    };

    for project in projects {
        let project_id = match project.get("projectId").and_then(|value| value.as_str()) {
            Some(id) => id,
            None => continue,
        };

        if project_id.starts_with("gen-lang-client") {
            return Ok(Some(project_id.to_string()));
        }

        if let Some(labels) = project.get("labels").and_then(|value| value.as_object()) {
            if labels.contains_key("generative-language") {
                return Ok(Some(project_id.to_string()));
            }
        }
    }

    Ok(None)
}

// --- Quota fetching ---

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaBucket {
    remaining_fraction: Option<f64>,
    reset_time: Option<String>,
    model_id: Option<String>,
    #[allow(dead_code)]
    token_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QuotaResponse {
    buckets: Option<Vec<QuotaBucket>>,
}

struct ModelQuota {
    model_id: String,
    percent_left: f64,
    reset_time: Option<OffsetDateTime>,
    reset_description: Option<String>,
}

fn fetch_quota(
    access_token: &str,
    project_id: Option<&str>,
) -> Result<Vec<ModelQuota>, ProviderErrorPayload> {
    let body = match project_id {
        Some(id) => format!(r#"{{"project":"{}"}}"#, id),
        None => "{}".to_string(),
    };

    let response = ureq::post(QUOTA_ENDPOINT)
        .timeout(std::time::Duration::from_millis(REQUEST_TIMEOUT_MS))
        .set("Authorization", &format!("Bearer {access_token}"))
        .set("Content-Type", "application/json")
        .send_string(&body);

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(401, _)) => {
            return Err(provider_error(
                "Not logged in to Gemini. Run 'gemini' in Terminal to authenticate.",
            ));
        }
        Err(ureq::Error::Status(code, response)) => {
            let body_text = response
                .into_string()
                .unwrap_or_else(|_| "<no body>".to_string());
            return Err(provider_error(&format!(
                "Gemini quota API error (HTTP {code}): {body_text}"
            )));
        }
        Err(error) => {
            return Err(provider_error(&format!(
                "Gemini quota API network error: {error}"
            )));
        }
    };

    let quota_response: QuotaResponse = response.into_json().map_err(|error| {
        provider_error(&format!("Failed to parse Gemini quota response: {error}"))
    })?;

    let buckets = match quota_response.buckets {
        Some(buckets) if !buckets.is_empty() => buckets,
        _ => {
            return Err(provider_error("No quota buckets in Gemini response."));
        }
    };

    // Group by model, keeping lowest remaining fraction per model
    let mut model_map: HashMap<String, (f64, Option<String>)> = HashMap::new();

    for bucket in &buckets {
        let model_id = match &bucket.model_id {
            Some(id) if !id.is_empty() => id.clone(),
            _ => continue,
        };
        let fraction = match bucket.remaining_fraction {
            Some(fraction) => fraction,
            None => continue,
        };

        let entry = model_map.entry(model_id);
        match entry {
            std::collections::hash_map::Entry::Occupied(mut occupied) => {
                if fraction < occupied.get().0 {
                    occupied.insert((fraction, bucket.reset_time.clone()));
                }
            }
            std::collections::hash_map::Entry::Vacant(vacant) => {
                vacant.insert((fraction, bucket.reset_time.clone()));
            }
        }
    }

    let mut quotas: Vec<ModelQuota> = model_map
        .into_iter()
        .map(|(model_id, (fraction, reset_time_str))| {
            let reset_time = reset_time_str.as_deref().and_then(parse_iso8601_datetime);
            let reset_description = reset_time_str.as_deref().map(format_reset_time);
            ModelQuota {
                model_id,
                percent_left: fraction * 100.0,
                reset_time,
                reset_description,
            }
        })
        .collect();

    quotas.sort_by(|a, b| a.model_id.cmp(&b.model_id));

    Ok(quotas)
}

// --- Usage snapshot construction ---

fn build_usage_snapshot(
    model_quotas: &[ModelQuota],
    email: Option<&str>,
    plan: Option<&str>,
    now: OffsetDateTime,
) -> UsageSnapshot {
    let pro_min = model_quotas
        .iter()
        .filter(|quota| quota.model_id.to_lowercase().contains("pro"))
        .min_by(|a, b| {
            a.percent_left
                .partial_cmp(&b.percent_left)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

    let flash_min = model_quotas
        .iter()
        .filter(|quota| quota.model_id.to_lowercase().contains("flash"))
        .min_by(|a, b| {
            a.percent_left
                .partial_cmp(&b.percent_left)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

    // Swift reference always creates a primary window (defaults to 0% used if no pro model)
    let primary = Some(match pro_min {
        Some(quota) => RateWindow {
            used_percent: 100.0 - quota.percent_left,
            window_minutes: Some(1440),
            resets_at: quota.reset_time,
            reset_description: quota.reset_description.clone(),
        },
        None => RateWindow {
            used_percent: 0.0,
            window_minutes: Some(1440),
            resets_at: None,
            reset_description: None,
        },
    });

    let secondary = flash_min.map(|quota| RateWindow {
        used_percent: 100.0 - quota.percent_left,
        window_minutes: Some(1440),
        resets_at: quota.reset_time,
        reset_description: quota.reset_description.clone(),
    });

    let identity = ProviderIdentitySnapshot {
        provider_id: Some(ProviderId::Gemini.to_string()),
        account_email: email.map(String::from),
        account_organization: None,
        login_method: plan.map(String::from),
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

// --- Plan detection ---

fn determine_plan(tier: Option<&GeminiTier>, hosted_domain: Option<&str>) -> Option<String> {
    match tier {
        Some(GeminiTier::Standard) => Some("Paid".to_string()),
        Some(GeminiTier::Free) => {
            if hosted_domain.is_some() {
                Some("Workspace".to_string())
            } else {
                Some("Free".to_string())
            }
        }
        Some(GeminiTier::Legacy) => Some("Legacy".to_string()),
        None => None,
    }
}

// --- Time parsing helpers ---

fn parse_iso8601_datetime(input: &str) -> Option<OffsetDateTime> {
    use time::format_description::well_known::Rfc3339;

    if let Ok(parsed) = OffsetDateTime::parse(input, &Rfc3339) {
        return Some(parsed);
    }

    // Try without fractional seconds by trimming fractional part
    if let Some(dot_pos) = input.rfind('.') {
        let after_dot = &input[dot_pos..];
        if after_dot.contains('Z') {
            let without_frac = format!("{}Z", &input[..dot_pos]);
            if let Ok(parsed) = OffsetDateTime::parse(&without_frac, &Rfc3339) {
                return Some(parsed);
            }
        } else if let Some(plus_pos) = after_dot.find('+') {
            let without_frac = format!("{}{}", &input[..dot_pos], &after_dot[plus_pos..]);
            if let Ok(parsed) = OffsetDateTime::parse(&without_frac, &Rfc3339) {
                return Some(parsed);
            }
        }
    }

    None
}

fn format_reset_time(iso_string: &str) -> String {
    let reset_date = match parse_iso8601_datetime(iso_string) {
        Some(date) => date,
        None => return "Resets soon".to_string(),
    };

    let now = OffsetDateTime::now_utc();
    let duration = reset_date - now;
    let total_seconds = duration.whole_seconds();

    if total_seconds <= 0 {
        return "Resets soon".to_string();
    }

    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;

    if hours > 0 {
        format!("Resets in {hours}h {minutes}m")
    } else {
        format!("Resets in {minutes}m")
    }
}

// --- Error helper ---

fn provider_error(message: &str) -> ProviderErrorPayload {
    ProviderErrorPayload {
        code: 1,
        message: message.to_string(),
        kind: Some("provider".to_string()),
    }
}

// --- Utility trait for logging errors without discarding ---

trait LogErrQuietly {
    fn log_err_quietly(self);
}

impl<T> LogErrQuietly for Result<T, String> {
    fn log_err_quietly(self) {
        if let Err(error) = self {
            log::warn!("Gemini credential update: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Auth type ---

    #[test]
    fn test_read_auth_type_missing_settings() {
        let temp = std::env::temp_dir().join("gemini_test_no_settings");
        let _ = std::fs::create_dir_all(&temp);
        assert_eq!(read_auth_type(&temp), AuthType::Unknown);
    }

    // --- JWT claims extraction ---

    #[test]
    fn test_extract_claims_valid_token() {
        let header = base64_url_encode(r#"{"alg":"RS256","typ":"JWT"}"#);
        let payload = base64_url_encode(r#"{"email":"user@corp.com","hd":"corp.com"}"#);
        let token = format!("{header}.{payload}.signature");

        let claims = extract_claims_from_id_token(Some(&token));
        assert_eq!(claims.email.as_deref(), Some("user@corp.com"));
        assert_eq!(claims.hosted_domain.as_deref(), Some("corp.com"));
    }

    #[test]
    fn test_extract_claims_email_only() {
        let header = base64_url_encode(r#"{"alg":"RS256"}"#);
        let payload = base64_url_encode(r#"{"email":"user@gmail.com"}"#);
        let token = format!("{header}.{payload}.sig");

        let claims = extract_claims_from_id_token(Some(&token));
        assert_eq!(claims.email.as_deref(), Some("user@gmail.com"));
        assert!(claims.hosted_domain.is_none());
    }

    #[test]
    fn test_extract_claims_none() {
        let claims = extract_claims_from_id_token(None);
        assert!(claims.email.is_none());
        assert!(claims.hosted_domain.is_none());
    }

    #[test]
    fn test_extract_claims_empty() {
        let claims = extract_claims_from_id_token(Some(""));
        assert!(claims.email.is_none());
    }

    #[test]
    fn test_extract_claims_invalid_parts() {
        let claims = extract_claims_from_id_token(Some("not-a-jwt"));
        assert!(claims.email.is_none());
    }

    #[test]
    fn test_extract_claims_invalid_base64() {
        let claims = extract_claims_from_id_token(Some("header.!!!invalid!!!.sig"));
        assert!(claims.email.is_none());
    }

    // --- Base64 URL decode ---

    #[test]
    fn test_base64_url_decode_simple() {
        let decoded = base64_url_decode("dGVzdA");
        assert_eq!(decoded, Some(b"test".to_vec()));
    }

    #[test]
    fn test_base64_url_decode_with_padding_chars() {
        // "test" with explicit padding stripped
        let decoded = base64_url_decode("dGVzdA");
        assert!(decoded.is_some());
        assert_eq!(
            String::from_utf8(decoded.expect("decoded")).ok(),
            Some("test".to_string())
        );
    }

    // --- Project ID extraction from Code Assist ---

    #[test]
    fn test_extract_project_id_string() {
        let json: Value = serde_json::json!({
            "cloudaicompanionProject": "gen-lang-client-12345"
        });
        assert_eq!(
            extract_project_id_from_code_assist(&json),
            Some("gen-lang-client-12345".to_string())
        );
    }

    #[test]
    fn test_extract_project_id_object_with_id() {
        let json: Value = serde_json::json!({
            "cloudaicompanionProject": { "id": "gen-lang-client-99" }
        });
        assert_eq!(
            extract_project_id_from_code_assist(&json),
            Some("gen-lang-client-99".to_string())
        );
    }

    #[test]
    fn test_extract_project_id_object_with_project_id() {
        let json: Value = serde_json::json!({
            "cloudaicompanionProject": { "projectId": "gen-lang-client-42" }
        });
        assert_eq!(
            extract_project_id_from_code_assist(&json),
            Some("gen-lang-client-42".to_string())
        );
    }

    #[test]
    fn test_extract_project_id_empty_string() {
        let json: Value = serde_json::json!({
            "cloudaicompanionProject": "   "
        });
        assert_eq!(extract_project_id_from_code_assist(&json), None);
    }

    #[test]
    fn test_extract_project_id_missing() {
        let json: Value = serde_json::json!({});
        assert_eq!(extract_project_id_from_code_assist(&json), None);
    }

    #[test]
    fn test_extract_project_id_null() {
        let json: Value = serde_json::json!({
            "cloudaicompanionProject": null
        });
        assert_eq!(extract_project_id_from_code_assist(&json), None);
    }

    // --- Plan detection ---

    #[test]
    fn test_determine_plan_standard() {
        assert_eq!(
            determine_plan(Some(&GeminiTier::Standard), None),
            Some("Paid".to_string())
        );
    }

    #[test]
    fn test_determine_plan_free_with_domain() {
        assert_eq!(
            determine_plan(Some(&GeminiTier::Free), Some("corp.com")),
            Some("Workspace".to_string())
        );
    }

    #[test]
    fn test_determine_plan_free_personal() {
        assert_eq!(
            determine_plan(Some(&GeminiTier::Free), None),
            Some("Free".to_string())
        );
    }

    #[test]
    fn test_determine_plan_legacy() {
        assert_eq!(
            determine_plan(Some(&GeminiTier::Legacy), None),
            Some("Legacy".to_string())
        );
    }

    #[test]
    fn test_determine_plan_none() {
        assert_eq!(determine_plan(None, None), None);
    }

    #[test]
    fn test_determine_plan_standard_with_domain() {
        assert_eq!(
            determine_plan(Some(&GeminiTier::Standard), Some("corp.com")),
            Some("Paid".to_string())
        );
    }

    // --- ISO 8601 datetime parsing ---

    #[test]
    fn test_parse_iso8601_valid() {
        let result = parse_iso8601_datetime("2026-01-28T15:00:00Z");
        assert!(result.is_some());
        let date = result.expect("parsed");
        assert_eq!(date.year(), 2026);
        assert_eq!(date.month() as u8, 1);
        assert_eq!(date.day(), 28);
    }

    #[test]
    fn test_parse_iso8601_with_fractional_seconds() {
        let result = parse_iso8601_datetime("2026-01-28T15:00:00.123Z");
        assert!(result.is_some());
    }

    #[test]
    fn test_parse_iso8601_empty() {
        assert!(parse_iso8601_datetime("").is_none());
    }

    #[test]
    fn test_parse_iso8601_invalid() {
        assert!(parse_iso8601_datetime("not-a-date").is_none());
    }

    // --- Reset time formatting ---

    #[test]
    fn test_format_reset_time_future() {
        let future =
            OffsetDateTime::now_utc() + time::Duration::hours(3) + time::Duration::minutes(15);
        let iso = future
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format");
        let result = format_reset_time(&iso);
        assert!(result.starts_with("Resets in 3h"));
    }

    #[test]
    fn test_format_reset_time_past() {
        let past = OffsetDateTime::now_utc() - time::Duration::hours(1);
        let iso = past
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format");
        assert_eq!(format_reset_time(&iso), "Resets soon");
    }

    #[test]
    fn test_format_reset_time_minutes_only() {
        let future = OffsetDateTime::now_utc() + time::Duration::minutes(45);
        let iso = future
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format");
        let result = format_reset_time(&iso);
        assert!(result.starts_with("Resets in "));
        assert!(result.contains('m'));
        assert!(!result.contains('h'));
    }

    #[test]
    fn test_format_reset_time_invalid() {
        assert_eq!(format_reset_time("not-a-date"), "Resets soon");
    }

    // --- JS constant extraction ---

    #[test]
    fn test_extract_js_string_constant_single_quotes() {
        let content = "const OAUTH_CLIENT_ID = '123456.apps.googleusercontent.com';";
        assert_eq!(
            extract_js_string_constant(content, "OAUTH_CLIENT_ID"),
            Some("123456.apps.googleusercontent.com".to_string())
        );
    }

    #[test]
    fn test_extract_js_string_constant_double_quotes() {
        let content = r#"const OAUTH_CLIENT_SECRET = "GOCSPX-secret-value";"#;
        assert_eq!(
            extract_js_string_constant(content, "OAUTH_CLIENT_SECRET"),
            Some("GOCSPX-secret-value".to_string())
        );
    }

    #[test]
    fn test_extract_js_string_constant_with_spaces() {
        let content = "const OAUTH_CLIENT_ID   =   'my-client-id' ;";
        assert_eq!(
            extract_js_string_constant(content, "OAUTH_CLIENT_ID"),
            Some("my-client-id".to_string())
        );
    }

    #[test]
    fn test_extract_js_string_constant_empty_value() {
        let content = "const OAUTH_CLIENT_ID = '';";
        assert_eq!(extract_js_string_constant(content, "OAUTH_CLIENT_ID"), None);
    }

    #[test]
    fn test_extract_js_string_constant_missing() {
        assert_eq!(extract_js_string_constant("nothing here", "MISSING"), None);
    }

    #[test]
    fn test_parse_oauth_credentials_from_source() {
        let content = r#"
            const OAUTH_CLIENT_ID = '123.apps.googleusercontent.com';
            const OAUTH_CLIENT_SECRET = 'GOCSPX-abcdef';
        "#;
        let creds = parse_oauth_credentials_from_source(content);
        assert!(creds.is_some());
        let creds = creds.expect("creds");
        assert_eq!(creds.client_id, "123.apps.googleusercontent.com");
        assert_eq!(creds.client_secret, "GOCSPX-abcdef");
    }

    #[test]
    fn test_parse_oauth_credentials_missing_secret() {
        let content = "const OAUTH_CLIENT_ID = 'id';";
        assert!(parse_oauth_credentials_from_source(content).is_none());
    }

    // --- Quota parsing (via fetch_quota internals) ---

    #[test]
    fn test_quota_bucket_deserialization() {
        let json = r#"{
            "buckets": [
                {
                    "remainingFraction": 0.6,
                    "resetTime": "2026-01-28T20:00:00Z",
                    "modelId": "gemini-2.5-pro",
                    "tokenType": "INPUT"
                },
                {
                    "remainingFraction": 0.9,
                    "resetTime": "2026-01-28T20:00:00Z",
                    "modelId": "gemini-2.5-flash",
                    "tokenType": "INPUT"
                }
            ]
        }"#;

        let response: QuotaResponse = serde_json::from_str(json).expect("parse");
        let buckets = response.buckets.expect("buckets");
        assert_eq!(buckets.len(), 2);
        assert_eq!(buckets[0].model_id.as_deref(), Some("gemini-2.5-pro"));
        assert_eq!(buckets[0].remaining_fraction, Some(0.6));
        assert_eq!(buckets[1].model_id.as_deref(), Some("gemini-2.5-flash"));
    }

    #[test]
    fn test_quota_response_empty_buckets() {
        let json = r#"{"buckets": []}"#;
        let response: QuotaResponse = serde_json::from_str(json).expect("parse");
        assert!(response.buckets.as_ref().expect("buckets").is_empty());
    }

    #[test]
    fn test_quota_response_no_buckets_field() {
        let json = r#"{}"#;
        let response: QuotaResponse = serde_json::from_str(json).expect("parse");
        assert!(response.buckets.is_none());
    }

    // --- Model grouping (lowest per model) ---

    #[test]
    fn test_model_grouping_keeps_lowest_fraction() {
        let buckets = vec![
            QuotaBucket {
                remaining_fraction: Some(0.8),
                reset_time: None,
                model_id: Some("gemini-2.5-pro".to_string()),
                token_type: Some("INPUT".to_string()),
            },
            QuotaBucket {
                remaining_fraction: Some(0.3),
                reset_time: Some("2026-01-28T22:00:00Z".to_string()),
                model_id: Some("gemini-2.5-pro".to_string()),
                token_type: Some("OUTPUT".to_string()),
            },
        ];

        let mut model_map: HashMap<String, (f64, Option<String>)> = HashMap::new();
        for bucket in &buckets {
            let model_id = bucket.model_id.clone().expect("model_id");
            let fraction = bucket.remaining_fraction.expect("fraction");
            let entry = model_map.entry(model_id);
            match entry {
                std::collections::hash_map::Entry::Occupied(mut occupied) => {
                    if fraction < occupied.get().0 {
                        occupied.insert((fraction, bucket.reset_time.clone()));
                    }
                }
                std::collections::hash_map::Entry::Vacant(vacant) => {
                    vacant.insert((fraction, bucket.reset_time.clone()));
                }
            }
        }

        assert_eq!(model_map.len(), 1);
        let (fraction, _) = model_map.get("gemini-2.5-pro").expect("pro");
        assert!((*fraction - 0.3).abs() < 0.01);
    }

    // --- Usage snapshot construction ---

    #[test]
    fn test_build_usage_snapshot_pro_and_flash() {
        let now = OffsetDateTime::now_utc();
        let quotas = vec![
            ModelQuota {
                model_id: "gemini-2.5-pro".to_string(),
                percent_left: 60.0,
                reset_time: None,
                reset_description: Some("Resets in 5h 30m".to_string()),
            },
            ModelQuota {
                model_id: "gemini-2.5-flash".to_string(),
                percent_left: 90.0,
                reset_time: None,
                reset_description: Some("Resets in 3h 10m".to_string()),
            },
        ];

        let snapshot = build_usage_snapshot(&quotas, Some("user@example.com"), Some("Paid"), now);

        let primary = snapshot.primary.as_ref().expect("primary");
        assert!((primary.used_percent - 40.0).abs() < 0.01);
        assert_eq!(primary.window_minutes, Some(1440));

        let secondary = snapshot.secondary.as_ref().expect("secondary");
        assert!((secondary.used_percent - 10.0).abs() < 0.01);
        assert_eq!(secondary.window_minutes, Some(1440));

        assert!(snapshot.tertiary.is_none());

        let identity = snapshot.identity.as_ref().expect("identity");
        assert_eq!(identity.account_email.as_deref(), Some("user@example.com"));
        assert_eq!(identity.login_method.as_deref(), Some("Paid"));
        assert_eq!(identity.provider_id.as_deref(), Some("gemini"));
    }

    #[test]
    fn test_build_usage_snapshot_pro_only() {
        let now = OffsetDateTime::now_utc();
        let quotas = vec![ModelQuota {
            model_id: "gemini-2.5-pro".to_string(),
            percent_left: 75.0,
            reset_time: None,
            reset_description: None,
        }];

        let snapshot = build_usage_snapshot(&quotas, None, None, now);

        assert!(snapshot.primary.is_some());
        assert!(snapshot.secondary.is_none());

        let primary = snapshot.primary.as_ref().expect("primary");
        assert!((primary.used_percent - 25.0).abs() < 0.01);
    }

    #[test]
    fn test_build_usage_snapshot_flash_only() {
        let now = OffsetDateTime::now_utc();
        let quotas = vec![ModelQuota {
            model_id: "gemini-2.5-flash".to_string(),
            percent_left: 50.0,
            reset_time: None,
            reset_description: None,
        }];

        let snapshot = build_usage_snapshot(&quotas, None, None, now);

        // Primary always exists (Swift parity), defaults to 0% used when no pro model
        let primary = snapshot.primary.as_ref().expect("primary");
        assert!(primary.used_percent.abs() < 0.01);
        assert!(snapshot.secondary.is_some());

        let secondary = snapshot.secondary.as_ref().expect("secondary");
        assert!((secondary.used_percent - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_build_usage_snapshot_no_matching_models() {
        let now = OffsetDateTime::now_utc();
        let quotas = vec![ModelQuota {
            model_id: "gemini-unknown-model".to_string(),
            percent_left: 80.0,
            reset_time: None,
            reset_description: None,
        }];

        let snapshot = build_usage_snapshot(&quotas, None, None, now);
        // Primary always exists (Swift parity), defaults to 0% used
        let primary = snapshot.primary.as_ref().expect("primary");
        assert!(primary.used_percent.abs() < 0.01);
        assert!(snapshot.secondary.is_none());
    }

    #[test]
    fn test_build_usage_snapshot_picks_lowest_pro() {
        let now = OffsetDateTime::now_utc();
        let quotas = vec![
            ModelQuota {
                model_id: "gemini-2.0-pro".to_string(),
                percent_left: 80.0,
                reset_time: None,
                reset_description: None,
            },
            ModelQuota {
                model_id: "gemini-2.5-pro".to_string(),
                percent_left: 30.0,
                reset_time: None,
                reset_description: None,
            },
        ];

        let snapshot = build_usage_snapshot(&quotas, None, None, now);
        let primary = snapshot.primary.as_ref().expect("primary");
        assert!((primary.used_percent - 70.0).abs() < 0.01);
    }

    #[test]
    fn test_build_usage_snapshot_boundary_values() {
        let now = OffsetDateTime::now_utc();

        // 0% left = 100% used
        let quotas = vec![ModelQuota {
            model_id: "gemini-2.5-pro".to_string(),
            percent_left: 0.0,
            reset_time: None,
            reset_description: None,
        }];
        let snapshot = build_usage_snapshot(&quotas, None, None, now);
        let primary = snapshot.primary.as_ref().expect("primary");
        assert!((primary.used_percent - 100.0).abs() < 0.01);

        // 100% left = 0% used
        let quotas = vec![ModelQuota {
            model_id: "gemini-2.5-pro".to_string(),
            percent_left: 100.0,
            reset_time: None,
            reset_description: None,
        }];
        let snapshot = build_usage_snapshot(&quotas, None, None, now);
        let primary = snapshot.primary.as_ref().expect("primary");
        assert!(primary.used_percent.abs() < 0.01);
    }

    #[test]
    fn test_build_usage_snapshot_empty_quotas_still_has_primary() {
        let now = OffsetDateTime::now_utc();
        let quotas: Vec<ModelQuota> = vec![];

        let snapshot = build_usage_snapshot(&quotas, Some("user@test.com"), Some("Free"), now);

        let primary = snapshot.primary.as_ref().expect("primary always present");
        assert!(primary.used_percent.abs() < 0.01);
        assert_eq!(primary.window_minutes, Some(1440));
        assert!(primary.resets_at.is_none());
        assert!(primary.reset_description.is_none());
        assert!(snapshot.secondary.is_none());
        assert!(snapshot.tertiary.is_none());

        let identity = snapshot.identity.as_ref().expect("identity");
        assert_eq!(identity.account_email.as_deref(), Some("user@test.com"));
        assert_eq!(identity.login_method.as_deref(), Some("Free"));
    }

    // --- Provider error helper ---

    #[test]
    fn test_provider_error() {
        let error = provider_error("test message");
        assert_eq!(error.code, 1);
        assert_eq!(error.message, "test message");
        assert_eq!(error.kind, Some("provider".to_string()));
    }

    // --- Base64 URL encode helper for tests ---

    fn base64_url_encode(input: &str) -> String {
        let encoded = base64_encode_standard(input.as_bytes());
        encoded
            .replace('+', "-")
            .replace('/', "_")
            .trim_end_matches('=')
            .to_string()
    }

    fn base64_encode_standard(input: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

        let mut output = String::with_capacity((input.len() + 2) / 3 * 4);
        let mut i = 0;

        while i + 2 < input.len() {
            let a = input[i];
            let b = input[i + 1];
            let c = input[i + 2];
            output.push(TABLE[(a >> 2) as usize] as char);
            output.push(TABLE[((a & 0x03) << 4 | b >> 4) as usize] as char);
            output.push(TABLE[((b & 0x0f) << 2 | c >> 6) as usize] as char);
            output.push(TABLE[(c & 0x3f) as usize] as char);
            i += 3;
        }

        let remaining = input.len() - i;
        if remaining == 1 {
            let a = input[i];
            output.push(TABLE[(a >> 2) as usize] as char);
            output.push(TABLE[((a & 0x03) << 4) as usize] as char);
            output.push('=');
            output.push('=');
        } else if remaining == 2 {
            let a = input[i];
            let b = input[i + 1];
            output.push(TABLE[(a >> 2) as usize] as char);
            output.push(TABLE[((a & 0x03) << 4 | b >> 4) as usize] as char);
            output.push(TABLE[((b & 0x0f) << 2) as usize] as char);
            output.push('=');
        }

        output
    }
}
