mod forms;
mod state;
mod types;
mod ui;

use gpui::{
    App, AppContext, Application, KeyBinding, Menu, MenuItem, SystemMenuType, TitlebarOptions,
    WindowBounds, WindowDecorations, WindowOptions, actions, point, px, size,
};
use gpui_component::Root;

use self::state::AgentStatsApp;

pub fn run() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();

    let app = Application::new();

    app.on_reopen(|cx| {
        if cx.windows().is_empty() {
            create_main_window(cx);
        } else if let Some(window) = cx.windows().first() {
            let _ = cx.update_window(*window, |_root, window, _cx| {
                window.activate_window();
            });
        }
    });

    app.run(|cx: &mut App| {
        gpui_component::init(cx);
        cx.set_menus(app_menus());
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
        #[cfg(target_os = "macos")]
        {
            cx.on_action(|_: &Hide, cx| cx.hide());
            cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
            cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
        }
        create_main_window(cx);
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

fn create_main_window(cx: &mut App) {
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
        let app = cx.new(|cx| AgentStatsApp::new(window, cx));
        cx.new(|cx| Root::new(app, window, cx))
    });
    if result.is_ok() {
        cx.activate(true);
    }
}
