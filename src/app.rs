mod forms;
mod alerts;
mod history;
mod notifications;
mod state;
mod tray;
mod tray_format;
mod tray_menu;
mod types;
mod ui;

use agent_stats_core::provider_metadata::ProviderId;
use gpui::{
    App, AppContext, Application, Bounds, Context, DisplayId, KeyBinding, Menu, MenuItem,
    PlatformDisplay, QuitMode, SystemMenuType, TitlebarOptions, Window, WindowBackgroundAppearance,
    WindowBounds, WindowDecorations, WindowKind, WindowOptions, actions, point, px, size,
};
use gpui_component::Root;

use self::state::AgentStatsApp;
use self::tray::TrayController;
use self::types::AppViewMode;

pub fn run() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();

    let app = Application::new().with_quit_mode(QuitMode::Explicit);

    app.on_reopen(|cx| {
        if cx.windows().is_empty() {
            create_main_window(cx, None);
        } else if let Some(window) = cx.windows().first() {
            let _ = cx.update_window(*window, |_root, window, _cx| {
                window.activate_window();
            });
        }
    });

    app.run(|cx: &mut App| {
        gpui_component::init(cx);
        notifications::init_notifications();
        cx.set_menus(app_menus());
        TrayController::init(cx);
        cx.bind_keys([
            #[cfg(target_os = "macos")]
            KeyBinding::new("cmd-q", Quit, Some(KEY_CONTEXT)),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-q", Quit, Some(KEY_CONTEXT)),
            #[cfg(target_os = "macos")]
            KeyBinding::new("cmd-,", OpenSettings, Some(KEY_CONTEXT)),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-,", OpenSettings, Some(KEY_CONTEXT)),
            #[cfg(target_os = "macos")]
            KeyBinding::new("cmd-r", Refresh, Some(KEY_CONTEXT)),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-r", Refresh, Some(KEY_CONTEXT)),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &OpenSettings, cx| {
            open_or_create_settings_window(cx);
            cx.stop_propagation();
        });
        cx.on_action(|_: &OpenHistory, cx| {
            open_or_create_history_window(cx);
            cx.stop_propagation();
        });
        cx.on_action(|_: &Refresh, cx| {
            let handled = update_active_app(cx, |app, _window, cx| {
                app.refresh_now(cx);
            });
            if handled.is_none() {
                create_main_window(cx, None);
            }
            let handled = handled.or_else(|| {
                update_active_app(cx, |app, _window, cx| {
                    app.refresh_now(cx);
                })
            });
            if handled.is_some() {
                cx.stop_propagation();
            }
        });
        cx.on_action(|_: &About, cx| {
            let handled = update_active_app(cx, |app, window, cx| {
                app.open_about_dialog(window, cx);
            });
            if handled.is_none() {
                create_main_window(cx, None);
            }
            let handled = handled.or_else(|| {
                update_active_app(cx, |app, window, cx| {
                    app.open_about_dialog(window, cx);
                })
            });
            if handled.is_some() {
                cx.stop_propagation();
            }
        });
        #[cfg(target_os = "macos")]
        {
            cx.on_action(|_: &Hide, cx| cx.hide());
            cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
            cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
        }
        create_main_window(cx, None);
    });
}

pub(crate) const APP_TITLE: &str = "Agent Stats";
pub(crate) const APP_ID: &str = "com.agentstats.app";
pub(crate) const KEY_CONTEXT: &str = "AgentStats";

actions!(agent_stats, [About, Quit, OpenSettings, OpenHistory, Refresh, Minimize, Zoom, Hide, HideOthers, ShowAll]);

fn app_menus() -> Vec<Menu> {
    let mut app_items = vec![
        MenuItem::action(format!("About {APP_TITLE}"), About),
        MenuItem::separator(),
        MenuItem::action("Settings...", OpenSettings),
        MenuItem::action("History...", OpenHistory),
        MenuItem::action("Refresh", Refresh),
        MenuItem::separator(),
    ];
    #[cfg(target_os = "macos")]
    {
        app_items.push(MenuItem::os_submenu("Services", SystemMenuType::Services));
        app_items.push(MenuItem::separator());
        app_items.push(MenuItem::action(format!("Hide {APP_TITLE}"), Hide));
        app_items.push(MenuItem::action("Hide Others", HideOthers));
        app_items.push(MenuItem::action("Show All", ShowAll));
        app_items.push(MenuItem::separator());
    }
    app_items.push(MenuItem::action(format!("Quit {APP_TITLE}"), Quit));

    vec![
        Menu {
            name: APP_TITLE.into(),
            items: app_items,
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Zoom", Zoom),
            ],
        },
    ]
}

fn update_active_app<R>(
    cx: &mut App,
    update: impl FnOnce(&mut AgentStatsApp, &mut Window, &mut Context<AgentStatsApp>) -> R,
) -> Option<R> {
    let window_handle = cx.active_window().or_else(|| cx.windows().first().copied())?;
    cx.update_window(window_handle, |root_view, window, cx| {
        let root_entity = match root_view.downcast::<Root>() {
            Ok(entity) => entity,
            Err(_) => return None,
        };
        let app_view = root_entity.read(cx).view().clone();
        let app_entity = match app_view.downcast::<AgentStatsApp>() {
            Ok(entity) => entity,
            Err(_) => return None,
        };
        Some(app_entity.update(cx, |app, cx| update(app, window, cx)))
    })
    .ok()
    .flatten()
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn open_or_create_main_window(cx: &mut App) {
    let handled = update_active_app(cx, |_app, window, _cx| {
        window.activate_window();
    });
    if handled.is_none() {
        create_main_window(cx, None);
    }
}

pub(crate) fn open_main_window_for_provider(provider_id: ProviderId, cx: &mut App) {
    let handled = update_app_by_mode(cx, AppViewMode::Main, |app, window, cx| {
        app.pending_main_provider_focus = Some(provider_id);
        cx.notify();
        window.activate_window();
    });
    if handled.is_none() {
        create_main_window(cx, Some(provider_id));
    } else {
        cx.activate(true);
    }
}

pub(crate) fn open_or_create_settings_window(cx: &mut App) {
    if let Some(window_handle) = find_window_by_mode(cx, AppViewMode::Settings) {
        let _ = cx.update_window(window_handle, |_root, window, _cx| {
            window.activate_window();
        });
        cx.activate(true);
        return;
    }

    create_settings_window(cx);
}

pub(crate) fn open_or_create_history_window(cx: &mut App) {
    if let Some(window_handle) = find_window_by_mode(cx, AppViewMode::History) {
        let _ = cx.update_window(window_handle, |_root, window, _cx| {
            window.activate_window();
        });
        cx.activate(true);
        return;
    }

    create_history_window(cx);
}

pub(crate) fn sync_windows_after_settings_change(
    current_mode: AppViewMode,
    settings: crate::settings::AppSettings,
    config: agent_stats_core::config::AppConfig,
    cx: &mut App,
) {
    let modes = [
        AppViewMode::Main,
        AppViewMode::Preview,
        AppViewMode::Settings,
        AppViewMode::History,
    ];
    for mode in modes {
        if mode == current_mode {
            continue;
        }
        let settings = settings.clone();
        let config = config.clone();
        let _ = update_app_by_mode(cx, mode, |app, window, cx| {
            app.apply_state_update(settings, config, window, cx);
        });
    }
}

pub(crate) fn toggle_preview_window_at(anchor: Bounds<gpui::Pixels>, cx: &mut App) {
    if let Some(window_handle) = find_window_by_mode(cx, AppViewMode::Preview) {
        let _ = cx.update_window(window_handle, |_root, window, _cx| {
            window.remove_window();
        });
        return;
    }

    create_preview_window(cx, Some(anchor));
}

pub(crate) fn toggle_preview_window(cx: &mut App) {
    if let Some(window_handle) = find_window_by_mode(cx, AppViewMode::Preview) {
        let _ = cx.update_window(window_handle, |_root, window, _cx| {
            window.remove_window();
        });
        return;
    }

    create_preview_window(cx, None);
}

pub(crate) fn close_preview_window(cx: &mut App) {
    if let Some(window_handle) = find_window_by_mode(cx, AppViewMode::Preview) {
        let _ = cx.update_window(window_handle, |_root, window, _cx| {
            window.remove_window();
        });
    }
}


fn create_main_window(cx: &mut App, initial_provider: Option<ProviderId>) {
    let window_options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(840.0), px(720.0)), cx)),
        titlebar: Some(TitlebarOptions {
            title: Some(APP_TITLE.into()),
            appears_transparent: false,
            traffic_light_position: Some(point(px(16.0), px(16.0))),
        }),
        app_id: Some(APP_ID.to_string()),
        window_decorations: Some(WindowDecorations::Server),
        ..Default::default()
    };

    let result = cx.open_window(window_options, |window, cx| {
        window.activate_window();
        window.set_window_title(APP_TITLE);
        let app = cx.new(|cx| AgentStatsApp::new(window, AppViewMode::Main, initial_provider, cx));
        cx.new(|cx| Root::new(app, window, cx))
    });
    if result.is_ok() {
        cx.activate(true);
    }
}

fn create_settings_window(cx: &mut App) {
    let window_options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(720.0), px(580.0)), cx)),
        titlebar: Some(TitlebarOptions {
            title: Some("Settings".into()),
            appears_transparent: false,
            traffic_light_position: Some(point(px(16.0), px(16.0))),
        }),
        app_id: Some(APP_ID.to_string()),
        window_decorations: Some(WindowDecorations::Server),
        ..Default::default()
    };

    let result = cx.open_window(window_options, |window, cx| {
        window.activate_window();
        window.set_window_title("Settings");
        let app = cx.new(|cx| AgentStatsApp::new(window, AppViewMode::Settings, None, cx));
        cx.new(|cx| Root::new(app, window, cx))
    });

    if result.is_ok() {
        cx.activate(true);
    }
}

fn create_history_window(cx: &mut App) {
    let window_options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(860.0), px(640.0)), cx)),
        titlebar: Some(TitlebarOptions {
            title: Some("History".into()),
            appears_transparent: false,
            traffic_light_position: Some(point(px(16.0), px(16.0))),
        }),
        app_id: Some(APP_ID.to_string()),
        window_decorations: Some(WindowDecorations::Server),
        ..Default::default()
    };

    let result = cx.open_window(window_options, |window, cx| {
        window.activate_window();
        window.set_window_title("History");
        let app = cx.new(|cx| AgentStatsApp::new(window, AppViewMode::History, None, cx));
        cx.new(|cx| Root::new(app, window, cx))
    });

    if result.is_ok() {
        cx.activate(true);
    }
}

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
fn create_preview_window(cx: &mut App, anchor: Option<Bounds<gpui::Pixels>>) {
    let size = size(px(360.0), px(520.0));
    let (bounds, display_id) = anchor
        .map(|anchor| preview_bounds_from_anchor(anchor, size, cx))
        .unwrap_or_else(|| (Bounds::centered(None, size, cx), None));

    let window_options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus: true,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some(APP_ID.to_string()),
        window_min_size: None,
        window_decorations: Some(WindowDecorations::Client),
        tabbing_identifier: None,
        ..Default::default()
    };

    let _ = cx.open_window(window_options, |window, cx| {
        let app = cx.new(|cx| AgentStatsApp::new(window, AppViewMode::Preview, None, cx));
        cx.new(|cx| Root::new(app, window, cx))
    });
}

fn preview_bounds_from_anchor(
    anchor: Bounds<gpui::Pixels>,
    size: gpui::Size<gpui::Pixels>,
    cx: &App,
) -> (Bounds<gpui::Pixels>, Option<DisplayId>) {
    let anchor_center = anchor.center();
    let display = display_for_point(anchor_center, cx)
        .or_else(|| cx.primary_display());
    let display_bounds = display
        .as_ref()
        .map(|display| display.visible_bounds())
        .unwrap_or_else(|| Bounds::centered(None, size, cx));
    let display_center = display_bounds.center();

    let icon_left = anchor.origin.x;
    let icon_top = anchor.origin.y;
    let icon_right = anchor.origin.x + anchor.size.width;
    let icon_bottom = anchor.origin.y + anchor.size.height;

    let margin = px(4.0);
    let open_left = anchor_center.x > display_center.x;
    let open_up = anchor_center.y > display_center.y;

    let mut origin_x = if open_left {
        icon_right - size.width
    } else {
        icon_left
    };
    let mut origin_y = if open_up {
        icon_top - size.height - margin
    } else {
        icon_bottom + margin
    };

    let min_x = display_bounds.origin.x + margin;
    let max_x = display_bounds.origin.x + display_bounds.size.width - size.width - margin;
    let min_y = display_bounds.origin.y + margin;
    let max_y = display_bounds.origin.y + display_bounds.size.height - size.height - margin;

    origin_x = clamp_pixels(origin_x, min_x, max_x);
    origin_y = clamp_pixels(origin_y, min_y, max_y);

    (
        Bounds::new(point(origin_x, origin_y), size),
        display.map(|display| display.id()),
    )
}

fn display_for_point(
    point: gpui::Point<gpui::Pixels>,
    cx: &App,
) -> Option<std::rc::Rc<dyn PlatformDisplay>> {
    cx.displays()
        .into_iter()
        .find(|display| display.bounds().contains(&point))
}

fn clamp_pixels(value: gpui::Pixels, min: gpui::Pixels, max: gpui::Pixels) -> gpui::Pixels {
    if max < min {
        return min;
    }
    if value < min {
        return min;
    }
    if value > max {
        return max;
    }
    value
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn find_window_by_mode(cx: &mut App, mode: AppViewMode) -> Option<gpui::AnyWindowHandle> {
    let windows = cx.windows();
    windows.into_iter().find(|window_handle| {
        cx.update_window(*window_handle, |root_view, _window, cx| {
            let root_entity = match root_view.downcast::<Root>() {
                Ok(entity) => entity,
                Err(_) => return None,
            };
            let app_view = root_entity.read(cx).view().clone();
            let app_entity = match app_view.downcast::<AgentStatsApp>() {
                Ok(entity) => entity,
                Err(_) => return None,
            };
            Some(app_entity.read(cx).view_mode == mode)
        })
        .ok()
        .flatten()
        .unwrap_or(false)
    })
}

fn update_app_by_mode<R>(
    cx: &mut App,
    mode: AppViewMode,
    update: impl FnOnce(&mut AgentStatsApp, &mut Window, &mut Context<AgentStatsApp>) -> R,
) -> Option<R> {
    let window_handle = find_window_by_mode(cx, mode)?;
    cx.update_window(window_handle, |root_view, window, cx| {
        let root_entity = match root_view.downcast::<Root>() {
            Ok(entity) => entity,
            Err(_) => return None,
        };
        let app_view = root_entity.read(cx).view().clone();
        let app_entity = match app_view.downcast::<AgentStatsApp>() {
            Ok(entity) => entity,
            Err(_) => return None,
        };
        Some(app_entity.update(cx, |app, cx| update(app, window, cx)))
    })
    .ok()
    .flatten()
}
