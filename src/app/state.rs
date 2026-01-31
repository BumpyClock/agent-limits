use agent_stats_core::backend::{FetchRequest, fetch_native_usage, fetch_usage};
use agent_stats_core::config::{
    AppConfig, ProviderCookieSource, ProviderSourceMode, TokenAccountData, TokenAccountEntry,
};
use agent_stats_core::models::ProviderPayload;
use agent_stats_core::provider_metadata::{ProviderId, ProviderMetadata, provider_metadata};
use agent_stats_core::{DeviceFlowPollResult, poll_for_token, request_device_code};
use gpui::{AppContext, AsyncApp, Context, ParentElement, ScrollHandle, Task, WeakEntity, Window};
use gpui_component::label::Label;
use gpui_component::WindowExt;
use std::collections::HashMap;
use std::time::Duration as StdDuration;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::settings::AppSettings;
use super::forms::{build_provider_forms, build_token_account_forms};
use super::types::{
    AppViewMode, CopilotDeviceFlowState, ProviderMoveDirection, ProviderSettingsForm,
    ProviderState, TokenAccountForm,
};
use crate::app::APP_TITLE;

pub(crate) struct AgentStatsApp {
    pub(crate) providers: Vec<ProviderState>,
    pub(crate) config: AppConfig,
    pub(crate) settings: AppSettings,
    pub(crate) provider_forms: HashMap<ProviderId, ProviderSettingsForm>,
    pub(crate) token_account_forms: HashMap<ProviderId, TokenAccountForm>,
    pub(crate) last_refresh: Option<OffsetDateTime>,
    pub(crate) refreshing: bool,
    pub(crate) error: Option<String>,
    pub(crate) refresh_task: Option<Task<()>>,
    pub(crate) show_settings: bool,
    pub(crate) copilot_device_flow: CopilotDeviceFlowState,
    pub(crate) copilot_device_flow_task: Option<Task<()>>,
    pub(crate) needs_form_rebuild: bool,
    pub(crate) view_mode: AppViewMode,
    pub(crate) preview_active_provider: Option<ProviderId>,
    pub(crate) main_scroll_handle: ScrollHandle,
    pub(crate) pending_main_provider_focus: Option<ProviderId>,
}

impl AgentStatsApp {
    pub(crate) fn new(
        window: &mut Window,
        view_mode: AppViewMode,
        initial_main_provider: Option<ProviderId>,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = AppSettings::load();
        let config = AppConfig::load_or_default().unwrap_or_else(|error| {
            log::error!("Failed to load config: {error}");
            AppConfig::default_config()
        });
        let providers = build_provider_states(&config);
        let provider_forms = build_provider_forms(&config, window, cx);
        let token_account_forms = build_token_account_forms(&config, window, cx);
        let preview_active_provider = providers
            .iter()
            .find(|provider| {
                provider
                    .config
                    .enabled
                    .unwrap_or(provider.metadata.default_enabled)
            })
            .map(|provider| provider.config.id);
        let pending_main_provider_focus = if view_mode == AppViewMode::Main {
            initial_main_provider
        } else {
            None
        };

        let mut app = Self {
            providers,
            config,
            settings,
            provider_forms,
            token_account_forms,
            last_refresh: None,
            refreshing: false,
            error: None,
            refresh_task: None,
            show_settings: false,
            copilot_device_flow: CopilotDeviceFlowState::Idle,
            copilot_device_flow_task: None,
            needs_form_rebuild: false,
            view_mode,
            preview_active_provider,
            main_scroll_handle: ScrollHandle::new(),
            pending_main_provider_focus,
        };
        app.schedule_refresh_loop(cx);
        app.refresh_now(cx);
        app
    }

    fn schedule_refresh_loop(&mut self, cx: &mut Context<Self>) {
        self.refresh_task = None;
        let cadence = self.settings.refresh_cadence.clone();
        let Some(duration) = cadence.duration() else { return };

        let task = cx.spawn(move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                loop {
                    cx.background_executor().timer(duration).await;
                    let should_continue = match this.update(&mut cx, |app, cx| {
                        let cadence = app.settings.refresh_cadence.clone();
                        if cadence.duration().is_some() {
                            app.refresh_now(cx);
                            true
                        } else {
                            false
                        }
                    }) {
                        Ok(value) => value,
                        Err(error) => {
                            log::error!("Refresh loop update failed: {error}");
                            false
                        }
                    };

                    if !should_continue {
                        break;
                    }
                }
            }
        });

        self.refresh_task = Some(task);
    }

    pub(crate) fn refresh_now(&mut self, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        self.error = None;
        cx.notify();

        let include_status = self.settings.include_status;
        let backend_mode =
            std::env::var("AGENT_STATS_BACKEND_MODE").unwrap_or_else(|_| "native".to_string());
        let use_native = backend_mode != "codexbar";
        let task = cx.background_spawn(async move {
            let request = FetchRequest {
                include_status,
                ..Default::default()
            };
            if use_native {
                fetch_native_usage(request)
            } else {
                fetch_usage(request)
            }
        });

        cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                let result = task.await;
                if let Err(error) = this.update(&mut cx, |app, cx| {
                    app.refreshing = false;
                    match result {
                        Ok(fetch_result) => {
                            let payload_map = fetch_result
                                .payloads
                                .into_iter()
                                .map(|payload| (payload.provider.clone(), payload))
                                .collect::<HashMap<_, _>>();
                            app.update_payloads(payload_map);
                            app.error = None;
                            app.last_refresh = Some(OffsetDateTime::now_utc());
                        }
                        Err(error) => {
                            app.error = Some(error.to_string());
                        }
                    }
                    cx.notify();
                }) {
                    log::error!("Refresh result update failed: {error}");
                }
            }
        })
        .detach();
    }

    fn update_payloads(&mut self, payload_map: HashMap<String, ProviderPayload>) {
        for provider in &mut self.providers {
            let payload = payload_map.get(&provider.config.id.to_string()).cloned();
            provider.payload = payload;
        }
    }

    pub(crate) fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        self.show_settings = !self.show_settings;
        cx.notify();
    }

    pub(crate) fn update_setting(&mut self, settings: AppSettings, cx: &mut Context<Self>) {
        self.settings = settings;
        if let Err(error) = self.settings.save() {
            log::error!("Failed to save settings: {error}");
        }
        self.schedule_refresh_loop(cx);
        cx.notify();
    }

    pub(crate) fn open_about_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let version = env!("CARGO_PKG_VERSION");
        window.open_dialog(cx, move |dialog, _window, _cx| {
            dialog
                .title(format!("About {APP_TITLE}"))
                .child(Label::new(format!("{APP_TITLE} v{version}")))
                .alert()
        });
    }

    pub(crate) fn update_provider_enabled(
        &mut self,
        provider_id: ProviderId,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(provider) = self
            .config
            .providers
            .iter_mut()
            .find(|provider| provider.id == provider_id)
        {
            provider.enabled = Some(enabled);
        }

        self.persist_config_and_refresh(cx);
    }

    pub(crate) fn update_provider_source_mode(
        &mut self,
        provider_id: ProviderId,
        mode: ProviderSourceMode,
        cx: &mut Context<Self>,
    ) {
        if let Some(form) = self.provider_forms.get_mut(&provider_id) {
            form.source_mode = mode.clone();
        }

        if let Some(provider) = self
            .config
            .providers
            .iter_mut()
            .find(|provider| provider.id == provider_id)
        {
            provider.source = if mode == ProviderSourceMode::Auto {
                None
            } else {
                Some(mode)
            };
        }

        self.persist_config_and_refresh(cx);
    }

    pub(crate) fn save_provider_settings(&mut self, provider_id: ProviderId, cx: &mut Context<Self>) {
        let Some(form) = self.provider_forms.get(&provider_id) else {
            return;
        };

        let api_key_value = form.api_key.read(cx).value();
        let api_key_text = api_key_value.trim();
        let api_key = if api_key_text.is_empty() {
            None
        } else {
            Some(api_key_text.to_string())
        };

        let cookie_value = form.cookie_header.read(cx).value();
        let cookie_text = cookie_value.trim();
        let cookie_header = if cookie_text.is_empty() {
            None
        } else {
            Some(cookie_text.to_string())
        };

        let region_value = form.region.read(cx).value();
        let region_text = region_value.trim();
        let region = if region_text.is_empty() {
            None
        } else {
            Some(region_text.to_string())
        };

        let workspace_value = form.workspace_id.read(cx).value();
        let workspace_text = workspace_value.trim();
        let workspace_id = if workspace_text.is_empty() {
            None
        } else {
            Some(workspace_text.to_string())
        };

        if let Some(provider) = self
            .config
            .providers
            .iter_mut()
            .find(|provider| provider.id == provider_id)
        {
            provider.api_key = api_key;
            provider.cookie_header = cookie_header.clone();
            provider.region = region;
            provider.workspace_id = workspace_id;
            provider.cookie_source = if cookie_header.is_some() {
                Some(ProviderCookieSource::Manual)
            } else {
                None
            };
            provider.source = if form.source_mode == ProviderSourceMode::Auto {
                None
            } else {
                Some(form.source_mode.clone())
            };
        }

        self.persist_config_and_refresh(cx);
    }

    pub(crate) fn add_token_account(
        &mut self,
        provider_id: ProviderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (label_state, token_state, label_value, token_value) = {
            let Some(form) = self.token_account_forms.get(&provider_id) else {
                return;
            };
            (
                form.label.clone(),
                form.token.clone(),
                form.label.read(cx).value(),
                form.token.read(cx).value(),
            )
        };
        let label_text = label_value.trim();
        let token_text = token_value.trim();

        if token_text.is_empty() {
            return;
        }

        {
            let Some(provider) = self
                .config
                .providers
                .iter_mut()
                .find(|provider| provider.id == provider_id)
            else {
                return;
            };

            let token_accounts = provider.token_accounts.get_or_insert_with(|| TokenAccountData {
                version: 1,
                active_index: 0,
                accounts: Vec::new(),
            });

            let account_index = token_accounts.accounts.len() + 1;
            let label = if label_text.is_empty() {
                format!("Account {}", account_index)
            } else {
                label_text.to_string()
            };

            token_accounts.accounts.push(TokenAccountEntry {
                id: Uuid::new_v4().to_string(),
                label,
                token: token_text.to_string(),
                added_at: OffsetDateTime::now_utc().unix_timestamp(),
                last_used: None,
            });

            token_accounts.active_index = token_accounts.accounts.len().saturating_sub(1);
        }

        self.persist_config_and_refresh(cx);
        label_state.update(cx, |state, cx| {
            state.set_value("", window, cx);
        });
        token_state.update(cx, |state, cx| {
            state.set_value("", window, cx);
        });
    }

    pub(crate) fn remove_selected_token_account(
        &mut self,
        provider_id: ProviderId,
        cx: &mut Context<Self>,
    ) {
        let mut should_clear = false;
        let mut should_clamp = false;
        {
            let Some(provider) = self
                .config
                .providers
                .iter_mut()
                .find(|provider| provider.id == provider_id)
            else {
                return;
            };

            let Some(token_accounts) = provider.token_accounts.as_mut() else {
                return;
            };

            if token_accounts.accounts.is_empty() {
                return;
            }

            let selected_index = token_accounts
                .active_index
                .min(token_accounts.accounts.len().saturating_sub(1));
            token_accounts.accounts.remove(selected_index);

            if token_accounts.accounts.is_empty() {
                should_clear = true;
            } else if token_accounts.active_index >= token_accounts.accounts.len() {
                should_clamp = true;
            }
        }

        if should_clear {
            if let Some(provider) = self
                .config
                .providers
                .iter_mut()
                .find(|provider| provider.id == provider_id)
            {
                provider.token_accounts = None;
            }
        } else if should_clamp {
            if let Some(provider) = self
                .config
                .providers
                .iter_mut()
                .find(|provider| provider.id == provider_id)
            {
                if let Some(token_accounts) = provider.token_accounts.as_mut() {
                    token_accounts.active_index = token_accounts.accounts.len().saturating_sub(1);
                }
            }
        }

        self.persist_config_and_refresh(cx);
    }

    pub(crate) fn set_active_token_account(
        &mut self,
        provider_id: ProviderId,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        {
            let Some(provider) = self
                .config
                .providers
                .iter_mut()
                .find(|provider| provider.id == provider_id)
            else {
                return;
            };

            let Some(token_accounts) = provider.token_accounts.as_mut() else {
                return;
            };

            if token_accounts.accounts.is_empty() {
                return;
            }

            let max_index = token_accounts.accounts.len().saturating_sub(1);
            token_accounts.active_index = index.min(max_index);
        }

        self.persist_config_and_refresh(cx);
    }

    fn persist_config_and_refresh(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.config.save() {
            log::error!("Failed to save config: {error}");
        } else {
            self.providers = build_provider_states(&self.config);
            self.refresh_now(cx);
        }
        cx.notify();
    }

    pub(crate) fn move_provider(
        &mut self,
        provider_id: ProviderId,
        direction: ProviderMoveDirection,
        cx: &mut Context<Self>,
    ) {
        if move_provider_in_config(&mut self.config, provider_id, direction) {
            self.persist_config_and_refresh(cx);
        }
    }

    pub(crate) fn start_copilot_device_flow(&mut self, cx: &mut Context<Self>) {
        self.copilot_device_flow = CopilotDeviceFlowState::RequestingCode;
        cx.notify();

        let task = cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                let result = cx
                    .background_executor()
                    .spawn(async move { request_device_code() })
                    .await;

                let _ = this.update(&mut cx, |app, cx| {
                    match result {
                        Ok(response) => {
                            app.copilot_device_flow = CopilotDeviceFlowState::ShowingCode {
                                user_code: response.user_code,
                                verification_uri: response.verification_uri,
                                device_code: response.device_code,
                                interval: response.interval,
                            };
                        }
                        Err(error) => {
                            app.copilot_device_flow =
                                CopilotDeviceFlowState::Error(error.message);
                        }
                    }
                    cx.notify();
                });
            }
        });

        self.copilot_device_flow_task = Some(task);
    }

    pub(crate) fn start_copilot_polling(
        &mut self,
        device_code: String,
        interval: u64,
        user_code: String,
        cx: &mut Context<Self>,
    ) {
        self.copilot_device_flow = CopilotDeviceFlowState::Polling { user_code };
        cx.notify();

        let task = cx.spawn(move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                let mut current_interval = interval;

                loop {
                    cx.background_executor()
                        .timer(StdDuration::from_secs(current_interval))
                        .await;

                    let device_code = device_code.clone();
                    let poll_result = cx
                        .background_executor()
                        .spawn(async move { poll_for_token(&device_code) })
                        .await;

                    enum PollAction {
                        Continue(u64),
                        Done,
                    }

                    let action = match this.update(&mut cx, |app, cx| {
                        match poll_result {
                            Ok(DeviceFlowPollResult::Token(token)) => {
                                app.complete_copilot_device_flow(token, cx);
                                PollAction::Done
                            }
                            Ok(DeviceFlowPollResult::Pending) => {
                                PollAction::Continue(current_interval)
                            }
                            Ok(DeviceFlowPollResult::SlowDown) => {
                                PollAction::Continue(current_interval + 5)
                            }
                            Err(error) => {
                                app.copilot_device_flow =
                                    CopilotDeviceFlowState::Error(error.message);
                                app.copilot_device_flow_task = None;
                                cx.notify();
                                PollAction::Done
                            }
                        }
                    }) {
                        Ok(action) => action,
                        Err(_) => PollAction::Done,
                    };

                    match action {
                        PollAction::Continue(interval) => {
                            current_interval = interval;
                        }
                        PollAction::Done => break,
                    }
                }
            }
        });

        self.copilot_device_flow_task = Some(task);
    }

    fn complete_copilot_device_flow(&mut self, token: String, cx: &mut Context<Self>) {
        if let Some(provider) = self
            .config
            .providers
            .iter_mut()
            .find(|provider| provider.id == ProviderId::Copilot)
        {
            provider.api_key = Some(token);
            provider.enabled = Some(true);
        }

        self.persist_config_and_refresh(cx);
        self.copilot_device_flow = CopilotDeviceFlowState::Success;
        self.copilot_device_flow_task = None;
        self.needs_form_rebuild = true;
        cx.notify();

        let task = cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                cx.background_executor()
                    .timer(StdDuration::from_secs(3))
                    .await;
                let _ = this.update(&mut cx, |app, cx| {
                    if matches!(app.copilot_device_flow, CopilotDeviceFlowState::Success) {
                        app.copilot_device_flow = CopilotDeviceFlowState::Idle;
                        cx.notify();
                    }
                });
            }
        });

        self.copilot_device_flow_task = Some(task);
    }

    pub(crate) fn cancel_copilot_device_flow(&mut self, cx: &mut Context<Self>) {
        self.copilot_device_flow = CopilotDeviceFlowState::Idle;
        self.copilot_device_flow_task = None;
        cx.notify();
    }
}

fn move_provider_in_config(
    config: &mut AppConfig,
    provider_id: ProviderId,
    direction: ProviderMoveDirection,
) -> bool {
    let current_index = config
        .providers
        .iter()
        .position(|provider| provider.id == provider_id);
    let Some(current_index) = current_index else {
        return false;
    };

    let target_index = match direction {
        ProviderMoveDirection::Up => current_index.saturating_sub(1),
        ProviderMoveDirection::Down => current_index.saturating_add(1),
    };

    if current_index == target_index {
        return false;
    }

    if target_index >= config.providers.len() {
        return false;
    }

    config.providers.swap(current_index, target_index);
    true
}

fn build_provider_states(config: &AppConfig) -> Vec<ProviderState> {
    let metadata_map: HashMap<ProviderId, ProviderMetadata> = provider_metadata()
        .iter()
        .map(|metadata| (metadata.id, metadata.clone()))
        .collect();

    config
        .providers
        .iter()
        .filter_map(|provider| {
            metadata_map.get(&provider.id).map(|metadata| ProviderState {
                config: provider.clone(),
                metadata: metadata.clone(),
                payload: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{ProviderId, ProviderMoveDirection, move_provider_in_config};
    use agent_stats_core::config::{AppConfig, ProviderConfig};

    fn config_with_providers(ids: &[ProviderId]) -> AppConfig {
        let providers = ids
            .iter()
            .map(|provider_id| ProviderConfig {
                id: *provider_id,
                enabled: None,
                source: None,
                cookie_source: None,
                cookie_header: None,
                api_key: None,
                region: None,
                workspace_id: None,
                token_accounts: None,
            })
            .collect();

        AppConfig { version: 1, providers }
    }

    fn provider_ids(config: &AppConfig) -> Vec<ProviderId> {
        config.providers.iter().map(|provider| provider.id).collect()
    }

    #[test]
    fn move_provider_up_within_bounds() {
        let mut config = config_with_providers(&[
            ProviderId::Codex,
            ProviderId::Claude,
            ProviderId::Cursor,
        ]);

        let moved = move_provider_in_config(
            &mut config,
            ProviderId::Cursor,
            ProviderMoveDirection::Up,
        );

        assert!(moved);
        assert_eq!(
            provider_ids(&config),
            vec![ProviderId::Codex, ProviderId::Cursor, ProviderId::Claude]
        );
    }

    #[test]
    fn move_provider_down_within_bounds() {
        let mut config = config_with_providers(&[
            ProviderId::Codex,
            ProviderId::Claude,
            ProviderId::Cursor,
        ]);

        let moved = move_provider_in_config(
            &mut config,
            ProviderId::Codex,
            ProviderMoveDirection::Down,
        );

        assert!(moved);
        assert_eq!(
            provider_ids(&config),
            vec![ProviderId::Claude, ProviderId::Codex, ProviderId::Cursor]
        );
    }

    #[test]
    fn move_provider_up_at_start_is_noop() {
        let mut config = config_with_providers(&[ProviderId::Codex, ProviderId::Claude]);

        let moved = move_provider_in_config(
            &mut config,
            ProviderId::Codex,
            ProviderMoveDirection::Up,
        );

        assert!(!moved);
        assert_eq!(
            provider_ids(&config),
            vec![ProviderId::Codex, ProviderId::Claude]
        );
    }

    #[test]
    fn move_provider_down_at_end_is_noop() {
        let mut config = config_with_providers(&[ProviderId::Codex, ProviderId::Claude]);

        let moved = move_provider_in_config(
            &mut config,
            ProviderId::Claude,
            ProviderMoveDirection::Down,
        );

        assert!(!moved);
        assert_eq!(
            provider_ids(&config),
            vec![ProviderId::Codex, ProviderId::Claude]
        );
    }
}
