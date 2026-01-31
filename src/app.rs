mod forms;
mod state;
mod tray;
mod types;
mod ui;

use agent_stats_core::provider_metadata::ProviderId;
use gpui::{
    App, AppContext, Application, Context, KeyBinding, Menu, MenuItem, QuitMode, SystemMenuType,
    TitlebarOptions, Window, WindowBounds, WindowDecorations, WindowOptions, actions, point, px,
    size,
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
            let handled = update_active_app(cx, |app, _window, cx| {
                app.toggle_settings(cx);
            });
            if handled.is_none() {
                create_main_window(cx, None);
            }
            let handled = handled.or_else(|| {
                update_active_app(cx, |app, _window, cx| {
                    app.toggle_settings(cx);
                })
            });
            if handled.is_some() {
                cx.stop_propagation();
            }
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

actions!(agent_stats, [About, Quit, OpenSettings, Refresh, Minimize, Zoom, Hide, HideOthers, ShowAll]);

fn app_menus() -> Vec<Menu> {
    let mut app_items = vec![
        MenuItem::action(format!("About {APP_TITLE}"), About),
        MenuItem::separator(),
        MenuItem::action("Settings...", OpenSettings),
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
        app.show_settings = false;
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

#[cfg(target_os = "windows")]
fn toggle_preview_window(cx: &mut App) {
    if let Some(window_handle) = find_window_by_mode(cx, AppViewMode::Preview) {
        let _ = cx.update_window(window_handle, |_root, window, _cx| {
            if window.is_window_active() {
                window.minimize_window();
            } else {
                window.activate_window();
            }
        });
        return;
    }

    create_preview_window(cx);
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

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn create_preview_window(cx: &mut App) {
    let window_options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(360.0), px(420.0)), cx)),
        titlebar: Some(TitlebarOptions {
            title: Some(format!("{APP_TITLE} Preview").into()),
            appears_transparent: false,
            traffic_light_position: Some(point(px(12.0), px(12.0))),
        }),
        app_id: Some(APP_ID.to_string()),
        window_decorations: Some(WindowDecorations::Server),
        ..Default::default()
    };

    let _ = cx.open_window(window_options, |window, cx| {
        window.activate_window();
        let title = format!("{APP_TITLE} Preview");
        window.set_window_title(&title);
        let app = cx.new(|cx| AgentStatsApp::new(window, AppViewMode::Preview, None, cx));
        cx.new(|cx| Root::new(app, window, cx))
    });
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
