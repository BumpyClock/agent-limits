use agent_stats_core::backend::{FetchRequest, fetch_usage};
use agent_stats_core::config::{AppConfig, ProviderConfig};
use agent_stats_core::models::{ProviderPayload, ProviderStatusIndicator, RateWindow, UsageSnapshot};
use agent_stats_core::provider_metadata::{ProviderId, ProviderMetadata, provider_metadata};
use gpui::{
    App, Application, AsyncApp, Context, Render, Task, WeakEntity, Window, WindowOptions, div, px,
    prelude::*, point,
};
use gpui_component::button::Button;
use gpui_component::checkbox::Checkbox;
use gpui_component::label::Label;
use gpui_component::progress::Progress;
use gpui_component::{Disableable, Selectable, StyledExt};
use gpui_component::Root;
use std::collections::HashMap;
use time::{Duration, OffsetDateTime};

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
        titlebar: Some(gpui::TitlebarOptions {
            title: Some("Agent Stats".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(16.0), px(16.0))),
        }),
        ..Default::default()
    };

    let _ = cx.open_window(window_options, |window, cx| {
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

pub struct AgentStatsApp {
    providers: Vec<ProviderState>,
    config: AppConfig,
    settings: AppSettings,
    last_refresh: Option<OffsetDateTime>,
    refreshing: bool,
    error: Option<String>,
    refresh_task: Option<Task<()>>,
    show_settings: bool,
}

impl AgentStatsApp {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = AppSettings::load();
        let config = AppConfig::load_or_default().unwrap_or_else(|error| {
            log::error!("Failed to load config: {error}");
            AppConfig::default_config()
        });
        let providers = build_provider_states(&config);

        let mut app = Self {
            providers,
            config,
            settings,
            last_refresh: None,
            refreshing: false,
            error: None,
            refresh_task: None,
            show_settings: false,
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
        let task = cx.background_spawn(async move {
            fetch_usage(FetchRequest {
                include_status,
                ..Default::default()
            })
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

        if let Err(error) = self.config.save() {
            log::error!("Failed to save config: {error}");
        } else {
            self.providers = build_provider_states(&self.config);
            self.refresh_now(cx);
        }
        cx.notify();
    }
}

impl Render for AgentStatsApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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

    let provider_toggles = div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .children(app.providers.iter().map(|provider| {
            let enabled = provider
                .config
                .enabled
                .unwrap_or(provider.metadata.default_enabled);
            let provider_id = provider.config.id;
            Checkbox::new(format!("provider-{}", provider_id))
                .label(provider.metadata.display_name)
                .checked(enabled)
                .on_click(cx.listener(move |app, checked, _window, cx| {
                    app.update_provider_enabled(provider_id, *checked, cx);
                }))
        }));

    div()
        .size_full()
        .flex()
        .flex_col()
        .gap(px(16.0))
        .paddings(px(16.0))
        .child(settings_header)
        .child(div().child(Label::new("Refresh cadence")))
        .child(cadence_buttons)
        .child(div().child(Label::new("Display")))
        .child(toggles)
        .child(div().child(Label::new("Providers")))
        .child(provider_toggles)
        .into_any_element()
}

fn render_provider_card(provider: &ProviderState, settings: &AppSettings) -> gpui::AnyElement {
    let payload = provider.payload.as_ref();
    let usage = payload.and_then(|payload| payload.usage.as_ref());
    let status_indicator = payload
        .and_then(|payload| payload.status.as_ref())
        .map(|status| status.indicator.clone());
    let error = payload.and_then(|payload| payload.error.as_ref());

    let header = div()
        .flex()
        .items_center()
        .justify_between()
        .child(Label::new(provider.metadata.display_name).text_lg())
        .child(render_status_indicator(status_indicator));

    let usage_section = usage
        .map(|snapshot| render_usage(snapshot, settings))
        .unwrap_or_else(|| {
            div().child(Label::new("No usage data")).into_any_element()
        });

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
        .child(usage_section)
        .when_some(credits, |this, credits| this.child(credits))
        .when_some(error_row, |this, error| this.child(error))
        .into_any_element()
}

fn render_usage(snapshot: &UsageSnapshot, settings: &AppSettings) -> gpui::AnyElement {
    let primary = snapshot.primary.as_ref().map(|window| {
        render_rate_window("Session", window, settings)
    });
    let secondary = snapshot.secondary.as_ref().map(|window| {
        render_rate_window("Weekly", window, settings)
    });

    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .when_some(primary, |this, window| this.child(window))
        .when_some(secondary, |this, window| this.child(window))
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
    if let Some(description) = reset_description {
        return format!("Resets {description}");
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

    let now = OffsetDateTime::now_utc();
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
