//! Lists local clones (auto-discovered plus configured) so the user can open
//! one for local review or jump to its PRs.

use crate::config::Config;
use github_types::RepoId;
use gpui::{
    App, Context, FocusHandle, Focusable, PathPromptOptions, SharedString, Window, div, prelude::*,
    px,
};
use gpui_component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub enum RepoPickerEvent {
    OpenLocal(PathBuf),
    OpenRepoPrs(RepoId),
}

#[derive(Clone)]
struct DiscoveredRepo {
    path: PathBuf,
    label: String,
    repo: Option<RepoId>,
}

enum PickerLoad {
    Loading,
    Ready(Vec<DiscoveredRepo>),
}

pub struct RepoPickerView {
    config: Config,
    load: PickerLoad,
    pub focus_handle: FocusHandle,
}

impl gpui::EventEmitter<RepoPickerEvent> for RepoPickerView {}

impl RepoPickerView {
    pub fn new(config: Config, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            config,
            load: PickerLoad::Loading,
            focus_handle: cx.focus_handle(),
        };
        this.discover(cx);
        this
    }

    fn discover(&mut self, cx: &mut Context<Self>) {
        self.load = PickerLoad::Loading;
        let roots = self.config.clone_roots.clone();
        let configured = self.config.clones.clone();
        cx.spawn(async move |view, cx| {
            let repos = cx
                .background_spawn(async move { discover_repos(&roots, &configured) })
                .await;
            view.update(cx, |this, cx| {
                this.load = PickerLoad::Ready(repos);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn pick_folder(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open repository".into()),
        });
        cx.spawn(async move |view, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                if let Some(path) = paths.into_iter().next() {
                    view.update(cx, |_, cx| cx.emit(RepoPickerEvent::OpenLocal(path))).ok();
                }
            }
        })
        .detach();
    }

    fn render_repo_row(&self, repo: &DiscoveredRepo, cx: &mut Context<Self>) -> impl IntoElement {
        let path_for_local = repo.path.clone();
        let path_str = repo.path.display().to_string();
        let repo_id = repo.repo.clone();
        div()
            .flex()
            .items_center()
            .gap_3()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .hover(|el| el.bg(cx.theme().secondary.opacity(0.4)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .truncate()
                            .child(SharedString::from(repo.label.clone())),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(SharedString::from(path_str)),
                    ),
            )
            .child(
                Button::new(SharedString::from(format!("local-{}", repo.path.display())))
                    .label("Review local changes")
                    .outline()
                    .xsmall()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(RepoPickerEvent::OpenLocal(path_for_local.clone()))
                    })),
            )
            .when_some(repo_id, |el, repo_id| {
                el.child(
                    Button::new(SharedString::from(format!("prs-{}", repo_id.slug())))
                        .label("View PRs")
                        .ghost()
                        .xsmall()
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(RepoPickerEvent::OpenRepoPrs(repo_id.clone()))
                        })),
                )
            })
    }

    fn render_body(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        match &self.load {
            PickerLoad::Loading => div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child("Scanning for local repositories…")
                .into_any_element(),
            PickerLoad::Ready(repos) if repos.is_empty() => div()
                .flex()
                .flex_col()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .p_8()
                .child(
                    div()
                        .text_color(cx.theme().muted_foreground)
                        .child("No local clones found under your configured roots."),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Use “Open folder…” or add paths to config.json → clone_roots."),
                )
                .into_any_element(),
            PickerLoad::Ready(repos) => div()
                .id("repo-rows")
                .flex_1()
                .overflow_y_scroll()
                .children(
                    repos
                        .iter()
                        .map(|r| self.render_repo_row(r, cx).into_any_element())
                        .collect::<Vec<_>>(),
                )
                .into_any_element(),
        }
    }
}

impl Focusable for RepoPickerView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RepoPickerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus_handle)
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .py_2()
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Repositories"))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("open-folder")
                                    .label("Open folder…")
                                    .ghost()
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| this.pick_folder(cx))),
                            )
                            .child(
                                Button::new("rescan")
                                    .label("Rescan")
                                    .ghost()
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| this.discover(cx))),
                            ),
                    ),
            )
            .child(div().h(px(1.)).bg(cx.theme().border))
            .child(self.render_body(cx))
    }
}

/// Blocking: filesystem scan + `git remote` per candidate. Runs on the
/// background executor.
fn discover_repos(
    roots: &[PathBuf],
    configured: &std::collections::HashMap<String, PathBuf>,
) -> Vec<DiscoveredRepo> {
    // Keyed by canonical path to dedupe configured + discovered.
    let mut found: BTreeMap<PathBuf, DiscoveredRepo> = BTreeMap::new();

    let mut add = |path: PathBuf| {
        let key = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if found.contains_key(&key) {
            return;
        }
        let repo = repo_local::origin_repo(&key);
        let label = repo
            .as_ref()
            .map(|r| r.slug())
            .or_else(|| key.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| key.display().to_string());
        found.insert(key.clone(), DiscoveredRepo { path: key, label, repo });
    };

    for path in configured.values() {
        if path.join(".git").exists() {
            add(path.clone());
        }
    }
    for root in roots {
        for path in scan_git_dirs(root, 3) {
            add(path);
        }
    }
    found.into_values().collect()
}

const SKIP_DIRS: &[&str] = &[
    "Library",
    "Applications",
    "Movies",
    "Music",
    "Pictures",
    "Downloads",
    "node_modules",
    "target",
];

/// Collects directories containing a `.git` up to `depth` levels below `dir`.
fn scan_git_dirs(dir: &std::path::Path, depth: usize) -> Vec<PathBuf> {
    if dir.join(".git").exists() {
        return vec![dir.to_path_buf()];
    }
    if depth == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
            continue;
        }
        out.extend(scan_git_dirs(&path, depth - 1));
    }
    out
}
