use crate::config::{AppConfig, ProviderCookieSource, ProviderSourceMode};
use crate::models::ProviderPayload;
use crate::provider_metadata::provider_metadata;
use crate::providers::{ProviderFetchContext, registry};
use crate::token_accounts::{
    TokenAccountInjection, is_claude_oauth_token, normalized_claude_oauth_token,
    normalized_cookie_header, token_account_support,
};
use anyhow::{Context, Result};
use serde_json::Value;
use std::process::Command;
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct FetchRequest {
    pub include_status: bool,
    pub provider: Option<String>,
    pub source: Option<String>,
    pub pretty: bool,
}

impl Default for FetchRequest {
    fn default() -> Self {
        Self {
            include_status: false,
            provider: None,
            source: None,
            pretty: false,
        }
    }
}

pub type ProviderPayloads = Vec<ProviderPayload>;

#[derive(Debug, Clone)]
pub struct FetchResult {
    pub payloads: ProviderPayloads,
    pub raw_json: String,
}

#[derive(thiserror::Error, Debug)]
pub enum BackendError {
    #[error("backend binary not found: {0}")]
    MissingBackend(String),
    #[error("backend exited with status {status}: {stderr}")]
    BackendFailure { status: i32, stderr: String },
    #[error("backend output was not valid UTF-8")]
    InvalidUtf8,
    #[error("backend returned invalid JSON: {0}")]
    InvalidJson(String),
}

pub fn fetch_usage(request: FetchRequest) -> Result<FetchResult> {
    let backend = resolve_backend()?;
    let mut command = Command::new(&backend);
    command.arg("usage");
    command.arg("--format");
    command.arg("json");
    command.arg("--json-only");
    if request.pretty {
        command.arg("--pretty");
    }
    if request.include_status {
        command.arg("--status");
    }
    if let Some(provider) = &request.provider {
        command.arg("--provider");
        command.arg(provider);
    }
    if let Some(source) = &request.source {
        command.arg("--source");
        command.arg(source);
    }

    let output = command.output().with_context(|| {
        format!("Failed to execute backend command: {}", backend.display())
    })?;

    if !output.status.success() {
        let status = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8(output.stderr).unwrap_or_else(|_| "<invalid utf8>".into());
        return Err(BackendError::BackendFailure { status, stderr }.into());
    }

    let stdout = String::from_utf8(output.stdout).map_err(|_| BackendError::InvalidUtf8)?;
    let value: Value = serde_json::from_str(stdout.trim())
        .map_err(|error| BackendError::InvalidJson(error.to_string()))?;
    let payloads: ProviderPayloads = if value.is_array() {
        serde_json::from_value(value).map_err(|error| BackendError::InvalidJson(error.to_string()))?
    } else {
        let payload: ProviderPayload =
            serde_json::from_value(value).map_err(|error| BackendError::InvalidJson(error.to_string()))?;
        vec![payload]
    };

    Ok(FetchResult {
        payloads,
        raw_json: stdout,
    })
}

pub fn fetch_native_usage(request: FetchRequest) -> Result<FetchResult> {
    let mut config = AppConfig::load_or_default()?;
    config.normalize();

    let now = OffsetDateTime::now_utc();
    let metadata = provider_metadata();
    let metadata_map: std::collections::HashMap<_, _> = metadata
        .iter()
        .map(|metadata| (metadata.id, metadata.default_enabled))
        .collect();

    let source_override = request
        .source
        .as_ref()
        .and_then(|raw| ProviderSourceMode::parse(raw));

    let mut payloads = Vec::new();
    let registry = registry();

    for provider_config in config.providers.iter() {
        let default_enabled = metadata_map
            .get(&provider_config.id)
            .copied()
            .unwrap_or(false);

        if !provider_config.is_enabled(default_enabled) {
            continue;
        }

        if let Some(filter) = &request.provider {
            if filter != "all" && filter.as_str() != provider_config.id.to_string() {
                continue;
            }
        }

        let provider = registry.provider(provider_config.id);
        let mut effective_config = provider_config.clone();
        let mut oauth_token_override: Option<String> = None;

        if let Some(token_accounts) = provider_config.token_accounts.as_ref() {
            if !token_accounts.accounts.is_empty() {
                if let Some(support) = token_account_support(provider_config.id) {
                    let active_index = token_accounts
                        .active_index
                        .min(token_accounts.accounts.len().saturating_sub(1));
                    if let Some(account) = token_accounts.accounts.get(active_index) {
                        let token = account.token.trim();
                        if !token.is_empty() {
                            match support.injection {
                                TokenAccountInjection::CookieHeader => {
                                    if provider_config.id == crate::provider_metadata::ProviderId::Claude
                                        && is_claude_oauth_token(token)
                                    {
                                        oauth_token_override = normalized_claude_oauth_token(token);
                                    } else {
                                        effective_config.cookie_header =
                                            Some(normalized_cookie_header(token, support));
                                        if support.requires_manual_cookie_source {
                                            effective_config.cookie_source =
                                                Some(ProviderCookieSource::Manual);
                                        }
                                    }
                                }
                                TokenAccountInjection::Environment { .. } => {
                                    effective_config.api_key = Some(token.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        let context = ProviderFetchContext {
            provider_id: provider_config.id,
            provider_config: effective_config,
            source_override: source_override.clone(),
            include_status: request.include_status,
            now,
            oauth_token_override,
        };

        match provider.fetch(&context) {
            Ok(payload) => payloads.push(payload),
            Err(error) => payloads.push(ProviderPayload {
                provider: provider_config.id.to_string(),
                account: None,
                version: None,
                source: source_override
                    .as_ref()
                    .map(|mode| mode.as_str().to_string())
                    .unwrap_or_else(|| "native".to_string()),
                status: None,
                usage: None,
                credits: None,
                antigravity_plan_info: None,
                openai_dashboard: None,
                error: Some(error),
            }),
        }
    }

    let raw_json = serde_json::to_string(&payloads)?;

    Ok(FetchResult { payloads, raw_json })
}

fn resolve_backend() -> Result<std::path::PathBuf> {
    if let Ok(path) = std::env::var("AGENT_STATS_BACKEND") {
        return Ok(std::path::PathBuf::from(path));
    }
    if let Ok(path) = std::env::var("CODEXBAR_BIN") {
        return Ok(std::path::PathBuf::from(path));
    }

    let path = which::which("codexbar")
        .map_err(|_| BackendError::MissingBackend("codexbar".into()))?;
    Ok(path)
}
