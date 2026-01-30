use agent_stats_core::models::{
    ProviderErrorPayload, ProviderIdentitySnapshot, ProviderPayload, ProviderStatusIndicator,
    RateWindow, UsageSnapshot,
};
use agent_stats_core::provider_metadata::ProviderId;
use agent_stats_core::token_accounts::{token_account_support, TokenAccountSupport};
use gpui::{
    div, prelude::*, px, Animation, AnimationExt, AnyElement, App, ClipboardItem, Context, Hsla,
    Render, Window,
};
use gpui_component::animation::cubic_bezier;
use gpui_component::button::Button;
use gpui_component::checkbox::Checkbox;
use gpui_component::input::Input;
use gpui_component::label::Label;
use gpui_component::progress::Progress;
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::skeleton::Skeleton;
use gpui_component::{ActiveTheme, Disableable, Selectable, StyledExt};
use std::time::Duration as StdDuration;
use time::{Duration, OffsetDateTime};

use super::forms::{
    build_provider_forms, build_token_account_forms, provider_settings_spec, source_mode_label,
};
use super::state::AgentStatsApp;
use super::types::{CopilotDeviceFlowState, ProviderMoveDirection, ProviderState};
use super::{About, KEY_CONTEXT, Minimize, OpenSettings, Refresh, Zoom};
use crate::settings::{AppSettings, RefreshCadence};

impl Render for AgentStatsApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.needs_form_rebuild {
            self.needs_form_rebuild = false;
            self.provider_forms = build_provider_forms(&self.config, window, cx);
            self.token_account_forms = build_token_account_forms(&self.config, window, cx);
        }

        let content: AnyElement = if self.show_settings {
            render_settings(self, cx)
        } else {
            let now = OffsetDateTime::now_utc();
            let header = div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(Label::new("Agent Limits").text_lg().font_semibold())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    Button::new("refresh")
                                        .label("Refresh")
                                        .disabled(self.refreshing)
                                        .on_click(cx.listener(|app, _event, _window, cx| {
                                            app.refresh_now(cx);
                                        })),
                                )
                                .child(Button::new("settings").label("Settings").on_click(
                                    cx.listener(|app, _event, _window, cx| {
                                        app.toggle_settings(cx);
                                    }),
                                )),
                        ),
                )
                .child(render_last_refresh(
                    self.last_refresh,
                    self.refreshing,
                    now,
                    cx,
                ));

            let error_banner = self.error.as_ref().map(|message| {
                div()
                    .rounded_md()
                    .border_1()
                    .bg(cx.theme().danger.opacity(0.12))
                    .paddings(px(10.0))
                    .child(
                        Label::new(format!("Refresh failed: {message}"))
                            .text_sm()
                            .text_color(cx.theme().danger_foreground),
                    )
            });

            let glance_strip = build_glance_summary(&self.providers, now)
                .map(|summary| render_glance_strip(&summary, now, cx));

            let provider_list = div().flex().flex_col().gap(px(16.0)).children(
                self.providers
                    .iter()
                    .filter(|provider| provider_is_enabled(provider))
                    .map(|provider| {
                        render_provider_card(provider, &self.settings, self.refreshing, now, cx)
                    }),
            );

            div()
                .size_full()
                .flex()
                .flex_col()
                .gap(px(16.0))
                .paddings(px(16.0))
                .overflow_y_scrollbar()
                .child(header)
                .when_some(error_banner, |this, banner| this.child(banner))
                .when_some(glance_strip, |this, strip| this.child(strip))
                .child(provider_list)
                .into_any_element()
        };

        div()
            .size_full()
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|app, _: &OpenSettings, _window, cx| {
                app.toggle_settings(cx);
            }))
            .on_action(cx.listener(|app, _: &Refresh, _window, cx| {
                app.refresh_now(cx);
            }))
            .on_action(cx.listener(|app, _: &About, window, cx| {
                app.open_about_dialog(window, cx);
            }))
            .on_action(|_: &Minimize, window, _cx| {
                window.minimize_window();
            })
            .on_action(|_: &Zoom, window, _cx| {
                window.toggle_fullscreen();
            })
            .child(content)
    }
}

fn render_settings(app: &mut AgentStatsApp, cx: &mut Context<AgentStatsApp>) -> AnyElement {
    let settings_header = div()
        .flex()
        .items_center()
        .justify_between()
        .child(Label::new("Settings").text_lg())
        .child(Button::new("back").label("Back").on_click(cx.listener(
            |app, _event, _window, cx| {
                app.toggle_settings(cx);
            },
        )));

    let cadence_buttons = div().flex().gap(px(8.0)).children(
        [
            RefreshCadence::Manual,
            RefreshCadence::OneMinute,
            RefreshCadence::TwoMinutes,
            RefreshCadence::FiveMinutes,
            RefreshCadence::FifteenMinutes,
            RefreshCadence::ThirtyMinutes,
        ]
        .into_iter()
        .map(|cadence| {
            let selected = app.settings.refresh_cadence == cadence;
            Button::new(format!("cadence-{}", cadence.label()))
                .label(cadence.label())
                .selected(selected)
                .on_click(cx.listener(move |app, _event, _window, cx| {
                    let mut settings = app.settings.clone();
                    settings.refresh_cadence = cadence.clone();
                    app.update_setting(settings, cx);
                }))
        }),
    );

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
    let provider_toggles =
        div()
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

    let provider_settings_cards: Vec<AnyElement> = app
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
) -> Option<AnyElement> {
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
) -> AnyElement {
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
        CopilotDeviceFlowState::RequestingCode => div()
            .child(Label::new("Requesting device code...").text_sm())
            .into_any_element(),
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
        CopilotDeviceFlowState::Polling { user_code } => div()
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
            .into_any_element(),
        CopilotDeviceFlowState::Success => div()
            .rounded_md()
            .bg(gpui::rgba(0x50C87826))
            .paddings(px(10.0))
            .child(Label::new("Login successful!").text_sm())
            .into_any_element(),
        CopilotDeviceFlowState::Error(message) => div()
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
            .into_any_element(),
    }
}

fn render_token_accounts_section(
    provider: &ProviderState,
    app: &AgentStatsApp,
    cx: &mut Context<AgentStatsApp>,
) -> Option<AnyElement> {
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

fn token_accounts_header(support: &TokenAccountSupport) -> AnyElement {
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

fn render_settings_input(label: &str, input: Input) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(Label::new(label.to_string()).text_sm())
        .child(input)
        .into_any_element()
}

fn render_provider_card(
    provider: &ProviderState,
    settings: &AppSettings,
    refreshing: bool,
    now: OffsetDateTime,
    cx: &App,
) -> AnyElement {
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
    let is_loading = refreshing && !has_snapshot && error.is_none();
    let account_text = provider_account_text(payload, usage);
    let account_label = account_text.clone().map(|label| {
        Label::new(label)
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .truncate()
            .into_any_element()
    });
    let identity_line = usage
        .and_then(|snapshot| snapshot.identity.as_ref())
        .and_then(|identity| render_identity_line(identity, account_text.as_deref(), cx));
    let updated_text = provider_updated_text(usage, error, is_loading, now);
    let status_badge = error.map(|_| {
        let label = if has_snapshot { "Stale" } else { "Failed" };
        status_badge(label, cx)
    });
    let status_cluster = div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .when_some(
            status_indicator.map(|indicator| render_status_indicator(Some(indicator), cx)),
            |this, indicator| this.child(indicator),
        )
        .when_some(status_badge, |this, badge| this.child(badge));

    let header = div()
        .flex()
        .items_center()
        .justify_between()
        .child(
            Label::new(provider.metadata.display_name)
                .text_lg()
                .font_semibold(),
        )
        .when_some(account_label, |this, label| this.child(label));

    let subheader = div()
        .flex()
        .items_center()
        .justify_between()
        .child(
            Label::new(updated_text)
                .text_sm()
                .text_color(cx.theme().muted_foreground),
        )
        .child(status_cluster);

    let usage_section = if is_loading {
        render_usage_loading(provider.metadata.id)
    } else {
        let content = usage
            .map(|snapshot| render_usage(snapshot, provider.metadata.id, settings, cx))
            .unwrap_or_else(|| {
                div()
                    .child(
                        Label::new("No usage data")
                            .text_sm()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .into_any_element()
            });
        div()
            .child(content)
            .with_animation(
                format!("usage-fade-{}", provider.metadata.id),
                Animation::new(StdDuration::from_millis(150))
                    .with_easing(cubic_bezier(0.25, 1.0, 0.5, 1.0)),
                |this, delta| this.opacity(delta),
            )
            .into_any_element()
    };

    let credits = payload
        .and_then(|payload| payload.credits.as_ref())
        .map(|credits| render_credits_row(credits.remaining, cx));

    let error_row = error.map(|error| render_error_row(&error.message, cx));

    div()
        .rounded_lg()
        .border_1()
        .bg(cx.theme().secondary.opacity(0.35))
        .paddings(px(16.0))
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(header)
        .child(subheader)
        .when_some(identity_line, |this, line| this.child(line))
        .child(usage_section)
        .when_some(credits, |this, credits| {
            this.child(section_divider(cx)).child(credits)
        })
        .when_some(error_row, |this, error| {
            this.child(section_divider(cx)).child(error)
        })
        .into_any_element()
}

fn provider_account_text(
    payload: Option<&ProviderPayload>,
    usage: Option<&UsageSnapshot>,
) -> Option<String> {
    let identity = usage.and_then(|snapshot| snapshot.identity.as_ref());
    let email = identity
        .and_then(|identity| identity.account_email.as_ref())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    let account = payload
        .and_then(|payload| payload.account.as_ref())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    let organization = identity
        .and_then(|identity| identity.account_organization.as_ref())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());

    email
        .or(account)
        .or(organization)
        .map(|value| value.to_string())
}

fn render_identity_line(
    identity: &ProviderIdentitySnapshot,
    account_label: Option<&str>,
    cx: &App,
) -> Option<AnyElement> {
    let mut parts = Vec::new();

    if let Some(organization) = identity.account_organization.as_ref() {
        let organization = organization.trim();
        if !organization.is_empty() && Some(organization) != account_label {
            parts.push(format!("Org: {organization}"));
        }
    }

    if let Some(method) = identity.login_method.as_ref() {
        let method = method.trim();
        if !method.is_empty() {
            parts.push(format!("Login: {method}"));
        }
    }

    if parts.is_empty() {
        return None;
    }

    Some(
        Label::new(parts.join(" | "))
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .into_any_element(),
    )
}

fn render_usage(
    snapshot: &UsageSnapshot,
    provider_id: ProviderId,
    settings: &AppSettings,
    cx: &App,
) -> AnyElement {
    let accent = provider_accent_color(provider_id, cx);
    let primary = snapshot
        .primary
        .as_ref()
        .map(|window| render_rate_window("Session", window, settings, accent, cx));
    let secondary = snapshot
        .secondary
        .as_ref()
        .map(|window| render_rate_window("Weekly", window, settings, accent, cx));
    let tertiary = snapshot.tertiary.as_ref().map(|window| {
        let label = match provider_id {
            ProviderId::Claude => "Sonnet",
            _ => "Tertiary",
        };
        render_rate_window(label, window, settings, accent, cx)
    });

    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .when_some(primary, |this, window| this.child(window))
        .when_some(secondary, |this, window| this.child(window))
        .when_some(tertiary, |this, window| this.child(window))
        .into_any_element()
}

fn render_usage_loading(provider_id: ProviderId) -> AnyElement {
    let row_count = if provider_id == ProviderId::Claude {
        3
    } else {
        2
    };
    let mut rows = Vec::with_capacity(row_count);
    for _ in 0..row_count {
        rows.push(render_loading_metric_row());
    }

    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .children(rows)
        .into_any_element()
}

fn render_loading_metric_row() -> AnyElement {
    let title = Skeleton::new().w(px(140.0)).h(px(12.0)).rounded_sm();
    let bar = Skeleton::new().w_full().h(px(6.0)).rounded_full();
    let left = Skeleton::new()
        .w(px(80.0))
        .h(px(10.0))
        .rounded_sm()
        .secondary();
    let right = Skeleton::new()
        .w(px(120.0))
        .h(px(10.0))
        .rounded_sm()
        .secondary();

    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(title)
        .child(bar)
        .child(div().flex().justify_between().child(left).child(right))
        .into_any_element()
}

fn render_rate_window(
    label: &str,
    window: &RateWindow,
    settings: &AppSettings,
    accent: Hsla,
    cx: &App,
) -> AnyElement {
    let percent = if settings.show_used {
        window.used_percent
    } else {
        window.remaining_percent()
    };
    let percent_label = if settings.show_used { "used" } else { "left" };
    let show_reset = window.resets_at.is_some()
        || window
            .reset_description
            .as_ref()
            .is_some_and(|description| !description.trim().is_empty());
    let reset_text = format_reset(
        window.resets_at,
        settings.show_absolute_reset,
        window.reset_description.as_deref(),
    );

    let bar = Progress::new(format!("bar-{}-{label}", percent as u64))
        .value(percent as f32)
        .color(accent);

    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(Label::new(label.to_string()).text_sm().font_semibold())
        .child(bar)
        .child(
            div()
                .flex()
                .justify_between()
                .child(
                    Label::new(format!("{percent:.0}% {percent_label}"))
                        .text_sm()
                        .text_color(cx.theme().muted_foreground),
                )
                .when(show_reset, |this| {
                    this.child(
                        Label::new(reset_text)
                            .text_sm()
                            .text_color(cx.theme().muted_foreground),
                    )
                }),
        )
        .into_any_element()
}

fn render_status_indicator(indicator: Option<ProviderStatusIndicator>, cx: &App) -> AnyElement {
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
        .child(
            Label::new(label)
                .text_sm()
                .text_color(cx.theme().muted_foreground),
        )
        .into_any_element()
}

struct GlanceSummary {
    next_reset: Option<OffsetDateTime>,
    highest_used: Option<f64>,
    stale_count: usize,
}

fn provider_is_enabled(provider: &ProviderState) -> bool {
    provider
        .config
        .enabled
        .unwrap_or(provider.metadata.default_enabled)
}

fn build_glance_summary(providers: &[ProviderState], now: OffsetDateTime) -> Option<GlanceSummary> {
    let mut has_snapshot = false;
    let mut next_reset: Option<OffsetDateTime> = None;
    let mut highest_used: Option<f64> = None;
    let mut stale_count = 0;

    for provider in providers
        .iter()
        .filter(|provider| provider_is_enabled(provider))
    {
        let usage = provider
            .payload
            .as_ref()
            .and_then(|payload| payload.usage.as_ref());
        let Some(snapshot) = usage else {
            continue;
        };
        has_snapshot = true;

        if now - snapshot.updated_at > Duration::minutes(30) {
            stale_count += 1;
        }

        for window in [
            snapshot.primary.as_ref(),
            snapshot.secondary.as_ref(),
            snapshot.tertiary.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            let used = clamp_percent(window.used_percent);
            highest_used = Some(highest_used.map_or(used, |current| current.max(used)));
            if let Some(resets_at) = window.resets_at {
                if resets_at > now {
                    next_reset =
                        Some(next_reset.map_or(resets_at, |current| current.min(resets_at)));
                }
            }
        }
    }

    if !has_snapshot {
        return None;
    }

    if next_reset.is_none() && highest_used.is_none() && stale_count == 0 {
        return None;
    }

    Some(GlanceSummary {
        next_reset,
        highest_used,
        stale_count,
    })
}

fn render_glance_strip(summary: &GlanceSummary, now: OffsetDateTime, cx: &App) -> AnyElement {
    let mut pills: Vec<AnyElement> = Vec::new();

    if let Some(next_reset) = summary.next_reset {
        let value = format_next_reset_value(next_reset, now);
        pills.push(render_glance_pill("Next reset", &value, cx));
    }

    if let Some(highest_used) = summary.highest_used {
        pills.push(render_glance_pill(
            "Highest used",
            &format!("{highest_used:.0}% used"),
            cx,
        ));
    }

    if summary.stale_count > 0 {
        pills.push(render_glance_pill(
            "Stale providers",
            &summary.stale_count.to_string(),
            cx,
        ));
    }

    div()
        .flex()
        .flex_wrap()
        .gap(px(8.0))
        .children(pills)
        .with_animation(
            "glance-fade",
            Animation::new(StdDuration::from_millis(150))
                .with_easing(cubic_bezier(0.25, 1.0, 0.5, 1.0)),
            |this, delta| this.opacity(0.4 + (0.6 * delta)),
        )
        .into_any_element()
}

fn render_glance_pill(label: &str, value: &str, cx: &App) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .paddings(px(8.0))
        .rounded_md()
        .border_1()
        .bg(cx.theme().secondary.opacity(0.5))
        .child(
            Label::new(label.to_string())
                .text_xs()
                .text_color(cx.theme().muted_foreground),
        )
        .child(Label::new(value.to_string()).text_sm().font_semibold())
        .into_any_element()
}

fn provider_updated_text(
    usage: Option<&UsageSnapshot>,
    error: Option<&ProviderErrorPayload>,
    is_loading: bool,
    now: OffsetDateTime,
) -> String {
    if is_loading {
        return "Updating...".to_string();
    }

    if let Some(snapshot) = usage {
        return format_updated_string(snapshot.updated_at, now);
    }

    if error.is_some() {
        return "Last refresh failed".to_string();
    }

    "No data yet".to_string()
}

fn format_updated_string(updated_at: OffsetDateTime, now: OffsetDateTime) -> String {
    let diff = now - updated_at;
    let seconds = diff.whole_seconds().abs();
    if seconds < 60 {
        return "Updated just now".to_string();
    }

    if seconds < 60 * 60 {
        let minutes = (seconds / 60).max(1);
        return format!("Updated {minutes}m ago");
    }

    if seconds < 60 * 60 * 24 {
        let hours = (seconds / 3600).max(1);
        return format!("Updated {hours}h ago");
    }

    let format =
        time::format_description::parse("[hour repr:12]:[minute padding:zero] [period]").ok();
    if let Some(format) = format {
        if let Ok(text) = updated_at.format(&format) {
            return format!("Updated {text}");
        }
    }

    "Updated".to_string()
}

fn format_next_reset_value(resets_at: OffsetDateTime, now: OffsetDateTime) -> String {
    let diff = resets_at - now;
    if diff.is_negative() {
        return "soon".to_string();
    }

    if diff.whole_seconds() < 60 {
        return "now".to_string();
    }

    let hours = diff.whole_hours();
    let minutes = (diff - Duration::hours(hours)).whole_minutes().max(1);
    if hours > 0 {
        format!("in {hours}h {minutes}m")
    } else {
        format!("in {minutes}m")
    }
}

fn status_badge(label: &str, cx: &App) -> AnyElement {
    div()
        .rounded_sm()
        .border_1()
        .bg(cx.theme().danger.opacity(0.12))
        .paddings(px(4.0))
        .child(
            Label::new(label.to_string())
                .text_xs()
                .text_color(cx.theme().danger_foreground),
        )
        .into_any_element()
}

fn render_credits_row(remaining: f64, cx: &App) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .child(
            Label::new("Credits remaining")
                .text_sm()
                .text_color(cx.theme().muted_foreground),
        )
        .child(
            Label::new(format!("{remaining:.2}"))
                .text_sm()
                .font_semibold(),
        )
        .into_any_element()
}

fn render_error_row(message: &str, cx: &App) -> AnyElement {
    div()
        .rounded_md()
        .border_1()
        .bg(cx.theme().danger.opacity(0.12))
        .paddings(px(10.0))
        .child(
            Label::new(message.to_string())
                .text_sm()
                .text_color(cx.theme().danger_foreground),
        )
        .into_any_element()
}

fn section_divider(cx: &App) -> AnyElement {
    div()
        .h(px(1.0))
        .bg(cx.theme().border.opacity(0.6))
        .into_any_element()
}

fn provider_accent_color(_provider_id: ProviderId, cx: &App) -> Hsla {
    cx.theme().accent
}

fn clamp_percent(value: f64) -> f64 {
    value.max(0.0).min(100.0)
}

fn render_last_refresh(
    last_refresh: Option<OffsetDateTime>,
    refreshing: bool,
    now: OffsetDateTime,
    cx: &App,
) -> AnyElement {
    let text = if refreshing {
        "Updating...".to_string()
    } else {
        last_refresh
            .map(|time| format_updated_string(time, now))
            .unwrap_or_else(|| "Not updated yet".to_string())
    };

    Label::new(text)
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .into_any_element()
}

fn format_reset(
    resets_at: Option<OffsetDateTime>,
    show_absolute: bool,
    reset_description: Option<&str>,
) -> String {
    format_reset_with_now(
        resets_at,
        show_absolute,
        reset_description,
        OffsetDateTime::now_utc(),
    )
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
        let format =
            time::format_description::parse("[hour repr:12]:[minute padding:zero] [period]").ok();
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
    use super::format_reset_with_now;
    use time::{Duration, OffsetDateTime};

    fn fixed_time() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000)
            .unwrap_or_else(|_| OffsetDateTime::now_utc())
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
}
