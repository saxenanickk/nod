//! prdesk — a GPUI-native GitHub PR review companion for Zed.

mod config;
mod diff_pane;
mod local_session;
mod pr_list;
mod pr_session;
mod repo_picker;
mod theme;
mod util;
mod workspace;

use gpui::{
    App, Application, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, actions,
    prelude::*, px, size,
};
use gpui_component::{Root, TitleBar};
use pr_session::{
    DismissOverlay, NextFile, NextThread, PrevFile, PrevThread, RefreshSession,
    SESSION_KEY_CONTEXT,
};
use workspace::Workspace;

actions!(prdesk, [Quit]);

fn main() {
    let config = config::load_or_init();
    // `prdesk <path>` opens that local repo's review directly.
    let open_local = std::env::args().nth(1).and_then(|arg| {
        let path = std::path::PathBuf::from(&arg);
        path.is_dir().then(|| std::fs::canonicalize(&path).unwrap_or(path))
    });

    Application::new()
        .with_assets(gpui_component_assets::Assets)
        .run(move |cx: &mut App| {
            gpui_component::init(cx);
            theme::apply(cx);

            // Single-letter nav bindings must NOT fire while a text input is
            // focused, or you can't type those letters into a composer. The
            // `&& !Input` predicate disables them whenever a gpui-component
            // Input (key context "Input") is in the focus path.
            let nav_ctx = format!("{SESSION_KEY_CONTEXT} && !Input");
            cx.bind_keys([
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("]", NextFile, Some(&nav_ctx)),
                KeyBinding::new("[", PrevFile, Some(&nav_ctx)),
                KeyBinding::new("n", NextThread, Some(&nav_ctx)),
                KeyBinding::new("p", PrevThread, Some(&nav_ctx)),
                KeyBinding::new("r", RefreshSession, Some(&nav_ctx)),
                // Escape is safe to keep session-scoped: it cancels the
                // composer/drawer (a non-text key, so it never blocks typing).
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
                    if let Some(path) = open_local {
                        view.update(cx, |ws, cx| ws.open_local(path, window, cx));
                    }
                    // Debug: PRDESK_DEBUG_PR="owner/repo#123" opens that PR directly.
                    if let Ok(spec) = std::env::var("PRDESK_DEBUG_PR") {
                        if let Some((slug, num)) = spec.rsplit_once('#') {
                            if let (Some(repo), Ok(n)) =
                                (github_types::RepoId::parse(slug), num.parse::<u64>())
                            {
                                view.update(cx, |ws, cx| {
                                    ws.open_pr_by_number(repo, n, None, cx)
                                });
                            }
                        }
                    }
                    window.focus(&view.read(cx).focus_handle);
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .expect("failed to open window");

            cx.activate(true);
        });
}
