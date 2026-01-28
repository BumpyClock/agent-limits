use crate::config::ProviderSourceMode;
use crate::models::{ProviderErrorPayload, ProviderIdentitySnapshot, ProviderPayload, RateWindow, UsageSnapshot};
use crate::provider_metadata::ProviderId;
use super::{Provider, ProviderFetchContext};
use regex::Regex;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use time::{Duration, OffsetDateTime};

pub struct JetBrainsProvider;

impl Provider for JetBrainsProvider {
    fn id(&self) -> ProviderId {
        ProviderId::JetBrains
    }

    fn display_name(&self) -> &'static str {
        "JetBrains AI"
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
                    "JetBrains supports Auto/Web source modes only.",
                ));
            }
        }

        let ide = detect_latest_ide().ok_or_else(|| {
            provider_error(
                "No JetBrains IDE with AI Assistant detected. Install a JetBrains IDE and enable AI Assistant.",
            )
        })?;

        let snapshot = parse_quota_file(&ide.quota_file_path, ide.display_name.clone())?;

        Ok(ProviderPayload {
            provider: ProviderId::JetBrains.to_string(),
            account: None,
            version: None,
            source: "local".to_string(),
            status: None,
            usage: Some(snapshot),
            credits: None,
            antigravity_plan_info: None,
            openai_dashboard: None,
            error: None,
        })
    }
}

#[derive(Clone)]
struct JetBrainsIDEInfo {
    quota_file_path: String,
    display_name: String,
}

fn detect_latest_ide() -> Option<JetBrainsIDEInfo> {
    let mut candidates = Vec::new();
    for base in jetbrains_base_paths() {
        let Ok(entries) = fs::read_dir(&base) else { continue };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let dirname = file_name.to_string_lossy();
            if let Some(info) = parse_ide_directory(&dirname, &base) {
                if Path::new(&info.quota_file_path).exists() {
                    candidates.push(info);
                }
            }
        }
    }

    if candidates.is_empty() {
        return None;
    }

    candidates.sort_by(|a, b| a.display_name.cmp(&b.display_name));

    let mut latest = candidates[0].clone();
    let mut latest_time = fs::metadata(&latest.quota_file_path)
        .and_then(|meta| meta.modified())
        .ok();

    for candidate in candidates.into_iter() {
        let modified = fs::metadata(&candidate.quota_file_path)
            .and_then(|meta| meta.modified())
            .ok();
        if let (Some(current), Some(best)) = (modified, latest_time) {
            if current > best {
                latest = candidate;
                latest_time = Some(current);
            }
        }
    }

    Some(latest)
}

fn jetbrains_base_paths() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    if cfg!(target_os = "macos") {
        vec![
            home.join("Library/Application Support/JetBrains"),
            home.join("Library/Application Support/Google"),
        ]
    } else {
        vec![
            home.join(".config/JetBrains"),
            home.join(".local/share/JetBrains"),
            home.join(".config/Google"),
        ]
    }
}

fn parse_ide_directory(dirname: &str, base_path: &Path) -> Option<JetBrainsIDEInfo> {
    let patterns = [
        ("IntelliJIdea", "IntelliJ IDEA"),
        ("PyCharm", "PyCharm"),
        ("WebStorm", "WebStorm"),
        ("GoLand", "GoLand"),
        ("CLion", "CLion"),
        ("DataGrip", "DataGrip"),
        ("RubyMine", "RubyMine"),
        ("Rider", "Rider"),
        ("PhpStorm", "PhpStorm"),
        ("AppCode", "AppCode"),
        ("Fleet", "Fleet"),
        ("AndroidStudio", "Android Studio"),
        ("RustRover", "RustRover"),
        ("Aqua", "Aqua"),
        ("DataSpell", "DataSpell"),
    ];

    let lower = dirname.to_lowercase();
    for (prefix, display_name) in patterns {
        if lower.starts_with(&prefix.to_lowercase()) {
            let version_part = dirname.strip_prefix(prefix).unwrap_or("");
            let version = if version_part.is_empty() {
                "Unknown"
            } else {
                version_part
            };
            let base = base_path.join(dirname);
            let quota_file_path = base.join("options").join("AIAssistantQuotaManager2.xml");
            let display_name = format!("{} {}", display_name, version);
            return Some(JetBrainsIDEInfo {
                quota_file_path: quota_file_path.to_string_lossy().to_string(),
                display_name,
            });
        }
    }
    None
}

fn parse_quota_file(path: &str, ide_name: String) -> Result<UsageSnapshot, ProviderErrorPayload> {
    if !Path::new(path).exists() {
        return Err(provider_error(&format!(
            "JetBrains AI quota file not found at {path}. Enable AI Assistant in your IDE.",
        )));
    }

    let content = fs::read_to_string(path)
        .map_err(|error| provider_error(&format!("Failed to read JetBrains quota file: {error}")))?;

    let component_block = extract_component_block(&content).ok_or_else(|| {
        provider_error("No quota information found in the JetBrains AI configuration.")
    })?;

    let quota_raw = extract_option_value(&component_block, "quotaInfo").ok_or_else(|| {
        provider_error("No quota information found in the JetBrains AI configuration.")
    })?;
    let quota_decoded = decode_html_entities(&quota_raw);
    let quota = parse_quota_info(&quota_decoded)?;

    let refill_raw = extract_option_value(&component_block, "nextRefill");
    let refill = refill_raw
        .map(|value| decode_html_entities(&value))
        .and_then(|decoded| parse_refill_info(&decoded).ok());

    let reset_description = format_reset_description(refill.as_ref().and_then(|info| info.next));

    let primary = RateWindow {
        used_percent: quota.used_percent,
        window_minutes: None,
        resets_at: refill.as_ref().and_then(|info| info.next),
        reset_description,
    };

    let identity = ProviderIdentitySnapshot {
        provider_id: Some(ProviderId::JetBrains.to_string()),
        account_email: None,
        account_organization: Some(ide_name),
        login_method: quota.quota_type.clone(),
    };

    Ok(UsageSnapshot {
        primary: Some(primary),
        secondary: None,
        tertiary: None,
        provider_cost: None,
        updated_at: OffsetDateTime::now_utc(),
        identity: Some(identity),
    })
}

fn extract_component_block(content: &str) -> Option<String> {
    let pattern = r#"<component[^>]*name\s*=\s*["']AIAssistantQuotaManager2["'][^>]*>[\s\S]*?</component>"#;
    let regex = Regex::new(pattern).ok()?;
    let capture = regex.find(content)?;
    Some(content[capture.start()..capture.end()].to_string())
}

fn extract_option_value(content: &str, name: &str) -> Option<String> {
    let patterns = [
        format!(r#"<option[^>]*name\s*=\s*["']{name}["'][^>]*value\s*=\s*["']([^"']*)["']"#),
        format!(r#"<option[^>]*value\s*=\s*["']([^"']*)["'][^>]*name\s*=\s*["']{name}["']"#),
    ];

    for pattern in patterns {
        if let Ok(regex) = Regex::new(&pattern) {
            if let Some(capture) = regex.captures(content).and_then(|captures| captures.get(1)) {
                let value = capture.as_str().trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

fn decode_html_entities(raw: &str) -> String {
    raw.replace("&#10;", "\n")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&apos;", "'")
}

struct QuotaInfo {
    quota_type: Option<String>,
    used_percent: f64,
}

fn parse_quota_info(json: &str) -> Result<QuotaInfo, ProviderErrorPayload> {
    let value: Value = serde_json::from_str(json)
        .map_err(|error| provider_error(&format!("Invalid JSON format: {error}")))?;

    let quota_type = value.get("type").and_then(|value| value.as_str()).map(|value| value.to_string());
    let used = value.get("current").and_then(string_to_double).unwrap_or(0.0);
    let maximum = value.get("maximum").and_then(string_to_double).unwrap_or(0.0);
    let used_percent = if maximum > 0.0 {
        (used / maximum * 100.0).min(100.0).max(0.0)
    } else {
        0.0
    };

    Ok(QuotaInfo {
        quota_type,
        used_percent,
    })
}

struct RefillInfo {
    next: Option<OffsetDateTime>,
}

fn parse_refill_info(json: &str) -> Result<RefillInfo, ProviderErrorPayload> {
    let value: Value = serde_json::from_str(json)
        .map_err(|error| provider_error(&format!("Invalid JSON format: {error}")))?;
    let next = value.get("next").and_then(string_to_datetime);
    Ok(RefillInfo { next })
}

fn string_to_double(value: &Value) -> Option<f64> {
    match value {
        Value::String(raw) => raw.parse::<f64>().ok(),
        Value::Number(number) => number.as_f64(),
        _ => None,
    }
}

fn string_to_datetime(value: &Value) -> Option<OffsetDateTime> {
    match value {
        Value::String(raw) => OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339).ok(),
        _ => None,
    }
}

fn format_reset_description(date: Option<OffsetDateTime>) -> Option<String> {
    let date = date?;
    let now = OffsetDateTime::now_utc();
    let interval = date - now;
    if interval <= Duration::ZERO {
        return Some("Expired".to_string());
    }

    let total_minutes = interval.whole_minutes();
    let hours = total_minutes / 60;
    let minutes = total_minutes % 60;

    if hours > 24 {
        let days = hours / 24;
        let remaining_hours = hours % 24;
        Some(format!("Resets in {days}d {remaining_hours}h"))
    } else if hours > 0 {
        Some(format!("Resets in {hours}h {minutes}m"))
    } else {
        Some(format!("Resets in {minutes}m"))
    }
}

fn provider_error(message: &str) -> ProviderErrorPayload {
    ProviderErrorPayload {
        code: 1,
        message: message.to_string(),
        kind: Some("provider".to_string()),
    }
}
