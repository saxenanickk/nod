//! prdesk — a GPUI-native GitHub PR review companion for Zed.

mod config;
mod diff_pane;
mod pr_list;
mod pr_session;
mod util;
mod workspace;

use gpui::{
    App, Application, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, actions,
    prelude::*, px, size,
};
use gpui_component::{Root, Theme, ThemeMode, TitleBar};
use pr_session::{
    DismissOverlay, NextFile, NextThread, PrevFile, PrevThread, RefreshSession,
    SESSION_KEY_CONTEXT,
};
use workspace::Workspace;

actions!(prdesk, [Quit]);

fn main() {
    let config = config::load_or_init();

    Application::new()
        .with_assets(gpui_component_assets::Assets)
        .run(move |cx: &mut App| {
            gpui_component::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);

            cx.bind_keys([
                KeyBinding::new("cmd-q", Quit, None),
                // Session navigation, scoped so typing in a composer isn't
                // hijacked (the Input's own context takes precedence there).
                KeyBinding::new("]", NextFile, Some(SESSION_KEY_CONTEXT)),
                KeyBinding::new("[", PrevFile, Some(SESSION_KEY_CONTEXT)),
                KeyBinding::new("n", NextThread, Some(SESSION_KEY_CONTEXT)),
                KeyBinding::new("p", PrevThread, Some(SESSION_KEY_CONTEXT)),
                KeyBinding::new("r", RefreshSession, Some(SESSION_KEY_CONTEXT)),
                KeyBinding::new("escape", DismissOverlay, Some(SESSION_KEY_CONTEXT)),
            ]);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(980.), px(720.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("prdesk".into()),
                        ..TitleBar::title_bar_options()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|cx| Workspace::new(config, cx));
                    window.focus(&view.read(cx).focus_handle);
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .expect("failed to open window");

            cx.activate(true);
        });
}
