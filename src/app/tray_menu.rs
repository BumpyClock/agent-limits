use anyhow::{Context as _, Result};
use time::OffsetDateTime;

use agent_stats_core::models::{
    CreditsSnapshot, ProviderErrorPayload, ProviderPayload, UsageSnapshot,
};
use agent_stats_core::provider_metadata::{
    ProviderId, provider_dashboard_url, provider_status_url,
};

use super::forms::provider_settings_spec;
use super::tray_format::{
    build_account_line, format_cost_line, format_credits_line, format_rate_line,
    format_updated_line, trim_menu_text,
};
use super::types::ProviderState;
use crate::settings::AppSettings;

use tray_icon::menu::{CheckMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem};

#[derive(Clone)]
pub(crate) struct TrayProviderTab {
    pub(crate) id: ProviderId,
    pub(crate) item: CheckMenuItem,
}

#[derive(Clone)]
pub(crate) struct TrayMenuState {
    pub(crate) provider_tabs: Vec<TrayProviderTab>,
    pub(crate) refresh: MenuId,
    pub(crate) switch_account: Option<MenuId>,
    pub(crate) dashboard: Option<MenuId>,
    pub(crate) status_page: Option<MenuId>,
    pub(crate) open_preview: Option<MenuId>,
    pub(crate) open_window: MenuId,
    pub(crate) settings: MenuId,
    pub(crate) about: MenuId,
    pub(crate) quit: MenuId,
}
#[derive(Clone)]
pub(crate) struct TrayProviderSnapshot {
    pub(crate) id: ProviderId,
    pub(crate) display_name: String,
    pub(crate) payload: Option<ProviderPayload>,
    pub(crate) usage: Option<UsageSnapshot>,
    pub(crate) credits: Option<CreditsSnapshot>,
    pub(crate) error: Option<ProviderErrorPayload>,
    pub(crate) has_account_switch: bool,
    pub(crate) dashboard_url: Option<String>,
    pub(crate) status_url: Option<String>,
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub(crate) fn build_tray_menu(
    snapshots: &[TrayProviderSnapshot],
    active_provider: Option<ProviderId>,
    settings: &AppSettings,
    refreshing: bool,
    now: OffsetDateTime,
) -> Result<(Menu, TrayMenuState)> {
    let menu = Menu::new();
    let mut provider_tabs = Vec::new();

    for snapshot in snapshots {
        let checked = Some(snapshot.id) == active_provider;
        let item = CheckMenuItem::with_id(
            format!("tray-provider-{}", snapshot.id),
            snapshot.display_name.clone(),
            true,
            checked,
            None,
        );
        menu.append(&item)
            .with_context(|| format!("append provider tab {}", snapshot.id))?;
        provider_tabs.push(TrayProviderTab { id: snapshot.id, item: item.clone() });
    }

    if !provider_tabs.is_empty() {
        menu.append(&PredefinedMenuItem::separator())
            .context("append provider separator")?;
    }

    let preview_lines = build_preview_lines(snapshots, active_provider, settings, refreshing, now);
    if preview_lines.is_empty() {
        let empty = MenuItem::new("No providers enabled", false, None);
        menu.append(&empty).context("append empty preview")?;
    } else {
        for line in preview_lines {
            menu.append(&line).context("append preview line")?;
        }
    }

    menu.append(&PredefinedMenuItem::separator())
        .context("append actions separator")?;

    let refresh = MenuItem::with_id("tray-refresh", "Refresh", !refreshing, None);
    menu.append(&refresh).context("append refresh")?;

    let active_snapshot = active_provider.and_then(|id| {
        snapshots.iter().find(|snapshot| snapshot.id == id)
    });

    let switch_account = if active_snapshot
        .is_some_and(|snapshot| snapshot.has_account_switch)
    {
        let item = MenuItem::with_id("tray-switch-account", "Switch Account...", true, None);
        menu.append(&item).context("append switch account")?;
        Some(item.id().clone())
    } else {
        None
    };

    let dashboard = if active_snapshot
        .and_then(|snapshot| snapshot.dashboard_url.as_ref())
        .is_some()
    {
        let item = MenuItem::with_id("tray-dashboard", "Usage Dashboard", true, None);
        menu.append(&item).context("append dashboard")?;
        Some(item.id().clone())
    } else {
        None
    };

    let status_page = if active_snapshot
        .and_then(|snapshot| snapshot.status_url.as_ref())
        .is_some()
    {
        let item = MenuItem::with_id("tray-status", "Status Page", true, None);
        menu.append(&item).context("append status")?;
        Some(item.id().clone())
    } else {
        None
    };

    #[cfg(target_os = "linux")]
    let open_preview = {
        let item = MenuItem::with_id("tray-open-preview", "Open Preview", true, None);
        menu.append(&item).context("append open preview")?;
        Some(item.id().clone())
    };
    #[cfg(not(target_os = "linux"))]
    let open_preview = None;

    let open_window = MenuItem::with_id("tray-open-window", "Open Window", true, None);
    menu.append(&open_window).context("append open window")?;

    menu.append(&PredefinedMenuItem::separator())
        .context("append footer separator")?;

    let settings_item = MenuItem::with_id("tray-settings", "Settings...", true, None);
    menu.append(&settings_item).context("append settings")?;

    let about_item = MenuItem::with_id("tray-about", "About", true, None);
    menu.append(&about_item).context("append about")?;

    let quit_item = MenuItem::with_id("tray-quit", "Quit", true, None);
    menu.append(&quit_item).context("append quit")?;

    let menu_state = TrayMenuState {
        provider_tabs,
        refresh: refresh.id().clone(),
        switch_account,
        dashboard,
        status_page,
        open_preview,
        open_window: open_window.id().clone(),
        settings: settings_item.id().clone(),
        about: about_item.id().clone(),
        quit: quit_item.id().clone(),
    };

    Ok((menu, menu_state))
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub(crate) fn build_snapshots(providers: &[ProviderState]) -> Vec<TrayProviderSnapshot> {
    providers
        .iter()
        .filter(|provider| provider_is_enabled(provider))
        .map(|provider| {
            let payload = provider.payload.clone();
            let usage = payload.as_ref().and_then(|payload| payload.usage.clone());
            let credits = payload.as_ref().and_then(|payload| payload.credits.clone());
            let error = payload.as_ref().and_then(|payload| payload.error.clone());
            let has_account_switch = provider_settings_spec(provider.config.id).is_visible();
            let dashboard_url = provider_dashboard_url(provider.config.id).map(|url| url.to_string());
            let status_url = payload
                .as_ref()
                .and_then(|payload| payload.status.as_ref())
                .map(|status| status.url.clone())
                .or_else(|| provider_status_url(provider.config.id).map(|url| url.to_string()));

            TrayProviderSnapshot {
                id: provider.config.id,
                display_name: provider.metadata.display_name.to_string(),
                payload,
                usage,
                credits,
                error,
                has_account_switch,
                dashboard_url,
                status_url,
            }
        })
        .collect()
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub(crate) fn active_provider_dashboard(
    snapshots: &[TrayProviderSnapshot],
    active_provider: Option<ProviderId>,
) -> Option<String> {
    active_provider
        .and_then(|id| snapshots.iter().find(|snapshot| snapshot.id == id))
        .and_then(|snapshot| snapshot.dashboard_url.clone())
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub(crate) fn active_provider_status(
    snapshots: &[TrayProviderSnapshot],
    active_provider: Option<ProviderId>,
) -> Option<String> {
    active_provider
        .and_then(|id| snapshots.iter().find(|snapshot| snapshot.id == id))
        .and_then(|snapshot| snapshot.status_url.clone())
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn build_preview_lines(
    snapshots: &[TrayProviderSnapshot],
    active_provider: Option<ProviderId>,
    settings: &AppSettings,
    refreshing: bool,
    now: OffsetDateTime,
) -> Vec<MenuItem> {
    let Some(snapshot) = active_provider.and_then(|id| {
        snapshots.iter().find(|snapshot| snapshot.id == id)
    }) else {
        return Vec::new();
    };

    let mut lines = Vec::new();
    let header = MenuItem::new(snapshot.display_name.clone(), false, None);
    lines.push(header);

    if let Some(account_line) =
        build_account_line(snapshot.payload.as_ref(), snapshot.usage.as_ref())
    {
        lines.push(MenuItem::new(account_line, false, None));
    }

    let updated_line = format_updated_line(
        snapshot.usage.as_ref(),
        snapshot.error.as_ref(),
        refreshing,
        now,
    );
    lines.push(MenuItem::new(updated_line, false, None));

    if let Some(error) = snapshot.error.as_ref() {
        let error_text = format!("Refresh failed: {}", trim_menu_text(&error.message));
        lines.push(MenuItem::new(error_text, false, None));
        return lines;
    }

    let mut has_usage = false;
    if let Some(usage) = snapshot.usage.as_ref() {
        if let Some(window) = usage.primary.as_ref() {
            lines.push(MenuItem::new(
                format_rate_line("Session", window, settings, now),
                false,
                None,
            ));
            has_usage = true;
        }
        if let Some(window) = usage.secondary.as_ref() {
            lines.push(MenuItem::new(
                format_rate_line("Weekly", window, settings, now),
                false,
                None,
            ));
            has_usage = true;
        }
        if let Some(window) = usage.tertiary.as_ref() {
            let label = match snapshot.id {
                ProviderId::Claude => "Sonnet",
                _ => "Tertiary",
            };
            lines.push(MenuItem::new(
                format_rate_line(label, window, settings, now),
                false,
                None,
            ));
            has_usage = true;
        }
    }

    if !has_usage {
        lines.push(MenuItem::new("No usage data", false, None));
    }

    if let Some(credits) = snapshot.credits.as_ref() {
        lines.push(MenuItem::new(format_credits_line(credits), false, None));
    }

    if let Some(cost) = snapshot
        .usage
        .as_ref()
        .and_then(|usage| usage.provider_cost.as_ref())
    {
        lines.push(MenuItem::new(format_cost_line(cost), false, None));
    }

    lines
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn provider_is_enabled(provider: &ProviderState) -> bool {
    provider
        .config
        .enabled
        .unwrap_or(provider.metadata.default_enabled)
}
