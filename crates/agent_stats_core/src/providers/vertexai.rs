use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use crate::config::ProviderSourceMode;
use crate::models::{ProviderErrorPayload, ProviderIdentitySnapshot, ProviderPayload, RateWindow, UsageSnapshot};
use crate::provider_metadata::ProviderId;
use super::{Provider, ProviderFetchContext};
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use time::{Duration, OffsetDateTime};

const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const MONITORING_ENDPOINT: &str = "https://monitoring.googleapis.com/v3/projects";
const USAGE_WINDOW_SECONDS: i64 = 24 * 60 * 60;

pub struct VertexAIProvider;

impl Provider for VertexAIProvider {
    fn id(&self) -> ProviderId {
        ProviderId::VertexAI
    }

    fn display_name(&self) -> &'static str {
        "Vertex AI"
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
                    "Vertex AI supports Auto/Api source modes only.",
                ));
            }
        }

        let mut credentials = load_credentials()?;
        if credentials.needs_refresh(context.now) {
            credentials = refresh_credentials(&credentials, context.now)?;
        }

        let project_id = credentials
            .project_id
            .clone()
            .ok_or_else(|| provider_error("No Google Cloud project configured."))?;

        let usage_percent = fetch_quota_usage(&credentials.access_token, &project_id, context.now)?;

        let primary = RateWindow {
            used_percent: usage_percent,
            window_minutes: None,
            resets_at: None,
            reset_description: None,
        };

        let identity = ProviderIdentitySnapshot {
            provider_id: Some(ProviderId::VertexAI.to_string()),
            account_email: credentials.email.clone(),
            account_organization: None,
            login_method: Some(project_id.clone()),
        };

        let usage_snapshot = UsageSnapshot {
            primary: Some(primary),
            secondary: None,
            tertiary: None,
            provider_cost: None,
            updated_at: context.now,
            identity: Some(identity),
        };

        Ok(ProviderPayload {
            provider: ProviderId::VertexAI.to_string(),
            account: credentials.email,
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

#[derive(Clone)]
struct VertexCredentials {
    access_token: String,
    refresh_token: String,
    client_id: String,
    client_secret: String,
    project_id: Option<String>,
    email: Option<String>,
    expiry_date: Option<OffsetDateTime>,
}

impl VertexCredentials {
    fn needs_refresh(&self, now: OffsetDateTime) -> bool {
        match self.expiry_date {
            Some(expiry) => now + Duration::minutes(5) >= expiry,
            None => true,
        }
    }
}

fn load_credentials() -> Result<VertexCredentials, ProviderErrorPayload> {
    let path = credentials_path()?;
    let data = fs::read(&path)
        .map_err(|_| provider_error("gcloud credentials not found. Run `gcloud auth application-default login`."))?;
    parse_credentials(&data)
}

fn credentials_path() -> Result<PathBuf, ProviderErrorPayload> {
    if let Ok(config_dir) = std::env::var("CLOUDSDK_CONFIG") {
        let trimmed = config_dir.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed).join("application_default_credentials.json"));
        }
    }

    let home = dirs::home_dir().ok_or_else(|| provider_error("Could not resolve home directory."))?;
    Ok(home
        .join(".config")
        .join("gcloud")
        .join("application_default_credentials.json"))
}

fn project_config_path() -> Result<PathBuf, ProviderErrorPayload> {
    if let Ok(config_dir) = std::env::var("CLOUDSDK_CONFIG") {
        let trimmed = config_dir.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed)
                .join("configurations")
                .join("config_default"));
        }
    }

    let home = dirs::home_dir().ok_or_else(|| provider_error("Could not resolve home directory."))?;
    Ok(home
        .join(".config")
        .join("gcloud")
        .join("configurations")
        .join("config_default"))
}

fn parse_credentials(data: &[u8]) -> Result<VertexCredentials, ProviderErrorPayload> {
    let json: serde_json::Value = serde_json::from_slice(data)
        .map_err(|error| provider_error(&format!("Failed to decode gcloud credentials: {error}")))?;

    if json.get("client_email").is_some() && json.get("private_key").is_some() {
        return Err(provider_error(
            "Service account credentials not yet supported. Use `gcloud auth application-default login`.",
        ));
    }

    let client_id = json
        .get("client_id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| provider_error("gcloud credentials missing client ID."))?
        .to_string();
    let client_secret = json
        .get("client_secret")
        .and_then(|value| value.as_str())
        .ok_or_else(|| provider_error("gcloud credentials missing client secret."))?
        .to_string();

    let refresh_token = json
        .get("refresh_token")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| provider_error("gcloud credentials contain no refresh token."))?
        .to_string();

    let access_token = json
        .get("access_token")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();

    let expiry_date = json
        .get("token_expiry")
        .and_then(|value| value.as_str())
        .and_then(|raw| OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339).ok());

    let project_id = load_project_id();
    let email = json
        .get("id_token")
        .and_then(|value| value.as_str())
        .and_then(extract_email_from_id_token);

    Ok(VertexCredentials {
        access_token,
        refresh_token,
        client_id,
        client_secret,
        project_id,
        email,
        expiry_date,
    })
}

fn load_project_id() -> Option<String> {
    if let Ok(value) = std::env::var("GOOGLE_CLOUD_PROJECT") {
        if !value.trim().is_empty() {
            return Some(value.trim().to_string());
        }
    }
    if let Ok(value) = std::env::var("GCLOUD_PROJECT") {
        if !value.trim().is_empty() {
            return Some(value.trim().to_string());
        }
    }
    if let Ok(value) = std::env::var("CLOUDSDK_CORE_PROJECT") {
        if !value.trim().is_empty() {
            return Some(value.trim().to_string());
        }
    }

    let path = project_config_path().ok()?;
    let content = fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("project") {
            let parts: Vec<&str> = trimmed.split('=').collect();
            if parts.len() >= 2 {
                return Some(parts[1].trim().to_string());
            }
        }
    }
    None
}

fn extract_email_from_id_token(token: &str) -> Option<String> {
    let mut parts = token.split('.');
    let _ = parts.next()?;
    let payload = parts.next()?;

    let payload_data = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&payload_data).ok()?;
    json.get("email")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
}

fn refresh_credentials(
    credentials: &VertexCredentials,
    now: OffsetDateTime,
) -> Result<VertexCredentials, ProviderErrorPayload> {
    if credentials.refresh_token.is_empty() {
        return Err(provider_error("No refresh token available."));
    }

    let body = format!(
        "client_id={}&client_secret={}&refresh_token={}&grant_type=refresh_token",
        urlencoding::encode(&credentials.client_id),
        urlencoding::encode(&credentials.client_secret),
        urlencoding::encode(&credentials.refresh_token)
    );

    let response = ureq::post(TOKEN_ENDPOINT)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_string(&body);

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            if code == 400 || code == 401 {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
                    if let Some(error_code) = value.get("error").and_then(|value| value.as_str()) {
                        if error_code.eq_ignore_ascii_case("invalid_grant") {
                            return Err(provider_error(
                                "Refresh token expired. Run `gcloud auth application-default login` again.",
                            ));
                        }
                        if error_code.eq_ignore_ascii_case("unauthorized_client") {
                            return Err(provider_error(
                                "Refresh token was revoked. Run `gcloud auth application-default login` again.",
                            ));
                        }
                    }
                }
                return Err(provider_error("Refresh token expired."));
            }
            return Err(provider_error(&format!(
                "Invalid refresh response (HTTP {code}): {body}"
            )));
        }
        Err(error) => {
            return Err(provider_error(&format!(
                "Network error during token refresh: {error}"
            )));
        }
    };

    let body = response
        .into_string()
        .map_err(|error| provider_error(&format!("Invalid refresh response: {error}")))?;

    let json: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| provider_error("Invalid refresh response."))?;

    let access_token = json
        .get("access_token")
        .and_then(|value| value.as_str())
        .unwrap_or(&credentials.access_token)
        .to_string();
    let expires_in = json
        .get("expires_in")
        .and_then(|value| value.as_f64())
        .unwrap_or(3600.0);
    let expiry_date = now + Duration::seconds_f64(expires_in);
    let email = json
        .get("id_token")
        .and_then(|value| value.as_str())
        .and_then(extract_email_from_id_token)
        .or_else(|| credentials.email.clone());

    Ok(VertexCredentials {
        access_token,
        refresh_token: credentials.refresh_token.clone(),
        client_id: credentials.client_id.clone(),
        client_secret: credentials.client_secret.clone(),
        project_id: credentials.project_id.clone(),
        email,
        expiry_date: Some(expiry_date),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MonitoringTimeSeriesResponse {
    time_series: Option<Vec<MonitoringTimeSeries>>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MonitoringTimeSeries {
    metric: MonitoringMetric,
    resource: MonitoringResource,
    points: Vec<MonitoringPoint>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MonitoringMetric {
    labels: Option<std::collections::HashMap<String, String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MonitoringResource {
    labels: Option<std::collections::HashMap<String, String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MonitoringPoint {
    value: MonitoringValue,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MonitoringValue {
    double_value: Option<f64>,
    int64_value: Option<String>,
}

#[derive(Hash, PartialEq, Eq)]
struct QuotaKey {
    quota_metric: String,
    limit_name: String,
    location: String,
}

fn fetch_quota_usage(
    access_token: &str,
    project_id: &str,
    now: OffsetDateTime,
) -> Result<f64, ProviderErrorPayload> {
    let usage_filter = "metric.type=\"serviceruntime.googleapis.com/quota/allocation/usage\" AND resource.type=\"consumer_quota\" AND resource.label.service=\"aiplatform.googleapis.com\"";
    let limit_filter = "metric.type=\"serviceruntime.googleapis.com/quota/limit\" AND resource.type=\"consumer_quota\" AND resource.label.service=\"aiplatform.googleapis.com\"";

    let usage_series = fetch_time_series(access_token, project_id, usage_filter, now)?;
    let limit_series = fetch_time_series(access_token, project_id, limit_filter, now)?;

    let usage_by_key = aggregate_series(&usage_series);
    let limit_by_key = aggregate_series(&limit_series);

    if usage_by_key.is_empty() || limit_by_key.is_empty() {
        return Err(provider_error("No Vertex AI usage data found for the current project."));
    }

    let mut max_percent: Option<f64> = None;
    let mut matched = 0;
    for (key, limit) in limit_by_key.iter() {
        if *limit <= 0.0 {
            continue;
        }
        if let Some(usage) = usage_by_key.get(key) {
            let percent = (usage / limit) * 100.0;
            max_percent = Some(max_percent.map_or(percent, |current| current.max(percent)));
            matched += 1;
        }
    }

    if matched == 0 {
        return Err(provider_error("No Vertex AI usage data found for the current project."));
    }

    Ok(max_percent.unwrap_or(0.0).max(0.0).min(100.0))
}

fn fetch_time_series(
    access_token: &str,
    project_id: &str,
    filter: &str,
    now: OffsetDateTime,
) -> Result<Vec<MonitoringTimeSeries>, ProviderErrorPayload> {
    let start = now - Duration::seconds(USAGE_WINDOW_SECONDS);
    let start_str = start
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| provider_error(&format!("Failed to format date: {error}")))?;
    let end_str = now
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| provider_error(&format!("Failed to format date: {error}")))?;

    let mut page_token: Option<String> = None;
    let mut all_series = Vec::new();

    loop {
        let url = format!("{MONITORING_ENDPOINT}/{project_id}/timeSeries");
        let mut request = ureq::get(&url)
            .set("Authorization", &format!("Bearer {access_token}"))
            .query("filter", filter)
            .query("interval.startTime", &start_str)
            .query("interval.endTime", &end_str)
            .query("aggregation.alignmentPeriod", "3600s")
            .query("aggregation.perSeriesAligner", "ALIGN_MAX")
            .query("view", "FULL");

        if let Some(token) = page_token.as_ref() {
            request = request.query("pageToken", token);
        }

        let response = request.call();
        let response = match response {
            Ok(response) => response,
            Err(ureq::Error::Status(code, response)) => {
                let body = response.into_string().unwrap_or_default();
                if code == 401 {
                    return Err(provider_error(
                        "Vertex AI request unauthorized. Run `gcloud auth application-default login`.",
                    ));
                }
                if code == 403 {
                    return Err(provider_error(
                        "Access forbidden. Check your IAM permissions for Cloud Monitoring.",
                    ));
                }
                return Err(provider_error(&format!(
                    "Vertex AI response was invalid: HTTP {code}: {body}"
                )));
            }
            Err(error) => {
                return Err(provider_error(&format!("Vertex AI network error: {error}")));
            }
        };

        let body = response
            .into_string()
            .map_err(|error| provider_error(&format!("Vertex AI response invalid: {error}")))?;

        let decoded: MonitoringTimeSeriesResponse = serde_json::from_str(&body)
            .map_err(|error| provider_error(&format!("Vertex AI response invalid: {error}")))?;

        if let Some(series) = decoded.time_series {
            all_series.extend(series);
        }

        page_token = decoded.next_page_token.filter(|token| !token.is_empty());
        if page_token.is_none() {
            break;
        }
    }

    Ok(all_series)
}

fn aggregate_series(series: &[MonitoringTimeSeries]) -> std::collections::HashMap<QuotaKey, f64> {
    let mut buckets = std::collections::HashMap::new();
    for entry in series {
        let key = match quota_key(entry) {
            Some(key) => key,
            None => continue,
        };
        let value = match max_point_value(&entry.points) {
            Some(value) => value,
            None => continue,
        };
        let entry_value = buckets.entry(key).or_insert(0.0);
        if value > *entry_value {
            *entry_value = value;
        }
    }
    buckets
}

fn quota_key(series: &MonitoringTimeSeries) -> Option<QuotaKey> {
    let metric_labels = series.metric.labels.as_ref();
    let resource_labels = series.resource.labels.as_ref();

    let quota_metric = metric_labels
        .and_then(|labels| labels.get("quota_metric"))
        .cloned()
        .or_else(|| {
            resource_labels
                .and_then(|labels| labels.get("quota_id"))
                .cloned()
        })?;

    if quota_metric.is_empty() {
        return None;
    }

    let limit_name = metric_labels
        .and_then(|labels| labels.get("limit_name"))
        .cloned()
        .unwrap_or_default();

    let location = resource_labels
        .and_then(|labels| labels.get("location"))
        .cloned()
        .unwrap_or_else(|| "global".to_string());

    Some(QuotaKey {
        quota_metric,
        limit_name,
        location,
    })
}

fn max_point_value(points: &[MonitoringPoint]) -> Option<f64> {
    points.iter().filter_map(point_value).max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
}

fn point_value(point: &MonitoringPoint) -> Option<f64> {
    if let Some(value) = point.value.double_value {
        return Some(value);
    }
    if let Some(raw) = point.value.int64_value.as_ref() {
        return raw.parse::<f64>().ok();
    }
    None
}

fn provider_error(message: &str) -> ProviderErrorPayload {
    ProviderErrorPayload {
        code: 1,
        message: message.to_string(),
        kind: Some("provider".to_string()),
    }
}
