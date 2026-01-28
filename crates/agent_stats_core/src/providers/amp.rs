use crate::config::ProviderSourceMode;
use crate::models::{ProviderErrorPayload, ProviderIdentitySnapshot, ProviderPayload, RateWindow, UsageSnapshot};
use crate::provider_metadata::ProviderId;
use super::{Provider, ProviderFetchContext};
use regex::Regex;
use time::{Duration, OffsetDateTime};

const SETTINGS_URL: &str = "https://ampcode.com/settings";
const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";

pub struct AmpProvider;

impl Provider for AmpProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Amp
    }

    fn display_name(&self) -> &'static str {
        "Amp"
    }

    fn fetch(
        &self,
        context: &ProviderFetchContext,
    ) -> Result<ProviderPayload, ProviderErrorPayload> {
        let source_mode = context.source_mode();
        match source_mode {
            ProviderSourceMode::Auto | ProviderSourceMode::Web => {}
            _ => {
                return Err(provider_error("Amp supports Auto/Web source modes only."));
            }
        }

        let cookie_header = context
            .provider_config
            .cookie_header
            .clone()
            .unwrap_or_default();

        if cookie_header.trim().is_empty() {
            return Err(provider_error(
                "Amp cookie header missing. Paste a Cookie header from ampcode.com.",
            ));
        }

        let html = fetch_settings_html(&cookie_header)?;
        let snapshot = parse_usage(&html, context.now)?;

        Ok(ProviderPayload {
            provider: ProviderId::Amp.to_string(),
            account: None,
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

fn fetch_settings_html(cookie_header: &str) -> Result<String, ProviderErrorPayload> {
    let response = ureq::get(SETTINGS_URL)
        .set("Cookie", cookie_header)
        .set("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .set("User-Agent", USER_AGENT)
        .set("Accept-Language", "en-US,en;q=0.9")
        .set("Origin", "https://ampcode.com")
        .set("Referer", SETTINGS_URL)
        .call();

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            return Err(status_error(code, &body));
        }
        Err(error) => {
            return Err(provider_error(&format!("Amp network error: {error}")));
        }
    };

    let body = response
        .into_string()
        .map_err(|error| provider_error(&format!("Amp response invalid: {error}")))?;

    Ok(body)
}

fn parse_usage(html: &str, now: OffsetDateTime) -> Result<UsageSnapshot, ProviderErrorPayload> {
    let object = extract_usage_object(html).ok_or_else(|| {
        if looks_signed_out(html) {
            provider_error("Not logged in to Amp. Please log in to ampcode.com.")
        } else {
            provider_error("Missing Amp Free usage data.")
        }
    })?;

    let quota = number_for("quota", &object).ok_or_else(|| {
        provider_error("Missing Amp quota value.")
    })?;
    let used = number_for("used", &object).ok_or_else(|| {
        provider_error("Missing Amp used value.")
    })?;
    let hourly = number_for("hourlyReplenishment", &object).ok_or_else(|| {
        provider_error("Missing Amp hourly replenishment value.")
    })?;
    let window_hours = number_for("windowHours", &object);

    let quota = quota.max(0.0);
    let used = used.max(0.0);
    let percent = if quota > 0.0 {
        (used / quota * 100.0).min(100.0)
    } else {
        0.0
    };

    let window_minutes = window_hours
        .and_then(|hours| if hours > 0.0 { Some((hours * 60.0).round() as i64) } else { None });

    let resets_at = if quota > 0.0 && hourly > 0.0 {
        let hours_to_full = used / hourly;
        let seconds = (hours_to_full * 3600.0).max(0.0);
        Some(now + Duration::seconds_f64(seconds))
    } else {
        None
    };

    let primary = RateWindow {
        used_percent: percent,
        window_minutes,
        resets_at,
        reset_description: None,
    };

    let identity = ProviderIdentitySnapshot {
        provider_id: Some(ProviderId::Amp.to_string()),
        account_email: None,
        account_organization: None,
        login_method: Some("Amp Free".to_string()),
    };

    Ok(UsageSnapshot {
        primary: Some(primary),
        secondary: None,
        tertiary: None,
        provider_cost: None,
        updated_at: now,
        identity: Some(identity),
    })
}

fn extract_usage_object(html: &str) -> Option<String> {
    for token in ["freeTierUsage", "getFreeTierUsage"] {
        if let Some(object) = extract_object_named(token, html) {
            return Some(object);
        }
    }
    None
}

fn extract_object_named(token: &str, html: &str) -> Option<String> {
    let token_index = html.find(token)?;
    let after = &html[token_index + token.len()..];
    let brace_offset = after.find('{')?;
    let start = token_index + token.len() + brace_offset;
    let tail = &html[start..];
    let end_offset = find_matching_brace(tail)?;
    Some(tail[..=end_offset].to_string())
}

fn number_for(key: &str, text: &str) -> Option<f64> {
    let pattern = format!(r"\b{}\b\s*:\s*([0-9]+(?:\.[0-9]+)?)", regex::escape(key));
    let regex = Regex::new(&pattern).ok()?;
    let captures = regex.captures(text)?;
    captures
        .get(1)
        .and_then(|value| value.as_str().parse::<f64>().ok())
}

fn looks_signed_out(html: &str) -> bool {
    let lower = html.to_lowercase();
    lower.contains("sign in")
        || lower.contains("log in")
        || lower.contains("login")
        || lower.contains("/login")
        || lower.contains("ampcode.com/login")
}

fn find_matching_brace(text: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;

    for (index, ch) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
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

fn status_error(code: u16, body: &str) -> ProviderErrorPayload {
    if code == 401 || code == 403 {
        return provider_error("Amp session cookie expired or invalid.");
    }
    let snippet = body.trim();
    let snippet = if snippet.is_empty() {
        String::new()
    } else {
        format!(": {}", snippet.chars().take(200).collect::<String>())
    };
    provider_error(&format!("Amp request failed (HTTP {code}){snippet}"))
}

fn provider_error(message: &str) -> ProviderErrorPayload {
    ProviderErrorPayload {
        code: 1,
        message: message.to_string(),
        kind: Some("provider".to_string()),
    }
}
