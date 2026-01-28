use crate::config::ProviderSourceMode;
use crate::models::{ProviderErrorPayload, ProviderIdentitySnapshot, ProviderPayload, RateWindow, UsageSnapshot};
use crate::provider_metadata::ProviderId;
use super::{Provider, ProviderFetchContext};
use regex::Regex;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration as StdDuration, Instant};
use time::{Date, Duration, Month, OffsetDateTime, Time};

pub struct KiroProvider;

impl Provider for KiroProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Kiro
    }

    fn display_name(&self) -> &'static str {
        "Kiro"
    }

    fn fetch(
        &self,
        context: &ProviderFetchContext,
    ) -> Result<ProviderPayload, ProviderErrorPayload> {
        let source_mode = context.source_mode();
        match source_mode {
            ProviderSourceMode::Auto | ProviderSourceMode::Cli => {}
            _ => {
                return Err(provider_error("Kiro supports Auto/CLI source modes only."));
            }
        }

        ensure_logged_in()?;
        let output = run_usage_command()?;
        let snapshot = parse_usage_output(&output, context.now)?;

        Ok(ProviderPayload {
            provider: ProviderId::Kiro.to_string(),
            account: None,
            version: detect_version(),
            source: "cli".to_string(),
            status: None,
            usage: Some(snapshot),
            credits: None,
            antigravity_plan_info: None,
            openai_dashboard: None,
            error: None,
        })
    }
}

fn detect_version() -> Option<String> {
    let output = Command::new("/usr/bin/env")
        .arg("kiro-cli")
        .arg("--version")
        .output()
        .ok()?;
    let combined = if output.stderr.is_empty() {
        output.stdout
    } else {
        output.stderr
    };
    let text = String::from_utf8_lossy(&combined).trim().to_string();
    if text.starts_with("kiro-cli ") {
        Some(text["kiro-cli ".len()..].trim().to_string())
    } else if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn ensure_logged_in() -> Result<(), ProviderErrorPayload> {
    let output = run_command(&["whoami"], StdDuration::from_secs(5))?;
    let combined = combine_output(&output.stdout, &output.stderr);
    let lowered = combined.to_lowercase();

    if lowered.contains("not logged in") || lowered.contains("login required") {
        return Err(provider_error("Not logged in to Kiro. Run 'kiro-cli login' first."));
    }

    if output.status_code != 0 {
        let message = if combined.is_empty() {
            format!("Kiro CLI failed with status {}.", output.status_code)
        } else {
            combined
        };
        return Err(provider_error(&message));
    }

    if combined.trim().is_empty() {
        return Err(provider_error("Kiro CLI whoami returned no output."));
    }

    Ok(())
}

fn run_usage_command() -> Result<String, ProviderErrorPayload> {
    let output = run_command(&["chat", "--no-interactive", "/usage"], StdDuration::from_secs(20))?;
    let combined = combine_output(&output.stdout, &output.stderr);
    let stripped = strip_ansi(&combined);
    let lowered = stripped.to_lowercase();

    if lowered.contains("not logged in")
        || lowered.contains("login required")
        || lowered.contains("failed to initialize auth portal")
        || lowered.contains("kiro-cli login")
        || lowered.contains("oauth error")
    {
        return Err(provider_error("Not logged in to Kiro. Run 'kiro-cli login' first."));
    }

    if output.timed_out && !is_usage_output_complete(&stripped) {
        return Err(provider_error("Kiro CLI timed out."));
    }

    if !output.stdout.trim().is_empty() {
        return Ok(output.stdout);
    }
    if !output.stderr.trim().is_empty() {
        return Ok(output.stderr);
    }

    if output.status_code != 0 {
        let message = if combined.is_empty() {
            format!("Kiro CLI failed with status {}.", output.status_code)
        } else {
            combined
        };
        return Err(provider_error(&message));
    }

    Ok(output.stdout)
}

struct CommandResult {
    stdout: String,
    stderr: String,
    status_code: i32,
    timed_out: bool,
}

fn run_command(args: &[&str], timeout: StdDuration) -> Result<CommandResult, ProviderErrorPayload> {
    let binary = which::which("kiro-cli")
        .map_err(|_| provider_error("kiro-cli not found. Install it from https://kiro.dev"))?;

    let mut command = Command::new(binary);
    command.args(args);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.stdin(Stdio::null());
    command.env("TERM", "xterm-256color");

    let mut child = command
        .spawn()
        .map_err(|error| provider_error(&format!("Failed to run kiro-cli: {error}")))?;

    let start = Instant::now();
    let mut timed_out = false;

    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    timed_out = true;
                    let _ = child.kill();
                    break;
                }
                thread::sleep(StdDuration::from_millis(100));
            }
            Err(error) => {
                return Err(provider_error(&format!(
                    "Failed to wait for kiro-cli: {error}"
                )));
            }
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|error| provider_error(&format!("Failed to read kiro-cli output: {error}")))?;

    Ok(CommandResult {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        status_code: output.status.code().unwrap_or(-1),
        timed_out,
    })
}

fn parse_usage_output(output: &str, now: OffsetDateTime) -> Result<UsageSnapshot, ProviderErrorPayload> {
    let stripped = strip_ansi(output);
    let trimmed = stripped.trim();
    if trimmed.is_empty() {
        return Err(provider_error("Empty output from kiro-cli."));
    }

    let lowered = stripped.to_lowercase();
    if lowered.contains("could not retrieve usage information") {
        return Err(provider_error("Kiro CLI could not retrieve usage information."));
    }

    if lowered.contains("not logged in")
        || lowered.contains("login required")
        || lowered.contains("failed to initialize auth portal")
        || lowered.contains("kiro-cli login")
        || lowered.contains("oauth error")
    {
        return Err(provider_error("Not logged in to Kiro. Run 'kiro-cli login' first."));
    }

    let mut matched_percent = false;
    let mut matched_credits = false;

    let plan_name = if let Some(capture) = Regex::new(r"\|\s*(KIRO\s+\w+)")
        .ok()
        .and_then(|regex| regex.captures(&stripped))
        .and_then(|captures| captures.get(1))
    {
        capture.as_str().trim().to_string()
    } else {
        "Kiro".to_string()
    };

    let resets_at = if let Some(capture) = Regex::new(r"resets on (\d{2}/\d{2})")
        .ok()
        .and_then(|regex| regex.captures(&stripped))
        .and_then(|captures| captures.get(1))
    {
        parse_reset_date(capture.as_str(), now)
    } else {
        None
    };

    let mut credits_percent = 0.0;
    if let Some(capture) = Regex::new(r"█+\s*(\d+)%")
        .ok()
        .and_then(|regex| regex.captures(&stripped))
        .and_then(|captures| captures.get(1))
    {
        credits_percent = capture.as_str().parse::<f64>().unwrap_or(0.0);
        matched_percent = true;
    }

    let mut credits_used = 0.0;
    let mut credits_total = 50.0;
    if let Some(captures) = Regex::new(r"\((\d+\.?\d*)\s+of\s+(\d+)\s+covered")
        .ok()
        .and_then(|regex| regex.captures(&stripped))
    {
        if let (Some(used), Some(total)) = (captures.get(1), captures.get(2)) {
            credits_used = used.as_str().parse::<f64>().unwrap_or(0.0);
            credits_total = total.as_str().parse::<f64>().unwrap_or(50.0);
            matched_credits = true;
        }
    }

    if !matched_percent && matched_credits && credits_total > 0.0 {
        credits_percent = (credits_used / credits_total) * 100.0;
    }

    let mut bonus_used: Option<f64> = None;
    let mut bonus_total: Option<f64> = None;
    if let Some(captures) = Regex::new(r"Bonus credits:\s*(\d+\.?\d*)/(\d+)")
        .ok()
        .and_then(|regex| regex.captures(&stripped))
    {
        bonus_used = captures.get(1).and_then(|value| value.as_str().parse::<f64>().ok());
        bonus_total = captures.get(2).and_then(|value| value.as_str().parse::<f64>().ok());
    }

    let bonus_expiry_days = Regex::new(r"expires in (\d+) days?")
        .ok()
        .and_then(|regex| regex.captures(&stripped))
        .and_then(|captures| captures.get(1))
        .and_then(|value| value.as_str().parse::<i64>().ok());

    if !matched_percent && !matched_credits {
        return Err(provider_error(
            "No recognizable usage patterns found. Kiro CLI output format may have changed.",
        ));
    }

    let primary = RateWindow {
        used_percent: credits_percent.max(0.0).min(100.0),
        window_minutes: None,
        resets_at,
        reset_description: None,
    };

    let secondary = if let (Some(bonus_used), Some(bonus_total)) = (bonus_used, bonus_total) {
        if bonus_total > 0.0 {
            let bonus_percent = (bonus_used / bonus_total) * 100.0;
            let expiry = bonus_expiry_days
                .and_then(|days| now.checked_add(Duration::days(days)));
            Some(RateWindow {
                used_percent: bonus_percent.max(0.0).min(100.0),
                window_minutes: None,
                resets_at: expiry,
                reset_description: bonus_expiry_days.map(|days| format!("expires in {days}d")),
            })
        } else {
            None
        }
    } else {
        None
    };

    let identity = ProviderIdentitySnapshot {
        provider_id: Some(ProviderId::Kiro.to_string()),
        account_email: None,
        account_organization: Some(plan_name.clone()),
        login_method: Some(plan_name),
    };

    Ok(UsageSnapshot {
        primary: Some(primary),
        secondary,
        tertiary: None,
        provider_cost: None,
        updated_at: now,
        identity: Some(identity),
    })
}

fn parse_reset_date(raw: &str, now: OffsetDateTime) -> Option<OffsetDateTime> {
    let parts: Vec<&str> = raw.split('/').collect();
    if parts.len() != 2 {
        return None;
    }
    let month = parts[0].parse::<u8>().ok()?;
    let day = parts[1].parse::<u8>().ok()?;
    let month = Month::try_from(month).ok()?;
    let current_year = now.year();

    let date = Date::from_calendar_date(current_year, month, day).ok()?;
    let candidate = OffsetDateTime::new_utc(date, Time::MIDNIGHT);
    if candidate > now {
        return Some(candidate);
    }

    let date = Date::from_calendar_date(current_year + 1, month, day).ok()?;
    Some(OffsetDateTime::new_utc(date, Time::MIDNIGHT))
}

fn strip_ansi(text: &str) -> String {
    let pattern = r"\x1B\[[0-9;?]*[A-Za-z]|\x1B\].*?\x07";
    if let Ok(regex) = Regex::new(pattern) {
        regex.replace_all(text, "").to_string()
    } else {
        text.to_string()
    }
}

fn combine_output(stdout: &str, stderr: &str) -> String {
    let trimmed_stdout = stdout.trim();
    let trimmed_stderr = stderr.trim();
    if trimmed_stderr.is_empty() {
        trimmed_stdout.to_string()
    } else if trimmed_stdout.is_empty() {
        trimmed_stderr.to_string()
    } else {
        format!("{trimmed_stdout}\n{trimmed_stderr}")
    }
}

fn is_usage_output_complete(output: &str) -> bool {
    let lowered = output.to_lowercase();
    lowered.contains("covered in plan")
        || lowered.contains("resets on")
        || lowered.contains("bonus credits")
}

fn provider_error(message: &str) -> ProviderErrorPayload {
    ProviderErrorPayload {
        code: 1,
        message: message.to_string(),
        kind: Some("provider".to_string()),
    }
}
