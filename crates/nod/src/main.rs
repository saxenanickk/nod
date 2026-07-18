//! Nod — a GPUI-native GitHub PR review companion for Zed.

mod chrome;
mod config;
mod diff_pane;
mod file_tree;
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

actions!(nod, [Quit]);

/// Apps launched from Finder/Launchpad inherit only a minimal PATH
/// (`/usr/bin:/bin:/usr/sbin:/sbin`), so Homebrew tools like `gh` (and the
/// `zed` CLI in `/usr/local/bin`) aren't found even though they work in a
/// terminal. Recover the login shell's PATH and ensure the common bin dirs are
/// present, before anything shells out. Must run before any threads spawn.
fn restore_full_path() {
    let mut path = std::env::var("PATH").unwrap_or_default();
    // Prefer the user's real login-shell PATH (covers Homebrew, asdf, etc.
    // wherever they installed it). `SHELL` is an absolute path, so this works
    // without a usable PATH itself.
    if let Ok(shell) = std::env::var("SHELL") {
        if let Ok(out) = std::process::Command::new(&shell)
            .args(["-lc", "printf %s \"$PATH\""])
            .output()
        {
            let shell_path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if out.status.success() && shell_path.contains('/') {
                path = shell_path;
            }
        }
    }
    // Belt-and-suspenders for the common macOS locations.
    for dir in ["/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin"] {
        if !path.split(':').any(|p| p == dir) {
            path = if path.is_empty() { dir.to_string() } else { format!("{dir}:{path}") };
        }
    }
    // SAFETY: called at the very start of `main`, before any threads spawn.
    unsafe { std::env::set_var("PATH", path) };
}

fn main() {
    restore_full_path();
    let config = config::load_or_init();
    // `nod <path>` opens that local repo's review directly.
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
                        title: Some("Nod".into()),
                        ..TitleBar::title_bar_options()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|cx| Workspace::new(config, cx));
                    if let Some(path) = open_local {
                        view.update(cx, |ws, cx| ws.open_local(path, window, cx));
                    }
                    // Debug: NOD_DEBUG_PR="owner/repo#123" opens that PR directly.
                    if let Ok(spec) = std::env::var("NOD_DEBUG_PR") {
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
