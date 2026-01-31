use std::time::Duration;

use anyhow::{Context as _, Result};
use gpui::{App, Global};

#[cfg(any(target_os = "macos", target_os = "windows"))]
use tray_icon::{
    MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
};

use crate::app::{About, APP_TITLE, OpenSettings, Refresh};

#[cfg(any(target_os = "macos", target_os = "windows"))]
const TRAY_ID: &str = "agent-stats-tray";

#[derive(Default)]
pub(crate) struct TrayController {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    tray_icon: Option<TrayIcon>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    menu_ids: Option<TrayMenuIds>,
}

impl Global for TrayController {}

impl TrayController {
    pub(crate) fn init(cx: &mut App) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let result = init_tray(cx);
            if let Err(error) = result {
                log::error!("Tray init failed: {error}");
            }
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Clone)]
struct TrayMenuIds {
    open_window: MenuId,
    toggle_preview: Option<MenuId>,
    settings: MenuId,
    refresh: MenuId,
    about: MenuId,
    quit: MenuId,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn init_tray(cx: &mut App) -> Result<()> {
    let (menu, menu_ids) = build_tray_menu()?;
    let icon = load_tray_icon()?;
    let mut builder = TrayIconBuilder::new()
        .with_id(TRAY_ID)
        .with_icon(icon)
        .with_tooltip(APP_TITLE)
        .with_menu(Box::new(menu));

    #[cfg(target_os = "macos")]
    {
        builder = builder.with_icon_as_template(true).with_menu_on_left_click(true);
    }

    #[cfg(target_os = "windows")]
    {
        builder = builder.with_menu_on_left_click(false);
    }

    let tray_icon = builder.build().context("create tray icon")?;
    let mut controller = TrayController::default();
    controller.tray_icon = Some(tray_icon);
    controller.menu_ids = Some(menu_ids.clone());
    cx.set_global(controller);

    start_tray_event_loop(cx, menu_ids);
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn build_tray_menu() -> Result<(Menu, TrayMenuIds)> {
    let menu = Menu::new();
    let open_window = MenuItem::with_id("open-window", "Open Window", true, None);
    menu.append(&open_window).context("append open window")?;

    #[cfg(target_os = "windows")]
    let toggle_preview = {
        let item = MenuItem::with_id("toggle-preview", "Toggle Preview Window", true, None);
        menu.append(&item).context("append toggle preview")?;
        Some(item.id().clone())
    };

    #[cfg(not(target_os = "windows"))]
    let toggle_preview = None;

    menu.append(&PredefinedMenuItem::separator())
        .context("append separator")?;

    let settings = MenuItem::with_id("tray-settings", "Settings...", true, None);
    menu.append(&settings).context("append settings")?;

    let refresh = MenuItem::with_id("tray-refresh", "Refresh", true, None);
    menu.append(&refresh).context("append refresh")?;

    let about = MenuItem::with_id("tray-about", "About", true, None);
    menu.append(&about).context("append about")?;

    menu.append(&PredefinedMenuItem::separator())
        .context("append separator 2")?;

    let quit = MenuItem::with_id("tray-quit", "Quit", true, None);
    menu.append(&quit).context("append quit")?;

    let menu_ids = TrayMenuIds {
        open_window: open_window.id().clone(),
        toggle_preview,
        settings: settings.id().clone(),
        refresh: refresh.id().clone(),
        about: about.id().clone(),
        quit: quit.id().clone(),
    };

    Ok((menu, menu_ids))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn start_tray_event_loop(cx: &mut App, menu_ids: TrayMenuIds) {
    let menu_ids_for_task = menu_ids.clone();
    cx.spawn(move |cx: &mut gpui::AsyncApp| {
        let cx = cx.clone();
        async move {
            loop {
                handle_menu_events(&cx, &menu_ids_for_task);
                handle_tray_events(&cx);
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
            }
        }
    })
    .detach();
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn handle_menu_events(cx: &gpui::AsyncApp, menu_ids: &TrayMenuIds) {
    while let Ok(event) = MenuEvent::receiver().try_recv() {
        let event_id = event.id;
        if event_id == menu_ids.open_window {
            cx.update(super::open_or_create_main_window);
        } else if menu_ids
            .toggle_preview
            .as_ref()
            .is_some_and(|id| event_id == *id)
        {
            #[cfg(target_os = "windows")]
            cx.update(super::toggle_preview_window);
        } else if event_id == menu_ids.settings {
            cx.update(|cx| cx.dispatch_action(&OpenSettings));
        } else if event_id == menu_ids.refresh {
            cx.update(|cx| cx.dispatch_action(&Refresh));
        } else if event_id == menu_ids.about {
            cx.update(|cx| cx.dispatch_action(&About));
        } else if event_id == menu_ids.quit {
            cx.update(|cx| cx.quit());
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn handle_tray_events(cx: &gpui::AsyncApp) {
    #[cfg(not(target_os = "windows"))]
    let _ = cx;

    while let Ok(event) = TrayIconEvent::receiver().try_recv() {
        match event {
            TrayIconEvent::Click {
                button,
                button_state,
                ..
            } => {
                if button == MouseButton::Left && button_state == MouseButtonState::Up {
                    #[cfg(target_os = "windows")]
                    cx.update(super::toggle_preview_window);
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

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn load_tray_icon() -> Result<tray_icon::Icon> {
    let bytes = include_bytes!("../../assets/icon.png");
    let image = image::load_from_memory(bytes).context("decode tray icon")?;
    let rgba = image.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    tray_icon::Icon::from_rgba(rgba.into_raw(), width, height).context("build tray icon")
}
