use agent_stats_core::config::{ProviderConfig, ProviderSourceMode};
use agent_stats_core::models::ProviderPayload;
use agent_stats_core::provider_metadata::ProviderMetadata;
use gpui::Entity;
use gpui_component::input::InputState;

#[derive(Clone)]
pub(crate) struct ProviderState {
    pub(crate) config: ProviderConfig,
    pub(crate) metadata: ProviderMetadata,
    pub(crate) payload: Option<ProviderPayload>,
}

#[derive(Clone)]
pub(crate) struct ProviderSettingsForm {
    pub(crate) source_mode: ProviderSourceMode,
    pub(crate) cookie_header: Entity<InputState>,
    pub(crate) api_key: Entity<InputState>,
    pub(crate) region: Entity<InputState>,
    pub(crate) workspace_id: Entity<InputState>,
}

#[derive(Clone)]
pub(crate) struct TokenAccountForm {
    pub(crate) label: Entity<InputState>,
    pub(crate) token: Entity<InputState>,
}

#[derive(Clone, Copy)]
pub(crate) enum ProviderMoveDirection {
    Up,
    Down,
}

#[derive(Clone)]
pub(crate) enum CopilotDeviceFlowState {
    Idle,
    RequestingCode,
    ShowingCode {
        user_code: String,
        verification_uri: String,
        device_code: String,
        interval: u64,
    },
    Polling {
        user_code: String,
    },
    Success,
    Error(String),
}
