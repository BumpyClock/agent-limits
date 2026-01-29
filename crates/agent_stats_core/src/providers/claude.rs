use crate::config::ProviderSourceMode;
use crate::models::{
    ProviderCostSnapshot, ProviderErrorPayload, ProviderIdentitySnapshot, ProviderPayload,
    RateWindow, UsageSnapshot,
};
use crate::provider_metadata::ProviderId;
use crate::token_storage::{SecureTokenStorage, accounts};

use super::{Provider, ProviderFetchContext};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::Deserialize;
use std::io::{Read, Write};
use std::sync::{Arc, OnceLock, RwLock};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use time::OffsetDateTime;

/// Simple synchronous cached token entry with expiration.
struct CachedOAuthToken {
    access_token: String,
    expires_at: Option<Instant>,
}

/// 30-minute memory cache for Claude OAuth tokens.
/// Uses std::sync::RwLock for synchronous access within the Provider trait.
static OAUTH_TOKEN_CACHE: OnceLock<RwLock<Option<CachedOAuthToken>>> = OnceLock::new();

fn get_cached_token() -> Option<String> {
    let cache = OAUTH_TOKEN_CACHE.get_or_init(|| RwLock::new(None));
    let guard = cache.read().ok()?;
    guard.as_ref().and_then(|entry| {
        // Check if token is still valid (not expired)
        if let Some(expires_at) = entry.expires_at {
            if Instant::now() < expires_at {
                Some(entry.access_token.clone())
            } else {
                None
            }
        } else {
            // No expiration set, treat as valid
            Some(entry.access_token.clone())
        }
    })
}

fn set_cached_token(access_token: String, ttl: Duration) {
    let cache = OAUTH_TOKEN_CACHE.get_or_init(|| RwLock::new(None));
    if let Ok(mut guard) = cache.write() {
        *guard = Some(CachedOAuthToken {
            access_token,
            expires_at: Some(Instant::now() + ttl),
        });
    }
}

pub struct ClaudeProvider;

impl Provider for ClaudeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Claude
    }

    fn display_name(&self) -> &'static str {
        "Claude"
    }

    fn fetch(
        &self,
        context: &ProviderFetchContext,
    ) -> Result<ProviderPayload, ProviderErrorPayload> {
        let source_mode = context.source_mode();
        let result = match source_mode {
            ProviderSourceMode::Auto => fetch_auto(context),
            ProviderSourceMode::Oauth => fetch_via_oauth(context),
            ProviderSourceMode::Web => fetch_via_web(context),
            ProviderSourceMode::Cli => fetch_via_cli(),
            ProviderSourceMode::Api => {
                return Err(provider_error(
                    "Claude provider does not support the 'api' source mode. \
                     Use 'auto', 'oauth', 'web', or 'cli'.",
                ));
            }
        };

        let (usage_snapshot, source_name) = result?;

        Ok(ProviderPayload {
            provider: ProviderId::Claude.to_string(),
            account: usage_snapshot
                .identity
                .as_ref()
                .and_then(|identity| identity.account_email.clone()),
            version: detect_cli_version(),
            source: source_name.to_string(),
            status: None,
            usage: Some(usage_snapshot),
            credits: None,
            antigravity_plan_info: None,
            openai_dashboard: None,
            error: None,
        })
    }
}

fn fetch_auto(
    context: &ProviderFetchContext,
) -> Result<(UsageSnapshot, &'static str), ProviderErrorPayload> {
    if context.oauth_token_override.as_ref().is_some() {
        log::debug!("Claude: using OAuth override token from token accounts");
        return fetch_via_oauth(context);
    }

    // Priority 1: OAuth credentials with user:profile scope
    match load_oauth_credentials() {
        Ok(credentials) if credentials.has_usage_scope() && !credentials.is_expired() => {
            log::debug!("Claude: using OAuth credentials (user:profile scope present)");
            return fetch_via_oauth(context);
        }
        Ok(credentials) if credentials.is_expired() => {
            log::debug!("Claude: OAuth credentials found but expired, falling back");
        }
        Ok(_) => {
            log::debug!("Claude: OAuth credentials found but missing user:profile scope, falling back");
        }
        Err(_) => {
            log::debug!("Claude: no OAuth credentials found, falling back");
        }
    }

    // Priority 2: Web API via cookie header from config
    if let Some(cookie_header) = &context.provider_config.cookie_header {
        if extract_session_key(cookie_header).is_some() {
            log::debug!("Claude: manual cookie header found, trying web API");
            match fetch_via_web(context) {
                Ok(result) => return Ok(result),
                Err(error) => {
                    log::debug!("Claude: web API failed ({error:?}), falling back to CLI");
                }
            }
        }
    }

    // Priority 3: CLI fallback
    log::debug!("Claude: falling back to CLI");
    fetch_via_cli()
}

// ---------------------------------------------------------------------------
// OAuth strategy
// ---------------------------------------------------------------------------

const OAUTH_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA_HEADER: &str = "oauth-2025-04-20";

fn fetch_via_oauth(
    context: &ProviderFetchContext,
) -> Result<(UsageSnapshot, &'static str), ProviderErrorPayload> {
    let credentials = load_oauth_credentials_with_override(
        context.oauth_token_override.as_deref(),
    )
    .map_err(|error| provider_error(&error))?;

    if credentials.is_expired() {
        return Err(provider_error(
            "Claude OAuth token expired. Run `claude` to refresh.",
        ));
    }
    if !credentials.has_usage_scope() {
        let scopes = credentials.scopes.join(", ");
        return Err(provider_error(&format!(
            "Claude OAuth token missing 'user:profile' scope (has: {scopes}). \
             Run `claude setup-token` to re-generate credentials, or switch Claude source to CLI."
        )));
    }

    let response = ureq::get(OAUTH_USAGE_URL)
        .set("Authorization", &format!("Bearer {}", credentials.access_token))
        .set("Accept", "application/json")
        .set("Content-Type", "application/json")
        .set("anthropic-beta", OAUTH_BETA_HEADER)
        .set("User-Agent", "AgentStats")
        .call();

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(401, _)) => {
            return Err(provider_error(
                "Claude OAuth request unauthorized. Run `claude` to re-authenticate.",
            ));
        }
        Err(ureq::Error::Status(403, response)) => {
            let body = response.into_string().unwrap_or_default();
            if body.contains("user:profile") {
                return Err(provider_error(
                    "Claude OAuth token does not meet scope requirement 'user:profile'. \
                     Run `claude setup-token` to re-generate credentials, or switch Claude source to CLI.",
                ));
            }
            return Err(provider_error(&format!(
                "Claude OAuth error: HTTP 403 – {}",
                truncate_body(&body, 400)
            )));
        }
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            return Err(provider_error(&format!(
                "Claude OAuth error: HTTP {code} – {}",
                truncate_body(&body, 400)
            )));
        }
        Err(error) => {
            return Err(provider_error(&format!(
                "Claude OAuth network error: {error}"
            )));
        }
    };

    let usage_response: OAuthUsageResponse = response.into_json().map_err(|error| {
        provider_error(&format!(
            "Failed to parse Claude OAuth usage response: {error}"
        ))
    })?;

    let snapshot = map_oauth_usage(&usage_response, &credentials)?;
    Ok((snapshot, "oauth"))
}

fn map_oauth_usage(
    usage: &OAuthUsageResponse,
    credentials: &OAuthCredentials,
) -> Result<UsageSnapshot, ProviderErrorPayload> {
    let primary = map_oauth_window(&usage.five_hour, Some(5 * 60)).ok_or_else(|| {
        provider_error("Claude OAuth: missing session (five_hour) data in usage response")
    })?;

    let secondary = map_oauth_window(&usage.seven_day, Some(7 * 24 * 60));
    let tertiary_source = if usage.seven_day_sonnet.is_some() {
        &usage.seven_day_sonnet
    } else {
        &usage.seven_day_opus
    };
    let tertiary = map_oauth_window(tertiary_source, Some(7 * 24 * 60));

    let login_method = infer_plan_from_tier(credentials.rate_limit_tier.as_deref());
    let provider_cost = map_oauth_extra_usage(&usage.extra_usage, login_method.as_deref());

    let identity = ProviderIdentitySnapshot {
        provider_id: Some(ProviderId::Claude.to_string()),
        account_email: None,
        account_organization: None,
        login_method,
    };

    Ok(UsageSnapshot {
        primary: Some(primary),
        secondary,
        tertiary,
        provider_cost,
        updated_at: OffsetDateTime::now_utc(),
        identity: Some(identity),
    })
}

fn map_oauth_window(
    window: &Option<OAuthUsageWindow>,
    window_minutes: Option<i64>,
) -> Option<RateWindow> {
    let window = window.as_ref()?;
    let utilization = window.utilization?;
    let resets_at = window
        .resets_at
        .as_deref()
        .and_then(parse_iso8601_datetime);
    let reset_description = resets_at.as_ref().map(format_reset_date);

    Some(RateWindow {
        used_percent: utilization,
        window_minutes,
        resets_at,
        reset_description,
    })
}

fn map_oauth_extra_usage(
    extra: &Option<OAuthExtraUsage>,
    login_method: Option<&str>,
) -> Option<ProviderCostSnapshot> {
    let extra = extra.as_ref()?;
    if extra.is_enabled != Some(true) {
        return None;
    }
    let used_cents = extra.used_credits?;
    let limit_cents = extra.monthly_limit?;

    let used = used_cents / 100.0;
    let limit = limit_cents / 100.0;

    let currency = extra
        .currency
        .as_deref()
        .map(|currency| currency.trim())
        .filter(|currency| !currency.is_empty())
        .unwrap_or("USD");

    let cost = ProviderCostSnapshot {
        used,
        limit,
        currency_code: currency.to_string(),
        period: Some("Monthly".to_string()),
        resets_at: None,
        updated_at: OffsetDateTime::now_utc(),
    };

    rescale_extra_usage_cost_if_needed(cost, login_method)
}

fn rescale_extra_usage_cost_if_needed(
    cost: ProviderCostSnapshot,
    login_method: Option<&str>,
) -> Option<ProviderCostSnapshot> {
    let normalized = login_method
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if normalized.contains("enterprise") {
        return Some(cost);
    }
    // Non-enterprise plans sometimes report values 100x too high
    if cost.limit >= 1000.0 {
        return Some(ProviderCostSnapshot {
            used: cost.used / 100.0,
            limit: cost.limit / 100.0,
            currency_code: cost.currency_code,
            period: cost.period,
            resets_at: cost.resets_at,
            updated_at: cost.updated_at,
        });
    }
    Some(cost)
}

fn infer_plan_from_tier(tier: Option<&str>) -> Option<String> {
    let tier = tier?.to_lowercase();
    if tier.contains("max") {
        return Some("Claude Max".to_string());
    }
    if tier.contains("pro") {
        return Some("Claude Pro".to_string());
    }
    if tier.contains("team") {
        return Some("Claude Team".to_string());
    }
    if tier.contains("enterprise") {
        return Some("Claude Enterprise".to_string());
    }
    None
}

// ---------------------------------------------------------------------------
// OAuth credentials loading
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<CredentialsOAuth>,
}

#[derive(Debug, Deserialize)]
struct CredentialsOAuth {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    #[serde(rename = "refreshToken")]
    refresh_token: Option<String>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<f64>,
    scopes: Option<Vec<String>>,
    #[serde(rename = "rateLimitTier")]
    rate_limit_tier: Option<String>,
}

struct OAuthCredentials {
    access_token: String,
    #[allow(dead_code)]
    refresh_token: Option<String>,
    expires_at: Option<OffsetDateTime>,
    scopes: Vec<String>,
    rate_limit_tier: Option<String>,
    from_environment: bool,
}

impl OAuthCredentials {
    fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(expires_at) => OffsetDateTime::now_utc() >= expires_at,
            None => !self.from_environment,
        }
    }

    fn has_usage_scope(&self) -> bool {
        self.scopes.iter().any(|scope| scope == "user:profile")
    }
}

fn load_oauth_credentials_with_override(
    override_token: Option<&str>,
) -> Result<OAuthCredentials, String> {
    if let Some(raw_token) = override_token {
        let token = raw_token.trim();
        if !token.is_empty() {
            return Ok(OAuthCredentials {
                access_token: token.to_string(),
                refresh_token: None,
                expires_at: None,
                scopes: vec!["user:profile".to_string()],
                rate_limit_tier: None,
                from_environment: true,
            });
        }
    }

    load_oauth_credentials()
}

fn load_oauth_credentials() -> Result<OAuthCredentials, String> {
    // Priority 1: Check memory cache first (fastest)
    if let Some(cached_token) = get_cached_token() {
        return Ok(OAuthCredentials {
            access_token: cached_token,
            refresh_token: None,
            expires_at: None,
            scopes: vec!["user:profile".to_string()],
            rate_limit_tier: None,
            from_environment: true,
        });
    }

    // Priority 2: Check environment variable override
    if let Ok(token) = std::env::var("CODEXBAR_CLAUDE_OAUTH_TOKEN") {
        let token = token.trim().to_string();
        if !token.is_empty() {
            // Store in cache for future requests
            set_cached_token(token.clone(), Duration::from_secs(30 * 60));

            let scopes = std::env::var("CODEXBAR_CLAUDE_OAUTH_SCOPES")
                .ok()
                .map(|raw| {
                    raw.split(',')
                        .map(|scope| scope.trim().to_string())
                        .filter(|scope| !scope.is_empty())
                        .collect::<Vec<_>>()
                })
                .filter(|scopes| !scopes.is_empty())
                .unwrap_or_else(|| vec!["user:profile".to_string()]);

            return Ok(OAuthCredentials {
                access_token: token,
                refresh_token: None,
                expires_at: None,
                scopes,
                rate_limit_tier: None,
                from_environment: true,
            });
        }
    }

    // Priority 3: Check secure storage
    let storage = SecureTokenStorage::default();
    if storage.is_available() {
        if let Ok(stored_token) = storage.retrieve(accounts::CLAUDE) {
            let token = stored_token.trim().to_string();
            if !token.is_empty() {
                // Store in cache for faster future access
                set_cached_token(token.clone(), Duration::from_secs(30 * 60));

                return Ok(OAuthCredentials {
                    access_token: token,
                    refresh_token: None,
                    expires_at: None,
                    scopes: vec!["user:profile".to_string()],
                    rate_limit_tier: None,
                    from_environment: true,
                });
            }
        }
    }

    // Priority 4: Try reading the credentials file
    let home = dirs::home_dir().ok_or("Could not resolve home directory")?;
    let credentials_path = home.join(".claude").join(".credentials.json");

    if !credentials_path.exists() {
        return Err(
            "Claude OAuth credentials not found. Run `claude` to authenticate.".to_string(),
        );
    }

    let contents = std::fs::read_to_string(&credentials_path).map_err(|error| {
        format!(
            "Failed to read Claude credentials at {}: {error}",
            credentials_path.display()
        )
    })?;

    let file: CredentialsFile = serde_json::from_str(&contents).map_err(|error| {
        format!("Claude OAuth credentials file is invalid JSON: {error}")
    })?;

    let oauth = file
        .claude_ai_oauth
        .ok_or("Claude OAuth credentials missing. Run `claude` to authenticate.")?;

    let access_token = oauth
        .access_token
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
        .ok_or("Claude OAuth access token missing. Run `claude` to authenticate.")?;

    let expires_at = oauth.expires_at.and_then(|millis| {
        OffsetDateTime::from_unix_timestamp((millis / 1000.0) as i64).ok()
    });

    // Store in memory cache for faster future access (30 min TTL)
    set_cached_token(access_token.clone(), Duration::from_secs(30 * 60));

    // Store in secure storage for persistence across restarts
    let storage = SecureTokenStorage::default();
    if storage.is_available() {
        if let Err(error) = storage.store(accounts::CLAUDE, &access_token) {
            log::warn!("Failed to store Claude OAuth token in secure storage: {error}");
        }
    }

    Ok(OAuthCredentials {
        access_token,
        refresh_token: oauth.refresh_token,
        expires_at,
        scopes: oauth.scopes.unwrap_or_default(),
        rate_limit_tier: oauth.rate_limit_tier,
        from_environment: false,
    })
}

// ---------------------------------------------------------------------------
// Web API strategy (cookie-based, claude.ai endpoints)
// ---------------------------------------------------------------------------

const WEB_API_BASE: &str = "https://claude.ai/api";

fn fetch_via_web(
    context: &ProviderFetchContext,
) -> Result<(UsageSnapshot, &'static str), ProviderErrorPayload> {
    let cookie_header = context
        .provider_config
        .cookie_header
        .as_deref()
        .unwrap_or("");

    let session_key = extract_session_key(cookie_header).ok_or_else(|| {
        provider_error(
            "No valid Claude session key found. Provide a Cookie header containing \
             'sessionKey=sk-ant-...' in the provider config.",
        )
    })?;

    let org = fetch_web_organization(&session_key)?;

    let usage = fetch_web_usage(&org.id, &session_key)?;

    let extra_cost = fetch_web_extra_usage_cost(&org.id, &session_key, usage.login_method.as_deref());

    let account = fetch_web_account_info(&session_key, Some(&org.id));

    let account_email = account
        .as_ref()
        .and_then(|account| account.email.clone())
        .or(usage.account_email);
    let login_method = account
        .as_ref()
        .and_then(|account| account.login_method.clone())
        .or(usage.login_method);
    let account_organization = usage
        .account_organization
        .or_else(|| org.name.clone());

    let identity = ProviderIdentitySnapshot {
        provider_id: Some(ProviderId::Claude.to_string()),
        account_email,
        account_organization,
        login_method,
    };

    Ok((
        UsageSnapshot {
            primary: Some(usage.primary),
            secondary: usage.secondary,
            tertiary: usage.tertiary,
            provider_cost: extra_cost.or(usage.provider_cost),
            updated_at: OffsetDateTime::now_utc(),
            identity: Some(identity),
        },
        "web",
    ))
}

fn extract_session_key(cookie_header: &str) -> Option<String> {
    for segment in cookie_header.split(';') {
        let segment = segment.trim();
        if let Some(value) = segment.strip_prefix("sessionKey=") {
            let value = value.trim();
            if value.starts_with("sk-ant-") {
                return Some(value.to_string());
            }
        }
    }
    // Also accept a bare token starting with sk-ant- (no cookie-name prefix)
    let trimmed = cookie_header.trim();
    if trimmed.starts_with("sk-ant-") && !trimmed.contains(';') && !trimmed.contains('=') {
        return Some(trimmed.to_string());
    }
    None
}

#[derive(Debug)]
struct WebOrganizationInfo {
    id: String,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WebOrganizationResponse {
    uuid: String,
    name: Option<String>,
    capabilities: Option<Vec<String>>,
}

impl WebOrganizationResponse {
    fn has_chat_capability(&self) -> bool {
        self.capabilities
            .as_ref()
            .map(|capabilities| {
                capabilities
                    .iter()
                    .any(|capability| capability.eq_ignore_ascii_case("chat"))
            })
            .unwrap_or(false)
    }

    fn is_api_only(&self) -> bool {
        match &self.capabilities {
            Some(capabilities) if !capabilities.is_empty() => {
                capabilities.len() == 1
                    && capabilities
                        .first()
                        .map(|capability| capability.eq_ignore_ascii_case("api"))
                        .unwrap_or(false)
            }
            _ => false,
        }
    }
}

fn fetch_web_organization(session_key: &str) -> Result<WebOrganizationInfo, ProviderErrorPayload> {
    let url = format!("{WEB_API_BASE}/organizations");
    let cookie = format!("sessionKey={session_key}");

    let response = ureq::get(&url)
        .set("Cookie", &cookie)
        .set("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(15))
        .call();

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(401 | 403, _)) => {
            return Err(provider_error(
                "Unauthorized. Your Claude session may have expired. \
                 Update the cookie header in provider config.",
            ));
        }
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            return Err(provider_error(&format!(
                "Claude web API organizations error: HTTP {code} – {}",
                truncate_body(&body, 400)
            )));
        }
        Err(error) => {
            return Err(provider_error(&format!(
                "Claude web API organizations network error: {error}"
            )));
        }
    };

    let organizations: Vec<WebOrganizationResponse> = response.into_json().map_err(|error| {
        provider_error(&format!(
            "Failed to parse Claude organizations response: {error}"
        ))
    })?;

    let selected = organizations
        .iter()
        .find(|org| org.has_chat_capability())
        .or_else(|| organizations.iter().find(|org| !org.is_api_only()))
        .or(organizations.first())
        .ok_or_else(|| {
            provider_error("No Claude organization found for this account.")
        })?;

    let name = selected
        .name
        .as_deref()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string());

    Ok(WebOrganizationInfo {
        id: selected.uuid.clone(),
        name,
    })
}

struct WebUsageData {
    primary: RateWindow,
    secondary: Option<RateWindow>,
    tertiary: Option<RateWindow>,
    provider_cost: Option<ProviderCostSnapshot>,
    account_email: Option<String>,
    account_organization: Option<String>,
    login_method: Option<String>,
}

fn fetch_web_usage(
    org_id: &str,
    session_key: &str,
) -> Result<WebUsageData, ProviderErrorPayload> {
    let url = format!("{WEB_API_BASE}/organizations/{org_id}/usage");
    let cookie = format!("sessionKey={session_key}");

    let response = ureq::get(&url)
        .set("Cookie", &cookie)
        .set("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(15))
        .call();

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(401 | 403, _)) => {
            return Err(provider_error(
                "Unauthorized. Your Claude session may have expired.",
            ));
        }
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            return Err(provider_error(&format!(
                "Claude web API usage error: HTTP {code} – {}",
                truncate_body(&body, 400)
            )));
        }
        Err(error) => {
            return Err(provider_error(&format!(
                "Claude web API usage network error: {error}"
            )));
        }
    };

    let json: serde_json::Value = response.into_json().map_err(|error| {
        provider_error(&format!(
            "Failed to parse Claude web usage response: {error}"
        ))
    })?;

    parse_web_usage_response(&json)
}

fn parse_web_usage_response(
    json: &serde_json::Value,
) -> Result<WebUsageData, ProviderErrorPayload> {
    let session_percent = json
        .get("five_hour")
        .and_then(|window| window.get("utilization"))
        .and_then(|value| value.as_f64())
        .or_else(|| {
            json.get("five_hour")
                .and_then(|window| window.get("utilization"))
                .and_then(|value| value.as_i64())
                .map(|value| value as f64)
        });

    let session_percent = session_percent.ok_or_else(|| {
        provider_error(
            "Claude web API: missing session (five_hour) utilization data. \
             The API response format may have changed.",
        )
    })?;

    let session_resets_at = json
        .get("five_hour")
        .and_then(|window| window.get("resets_at"))
        .and_then(|value| value.as_str())
        .and_then(parse_iso8601_datetime);

    let primary = RateWindow {
        used_percent: session_percent,
        window_minutes: Some(5 * 60),
        resets_at: session_resets_at,
        reset_description: session_resets_at.as_ref().map(format_reset_date),
    };

    let secondary = parse_web_window(json, "seven_day", Some(7 * 24 * 60));
    let tertiary = parse_web_window(json, "seven_day_sonnet", Some(7 * 24 * 60))
        .or_else(|| parse_web_window(json, "seven_day_opus", Some(7 * 24 * 60)));

    Ok(WebUsageData {
        primary,
        secondary,
        tertiary,
        provider_cost: None,
        account_email: None,
        account_organization: None,
        login_method: None,
    })
}

fn parse_web_window(
    json: &serde_json::Value,
    key: &str,
    window_minutes: Option<i64>,
) -> Option<RateWindow> {
    let window = json.get(key)?;
    let utilization = window
        .get("utilization")
        .and_then(|value| value.as_f64().or_else(|| value.as_i64().map(|value| value as f64)))?;

    let resets_at = window
        .get("resets_at")
        .and_then(|value| value.as_str())
        .and_then(parse_iso8601_datetime);

    Some(RateWindow {
        used_percent: utilization,
        window_minutes,
        resets_at,
        reset_description: resets_at.as_ref().map(format_reset_date),
    })
}

#[derive(Debug, Deserialize)]
struct WebOverageSpendLimitResponse {
    #[serde(rename = "monthly_credit_limit")]
    monthly_credit_limit: Option<f64>,
    currency: Option<String>,
    #[serde(rename = "used_credits")]
    used_credits: Option<f64>,
    #[serde(rename = "is_enabled")]
    is_enabled: Option<bool>,
}

fn fetch_web_extra_usage_cost(
    org_id: &str,
    session_key: &str,
    login_method: Option<&str>,
) -> Option<ProviderCostSnapshot> {
    let url = format!("{WEB_API_BASE}/organizations/{org_id}/overage_spend_limit");
    let cookie = format!("sessionKey={session_key}");

    let response = ureq::get(&url)
        .set("Cookie", &cookie)
        .set("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(15))
        .call()
        .ok()?;

    let decoded: WebOverageSpendLimitResponse = response.into_json().ok()?;

    if decoded.is_enabled != Some(true) {
        return None;
    }

    let used_cents = decoded.used_credits?;
    let limit_cents = decoded.monthly_credit_limit?;
    let currency = decoded
        .currency
        .as_deref()
        .map(|currency| currency.trim())
        .filter(|currency| !currency.is_empty())
        .unwrap_or("USD");

    let used = used_cents / 100.0;
    let limit = limit_cents / 100.0;

    let cost = ProviderCostSnapshot {
        used,
        limit,
        currency_code: currency.to_string(),
        period: Some("Monthly".to_string()),
        resets_at: None,
        updated_at: OffsetDateTime::now_utc(),
    };

    rescale_extra_usage_cost_if_needed(cost, login_method)
}

struct WebAccountInfo {
    email: Option<String>,
    login_method: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WebAccountResponse {
    #[serde(rename = "email_address")]
    email_address: Option<String>,
    memberships: Option<Vec<WebAccountMembership>>,
}

#[derive(Debug, Deserialize)]
struct WebAccountMembership {
    organization: WebAccountOrganization,
}

#[derive(Debug, Deserialize)]
struct WebAccountOrganization {
    uuid: Option<String>,
    #[allow(dead_code)]
    name: Option<String>,
    #[serde(rename = "rate_limit_tier")]
    rate_limit_tier: Option<String>,
    #[serde(rename = "billing_type")]
    billing_type: Option<String>,
}

fn fetch_web_account_info(session_key: &str, org_id: Option<&str>) -> Option<WebAccountInfo> {
    let url = format!("{WEB_API_BASE}/account");
    let cookie = format!("sessionKey={session_key}");

    let response = ureq::get(&url)
        .set("Cookie", &cookie)
        .set("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(15))
        .call()
        .ok()?;

    let account: WebAccountResponse = response.into_json().ok()?;

    let email = account
        .email_address
        .as_deref()
        .map(|email| email.trim())
        .filter(|email| !email.is_empty())
        .map(|email| email.to_string());

    let membership = select_membership(&account.memberships, org_id);
    let login_method = membership.and_then(|membership| {
        infer_plan_from_web_account(
            membership.organization.rate_limit_tier.as_deref(),
            membership.organization.billing_type.as_deref(),
        )
    });

    Some(WebAccountInfo {
        email,
        login_method,
    })
}

fn select_membership<'a>(
    memberships: &'a Option<Vec<WebAccountMembership>>,
    org_id: Option<&str>,
) -> Option<&'a WebAccountMembership> {
    let memberships = memberships.as_ref()?;
    if memberships.is_empty() {
        return None;
    }
    if let Some(org_id) = org_id {
        if let Some(matched) = memberships
            .iter()
            .find(|membership| membership.organization.uuid.as_deref() == Some(org_id))
        {
            return Some(matched);
        }
    }
    memberships.first()
}

fn infer_plan_from_web_account(
    rate_limit_tier: Option<&str>,
    billing_type: Option<&str>,
) -> Option<String> {
    let tier = rate_limit_tier.unwrap_or("").to_lowercase();
    let billing = billing_type.unwrap_or("").to_lowercase();

    if tier.contains("max") {
        return Some("Claude Max".to_string());
    }
    if tier.contains("pro") {
        return Some("Claude Pro".to_string());
    }
    if tier.contains("team") {
        return Some("Claude Team".to_string());
    }
    if tier.contains("enterprise") {
        return Some("Claude Enterprise".to_string());
    }
    if billing.contains("stripe") && tier.contains("claude") {
        return Some("Claude Pro".to_string());
    }
    None
}

// ---------------------------------------------------------------------------
// CLI fallback strategy
// ---------------------------------------------------------------------------

fn fetch_via_cli() -> Result<(UsageSnapshot, &'static str), ProviderErrorPayload> {
    let claude_binary = resolve_claude_binary().ok_or_else(|| {
        let diagnostics = format_cli_environment_diagnostics(resolve_login_shell_path().as_deref());
        provider_error(
            &format!(
                "Claude CLI is not installed or not on PATH. \
                 Install it from https://docs.anthropic.com/en/docs/claude-code.{diagnostics}"
            ),
        )
    })?;

    let usage_output = run_claude_pty_usage(&claude_binary)
        .map_err(|error| provider_error(&format!("Claude CLI usage probe failed: {error}")))?;

    let clean = strip_ansi_codes(&usage_output);

    if let Some(cli_error) = detect_cli_error(&clean) {
        return Err(provider_error(&cli_error));
    }

    let snapshot = parse_cli_usage(&clean)?;
    Ok((snapshot, "cli"))
}

fn resolve_claude_binary() -> Option<String> {
    // Check CLAUDE_BINARY environment variable
    if let Ok(binary) = std::env::var("CLAUDE_BINARY") {
        let binary = binary.trim().to_string();
        if !binary.is_empty() {
            return Some(binary);
        }
    }

    // Use `which` crate to locate the binary on PATH
    if let Some(path) = which::which("claude")
        .ok()
        .and_then(|path| path.to_str().map(|path| path.to_string()))
    {
        return Some(path);
    }

    if let Some(path) = resolve_claude_binary_from_login_shell() {
        return Some(path);
    }

    resolve_claude_binary_from_common_paths()
}

fn resolve_claude_binary_from_login_shell() -> Option<String> {
    run_login_shell_command("command -v claude")
}

fn resolve_login_shell_path() -> Option<String> {
    run_login_shell_command("echo $PATH")
}

fn resolve_claude_binary_from_common_paths() -> Option<String> {
    const COMMON_CLAUDE_PATHS: &[&str] = &["/opt/homebrew/bin/claude", "/usr/local/bin/claude"];
    for candidate in COMMON_CLAUDE_PATHS {
        if std::path::Path::new(candidate).is_file() {
            return Some((*candidate).to_string());
        }
    }
    None
}

fn run_login_shell_command(command: &str) -> Option<String> {
    let output = std::process::Command::new("/bin/zsh")
        .args(["-lc", command])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let trimmed = stdout.lines().next().unwrap_or("").trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn run_claude_pty_usage(binary: &str) -> Result<String, String> {
    let output = run_claude_pty_usage_with_args(binary, &["--allowed-tools", ""]);
    match output {
        Ok(value) => Ok(value),
        Err(error) => {
            if is_cli_unknown_option_allowed_tools(&error) {
                run_claude_pty_usage_with_args(binary, &[])
            } else {
                Err(error)
            }
        }
    }
}

#[derive(Debug, Clone)]
enum ClaudePtyStopReason {
    Timeout,
    StopNeedle(String),
    ProcessExit,
    ChannelClosed,
}

#[derive(Debug, Clone, Copy)]
struct ClaudePtyPromptInfo {
    prompt_seen: bool,
    prompt_wait_ms: u128,
}

fn run_claude_pty_usage_with_args(binary: &str, extra_args: &[&str]) -> Result<String, String> {
    let pty_system = native_pty_system();
    let pty_pair = pty_system
        .openpty(PtySize {
            rows: 50,
            cols: 160,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("Failed to open Claude PTY: {error}"))?;

    let working_dir = claude_probe_working_directory();
    if claude_cli_debug_enabled() {
        log::debug!(
            "Claude PTY starting (binary={binary}, args={:?}, cwd={})",
            extra_args,
            working_dir.display()
        );
    }
    let mut command = CommandBuilder::new(binary);
    command.args(extra_args);
    command.cwd(&working_dir);

    for (key, value) in build_claude_cli_environment(&working_dir) {
        command.env(key, value);
    }

    let mut child = pty_pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("Failed to launch Claude CLI session: {error}"))?;
    drop(pty_pair.slave);

    let mut reader = pty_pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("Failed to capture Claude CLI output: {error}"))?;
    let mut writer = pty_pair
        .master
        .take_writer()
        .map_err(|error| format!("Failed to send Claude CLI input: {error}"))?;

    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let reader_thread = thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    if tx.send(buffer[..count].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let boot_delay = Duration::from_millis(400);
    thread::sleep(boot_delay);
    while rx.try_recv().is_ok() {}

    let stop_needles = [
        "Current session",
        "Failed to load usage data",
        "failed to load usage data",
    ];
    let send_on_substrings = [
        ("Do you trust the files in this folder?", "y\r"),
        ("Ready to code here?", "\r"),
        ("Press Enter to continue", "\r"),
        ("Show plan usage limits", "\r"),
        ("Show Claude Code status", "\r"),
    ];

    let stop_needles: Vec<Vec<u8>> = stop_needles.iter().map(|item| item.as_bytes().to_vec()).collect();
    let send_on_substrings: Vec<(Vec<u8>, Vec<u8>)> = send_on_substrings
        .iter()
        .map(|(needle, response)| (needle.as_bytes().to_vec(), response.as_bytes().to_vec()))
        .collect();
    let max_needle = stop_needles
        .iter()
        .map(|item| item.len())
        .chain(send_on_substrings.iter().map(|item| item.0.len()))
        .max()
        .unwrap_or(1);

    let timeout = Duration::from_secs(20);
    let settle_after_stop = Duration::from_secs(2);
    let send_enter_every = Duration::from_millis(800);
    let cursor_query = [0x1B, 0x5B, 0x36, 0x6E];
    let cursor_reply = [0x1B, 0x5B, 0x31, 0x3B, 0x31, 0x52];
    let mut next_cursor_check = Instant::now();
    let prompt_needles = [
        "❯",
        "Try \"",
        "bypass permissions on",
    ];
    let prompt_needles: Vec<Vec<u8>> = prompt_needles
        .iter()
        .map(|needle| needle.as_bytes().to_vec())
        .collect();

    let start = Instant::now();
    let mut last_enter = Instant::now();
    let mut buffer: Vec<u8> = Vec::new();
    let mut rolling = RollingBuffer::new(max_needle);
    let mut triggered_send = vec![false; send_on_substrings.len()];
    let mut stopped_early = false;
    let mut stop_reason = ClaudePtyStopReason::Timeout;

    let prompt_start = Instant::now();
    let prompt_deadline = prompt_start + Duration::from_secs(6);
    let mut prompt_seen = false;
    let mut prompt_scan = RollingBuffer::new(
        prompt_needles
            .iter()
            .map(|needle| needle.len())
            .max()
            .unwrap_or(1),
    );
    while Instant::now() < prompt_deadline {
        match rx.recv_timeout(Duration::from_millis(80)) {
            Ok(chunk) => {
                let scan = prompt_scan.append(&chunk);
                if Instant::now() >= next_cursor_check
                    && !scan.is_empty()
                    && contains_bytes(&scan, &cursor_query)
                {
                    let _ = writer.write_all(&cursor_reply);
                    next_cursor_check = Instant::now() + Duration::from_secs(1);
                }
                for (index, (needle, response)) in send_on_substrings.iter().enumerate() {
                    if !triggered_send[index] && contains_bytes(&scan, needle) {
                        let _ = writer.write_all(response);
                        triggered_send[index] = true;
                    }
                }
                if prompt_needles.iter().any(|needle| contains_bytes(&scan, needle)) {
                    prompt_seen = true;
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }

    if claude_cli_debug_enabled() && !prompt_seen {
        log::debug!("Claude PTY prompt not detected before sending /usage");
    }

    while rx.try_recv().is_ok() {}

    writer
        .write_all(b"/usage\r")
        .map_err(|error| format!("Failed to send Claude CLI command: {error}"))?;
    let prompt_wait_ms = prompt_start.elapsed().as_millis();


    loop {
        if start.elapsed() >= timeout {
            break;
        }

        match rx.recv_timeout(Duration::from_millis(80)) {
            Ok(chunk) => {
                buffer.extend_from_slice(&chunk);
                let scan = rolling.append(&chunk);
                if Instant::now() >= next_cursor_check
                    && !scan.is_empty()
                    && contains_bytes(&scan, &cursor_query)
                {
                    let _ = writer.write_all(&cursor_reply);
                    next_cursor_check = Instant::now() + Duration::from_secs(1);
                }

                for (index, (needle, response)) in send_on_substrings.iter().enumerate() {
                    if !triggered_send[index] && contains_bytes(&scan, needle) {
                        let _ = writer.write_all(response);
                        triggered_send[index] = true;
                    }
                }
                if let Some((index, _)) = stop_needles
                    .iter()
                    .enumerate()
                    .find(|(_, needle)| contains_bytes(&scan, needle))
                {
                    stopped_early = true;
                    stop_reason = ClaudePtyStopReason::StopNeedle(
                        String::from_utf8_lossy(&stop_needles[index]).to_string(),
                    );
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                stop_reason = ClaudePtyStopReason::ChannelClosed;
                break;
            }
        }

        if last_enter.elapsed() >= send_enter_every {
            let _ = writer.write_all(b"\r");
            last_enter = Instant::now();
        }

        match child.try_wait() {
            Ok(Some(_)) => {
                stop_reason = ClaudePtyStopReason::ProcessExit;
                break;
            }
            Ok(None) => {}
            Err(_) => {
                stop_reason = ClaudePtyStopReason::ProcessExit;
                break;
            }
        }

    }

    if stopped_early {
        let settle_deadline = Instant::now() + settle_after_stop;
        while Instant::now() < settle_deadline {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(chunk) => buffer.extend_from_slice(&chunk),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    let _ = writer.write_all(b"/exit\r");
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader_thread.join();

    if buffer.is_empty() {
        return Err("Claude CLI session timed out.".to_string());
    }

    if claude_cli_debug_enabled() {
        dump_claude_cli_debug(
            binary,
            extra_args,
            &working_dir,
            &buffer,
            &stop_reason,
            ClaudePtyPromptInfo {
                prompt_seen,
                prompt_wait_ms,
            },
        );
    }

    String::from_utf8(buffer).map_err(|error| {
        format!("Claude CLI output was not valid UTF-8: {error}")
    })
}

fn is_cli_unknown_option_allowed_tools(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    let mentions_unknown = lower.contains("unknown option")
        || lower.contains("unrecognized option")
        || lower.contains("unknown flag")
        || lower.contains("unrecognized flag");
    let mentions_allowed_tools =
        lower.contains("--allowed-tools") || lower.contains("--allowed tools");
    mentions_unknown && mentions_allowed_tools
}

fn claude_cli_debug_enabled() -> bool {
    match std::env::var("AGENT_STATS_CLAUDE_PTY_DEBUG") {
        Ok(value) => {
            let trimmed = value.trim();
            !trimmed.is_empty() && trimmed != "0"
        }
        Err(_) => false,
    }
}

fn dump_claude_cli_debug(
    binary: &str,
    extra_args: &[&str],
    working_dir: &std::path::Path,
    output: &[u8],
    stop_reason: &ClaudePtyStopReason,
    prompt_info: ClaudePtyPromptInfo,
) {
    let Some(path) = claude_cli_debug_path() else {
        return;
    };

    let timestamp = OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| OffsetDateTime::now_utc().to_string());
    let reason = format_stop_reason(stop_reason);
    let content = String::from_utf8_lossy(output);
    let log_body = format!(
        "timestamp={timestamp}\nreason={reason}\nbinary={binary}\nargs={:?}\ncwd={}\nprompt_seen={}\nprompt_wait_ms={}\nraw_output:\n{}\n",
        extra_args,
        working_dir.display(),
        prompt_info.prompt_seen,
        prompt_info.prompt_wait_ms,
        content
    );

    if let Err(error) = std::fs::write(&path, log_body) {
        log::error!("Failed to write Claude CLI debug output: {error}");
    } else {
        log::debug!("Claude CLI debug output written to {}", path.display());
    }
}

fn format_stop_reason(reason: &ClaudePtyStopReason) -> String {
    match reason {
        ClaudePtyStopReason::Timeout => "Timeout".to_string(),
        ClaudePtyStopReason::StopNeedle(needle) => format!("StopNeedle({needle})"),
        ClaudePtyStopReason::ProcessExit => "ProcessExit".to_string(),
        ClaudePtyStopReason::ChannelClosed => "ChannelClosed".to_string(),
    }
}

fn claude_cli_debug_path() -> Option<std::path::PathBuf> {
    let base = dirs::home_dir().unwrap_or_else(|| std::env::temp_dir());
    let dir = base.join(".agent-stats");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        log::error!("Failed to create Claude CLI debug directory: {error}");
        return None;
    }
    Some(dir.join("claude-cli-debug.txt"))
}

fn build_claude_cli_environment(working_dir: &std::path::Path) -> Vec<(String, String)> {
    let mut env: std::collections::HashMap<String, String> = std::env::vars().collect();
    env.remove("CODEXBAR_CLAUDE_OAUTH_TOKEN");
    env.remove("CODEXBAR_CLAUDE_OAUTH_SCOPES");
    let keys: Vec<String> = env.keys().cloned().collect();
    for key in keys {
        if key.starts_with("ANTHROPIC_") {
            env.remove(&key);
        }
    }
    if let Some(path) = resolve_login_shell_path() {
        env.insert("PATH".to_string(), path);
    }
    env.insert(
        "PWD".to_string(),
        working_dir.to_string_lossy().to_string(),
    );
    env.into_iter().collect()
}

fn claude_probe_working_directory() -> std::path::PathBuf {
    let base = dirs::data_local_dir()
        .or_else(|| dirs::home_dir())
        .unwrap_or_else(|| std::env::temp_dir());
    let probe_dir = base.join("AgentStats").join("ClaudeProbe");
    if std::fs::create_dir_all(&probe_dir).is_ok() {
        probe_dir
    } else {
        std::env::temp_dir()
    }
}

struct RollingBuffer {
    max_len: usize,
    tail: Vec<u8>,
}

impl RollingBuffer {
    fn new(max_len: usize) -> Self {
        Self {
            max_len: max_len.max(1),
            tail: Vec::new(),
        }
    }

    fn append(&mut self, data: &[u8]) -> Vec<u8> {
        if data.is_empty() {
            return Vec::new();
        }
        let mut combined = Vec::with_capacity(self.tail.len() + data.len());
        combined.extend_from_slice(&self.tail);
        combined.extend_from_slice(data);
        if self.max_len > 1 {
            if combined.len() >= self.max_len - 1 {
                self.tail = combined[combined.len() - (self.max_len - 1)..].to_vec();
            } else {
                self.tail = combined.clone();
            }
        } else {
            self.tail.clear();
        }
        combined
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn format_cli_environment_diagnostics(login_shell_path: Option<&str>) -> String {
    let current_path = std::env::var("PATH").ok();
    let configured_binary = std::env::var("CLAUDE_BINARY").ok();
    let mut parts = Vec::new();
    parts.push(format!(
        "PATH={}",
        current_path.as_deref().unwrap_or("<unset>")
    ));
    parts.push(format!(
        "CLAUDE_BINARY={}",
        configured_binary.as_deref().unwrap_or("<unset>")
    ));
    if let Some(path) = login_shell_path {
        parts.push(format!("login_shell_PATH={path}"));
    } else {
        parts.push("login_shell_PATH=<unavailable>".to_string());
    }
    format!(" ({})", parts.join(", "))
}

fn detect_cli_error(text: &str) -> Option<String> {
    let lower = text.to_lowercase();

    if lower.contains("token_expired") || lower.contains("token has expired") {
        return Some("Claude CLI token expired. Run `claude login` to refresh.".to_string());
    }
    if lower.contains("authentication_error") {
        return Some("Claude CLI authentication error. Run `claude login`.".to_string());
    }
    if lower.contains("failed to load usage data") {
        // Try to extract JSON error details
        if let Some(json_error) = extract_json_error_from_cli(text) {
            return Some(json_error);
        }
        return Some(
            "Claude CLI could not load usage data. Open the CLI and retry `/usage`.".to_string(),
        );
    }
    if lower.contains("do you trust the files in this folder?") && !lower.contains("current session") {
        return Some(
            "Claude CLI is waiting for a folder trust prompt. \
             Open `claude` once, choose \"Yes, proceed\", then retry."
                .to_string(),
        );
    }

    None
}

fn extract_json_error_from_cli(text: &str) -> Option<String> {
    // Look for a JSON object after "Failed to load usage data:"
    let marker = "Failed to load usage data:";
    let start = text.find(marker)?;
    let after = &text[start + marker.len()..];
    let brace_start = after.find('{')?;
    let brace_end = find_matching_brace(&after[brace_start..])?;
    let json_str = &after[brace_start..brace_start + brace_end + 1];

    let parsed: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let error_obj = parsed.get("error")?;
    let message = error_obj
        .get("message")
        .and_then(|value| value.as_str())
        .map(|value| value.trim());
    let error_code = error_obj
        .get("details")
        .and_then(|details| details.get("error_code"))
        .and_then(|value| value.as_str())
        .map(|value| value.trim());

    let mut parts = Vec::new();
    if let Some(message) = message {
        if !message.is_empty() {
            parts.push(message.to_string());
        }
    }
    if let Some(code) = error_code {
        if !code.is_empty() {
            parts.push(format!("({code})"));
        }
    }

    if parts.is_empty() {
        return None;
    }

    let hint = parts.join(" ");
    if let Some(code) = error_code {
        if code.to_lowercase().contains("token") {
            return Some(format!("{hint}. Run `claude login` to refresh."));
        }
    }
    Some(format!("Claude CLI error: {hint}"))
}

fn find_matching_brace(text: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (index, character) in text.char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_cli_usage(text: &str) -> Result<UsageSnapshot, ProviderErrorPayload> {
    let lines: Vec<&str> = text.lines().collect();

    let session_percent = extract_percent_near_label(&lines, "Current session");
    let weekly_percent = extract_percent_near_label(&lines, "Current week");
    let opus_percent = extract_percent_near_labels(
        &lines,
        &["Current week (Opus)", "Current week (Sonnet only)", "Current week (Sonnet)"],
    );

    // Fallback: if labels moved, try order-based extraction
    let (session_percent, weekly_percent, opus_percent) =
        if session_percent.is_none() {
            let ordered = all_percents_from_text(text);
            (
                ordered.first().copied(),
                if has_label_ci(text, "current week") {
                    ordered.get(1).copied()
                } else {
                    weekly_percent
                },
                if has_label_ci(text, "opus") || has_label_ci(text, "sonnet") {
                    ordered.get(2).copied()
                } else {
                    opus_percent
                },
            )
        } else {
            (session_percent, weekly_percent, opus_percent)
        };

    let session_percent = session_percent.ok_or_else(|| {
        log::debug!(
            "Claude CLI parse missing 'Current session' label. Output snippet: {}",
            truncate_body(text, 800)
        );
        provider_error(
            "Could not parse Claude CLI usage: missing 'Current session' data. \
             The CLI output format may have changed.",
        )
    })?;

    let session_reset = extract_reset_near_label(&lines, "Current session");
    let weekly_reset = extract_reset_near_label(&lines, "Current week");
    let opus_reset = extract_reset_near_labels(
        &lines,
        &["Current week (Opus)", "Current week (Sonnet only)", "Current week (Sonnet)"],
    );

    let identity = parse_cli_identity(text);

    let primary = make_cli_rate_window(session_percent, session_reset.as_deref());
    let secondary = weekly_percent.map(|percent| make_cli_rate_window(percent, weekly_reset.as_deref()));
    let tertiary = opus_percent.map(|percent| make_cli_rate_window(percent, opus_reset.as_deref()));

    Ok(UsageSnapshot {
        primary: Some(primary),
        secondary,
        tertiary,
        provider_cost: None,
        updated_at: OffsetDateTime::now_utc(),
        identity: Some(identity),
    })
}

fn make_cli_rate_window(percent_left: f64, reset_text: Option<&str>) -> RateWindow {
    let used_percent = (100.0 - percent_left).clamp(0.0, 100.0);
    let reset_description = reset_text
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());

    RateWindow {
        used_percent,
        window_minutes: None,
        resets_at: None,
        reset_description,
    }
}

fn parse_cli_identity(text: &str) -> ProviderIdentitySnapshot {
    let email = extract_cli_field(text, "Account:")
        .or_else(|| extract_cli_field(text, "Email:"))
        .or_else(|| find_email_in_text(text));

    let organization = extract_cli_field(text, "Org:")
        .or_else(|| extract_cli_field(text, "Organization:"))
        .and_then(|org| {
            let org = org.trim().to_string();
            if org.is_empty() {
                return None;
            }
            // Suppress if the org is just the email prefix
            if let Some(ref email) = email {
                if org.to_lowercase().starts_with(&email.to_lowercase()) {
                    return None;
                }
            }
            Some(org)
        });

    let login_method = extract_login_method(text);

    ProviderIdentitySnapshot {
        provider_id: Some(ProviderId::Claude.to_string()),
        account_email: email,
        account_organization: organization,
        login_method,
    }
}

fn extract_cli_field(text: &str, label: &str) -> Option<String> {
    let lower_text = text.to_lowercase();
    let lower_label = label.to_lowercase();
    let pos = lower_text.find(&lower_label)?;
    // Advance past the label in the lowercased text first, then map back to the original text.
    // Both `pos` and the label length are measured in the lowercased domain; for ASCII labels
    // this matches the original, but for safety we index the lowercased text and then use
    // char-count to find the equivalent position in the original.
    let label_end_in_lower = pos + lower_label.len();
    let char_offset = lower_text[..label_end_in_lower].chars().count();
    let byte_offset: usize = text.char_indices().nth(char_offset).map(|(index, _)| index)?;
    let after = &text[byte_offset..];
    let line_end = after.find('\n').unwrap_or(after.len());
    let value = after[..line_end].trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn find_email_in_text(text: &str) -> Option<String> {
    // Simple email detection: look for word@word.word patterns
    for word in text.split_whitespace() {
        let trimmed = word.trim_matches(|character: char| !character.is_alphanumeric() && character != '@' && character != '.' && character != '+' && character != '-' && character != '_');
        if let Some(at_pos) = trimmed.find('@') {
            if at_pos > 0 {
                if let Some(dot_pos) = trimmed[at_pos..].find('.') {
                    if dot_pos > 1 && at_pos + dot_pos + 1 < trimmed.len() {
                        return Some(trimmed.to_string());
                    }
                }
            }
        }
    }
    None
}

fn extract_login_method(text: &str) -> Option<String> {
    // Try explicit "Login method:" field
    if let Some(method) = extract_cli_field(text, "Login method:") {
        let cleaned = clean_plan_name(&method);
        if !cleaned.is_empty() {
            return Some(cleaned);
        }
    }

    // Look for "Claude <plan>" patterns like "Claude Max", "Claude Pro", etc.
    let lower = text.to_lowercase();
    for plan in ["max", "pro", "team", "enterprise", "ultra"] {
        let needle = format!("claude {plan}");
        if lower.contains(&needle) {
            let capitalized_plan = plan[..1].to_uppercase() + &plan[1..];
            return Some(format!("Claude {capitalized_plan}"));
        }
    }

    None
}

fn clean_plan_name(text: &str) -> String {
    // Strip residual ANSI-like bracket codes (e.g. "[22m")
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '[' {
            // Consume digits + trailing letter
            let mut is_ansi = false;
            let mut skipped = String::new();
            while let Some(&next) = chars.peek() {
                if next.is_ascii_digit() || next == ';' {
                    skipped.push(next);
                    chars.next();
                } else if next.is_ascii_alphabetic() && !skipped.is_empty() {
                    chars.next();
                    is_ansi = true;
                    break;
                } else {
                    break;
                }
            }
            if !is_ansi {
                result.push('[');
                result.push_str(&skipped);
            }
        } else {
            result.push(character);
        }
    }
    result.trim().to_string()
}

// ---------------------------------------------------------------------------
// CLI output parsing helpers
// ---------------------------------------------------------------------------

fn extract_percent_near_label(lines: &[&str], label: &str) -> Option<f64> {
    let label_lower = label.to_lowercase();
    let label_normalized = normalize_for_label(&label_lower);

    for (index, line) in lines.iter().enumerate() {
        let line_normalized = normalize_for_label(&line.to_lowercase());
        if line_normalized.contains(&label_normalized) {
            // Scan up to 12 lines ahead for a percent value
            let window_end = (index + 12).min(lines.len());
            for candidate in &lines[index..window_end] {
                if let Some(percent) = parse_percent_from_line(candidate) {
                    return Some(percent);
                }
            }
        }
    }
    None
}

fn extract_percent_near_labels(lines: &[&str], labels: &[&str]) -> Option<f64> {
    for label in labels {
        if let Some(value) = extract_percent_near_label(lines, label) {
            return Some(value);
        }
    }
    None
}

fn parse_percent_from_line(line: &str) -> Option<f64> {
    parse_percent_from_line_inner(line, false)
}

fn parse_percent_from_line_inner(line: &str, assume_remaining: bool) -> Option<f64> {
    // Look for patterns like "42%", "42.5 %", "42.5%"
    // Uses char iteration to handle Unicode whitespace (e.g. non-breaking space) before '%',
    // matching the Swift reference's \p{Zs}* pattern.
    let chars: Vec<char> = line.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if chars[index].is_ascii_digit() {
            let start = index;
            while index < chars.len() && (chars[index].is_ascii_digit() || chars[index] == '.') {
                index += 1;
            }
            let number_str: String = chars[start..index].iter().collect();
            // Skip optional whitespace (including Unicode separator spaces)
            let mut search = index;
            while search < chars.len() && chars[search].is_whitespace() {
                search += 1;
            }
            if search < chars.len() && chars[search] == '%' {
                if let Ok(value) = number_str.parse::<f64>() {
                    let clamped = value.clamp(0.0, 100.0);
                    let lower = line.to_lowercase();
                    let used_keywords = ["used", "spent", "consumed"];
                    let remaining_keywords = ["left", "remaining", "available"];

                    if used_keywords.iter().any(|keyword| lower.contains(keyword)) {
                        return Some((100.0 - clamped).max(0.0));
                    }
                    if remaining_keywords.iter().any(|keyword| lower.contains(keyword)) {
                        return Some(clamped);
                    }
                    if assume_remaining {
                        return Some(clamped);
                    }
                    return None;
                }
            }
        }
        index += 1;
    }
    None
}

fn all_percents_from_text(text: &str) -> Vec<f64> {
    text.lines()
        .filter_map(parse_percent_from_line_assume_remaining)
        .collect()
}

fn parse_percent_from_line_assume_remaining(line: &str) -> Option<f64> {
    parse_percent_from_line_inner(line, true)
}

fn extract_reset_near_label(lines: &[&str], label: &str) -> Option<String> {
    let label_lower = label.to_lowercase();
    let label_normalized = normalize_for_label(&label_lower);

    for (index, line) in lines.iter().enumerate() {
        let line_normalized = normalize_for_label(&line.to_lowercase());
        if line_normalized.contains(&label_normalized) {
            let window_end = (index + 14).min(lines.len());
            for candidate in &lines[index..window_end] {
                if let Some(reset_text) = extract_reset_from_line(candidate) {
                    return Some(clean_reset_line(&reset_text));
                }
            }
        }
    }
    None
}

fn extract_reset_from_line(line: &str) -> Option<String> {
    let lower = line.to_lowercase();
    let pos_in_lower = lower.find("resets")?;
    // Map byte position from lowercased back to original using char count
    let char_offset = lower[..pos_in_lower].chars().count();
    let byte_offset: usize = line.char_indices().nth(char_offset).map(|(index, _)| index)?;
    Some(line[byte_offset..].trim().to_string())
}

fn extract_reset_near_labels(lines: &[&str], labels: &[&str]) -> Option<String> {
    for label in labels {
        if let Some(value) = extract_reset_near_label(lines, label) {
            return Some(value);
        }
    }
    None
}

fn clean_reset_line(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches(')').trim_end();
    let open_count = trimmed.chars().filter(|&character| character == '(').count();
    let close_count = trimmed.chars().filter(|&character| character == ')').count();
    if open_count > close_count {
        format!("{trimmed})")
    } else {
        trimmed.to_string()
    }
}

fn normalize_for_label(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn has_label_ci(text: &str, label: &str) -> bool {
    let normalized_text = normalize_for_label(&text.to_lowercase());
    let normalized_label = normalize_for_label(&label.to_lowercase());
    normalized_text.contains(&normalized_label)
}

// ---------------------------------------------------------------------------
// Version detection
// ---------------------------------------------------------------------------

fn detect_cli_version() -> Option<String> {
    let binary = resolve_claude_binary()?;
    let output = std::process::Command::new(&binary)
        .args(["--version"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8(output.stdout).ok()?;
    let cleaned = strip_ansi_codes(&text).trim().to_string();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

// ---------------------------------------------------------------------------
// ANSI stripping
// ---------------------------------------------------------------------------

fn strip_ansi_codes(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\x1b' {
            // Consume ESC [ ... <final byte>
            if chars.peek() == Some(&'[') {
                chars.next();
                loop {
                    match chars.next() {
                        Some(next) if next.is_ascii_alphabetic() || next == '~' => break,
                        Some(_) => continue,
                        None => break,
                    }
                }
            }
        } else {
            result.push(character);
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Date/time helpers
// ---------------------------------------------------------------------------

fn parse_iso8601_datetime(text: &str) -> Option<OffsetDateTime> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    OffsetDateTime::parse(trimmed, &time::format_description::well_known::Rfc3339).ok()
}

fn format_reset_date(date: &OffsetDateTime) -> String {
    let format_description = time::format_description::parse(
        "[month repr:short] [day] at [hour repr:12]:[minute][period case:lower]",
    );
    match format_description {
        Ok(format) => date.format(&format).unwrap_or_else(|_| date.to_string()),
        Err(_) => date.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
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
        // Find a char boundary at or before max_length to avoid panicking on multi-byte chars
        let truncation_point = (0..=max_length)
            .rev()
            .find(|&index| cleaned.is_char_boundary(index))
            .unwrap_or(0);
        format!("{}…", &cleaned[..truncation_point])
    } else {
        cleaned
    }
}

// ---------------------------------------------------------------------------
// OAuth API response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct OAuthUsageResponse {
    five_hour: Option<OAuthUsageWindow>,
    seven_day: Option<OAuthUsageWindow>,
    #[allow(dead_code)]
    seven_day_oauth_apps: Option<OAuthUsageWindow>,
    seven_day_opus: Option<OAuthUsageWindow>,
    seven_day_sonnet: Option<OAuthUsageWindow>,
    #[allow(dead_code)]
    iguana_necktie: Option<OAuthUsageWindow>,
    extra_usage: Option<OAuthExtraUsage>,
}

#[derive(Debug, Deserialize)]
struct OAuthUsageWindow {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OAuthExtraUsage {
    is_enabled: Option<bool>,
    monthly_limit: Option<f64>,
    used_credits: Option<f64>,
    #[allow(dead_code)]
    utilization: Option<f64>,
    currency: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi_codes() {
        assert_eq!(strip_ansi_codes("hello \x1b[32mworld\x1b[0m"), "hello world");
        assert_eq!(strip_ansi_codes("no codes"), "no codes");
        assert_eq!(strip_ansi_codes("\x1b[1;34mblue\x1b[0m"), "blue");
    }

    #[test]
    fn test_parse_percent_from_line() {
        assert_eq!(parse_percent_from_line("42% remaining"), Some(42.0));
        assert_eq!(parse_percent_from_line("75% used"), Some(25.0));
        assert_eq!(parse_percent_from_line("100% left"), Some(100.0));
        assert_eq!(parse_percent_from_line("0% remaining"), Some(0.0));
        assert_eq!(parse_percent_from_line("50.5 % remaining"), Some(50.5));
        assert_eq!(parse_percent_from_line("no percent here"), None);
    }

    #[test]
    fn test_extract_percent_near_label() {
        let lines = vec![
            "Current session",
            "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━",
            "75% remaining",
            "",
            "Current week (all models)",
            "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━",
            "60% remaining",
        ];
        assert_eq!(
            extract_percent_near_label(&lines, "Current session"),
            Some(75.0)
        );
        assert_eq!(
            extract_percent_near_label(&lines, "Current week"),
            Some(60.0)
        );
    }

    #[test]
    fn test_infer_plan_from_tier() {
        assert_eq!(
            infer_plan_from_tier(Some("max_5x")),
            Some("Claude Max".to_string())
        );
        assert_eq!(
            infer_plan_from_tier(Some("pro")),
            Some("Claude Pro".to_string())
        );
        assert_eq!(
            infer_plan_from_tier(Some("team_default")),
            Some("Claude Team".to_string())
        );
        assert_eq!(
            infer_plan_from_tier(Some("enterprise")),
            Some("Claude Enterprise".to_string())
        );
        assert_eq!(infer_plan_from_tier(Some("free")), None);
        assert_eq!(infer_plan_from_tier(None), None);
    }

    #[test]
    fn test_is_cli_unknown_option_allowed_tools() {
        assert!(is_cli_unknown_option_allowed_tools(
            "error: unknown option '--allowed-tools'"
        ));
        assert!(is_cli_unknown_option_allowed_tools(
            "Error: unrecognized option '--allowed tools'"
        ));
        assert!(!is_cli_unknown_option_allowed_tools(
            "error: unknown option '--no-input'"
        ));
        assert!(!is_cli_unknown_option_allowed_tools(
            "claude exited with status 1"
        ));
    }

    #[test]
    fn test_detect_cli_error() {
        assert!(detect_cli_error("Your token_expired, please login again").is_some());
        assert!(detect_cli_error("authentication_error occurred").is_some());
        assert!(detect_cli_error("Failed to load usage data: something").is_some());
        assert!(detect_cli_error("Current session\n75% remaining").is_none());
    }

    #[test]
    fn test_find_email_in_text() {
        assert_eq!(
            find_email_in_text("Account: user@example.com"),
            Some("user@example.com".to_string())
        );
        assert_eq!(find_email_in_text("no email here"), None);
    }

    #[test]
    fn test_clean_plan_name() {
        assert_eq!(clean_plan_name("Claude Max"), "Claude Max");
        assert_eq!(clean_plan_name("[22mClaude Pro"), "Claude Pro");
        assert_eq!(clean_plan_name("Claude [1mMax[22m"), "Claude Max");
    }

    #[test]
    fn test_truncate_body() {
        assert_eq!(truncate_body("short", 100), "short");
        assert_eq!(truncate_body("hello world", 5), "hello…");
    }

    #[test]
    fn test_extract_reset_near_label() {
        let lines = vec![
            "Current session",
            "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━",
            "75% remaining",
            "Resets at 3pm (America/New_York)",
        ];
        let result = extract_reset_near_label(&lines, "Current session");
        assert!(result.is_some());
        let reset = result.as_deref().unwrap_or("");
        assert!(reset.contains("Resets"));
    }

    #[test]
    fn test_extract_json_error_from_cli() {
        let text = r#"Failed to load usage data: {"error":{"message":"Token expired","details":{"error_code":"token_expired"}}}"#;
        let result = extract_json_error_from_cli(text);
        assert!(result.is_some());
        let msg = result.as_deref().unwrap_or("");
        assert!(msg.contains("Token expired"));
        assert!(msg.contains("token_expired"));
    }

    #[test]
    fn test_find_matching_brace() {
        assert_eq!(find_matching_brace("{}"), Some(1));
        assert_eq!(find_matching_brace("{\"a\":{}}"), Some(7));
        assert_eq!(find_matching_brace("{unclosed"), None);
    }

    #[test]
    fn test_normalize_for_label() {
        assert_eq!(normalize_for_label("  current   session  "), "current session");
    }

    #[test]
    fn test_has_label_ci() {
        assert!(has_label_ci("  Current  WEEK  stuff", "current week"));
        assert!(!has_label_ci("something else", "current week"));
    }

    #[test]
    fn test_clean_reset_line() {
        assert_eq!(
            clean_reset_line("Resets at 3pm (America/New_York)"),
            "Resets at 3pm (America/New_York)"
        );
        assert_eq!(
            clean_reset_line("Resets at 3pm (America/New_York))"),
            "Resets at 3pm (America/New_York)"
        );
    }

    #[test]
    fn test_map_oauth_window() {
        let window = OAuthUsageWindow {
            utilization: Some(42.5),
            resets_at: Some("2026-01-28T15:00:00Z".to_string()),
        };
        let result = map_oauth_window(&Some(window), Some(300));
        assert!(result.is_some());
        let rate_window = result.as_ref().unwrap();
        assert_eq!(rate_window.used_percent, 42.5);
        assert_eq!(rate_window.window_minutes, Some(300));
        assert!(rate_window.resets_at.is_some());
    }

    #[test]
    fn test_map_oauth_window_missing_utilization() {
        let window = OAuthUsageWindow {
            utilization: None,
            resets_at: Some("2026-01-28T15:00:00Z".to_string()),
        };
        let result = map_oauth_window(&Some(window), Some(300));
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_cli_usage_full() {
        let text = "\
Current session
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
75% remaining
Resets at 5pm (UTC)

Current week (all models)
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
60% remaining
Resets Jan 30 at 12am (UTC)

Account: user@example.com
Org: My Team
";
        let result = parse_cli_usage(text);
        assert!(result.is_ok());
        let snapshot = result.unwrap();
        let primary = snapshot.primary.as_ref().unwrap();
        assert!((primary.used_percent - 25.0).abs() < 0.01);
        let secondary = snapshot.secondary.as_ref().unwrap();
        assert!((secondary.used_percent - 40.0).abs() < 0.01);
        let identity = snapshot.identity.as_ref().unwrap();
        assert_eq!(identity.account_email.as_deref(), Some("user@example.com"));
        assert_eq!(identity.account_organization.as_deref(), Some("My Team"));
    }

    #[test]
    fn test_parse_cli_usage_current_format() {
        let text = "\
▐▛███▜▌   Claude Code v2.1.20
▝▜█████▛▘  Opus 4.5 · Claude Max
  ▘▘ ▝▝    ~/Projects/agent-limits

❯ /usage
────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────
 ──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────
  Settings:  Status   Config   Usage  (←/→ or tab to cycle)


  Current session
  ██████████                                         20% used
  Resets 8:59pm (America/Los_Angeles)

  Current week (all models)
  ███████████                                        22% used
  Resets 4:59am (America/Los_Angeles)

  Current week (Sonnet only)
  █▌                                                 3% used
  Resets Feb 2 at 11:59am (America/Los_Angeles)

  Esc to cancel
";
        let result = parse_cli_usage(text);
        assert!(result.is_ok());
        let snapshot = result.unwrap();
        let primary = snapshot.primary.as_ref().unwrap();
        assert!((primary.used_percent - 20.0).abs() < 0.01);
        let secondary = snapshot.secondary.as_ref().unwrap();
        assert!((secondary.used_percent - 22.0).abs() < 0.01);
        let tertiary = snapshot.tertiary.as_ref().unwrap();
        assert!((tertiary.used_percent - 3.0).abs() < 0.01);
    }

    #[test]
    fn test_rescale_extra_usage_non_enterprise() {
        let cost = ProviderCostSnapshot {
            used: 50.0,
            limit: 2000.0,
            currency_code: "USD".to_string(),
            period: Some("Monthly".to_string()),
            resets_at: None,
            updated_at: OffsetDateTime::now_utc(),
        };
        let rescaled = rescale_extra_usage_cost_if_needed(cost, Some("Claude Pro"));
        assert!(rescaled.is_some());
        let rescaled = rescaled.unwrap();
        assert!((rescaled.limit - 20.0).abs() < 0.01);
        assert!((rescaled.used - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_rescale_extra_usage_enterprise_no_rescale() {
        let cost = ProviderCostSnapshot {
            used: 5000.0,
            limit: 50000.0,
            currency_code: "USD".to_string(),
            period: Some("Monthly".to_string()),
            resets_at: None,
            updated_at: OffsetDateTime::now_utc(),
        };
        let rescaled = rescale_extra_usage_cost_if_needed(cost, Some("Claude Enterprise"));
        assert!(rescaled.is_some());
        let rescaled = rescaled.unwrap();
        assert!((rescaled.limit - 50000.0).abs() < 0.01);
    }

    // -----------------------------------------------------------------------
    // Web API tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_session_key_from_cookie_header() {
        assert_eq!(
            extract_session_key("sessionKey=sk-ant-abc123"),
            Some("sk-ant-abc123".to_string())
        );
        assert_eq!(
            extract_session_key("other=foo; sessionKey=sk-ant-xyz; bar=baz"),
            Some("sk-ant-xyz".to_string())
        );
        assert_eq!(extract_session_key("sessionKey=not-valid"), None);
        assert_eq!(extract_session_key("nokey=here"), None);
        assert_eq!(extract_session_key(""), None);
    }

    #[test]
    fn test_extract_session_key_bare_token() {
        assert_eq!(
            extract_session_key("sk-ant-bare-token"),
            Some("sk-ant-bare-token".to_string())
        );
        // With semicolons it should not match as bare token
        assert_eq!(extract_session_key("sk-ant-x; other=y"), None);
    }

    #[test]
    fn test_parse_web_usage_response() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{
                "five_hour": {"utilization": 35.0, "resets_at": "2026-01-28T15:00:00Z"},
                "seven_day": {"utilization": 20.5, "resets_at": "2026-02-03T00:00:00Z"},
                "seven_day_sonnet": {"utilization": 10.0, "resets_at": "2026-02-03T00:00:00Z"}
            }"#,
        )
        .expect("valid JSON");

        let result = parse_web_usage_response(&json);
        assert!(result.is_ok());
        let data = result.unwrap();
        assert!((data.primary.used_percent - 35.0).abs() < 0.01);
        assert_eq!(data.primary.window_minutes, Some(300));
        assert!(data.primary.resets_at.is_some());
        assert!(data.secondary.is_some());
        let secondary = data.secondary.unwrap();
        assert!((secondary.used_percent - 20.5).abs() < 0.01);
        assert!(data.tertiary.is_some());
        let tertiary = data.tertiary.unwrap();
        assert!((tertiary.used_percent - 10.0).abs() < 0.01);
    }

    #[test]
    fn test_parse_web_usage_response_integer_utilization() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"five_hour": {"utilization": 42, "resets_at": null}}"#,
        )
        .expect("valid JSON");

        let result = parse_web_usage_response(&json);
        assert!(result.is_ok());
        let data = result.unwrap();
        assert!((data.primary.used_percent - 42.0).abs() < 0.01);
    }

    #[test]
    fn test_parse_web_usage_response_missing_session() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"seven_day": {"utilization": 20.0}}"#,
        )
        .expect("valid JSON");

        let result = parse_web_usage_response(&json);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_web_window() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"seven_day": {"utilization": 55.5, "resets_at": "2026-02-03T00:00:00Z"}}"#,
        )
        .expect("valid JSON");

        let window = parse_web_window(&json, "seven_day", Some(10080));
        assert!(window.is_some());
        let window = window.unwrap();
        assert!((window.used_percent - 55.5).abs() < 0.01);
        assert_eq!(window.window_minutes, Some(10080));
        assert!(window.resets_at.is_some());
    }

    #[test]
    fn test_parse_web_window_missing_key() {
        let json: serde_json::Value = serde_json::from_str(r#"{}"#).expect("valid JSON");
        assert!(parse_web_window(&json, "seven_day", None).is_none());
    }

    #[test]
    fn test_parse_web_window_missing_utilization() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"seven_day": {"resets_at": "2026-02-03T00:00:00Z"}}"#,
        )
        .expect("valid JSON");
        assert!(parse_web_window(&json, "seven_day", None).is_none());
    }

    #[test]
    fn test_infer_plan_from_web_account() {
        assert_eq!(
            infer_plan_from_web_account(Some("max_5x"), None),
            Some("Claude Max".to_string())
        );
        assert_eq!(
            infer_plan_from_web_account(Some("pro"), None),
            Some("Claude Pro".to_string())
        );
        assert_eq!(
            infer_plan_from_web_account(Some("team_default"), None),
            Some("Claude Team".to_string())
        );
        assert_eq!(
            infer_plan_from_web_account(Some("enterprise"), None),
            Some("Claude Enterprise".to_string())
        );
        assert_eq!(
            infer_plan_from_web_account(Some("claude_tier"), Some("stripe")),
            Some("Claude Pro".to_string())
        );
        assert_eq!(
            infer_plan_from_web_account(Some("free"), None),
            None
        );
        assert_eq!(infer_plan_from_web_account(None, None), None);
    }

    #[test]
    fn test_web_organization_response_capabilities() {
        let org = WebOrganizationResponse {
            uuid: "test-uuid".to_string(),
            name: Some("Test Org".to_string()),
            capabilities: Some(vec!["chat".to_string(), "api".to_string()]),
        };
        assert!(org.has_chat_capability());
        assert!(!org.is_api_only());

        let api_only = WebOrganizationResponse {
            uuid: "api-uuid".to_string(),
            name: None,
            capabilities: Some(vec!["api".to_string()]),
        };
        assert!(!api_only.has_chat_capability());
        assert!(api_only.is_api_only());

        let empty = WebOrganizationResponse {
            uuid: "empty-uuid".to_string(),
            name: None,
            capabilities: None,
        };
        assert!(!empty.has_chat_capability());
        assert!(!empty.is_api_only());
    }

    // -----------------------------------------------------------------------
    // truncate_body UTF-8 safety
    // -----------------------------------------------------------------------

    #[test]
    fn test_truncate_body_multibyte_safety() {
        // "héllo" – 'é' is 2 bytes in UTF-8, so truncating at byte 2 would split it
        let result = truncate_body("héllo", 2);
        // Should truncate to "h" (1 char) rather than panicking
        assert_eq!(result, "h…");
    }

    #[test]
    fn test_truncate_body_emoji_safety() {
        // "🎉ab" – '🎉' is 4 bytes
        let result = truncate_body("🎉ab", 3);
        // max_length=3 is inside the emoji, so it truncates before it
        assert_eq!(result, "…");
    }

    // -----------------------------------------------------------------------
    // Unicode whitespace before % (parity with Swift's \p{Zs}*)
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_percent_unicode_whitespace() {
        // Non-breaking space (U+00A0) before %
        assert_eq!(
            parse_percent_from_line("42\u{00A0}% remaining"),
            Some(42.0)
        );
    }

    #[test]
    fn test_parse_percent_no_whitespace() {
        assert_eq!(parse_percent_from_line("75% remaining"), Some(75.0));
    }

    // -----------------------------------------------------------------------
    // extract_cli_field
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_cli_field_basic() {
        assert_eq!(
            extract_cli_field("Account: user@example.com\nOrg: test", "Account:"),
            Some("user@example.com".to_string())
        );
        assert_eq!(
            extract_cli_field("Account: user@example.com\nOrg: test", "Org:"),
            Some("test".to_string())
        );
    }

    #[test]
    fn test_extract_cli_field_case_insensitive() {
        assert_eq!(
            extract_cli_field("ACCOUNT: user@example.com", "Account:"),
            Some("user@example.com".to_string())
        );
    }

    #[test]
    fn test_extract_cli_field_missing() {
        assert_eq!(extract_cli_field("no label here", "Account:"), None);
    }

    // -----------------------------------------------------------------------
    // extract_reset_from_line
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_reset_from_line() {
        let result = extract_reset_from_line("Resets at 3pm (America/New_York)");
        assert_eq!(
            result.as_deref(),
            Some("Resets at 3pm (America/New_York)")
        );
    }

    #[test]
    fn test_extract_reset_from_line_no_reset() {
        assert_eq!(extract_reset_from_line("75% remaining"), None);
    }
}
