use agent_stats_core::config::{AppConfig, ProviderSourceMode};
use agent_stats_core::provider_metadata::ProviderId;
use agent_stats_core::token_accounts::token_account_support;
use gpui::{AppContext, Context, Entity, SharedString, Window};
use gpui_component::input::InputState;
use std::collections::HashMap;

use super::state::AgentStatsApp;
use super::types::{ProviderSettingsForm, TokenAccountForm};

const SOURCE_MODES_NONE: &[ProviderSourceMode] = &[];
const SOURCE_MODES_AUTO_WEB: &[ProviderSourceMode] =
    &[ProviderSourceMode::Auto, ProviderSourceMode::Web];
const SOURCE_MODES_AUTO_API: &[ProviderSourceMode] =
    &[ProviderSourceMode::Auto, ProviderSourceMode::Api];
const SOURCE_MODES_AUTO_API_OAUTH: &[ProviderSourceMode] = &[
    ProviderSourceMode::Auto,
    ProviderSourceMode::Api,
    ProviderSourceMode::Oauth,
];
const SOURCE_MODES_AUTO_WEB_API: &[ProviderSourceMode] = &[
    ProviderSourceMode::Auto,
    ProviderSourceMode::Web,
    ProviderSourceMode::Api,
];
const SOURCE_MODES_AUTO_CLI: &[ProviderSourceMode] =
    &[ProviderSourceMode::Auto, ProviderSourceMode::Cli];
const SOURCE_MODES_AUTO_OAUTH_CLI: &[ProviderSourceMode] = &[
    ProviderSourceMode::Auto,
    ProviderSourceMode::Oauth,
    ProviderSourceMode::Cli,
];
const SOURCE_MODES_AUTO_OAUTH_WEB_CLI: &[ProviderSourceMode] = &[
    ProviderSourceMode::Auto,
    ProviderSourceMode::Oauth,
    ProviderSourceMode::Web,
    ProviderSourceMode::Cli,
];

#[derive(Clone, Copy)]
pub(crate) struct ProviderSettingsSpec {
    pub(crate) source_modes: &'static [ProviderSourceMode],
    pub(crate) show_cookie_header: bool,
    pub(crate) show_api_key: bool,
    pub(crate) show_region: bool,
    pub(crate) show_workspace_id: bool,
}

impl ProviderSettingsSpec {
    pub(crate) fn is_visible(&self) -> bool {
        !self.source_modes.is_empty()
            || self.show_cookie_header
            || self.show_api_key
            || self.show_region
            || self.show_workspace_id
    }
}

pub(crate) fn provider_settings_spec(provider_id: ProviderId) -> ProviderSettingsSpec {
    match provider_id {
        ProviderId::Codex => ProviderSettingsSpec {
            source_modes: SOURCE_MODES_AUTO_OAUTH_CLI,
            show_cookie_header: false,
            show_api_key: false,
            show_region: false,
            show_workspace_id: false,
        },
        ProviderId::Claude => ProviderSettingsSpec {
            source_modes: SOURCE_MODES_AUTO_OAUTH_WEB_CLI,
            show_cookie_header: true,
            show_api_key: false,
            show_region: false,
            show_workspace_id: false,
        },
        ProviderId::Cursor => ProviderSettingsSpec {
            source_modes: SOURCE_MODES_AUTO_WEB,
            show_cookie_header: true,
            show_api_key: false,
            show_region: false,
            show_workspace_id: false,
        },
        ProviderId::OpenCode => ProviderSettingsSpec {
            source_modes: SOURCE_MODES_AUTO_WEB,
            show_cookie_header: true,
            show_api_key: false,
            show_region: false,
            show_workspace_id: true,
        },
        ProviderId::Factory => ProviderSettingsSpec {
            source_modes: SOURCE_MODES_AUTO_WEB,
            show_cookie_header: true,
            show_api_key: false,
            show_region: false,
            show_workspace_id: false,
        },
        ProviderId::Gemini => ProviderSettingsSpec {
            source_modes: SOURCE_MODES_AUTO_API_OAUTH,
            show_cookie_header: false,
            show_api_key: false,
            show_region: false,
            show_workspace_id: false,
        },
        ProviderId::Antigravity => ProviderSettingsSpec {
            source_modes: SOURCE_MODES_NONE,
            show_cookie_header: false,
            show_api_key: false,
            show_region: false,
            show_workspace_id: false,
        },
        ProviderId::Copilot => ProviderSettingsSpec {
            source_modes: SOURCE_MODES_AUTO_API,
            show_cookie_header: false,
            show_api_key: true,
            show_region: false,
            show_workspace_id: false,
        },
        ProviderId::Zai => ProviderSettingsSpec {
            source_modes: SOURCE_MODES_AUTO_API,
            show_cookie_header: false,
            show_api_key: true,
            show_region: false,
            show_workspace_id: false,
        },
        ProviderId::MiniMax => ProviderSettingsSpec {
            source_modes: SOURCE_MODES_AUTO_WEB_API,
            show_cookie_header: true,
            show_api_key: true,
            show_region: true,
            show_workspace_id: true,
        },
        ProviderId::Kimi => ProviderSettingsSpec {
            source_modes: SOURCE_MODES_AUTO_WEB_API,
            show_cookie_header: true,
            show_api_key: true,
            show_region: false,
            show_workspace_id: false,
        },
        ProviderId::Kiro => ProviderSettingsSpec {
            source_modes: SOURCE_MODES_AUTO_CLI,
            show_cookie_header: false,
            show_api_key: false,
            show_region: false,
            show_workspace_id: false,
        },
        ProviderId::VertexAI => ProviderSettingsSpec {
            source_modes: SOURCE_MODES_AUTO_API,
            show_cookie_header: false,
            show_api_key: false,
            show_region: false,
            show_workspace_id: false,
        },
        ProviderId::Augment => ProviderSettingsSpec {
            source_modes: SOURCE_MODES_AUTO_WEB,
            show_cookie_header: true,
            show_api_key: false,
            show_region: false,
            show_workspace_id: false,
        },
        ProviderId::JetBrains => ProviderSettingsSpec {
            source_modes: SOURCE_MODES_AUTO_WEB,
            show_cookie_header: false,
            show_api_key: false,
            show_region: false,
            show_workspace_id: false,
        },
        ProviderId::KimiK2 => ProviderSettingsSpec {
            source_modes: SOURCE_MODES_AUTO_API,
            show_cookie_header: false,
            show_api_key: true,
            show_region: false,
            show_workspace_id: false,
        },
        ProviderId::Amp => ProviderSettingsSpec {
            source_modes: SOURCE_MODES_AUTO_WEB,
            show_cookie_header: true,
            show_api_key: false,
            show_region: false,
            show_workspace_id: false,
        },
        ProviderId::Synthetic => ProviderSettingsSpec {
            source_modes: SOURCE_MODES_AUTO_API,
            show_cookie_header: false,
            show_api_key: true,
            show_region: false,
            show_workspace_id: false,
        },
    }
}

pub(crate) fn source_mode_label(mode: &ProviderSourceMode) -> &'static str {
    match mode {
        ProviderSourceMode::Auto => "Auto",
        ProviderSourceMode::Web => "Web",
        ProviderSourceMode::Cli => "CLI",
        ProviderSourceMode::Oauth => "OAuth",
        ProviderSourceMode::Api => "API",
    }
}

pub(crate) fn build_provider_forms(
    config: &AppConfig,
    window: &mut Window,
    cx: &mut Context<AgentStatsApp>,
) -> HashMap<ProviderId, ProviderSettingsForm> {
    let mut forms = HashMap::new();
    for provider in &config.providers {
        let cookie_header = build_input_state(
            window,
            cx,
            provider.cookie_header.as_deref(),
            "Cookie header",
            true,
        );
        let api_key = build_input_state(window, cx, provider.api_key.as_deref(), "API key", true);
        let region = build_input_state(window, cx, provider.region.as_deref(), "Region", false);
        let workspace_id = build_input_state(
            window,
            cx,
            provider.workspace_id.as_deref(),
            "Workspace ID",
            false,
        );
        let source_mode = provider.source.clone().unwrap_or(ProviderSourceMode::Auto);

        forms.insert(
            provider.id,
            ProviderSettingsForm {
                source_mode,
                cookie_header,
                api_key,
                region,
                workspace_id,
            },
        );
    }
    forms
}

pub(crate) fn build_token_account_forms(
    config: &AppConfig,
    window: &mut Window,
    cx: &mut Context<AgentStatsApp>,
) -> HashMap<ProviderId, TokenAccountForm> {
    let mut forms = HashMap::new();
    for provider in &config.providers {
        if let Some(support) = token_account_support(provider.id) {
            let label = build_input_state(window, cx, None, "Account label", false);
            let token = build_input_state(window, cx, None, support.placeholder, true);
            forms.insert(provider.id, TokenAccountForm { label, token });
        }
    }
    forms
}

fn build_input_state(
    window: &mut Window,
    cx: &mut Context<AgentStatsApp>,
    value: Option<&str>,
    placeholder: &'static str,
    masked: bool,
) -> Entity<InputState> {
    let default_value: SharedString = value.unwrap_or("").to_string().into();
    let placeholder: SharedString = placeholder.to_string().into();
    cx.new(move |cx| {
        let mut state = InputState::new(window, cx)
            .placeholder(placeholder)
            .default_value(default_value);
        if masked {
            state = state.masked(true);
        }
        state
    })
}
