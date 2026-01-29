use agent_stats_core::backend::{FetchRequest, fetch_native_usage, fetch_usage};
use agent_stats_core::config::{
    AppConfig, ProviderConfig, ProviderCookieSource, ProviderSourceMode, TokenAccountData,
    TokenAccountEntry,
};
use agent_stats_core::models::{
    ProviderIdentitySnapshot, ProviderPayload, ProviderStatusIndicator, RateWindow, UsageSnapshot,
};
use agent_stats_core::provider_metadata::{ProviderId, ProviderMetadata, provider_metadata};
use agent_stats_core::token_accounts::{TokenAccountSupport, token_account_support};
use agent_stats_core::{DeviceFlowPollResult, request_device_code, poll_for_token};
use gpui::{
    App, Application, AsyncApp, ClipboardItem, Context, Entity, Render, Task, WeakEntity, Window,
    WindowBounds, WindowOptions, div, px, size, prelude::*,
};
use gpui_component::button::Button;
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputState};
use gpui_component::label::Label;
use gpui_component::progress::Progress;
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{Disableable, Selectable, StyledExt};
use gpui_component::Root;
use std::collections::HashMap;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::settings::{AppSettings, RefreshCadence};

pub fn run() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();

    let app = Application::new();

    app.run(|cx: &mut App| {
        gpui_component::init(cx);
        create_main_window(cx);
    });
}

fn create_main_window(cx: &mut App) {
    let window_options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(840.0), px(720.0)), cx)),
        ..Default::default()
    };

    let _ = cx.open_window(window_options, |window, cx| {
        window.activate_window();
        window.set_window_title("Agent Stats");
        let app = cx.new(|cx| AgentStatsApp::new(window, cx));
        cx.new(|cx| Root::new(app, window, cx))
    });
}

#[derive(Clone)]
struct ProviderState {
    config: ProviderConfig,
    metadata: ProviderMetadata,
    payload: Option<ProviderPayload>,
}

#[derive(Clone)]
struct ProviderSettingsForm {
    source_mode: ProviderSourceMode,
    cookie_header: Entity<InputState>,
    api_key: Entity<InputState>,
    region: Entity<InputState>,
    workspace_id: Entity<InputState>,
}

#[derive(Clone)]
struct TokenAccountForm {
    label: Entity<InputState>,
    token: Entity<InputState>,
}

#[derive(Clone, Copy)]
struct ProviderSettingsSpec {
    source_modes: &'static [ProviderSourceMode],
    show_cookie_header: bool,
    show_api_key: bool,
    show_region: bool,
    show_workspace_id: bool,
}

#[derive(Clone, Copy)]
enum ProviderMoveDirection {
    Up,
    Down,
}

#[derive(Clone)]
enum CopilotDeviceFlowState {
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

pub struct AgentStatsApp {
    providers: Vec<ProviderState>,
    config: AppConfig,
    settings: AppSettings,
    provider_forms: HashMap<ProviderId, ProviderSettingsForm>,
    token_account_forms: HashMap<ProviderId, TokenAccountForm>,
    last_refresh: Option<OffsetDateTime>,
    refreshing: bool,
    error: Option<String>,
    refresh_task: Option<Task<()>>,
    show_settings: bool,
    copilot_device_flow: CopilotDeviceFlowState,
    copilot_device_flow_task: Option<Task<()>>,
    needs_form_rebuild: bool,
}

impl AgentStatsApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = AppSettings::load();
        let config = AppConfig::load_or_default().unwrap_or_else(|error| {
            log::error!("Failed to load config: {error}");
            AppConfig::default_config()
        });
        let providers = build_provider_states(&config);
        let provider_forms = build_provider_forms(&config, window, cx);
        let token_account_forms = build_token_account_forms(&config, window, cx);

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

    fn refresh_now(&mut self, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        self.error = None;
        cx.notify();

        let include_status = self.settings.include_status;
        let backend_mode = std::env::var("AGENT_STATS_BACKEND_MODE").unwrap_or_else(|_| "native".to_string());
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

    fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        self.show_settings = !self.show_settings;
        cx.notify();
    }

    fn update_setting(&mut self, settings: AppSettings, cx: &mut Context<Self>) {
        self.settings = settings;
        if let Err(error) = self.settings.save() {
            log::error!("Failed to save settings: {error}");
        }
        self.schedule_refresh_loop(cx);
        cx.notify();
    }

    fn update_provider_enabled(&mut self, provider_id: ProviderId, enabled: bool, cx: &mut Context<Self>) {
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

    fn update_provider_source_mode(
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

    fn save_provider_settings(&mut self, provider_id: ProviderId, cx: &mut Context<Self>) {
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

    fn add_token_account(
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

    fn remove_selected_token_account(
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
                    token_accounts.active_index =
                        token_accounts.accounts.len().saturating_sub(1);
                }
            }
        }

        self.persist_config_and_refresh(cx);
    }

    fn set_active_token_account(
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

    fn move_provider(
        &mut self,
        provider_id: ProviderId,
        direction: ProviderMoveDirection,
        cx: &mut Context<Self>,
    ) {
        if move_provider_in_config(&mut self.config, provider_id, direction) {
            self.persist_config_and_refresh(cx);
        }
    }

    fn start_copilot_device_flow(&mut self, cx: &mut Context<Self>) {
        self.copilot_device_flow = CopilotDeviceFlowState::RequestingCode;
        cx.notify();

        let task = cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                let result = cx.background_executor().spawn(async move {
                    request_device_code()
                }).await;

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

    fn start_copilot_polling(
        &mut self,
        device_code: String,
        interval: u64,
        user_code: String,
        cx: &mut Context<Self>,
    ) {
        self.copilot_device_flow = CopilotDeviceFlowState::Polling {
            user_code,
        };
        cx.notify();

        let task = cx.spawn(move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                let mut current_interval = interval;

                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(current_interval))
                        .await;

                    let device_code = device_code.clone();
                    let poll_result = cx.background_executor().spawn(async move {
                        poll_for_token(&device_code)
                    }).await;

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
                    .timer(std::time::Duration::from_secs(3))
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

    fn cancel_copilot_device_flow(&mut self, cx: &mut Context<Self>) {
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

impl Render for AgentStatsApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.needs_form_rebuild {
            self.needs_form_rebuild = false;
            self.provider_forms = build_provider_forms(&self.config, window, cx);
            self.token_account_forms = build_token_account_forms(&self.config, window, cx);
        }

        let content: gpui::AnyElement = if self.show_settings {
            render_settings(self, cx)
        } else {
            let header = div()
                .flex()
                .items_center()
                .justify_between()
                .child(Label::new("Agent Stats").text_lg())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(render_last_refresh(self.last_refresh))
                        .child(
                            Button::new("refresh")
                                .label("Refresh")
                                .disabled(self.refreshing)
                                .on_click(cx.listener(|app, _event, _window, cx| {
                                    app.refresh_now(cx);
                                })),
                        )
                        .child(
                            Button::new("settings")
                                .label("Settings")
                                .on_click(cx.listener(|app, _event, _window, cx| {
                                    app.toggle_settings(cx);
                                })),
                        ),
                );

            let error_banner = self
                .error
                .as_ref()
                .map(|message| {
                    div()
                        .rounded_md()
                        .bg(gpui::rgba(0xFF5A5A26))
                        .paddings(px(8.0))
                        .child(Label::new(format!("Refresh failed: {message}")))
                });

            let provider_list = div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .children(self.providers.iter().filter(|provider| {
                    provider
                        .config
                        .enabled
                        .unwrap_or(provider.metadata.default_enabled)
                }).map(|provider| render_provider_card(provider, &self.settings)));

            div()
                .size_full()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .paddings(px(16.0))
                .overflow_y_scrollbar()
                .child(header)
                .when_some(error_banner, |this, banner| this.child(banner))
                .child(provider_list)
                .into_any_element()
        };

        content
    }
}

fn render_settings(app: &mut AgentStatsApp, cx: &mut Context<AgentStatsApp>) -> gpui::AnyElement {
    let settings_header = div()
        .flex()
        .items_center()
        .justify_between()
        .child(Label::new("Settings").text_lg())
        .child(
            Button::new("back")
                .label("Back")
                .on_click(cx.listener(|app, _event, _window, cx| {
                    app.toggle_settings(cx);
                })),
        );

    let cadence_buttons = div()
        .flex()
        .gap(px(8.0))
        .children([
            RefreshCadence::Manual,
            RefreshCadence::OneMinute,
            RefreshCadence::TwoMinutes,
            RefreshCadence::FiveMinutes,
            RefreshCadence::FifteenMinutes,
            RefreshCadence::ThirtyMinutes,
        ].into_iter().map(|cadence| {
            let selected = app.settings.refresh_cadence == cadence;
            Button::new(format!("cadence-{}", cadence.label()))
                .label(cadence.label())
                .selected(selected)
                .on_click(cx.listener(move |app, _event, _window, cx| {
                    let mut settings = app.settings.clone();
                    settings.refresh_cadence = cadence.clone();
                    app.update_setting(settings, cx);
                }))
        }));

    let toggles = div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            Checkbox::new("show-used")
                .label("Show used percentage")
                .checked(app.settings.show_used)
                .on_click(cx.listener(|app, checked, _window, cx| {
                    let mut settings = app.settings.clone();
                    settings.show_used = *checked;
                    app.update_setting(settings, cx);
                })),
        )
        .child(
            Checkbox::new("show-absolute")
                .label("Show absolute reset times")
                .checked(app.settings.show_absolute_reset)
                .on_click(cx.listener(|app, checked, _window, cx| {
                    let mut settings = app.settings.clone();
                    settings.show_absolute_reset = *checked;
                    app.update_setting(settings, cx);
                })),
        )
        .child(
            Checkbox::new("include-status")
                .label("Include status page checks")
                .checked(app.settings.include_status)
                .on_click(cx.listener(|app, checked, _window, cx| {
                    let mut settings = app.settings.clone();
                    settings.include_status = *checked;
                    app.update_setting(settings, cx);
                })),
        );

    let provider_count = app.providers.len();
    let provider_toggles = div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .children(app.providers.iter().enumerate().map(|(index, provider)| {
            let enabled = provider
                .config
                .enabled
                .unwrap_or(provider.metadata.default_enabled);
            let provider_id = provider.config.id;
            let is_first = index == 0;
            let is_last = index.saturating_add(1) >= provider_count;

            let toggle = Checkbox::new(format!("provider-{}", provider_id))
                .label(provider.metadata.display_name)
                .checked(enabled)
                .on_click(cx.listener(move |app, checked, _window, cx| {
                    app.update_provider_enabled(provider_id, *checked, cx);
                }));

            let move_up = Button::new(format!("provider-{}-up", provider_id))
                .label("Up")
                .disabled(is_first)
                .on_click(cx.listener(move |app, _event, _window, cx| {
                    app.move_provider(provider_id, ProviderMoveDirection::Up, cx);
                }));

            let move_down = Button::new(format!("provider-{}-down", provider_id))
                .label("Down")
                .disabled(is_last)
                .on_click(cx.listener(move |app, _event, _window, cx| {
                    app.move_provider(provider_id, ProviderMoveDirection::Down, cx);
                }));

            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(toggle)
                .child(move_up)
                .child(move_down)
        }));

    let provider_settings_cards: Vec<gpui::AnyElement> = app
        .providers
        .iter()
        .filter_map(|provider| render_provider_settings_card(provider, app, cx))
        .collect();

    let has_provider_settings = !provider_settings_cards.is_empty();
    let provider_settings = div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .children(provider_settings_cards);

    div()
        .size_full()
        .flex()
        .flex_col()
        .gap(px(16.0))
        .paddings(px(16.0))
        .overflow_y_scrollbar()
        .child(settings_header)
        .child(div().child(Label::new("Refresh cadence")))
        .child(cadence_buttons)
        .child(div().child(Label::new("Display")))
        .child(toggles)
        .child(div().child(Label::new("Providers")))
        .child(provider_toggles)
        .when(has_provider_settings, |this| {
            this.child(div().child(Label::new("Provider settings")))
                .child(provider_settings)
        })
        .into_any_element()
}

fn render_provider_settings_card(
    provider: &ProviderState,
    app: &AgentStatsApp,
    cx: &mut Context<AgentStatsApp>,
) -> Option<gpui::AnyElement> {
    let spec = provider_settings_spec(provider.config.id);
    if !spec.is_visible() {
        return None;
    }

    let provider_id = provider.config.id;
    let form = app.provider_forms.get(&provider_id)?;

    let save_button = Button::new(format!("save-{}", provider_id))
        .label("Save")
        .on_click(cx.listener(move |app, _event, _window, cx| {
            app.save_provider_settings(provider_id, cx);
        }));

    let header = div()
        .flex()
        .items_center()
        .justify_between()
        .child(Label::new(provider.metadata.display_name).text_sm())
        .child(save_button);

    let source_buttons = if spec.source_modes.is_empty() {
        None
    } else {
        let buttons = div()
            .flex()
            .gap(px(6.0))
            .children(spec.source_modes.iter().map(|mode| {
                let selected = form.source_mode == *mode;
                let mode_label = source_mode_label(mode);
                let mode = mode.clone();
                Button::new(format!("source-{}-{}", provider_id, mode.as_str()))
                    .label(mode_label)
                    .selected(selected)
                    .on_click(cx.listener(move |app, _event, _window, cx| {
                        app.update_provider_source_mode(provider_id, mode.clone(), cx);
                    }))
            }));

        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(Label::new("Source").text_sm())
                .child(buttons)
                .into_any_element(),
        )
    };

    let cookie_header_input = if spec.show_cookie_header {
        Some(render_settings_input(
            "Cookie header",
            Input::new(&form.cookie_header)
                .mask_toggle()
                .cleanable(true)
                .flex_1(),
        ))
    } else {
        None
    };

    let api_key_input = if spec.show_api_key {
        Some(render_settings_input(
            "API key",
            Input::new(&form.api_key)
                .mask_toggle()
                .cleanable(true)
                .flex_1(),
        ))
    } else {
        None
    };

    let region_input = if spec.show_region {
        Some(render_settings_input(
            "Region",
            Input::new(&form.region).cleanable(true).flex_1(),
        ))
    } else {
        None
    };

    let workspace_input = if spec.show_workspace_id {
        Some(render_settings_input(
            "Workspace ID",
            Input::new(&form.workspace_id).cleanable(true).flex_1(),
        ))
    } else {
        None
    };

    let device_flow_section = if provider.config.id == ProviderId::Copilot {
        Some(render_copilot_device_flow_section(app, cx))
    } else {
        None
    };

    let token_accounts = render_token_accounts_section(provider, app, cx);

    Some(
        div()
            .rounded_lg()
            .border_1()
            .paddings(px(12.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(header)
            .when_some(source_buttons, |this, buttons| this.child(buttons))
            .when_some(cookie_header_input, |this, input| this.child(input))
            .when_some(api_key_input, |this, input| this.child(input))
            .when_some(device_flow_section, |this, section| this.child(section))
            .when_some(region_input, |this, input| this.child(input))
            .when_some(workspace_input, |this, input| this.child(input))
            .when_some(token_accounts, |this, section| this.child(section))
            .into_any_element(),
    )
}

fn render_copilot_device_flow_section(
    app: &AgentStatsApp,
    cx: &mut Context<AgentStatsApp>,
) -> gpui::AnyElement {
    match &app.copilot_device_flow {
        CopilotDeviceFlowState::Idle => {
            let has_token = app
                .config
                .providers
                .iter()
                .find(|provider| provider.id == ProviderId::Copilot)
                .and_then(|provider| provider.api_key.as_ref())
                .map(|key| !key.trim().is_empty())
                .unwrap_or(false);

            let label = if has_token {
                "Sign in again with GitHub"
            } else {
                "Sign in with GitHub"
            };

            div()
                .child(
                    Button::new("copilot-device-flow-start")
                        .label(label)
                        .on_click(cx.listener(|app, _event, _window, cx| {
                            app.start_copilot_device_flow(cx);
                        })),
                )
                .into_any_element()
        }
        CopilotDeviceFlowState::RequestingCode => {
            div()
                .child(Label::new("Requesting device code...").text_sm())
                .into_any_element()
        }
        CopilotDeviceFlowState::ShowingCode {
            user_code,
            verification_uri,
            device_code,
            interval,
        } => {
            let user_code = user_code.clone();
            let verification_uri = verification_uri.clone();
            let device_code = device_code.clone();
            let interval = *interval;

            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(Label::new("Enter this code on GitHub:").text_sm())
                .child(
                    div()
                        .rounded_md()
                        .bg(gpui::rgba(0x50AAFF26))
                        .paddings(px(10.0))
                        .child(Label::new(user_code.clone()).text_lg()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            Button::new("copilot-open-browser")
                                .label("Open Browser & Start")
                                .on_click(cx.listener(move |app, _event, _window, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        user_code.clone(),
                                    ));
                                    cx.open_url(&verification_uri);
                                    app.start_copilot_polling(
                                        device_code.clone(),
                                        interval,
                                        user_code.clone(),
                                        cx,
                                    );
                                })),
                        )
                        .child(
                            Button::new("copilot-cancel-showing")
                                .label("Cancel")
                                .on_click(cx.listener(|app, _event, _window, cx| {
                                    app.cancel_copilot_device_flow(cx);
                                })),
                        ),
                )
                .into_any_element()
        }
        CopilotDeviceFlowState::Polling { user_code } => {
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(Label::new("Waiting for authentication...").text_sm())
                .child(
                    div()
                        .rounded_md()
                        .bg(gpui::rgba(0x50AAFF26))
                        .paddings(px(10.0))
                        .child(Label::new(format!("Code: {user_code}")).text_sm()),
                )
                .child(
                    Button::new("copilot-cancel-polling")
                        .label("Cancel")
                        .on_click(cx.listener(|app, _event, _window, cx| {
                            app.cancel_copilot_device_flow(cx);
                        })),
                )
                .into_any_element()
        }
        CopilotDeviceFlowState::Success => {
            div()
                .rounded_md()
                .bg(gpui::rgba(0x50C87826))
                .paddings(px(10.0))
                .child(Label::new("Login successful!").text_sm())
                .into_any_element()
        }
        CopilotDeviceFlowState::Error(message) => {
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .rounded_md()
                        .bg(gpui::rgba(0xFF5A5A26))
                        .paddings(px(8.0))
                        .child(Label::new(message.clone()).text_sm()),
                )
                .child(
                    Button::new("copilot-retry")
                        .label("Try again")
                        .on_click(cx.listener(|app, _event, _window, cx| {
                            app.start_copilot_device_flow(cx);
                        })),
                )
                .into_any_element()
        }
    }
}

fn render_token_accounts_section(
    provider: &ProviderState,
    app: &AgentStatsApp,
    cx: &mut Context<AgentStatsApp>,
) -> Option<gpui::AnyElement> {
    let support = token_account_support(provider.config.id)?;
    let form = app.token_account_forms.get(&provider.config.id)?;

    let accounts = provider
        .config
        .token_accounts
        .as_ref()
        .map(|data| data.accounts.as_slice())
        .unwrap_or(&[]);
    let active_index = provider
        .config
        .token_accounts
        .as_ref()
        .map(|data| data.active_index)
        .unwrap_or(0);
    let active_index = if accounts.is_empty() {
        0
    } else {
        active_index.min(accounts.len().saturating_sub(1))
    };

    let token_value = form.token.read(cx).value();
    let can_add = !token_value.trim().is_empty();

    let header = token_accounts_header(support);

    let account_rows = if accounts.is_empty() {
        div()
            .child(Label::new("No token accounts yet.").text_sm())
            .into_any_element()
    } else {
        let provider_id = provider.config.id;
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .children(accounts.iter().enumerate().map(|(index, account)| {
                let selected = index == active_index;
                let label = account.label.clone();
                Button::new(format!("token-account-{}-{}", provider_id, index))
                    .label(label)
                    .selected(selected)
                    .on_click(cx.listener(move |app, _event, _window, cx| {
                        app.set_active_token_account(provider_id, index, cx);
                    }))
            }))
            .into_any_element()
    };

    let provider_id = provider.config.id;
    let add_button = Button::new(format!("token-add-{}", provider_id))
        .label("Add account")
        .disabled(!can_add)
        .on_click(cx.listener(move |app, _event, window, cx| {
            app.add_token_account(provider_id, window, cx);
        }));

    let remove_button = Button::new(format!("token-remove-{}", provider_id))
        .label("Remove selected account")
        .disabled(accounts.is_empty())
        .on_click(cx.listener(move |app, _event, _window, cx| {
            app.remove_selected_token_account(provider_id, cx);
        }));

    let inputs = div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(render_settings_input(
            "Label",
            Input::new(&form.label).cleanable(true).flex_1(),
        ))
        .child(render_settings_input(
            "Token",
            Input::new(&form.token)
                .mask_toggle()
                .cleanable(true)
                .flex_1(),
        ))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(add_button)
                .child(remove_button),
        );

    Some(
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(header)
            .child(account_rows)
            .child(inputs)
            .into_any_element(),
    )
}

fn token_accounts_header(support: &TokenAccountSupport) -> gpui::AnyElement {
    let subtitle = support.subtitle.trim();
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(Label::new(support.title).text_sm())
        .when(!subtitle.is_empty(), |this| {
            this.child(Label::new(subtitle).text_sm())
        })
        .into_any_element()
}

fn render_settings_input(label: &str, input: Input) -> gpui::AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(Label::new(label.to_string()).text_sm())
        .child(input)
        .into_any_element()
}

fn render_provider_card(provider: &ProviderState, settings: &AppSettings) -> gpui::AnyElement {
    let payload = provider.payload.as_ref();
    let usage = payload.and_then(|payload| payload.usage.as_ref());
    let status_indicator = payload
        .and_then(|payload| payload.status.as_ref())
        .map(|status| status.indicator.clone());
    let error = payload.and_then(|payload| payload.error.as_ref());
    let has_snapshot = usage.is_some()
        || payload
            .and_then(|payload| payload.credits.as_ref())
            .is_some();
    let stale_badge = error.map(|_| {
        let label = if has_snapshot {
            "Stale"
        } else {
            "Last refresh failed"
        };
        div()
            .rounded_sm()
            .bg(gpui::rgba(0xFF5A5A26))
            .paddings(px(4.0))
            .child(Label::new(label).text_sm())
    });

    let header = div()
        .flex()
        .items_center()
        .justify_between()
        .child(Label::new(provider.metadata.display_name).text_lg())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(render_status_indicator(status_indicator))
                .when_some(stale_badge, |this, badge| this.child(badge)),
        );

    let usage_section = usage
        .map(|snapshot| render_usage(snapshot, provider.metadata.id, settings))
        .unwrap_or_else(|| {
            div().child(Label::new("No usage data")).into_any_element()
        });

    let identity_section = usage
        .and_then(|snapshot| snapshot.identity.as_ref())
        .and_then(render_identity_section);

    let credits = payload
        .and_then(|payload| payload.credits.as_ref())
        .map(|credits| {
            div().child(Label::new(format!("Credits remaining: {:.2}", credits.remaining)))
        });

    let error_row = error.map(|error| {
        div()
            .rounded_md()
            .bg(gpui::rgba(0xFF5A5A1F))
            .paddings(px(8.0))
            .child(Label::new(&error.message))
    });

    div()
        .rounded_lg()
        .border_1()
        .paddings(px(12.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(header)
        .when_some(identity_section, |this, section| this.child(section))
        .child(usage_section)
        .when_some(credits, |this, credits| this.child(credits))
        .when_some(error_row, |this, error| this.child(error))
        .into_any_element()
}

fn render_identity_section(identity: &ProviderIdentitySnapshot) -> Option<gpui::AnyElement> {
    let mut lines: Vec<gpui::AnyElement> = Vec::new();

    if let Some(email) = identity.account_email.as_ref() {
        let email = email.trim();
        if !email.is_empty() {
            lines.push(Label::new(format!("Account: {email}")).text_sm().into_any_element());
        }
    }

    if let Some(organization) = identity.account_organization.as_ref() {
        let organization = organization.trim();
        if !organization.is_empty() {
            lines.push(
                Label::new(format!("Organization: {organization}"))
                    .text_sm()
                    .into_any_element(),
            );
        }
    }

    if let Some(method) = identity.login_method.as_ref() {
        let method = method.trim();
        if !method.is_empty() {
            lines.push(
                Label::new(format!("Login: {method}"))
                    .text_sm()
                    .into_any_element(),
            );
        }
    }

    if lines.is_empty() {
        return None;
    }

    Some(
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .children(lines)
            .into_any_element(),
    )
}

fn render_usage(
    snapshot: &UsageSnapshot,
    provider_id: ProviderId,
    settings: &AppSettings,
) -> gpui::AnyElement {
    let primary = snapshot.primary.as_ref().map(|window| {
        render_rate_window("Session", window, settings)
    });
    let secondary = snapshot.secondary.as_ref().map(|window| {
        render_rate_window("Weekly", window, settings)
    });
    let tertiary = snapshot.tertiary.as_ref().map(|window| {
        let label = match provider_id {
            ProviderId::Claude => "Sonnet",
            _ => "Tertiary",
        };
        render_rate_window(label, window, settings)
    });

    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .when_some(primary, |this, window| this.child(window))
        .when_some(secondary, |this, window| this.child(window))
        .when_some(tertiary, |this, window| this.child(window))
        .into_any_element()
}

fn render_rate_window(label: &str, window: &RateWindow, settings: &AppSettings) -> gpui::AnyElement {
    let percent = if settings.show_used {
        window.used_percent
    } else {
        window.remaining_percent()
    };
    let percent_label = if settings.show_used { "used" } else { "left" };
    let reset_text = format_reset(
        window.resets_at,
        settings.show_absolute_reset,
        window.reset_description.as_deref(),
    );

    let bar = Progress::new(format!("bar-{}-{label}", percent as u64))
        .value(percent as f32)
        .color(gpui::rgba(0x50AAFFFF));

    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .flex()
                .justify_between()
                .child(Label::new(label.to_string()))
                .child(Label::new(format!("{percent:.0}% {percent_label}")).text_sm()),
        )
        .child(bar)
        .child(Label::new(reset_text).text_sm())
        .into_any_element()
}

fn render_status_indicator(indicator: Option<ProviderStatusIndicator>) -> gpui::AnyElement {
    let (color, label) = match indicator.unwrap_or(ProviderStatusIndicator::Unknown) {
        ProviderStatusIndicator::None => (gpui::rgba(0x50C878FF), "Operational"),
        ProviderStatusIndicator::Minor => (gpui::rgba(0xFFC850FF), "Minor"),
        ProviderStatusIndicator::Major => (gpui::rgba(0xFF8C50FF), "Major"),
        ProviderStatusIndicator::Critical => (gpui::rgba(0xFF5A5AFF), "Critical"),
        ProviderStatusIndicator::Maintenance => (gpui::rgba(0x7878FFFF), "Maintenance"),
        ProviderStatusIndicator::Unknown => (gpui::rgba(0x969696FF), "Unknown"),
    };

    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(div().w(px(8.0)).h(px(8.0)).rounded_full().bg(color))
        .child(Label::new(label))
        .into_any_element()
}

fn render_last_refresh(last_refresh: Option<OffsetDateTime>) -> gpui::AnyElement {
    let text = last_refresh
        .map(|time| {
            let minutes = (OffsetDateTime::now_utc() - time).whole_minutes();
            if minutes <= 1 {
                "Updated just now".to_string()
            } else {
                format!("Updated {minutes}m ago")
            }
        })
        .unwrap_or_else(|| "Not updated yet".to_string());
    Label::new(text).into_any_element()
}

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

impl ProviderSettingsSpec {
    fn is_visible(&self) -> bool {
        !self.source_modes.is_empty()
            || self.show_cookie_header
            || self.show_api_key
            || self.show_region
            || self.show_workspace_id
    }
}

fn provider_settings_spec(provider_id: ProviderId) -> ProviderSettingsSpec {
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

fn source_mode_label(mode: &ProviderSourceMode) -> &'static str {
    match mode {
        ProviderSourceMode::Auto => "Auto",
        ProviderSourceMode::Web => "Web",
        ProviderSourceMode::Cli => "CLI",
        ProviderSourceMode::Oauth => "OAuth",
        ProviderSourceMode::Api => "API",
    }
}

fn build_provider_forms(
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
        let api_key = build_input_state(
            window,
            cx,
            provider.api_key.as_deref(),
            "API key",
            true,
        );
        let region = build_input_state(
            window,
            cx,
            provider.region.as_deref(),
            "Region",
            false,
        );
        let workspace_id = build_input_state(
            window,
            cx,
            provider.workspace_id.as_deref(),
            "Workspace ID",
            false,
        );
        let source_mode = provider
            .source
            .clone()
            .unwrap_or(ProviderSourceMode::Auto);

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

fn build_token_account_forms(
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
    let default_value: gpui::SharedString = value.unwrap_or("").to_string().into();
    let placeholder: gpui::SharedString = placeholder.to_string().into();
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

fn format_reset(
    resets_at: Option<OffsetDateTime>,
    show_absolute: bool,
    reset_description: Option<&str>,
) -> String {
    format_reset_with_now(resets_at, show_absolute, reset_description, OffsetDateTime::now_utc())
}

fn format_reset_with_now(
    resets_at: Option<OffsetDateTime>,
    show_absolute: bool,
    reset_description: Option<&str>,
    now: OffsetDateTime,
) -> String {
    if let Some(description) = reset_description {
        let trimmed = description.trim();
        if trimmed.is_empty() {
            return "Reset time unavailable".to_string();
        }

        if trimmed.to_lowercase().starts_with("resets") {
            return trimmed.to_string();
        }

        return format!("Resets {trimmed}");
    }

    let Some(resets_at) = resets_at else {
        return "Reset time unavailable".to_string();
    };

    if show_absolute {
        let format = time::format_description::parse("[hour repr:12]:[minute padding:zero] [period]")
            .ok();
        if let Some(format) = format {
            if let Ok(text) = resets_at.format(&format) {
                return format!("Resets at {text}");
            }
        }
    }

    let diff = resets_at - now;
    if diff.is_negative() {
        return "Resetting soon".to_string();
    }

    let hours = diff.whole_hours();
    let minutes = (diff - Duration::hours(hours)).whole_minutes();
    if hours > 0 {
        format!("Resets in {hours}h {minutes}m")
    } else {
        format!("Resets in {minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::{ProviderId, ProviderMoveDirection, format_reset_with_now, move_provider_in_config};
    use agent_stats_core::config::{AppConfig, ProviderConfig};
    use time::{Duration, OffsetDateTime};

    fn fixed_time() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000)
            .unwrap_or_else(|_| OffsetDateTime::now_utc())
    }

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
    fn format_reset_preserves_existing_prefix() {
        let now = fixed_time();
        let result = format_reset_with_now(None, false, Some("Resets tomorrow"), now);
        assert_eq!(result, "Resets tomorrow");
    }

    #[test]
    fn format_reset_adds_prefix_when_missing() {
        let now = fixed_time();
        let result = format_reset_with_now(None, false, Some("in 2h"), now);
        assert_eq!(result, "Resets in 2h");
    }

    #[test]
    fn format_reset_relative_when_no_description() {
        let now = fixed_time();
        let resets_at = now + Duration::hours(2) + Duration::minutes(5);
        let result = format_reset_with_now(Some(resets_at), false, None, now);
        assert_eq!(result, "Resets in 2h 5m");
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
