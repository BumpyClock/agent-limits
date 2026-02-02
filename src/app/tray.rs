use std::time::Duration;

use anyhow::{Context as _, Result};
use gpui::{App, Bounds, Global, Pixels, point, px, size};
use time::OffsetDateTime;

use super::state::AgentStatsApp;
use crate::app::{About, APP_TITLE, Refresh};
use crate::settings::AppSettings;

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
use super::tray_menu::{
    TrayMenuState, TrayProviderSnapshot, active_provider_dashboard, active_provider_status,
    build_snapshots, build_tray_menu,
};

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
use tray_icon::{
    dpi,
    MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::MenuEvent,
};

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
const TRAY_ID: &str = "agent-stats-tray";

#[derive(Default)]
pub(crate) struct TrayController {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    tray_icon: Option<TrayIcon>,
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    menu_state: Option<TrayMenuState>,
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    snapshots: Vec<TrayProviderSnapshot>,
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    active_provider: Option<agent_stats_core::provider_metadata::ProviderId>,
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    refreshing: bool,
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    settings: Option<AppSettings>,
}

impl Global for TrayController {}

impl TrayController {
    pub(crate) fn init(cx: &mut App) {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            let result = init_tray(cx);
            if let Err(error) = result {
                log::error!("Tray init failed: {error}");
            }
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    fn update_from_app(&mut self, app: &AgentStatsApp) {
        let settings = app.settings.clone();
        let snapshots = build_snapshots(&app.providers);
        let provider_ids: Vec<_> = snapshots.iter().map(|snap| snap.id).collect();
        let active_provider = self
            .active_provider
            .filter(|id| provider_ids.contains(id))
            .or_else(|| provider_ids.first().copied());

        self.snapshots = snapshots;
        self.active_provider = active_provider;
        self.refreshing = app.refreshing;
        self.settings = Some(settings.clone());

        if let Err(error) = self.rebuild_menu(&settings) {
            log::error!("Tray menu rebuild failed: {error}");
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    fn rebuild_menu(&mut self, settings: &AppSettings) -> Result<()> {
        let now = OffsetDateTime::now_utc();
        let (menu, menu_state) = build_tray_menu(
            &self.snapshots,
            self.active_provider,
            settings,
            self.refreshing,
            now,
        )?;

        if let Some(tray_icon) = self.tray_icon.as_ref() {
            tray_icon.set_menu(Some(Box::new(menu)));
        }

        self.menu_state = Some(menu_state);
        Ok(())
    }

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    fn handle_menu_event(&mut self, event_id: tray_icon::menu::MenuId) -> TrayMenuAction {
        let Some(menu_state) = self.menu_state.as_ref() else {
            return TrayMenuAction::None;
        };

        if let Some(tab) = menu_state
            .provider_tabs
            .iter()
            .find(|tab| *tab.item.id() == event_id)
        {
            self.active_provider = Some(tab.id);
            if let Some(settings) = self.settings.clone() {
                if let Err(error) = self.rebuild_menu(&settings) {
                    log::error!("Tray menu rebuild failed: {error}");
                }
            }
            return TrayMenuAction::None;
        }

        if event_id == menu_state.refresh {
            return TrayMenuAction::Refresh;
        }

        if menu_state
            .switch_account
            .as_ref()
            .is_some_and(|id| *id == event_id)
        {
            return TrayMenuAction::SwitchAccount;
        }

        if menu_state
            .dashboard
            .as_ref()
            .is_some_and(|id| *id == event_id)
        {
            let url = active_provider_dashboard(&self.snapshots, self.active_provider);
            return TrayMenuAction::OpenDashboard(url);
        }

        if menu_state
            .status_page
            .as_ref()
            .is_some_and(|id| *id == event_id)
        {
            let url = active_provider_status(&self.snapshots, self.active_provider);
            return TrayMenuAction::OpenStatus(url);
        }

        if menu_state
            .open_preview
            .as_ref()
            .is_some_and(|id| *id == event_id)
        {
            return TrayMenuAction::OpenPreview;
        }

        if event_id == menu_state.open_window {
            return TrayMenuAction::OpenWindow;
        }

        if event_id == menu_state.settings {
            return TrayMenuAction::OpenSettings;
        }

        if event_id == menu_state.about {
            return TrayMenuAction::About;
        }

        if event_id == menu_state.quit {
            return TrayMenuAction::Quit;
        }

        TrayMenuAction::None
    }
}

pub(crate) fn sync_tray_from_app(app: &AgentStatsApp, cx: &mut App) {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        if cx.try_global::<TrayController>().is_none() {
            return;
        }
        let controller = cx.global_mut::<TrayController>();
        controller.update_from_app(app);
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
enum TrayMenuAction {
    None,
    Refresh,
    SwitchAccount,
    OpenDashboard(Option<String>),
    OpenStatus(Option<String>),
    OpenPreview,
    OpenWindow,
    OpenSettings,
    About,
    Quit,
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn init_tray(cx: &mut App) -> Result<()> {
    let settings = AppSettings::load();
    let (menu, menu_state) = build_tray_menu(&[], None, &settings, false, OffsetDateTime::now_utc())?;
    let icon = load_tray_icon()?;
    let mut builder = TrayIconBuilder::new()
        .with_id(TRAY_ID)
        .with_icon(icon)
        .with_tooltip(APP_TITLE)
        .with_menu(Box::new(menu));

    #[cfg(target_os = "macos")]
    {
        builder = builder.with_icon_as_template(true).with_menu_on_left_click(false);
    }

    #[cfg(target_os = "windows")]
    {
        builder = builder.with_menu_on_left_click(false);
    }

    #[cfg(target_os = "linux")]
    {
        builder = builder.with_menu_on_left_click(true);
    }

    let tray_icon = builder.build().context("create tray icon")?;
    let mut controller = TrayController::default();
    controller.tray_icon = Some(tray_icon);
    controller.menu_state = Some(menu_state.clone());
    controller.settings = Some(settings);
    cx.set_global(controller);

    start_tray_event_loop(cx);
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn start_tray_event_loop(cx: &mut App) {
    cx.spawn(move |cx: &mut gpui::AsyncApp| {
        let cx = cx.clone();
        async move {
            loop {
                handle_menu_events(&cx);
                handle_tray_events(&cx);
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
            }
        }
    })
    .detach();
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn handle_menu_events(cx: &gpui::AsyncApp) {
    while let Ok(event) = MenuEvent::receiver().try_recv() {
        let event_id = event.id;
        cx.update(|cx| {
            if cx.try_global::<TrayController>().is_none() {
                return;
            }
            let action = {
                let controller = cx.global_mut::<TrayController>();
                controller.handle_menu_event(event_id.clone())
            };

            match action {
                TrayMenuAction::None => {}
                TrayMenuAction::Refresh => {
                    cx.dispatch_action(&Refresh);
                }
                TrayMenuAction::SwitchAccount | TrayMenuAction::OpenSettings => {
                    super::open_or_create_settings_window(cx);
                }
                TrayMenuAction::OpenDashboard(url) => {
                    if let Some(url) = url {
                        cx.open_url(&url);
                    }
                }
                TrayMenuAction::OpenStatus(url) => {
                    if let Some(url) = url {
                        cx.open_url(&url);
                    }
                }
                TrayMenuAction::OpenPreview => {
                    super::toggle_preview_window(cx);
                }
                TrayMenuAction::OpenWindow => {
                    super::open_or_create_main_window(cx);
                }
                TrayMenuAction::About => {
                    cx.dispatch_action(&About);
                }
                TrayMenuAction::Quit => {
                    cx.quit();
                }
            }
        });
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn handle_tray_events(cx: &gpui::AsyncApp) {
    #[cfg(not(target_os = "windows"))]
    let _ = cx;

    while let Ok(event) = TrayIconEvent::receiver().try_recv() {
        match event {
            TrayIconEvent::Click {
                button,
                button_state,
                rect,
                position,
                ..
            } => {
                if button == MouseButton::Left && button_state == MouseButtonState::Up {
                    #[cfg(any(target_os = "macos", target_os = "windows"))]
                    cx.update(move |cx| {
                        super::toggle_preview_window_at(tray_click_to_bounds(rect, position, cx), cx);
                    });
                }
            }
            TrayIconEvent::DoubleClick { .. } => {
                #[cfg(target_os = "windows")]
                cx.update(super::open_or_create_main_window);
            }
            _ => {}
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn load_tray_icon() -> Result<tray_icon::Icon> {
    let bytes = include_bytes!("../../assets/icon.png");
    let image = image::load_from_memory(bytes).context("decode tray icon")?;
    let rgba = image.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    tray_icon::Icon::from_rgba(rgba.into_raw(), width, height).context("build tray icon")
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn tray_click_to_bounds(
    rect: tray_icon::Rect,
    position: dpi::PhysicalPosition<f64>,
    cx: &App,
) -> Bounds<Pixels> {
    let candidates = tray_scale_candidates(rect);
    let displays = cx.displays();
    if displays.is_empty() {
        let scale = tray_scale_factor(rect);
        let click_point = tray_click_point(position, scale);
        let size = tray_bounds_from_scale(rect, scale).size;
        return tray_anchor_from_click(click_point, size);
    }

    let target_height = tray_target_icon_height();
    let mut best: Option<(f32, f32, Bounds<Pixels>, gpui::Point<Pixels>)> = None;
    for display in displays {
        let display_bounds = display.bounds();
        for scale in &candidates {
            let click_point = tray_click_point(position, *scale);
            let score = tray_scale_score(click_point, display_bounds, target_height, rect, *scale);
            if best
                .as_ref()
                .map_or(true, |(best_score, _, _, _)| score < *best_score)
            {
                best = Some((score, *scale, display_bounds, click_point));
            }
        }
    }

    if let Some((_score, scale, display_bounds, click_point)) = best {
        let size = tray_bounds_from_scale(rect, scale).size;
        if tray_log_enabled() {
            log::info!(
                "tray click: rect=({},{} {}x{}), pos=({:.1},{:.1}), scale={:.2}, click=({:.1},{:.1}), display=({},{} {}x{})",
                rect.position.x,
                rect.position.y,
                rect.size.width,
                rect.size.height,
                position.x,
                position.y,
                scale,
                f32::from(click_point.x),
                f32::from(click_point.y),
                f32::from(display_bounds.origin.x),
                f32::from(display_bounds.origin.y),
                f32::from(display_bounds.size.width),
                f32::from(display_bounds.size.height),
            );
        }
        return tray_anchor_from_click(click_point, size);
    }

    let fallback_scale = tray_scale_factor(rect);
    let click_point = tray_click_point(position, fallback_scale);
    let size = tray_bounds_from_scale(rect, fallback_scale).size;
    tray_anchor_from_click(click_point, size)
}

fn tray_bounds_from_scale(rect: tray_icon::Rect, scale: f32) -> Bounds<Pixels> {
    let origin = point(
        px(rect.position.x as f32 / scale),
        px(rect.position.y as f32 / scale),
    );
    let size = size(
        px(rect.size.width as f32 / scale),
        px(rect.size.height as f32 / scale),
    );
    Bounds::new(origin, size)
}

fn tray_scale_candidates(rect: tray_icon::Rect) -> Vec<f32> {
    let mut candidates = vec![1.0, tray_scale_factor(rect)];
    candidates.extend(tray_common_scales().iter().copied());

    let height = rect.size.height as f32;
    for base in tray_scale_bases() {
        let scale = (height / base).clamp(1.0, 3.0);
        candidates.push(scale);
    }

    candidates.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    candidates.dedup_by(|a, b| (*a - *b).abs() < 0.02);
    candidates
}

fn tray_common_scales() -> &'static [f32] {
    &[1.0, 1.25, 1.5, 2.0, 2.5, 3.0]
}

#[cfg(target_os = "macos")]
fn tray_scale_bases() -> &'static [f32] {
    &[22.0, 24.0]
}

#[cfg(target_os = "windows")]
fn tray_scale_bases() -> &'static [f32] {
    &[16.0, 20.0]
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn tray_scale_bases() -> &'static [f32] {
    &[22.0]
}

#[cfg(target_os = "macos")]
fn tray_target_icon_height() -> f32 {
    22.0
}

#[cfg(target_os = "windows")]
fn tray_target_icon_height() -> f32 {
    16.0
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn tray_target_icon_height() -> f32 {
    22.0
}

fn tray_scale_score(
    click_point: gpui::Point<Pixels>,
    display_bounds: Bounds<Pixels>,
    target_height: f32,
    rect: tray_icon::Rect,
    scale: f32,
) -> f32 {
    let out_of_bounds = distance_to_bounds(click_point, display_bounds);
    let height = rect.size.height as f32 / scale;
    let size_error = (height - target_height).abs();
    let display_penalty = if out_of_bounds > 0.0 { 1000.0 } else { 0.0 };

    #[cfg(target_os = "macos")]
    let edge_distance = {
        let right = display_bounds.origin.x + display_bounds.size.width;
        let top = display_bounds.origin.y;
        f32::from((right - click_point.x).abs())
            + f32::from((click_point.y - top).abs())
    };

    #[cfg(target_os = "windows")]
    let edge_distance = {
        let right = display_bounds.origin.x + display_bounds.size.width;
        let bottom = display_bounds.origin.y + display_bounds.size.height;
        f32::from((right - click_point.x).abs())
            + f32::from((bottom - click_point.y).abs())
    };

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let edge_distance = {
        let right = display_bounds.origin.x + display_bounds.size.width;
        let top = display_bounds.origin.y;
        f32::from((right - click_point.x).abs())
            + f32::from((click_point.y - top).abs())
    };

    if tray_log_enabled() {
        log::info!(
            "tray scale candidate: scale={:.2} height={:.1} size_err={:.1} edge_dist={:.1} out_bounds={:.1}",
            scale,
            height,
            size_error,
            edge_distance,
            out_of_bounds
        );
    }

    edge_distance + (size_error * 0.5) + out_of_bounds + display_penalty
}

fn distance_to_bounds(point: gpui::Point<Pixels>, bounds: Bounds<Pixels>) -> f32 {
    let left = bounds.origin.x;
    let right = bounds.origin.x + bounds.size.width;
    let top = bounds.origin.y;
    let bottom = bounds.origin.y + bounds.size.height;

    let dx = if point.x < left {
        f32::from(left - point.x)
    } else if point.x > right {
        f32::from(point.x - right)
    } else {
        0.0
    };

    let dy = if point.y < top {
        f32::from(top - point.y)
    } else if point.y > bottom {
        f32::from(point.y - bottom)
    } else {
        0.0
    };

    dx + dy
}

fn tray_click_point(position: dpi::PhysicalPosition<f64>, scale: f32) -> gpui::Point<Pixels> {
    point(
        px(position.x as f32 / scale),
        px(position.y as f32 / scale),
    )
}

fn tray_anchor_from_click(
    click_point: gpui::Point<Pixels>,
    size: gpui::Size<Pixels>,
) -> Bounds<Pixels> {
    let half_width = px(f32::from(size.width) / 2.0);
    let half_height = px(f32::from(size.height) / 2.0);
    let origin = point(click_point.x - half_width, click_point.y - half_height);
    Bounds::new(origin, size)
}

fn tray_log_enabled() -> bool {
    match std::env::var("AGENT_STATS_TRAY_LOG") {
        Ok(value) => value != "0",
        Err(_) => false,
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn tray_scale_factor(rect: tray_icon::Rect) -> f32 {
    #[cfg(target_os = "macos")]
    {
        let raw = rect.size.height as f32 / 22.0;
        return raw.clamp(1.0, 3.0);
    }

    #[cfg(target_os = "windows")]
    {
        let raw = rect.size.height as f32 / 16.0;
        let candidates = [1.0_f32, 1.25, 1.5, 2.0, 2.5, 3.0];
        let mut best = 1.0;
        let mut best_diff = f32::MAX;
        for candidate in candidates {
            let diff = (raw - candidate).abs();
            if diff < best_diff {
                best = candidate;
                best_diff = diff;
            }
        }
        return best;
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        1.0
    }
}
