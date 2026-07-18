//! Review a local repository's diff — branch-vs-base or uncommitted — with no
//! PR involved. Read-only: it renders through the shared [`crate::diff_pane`]
//! with no comment hooks, so you view and then jump to Zed to edit.

use crate::diff_pane::{self, PaneData, SidebarCallbacks, ViewMode};
use crate::file_tree::{self, SidebarRow};
use crate::pr_session::{
    DismissOverlay, NextFile, PrevFile, RefreshSession, SESSION_KEY_CONTEXT,
};
use diff_kit::{
    DiffLine, DiffRow, FileDiff, LayoutOptions, SplitRow, layout_split, layout_unified,
};
use github_client::GithubClient;
use github_types::NodeId;
use std::collections::HashSet;
use std::rc::Rc;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, ListAlignment, ListOffset, ListState,
    SharedString, UniformListScrollHandle, Window, div, list, prelude::*, px, rgb, uniform_list,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
};
use std::path::PathBuf;

pub enum LocalSessionEvent {
    Close,
    /// Open the PR associated with this branch (for commenting/review), using
    /// the gh account that can access the repo.
    OpenPr {
        repo: github_types::RepoId,
        number: u64,
        account: Option<String>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LocalMode {
    BranchVsBase,
    Uncommitted,
}

enum LocalLoad {
    Loading,
    Failed(String),
    Ready,
}

struct LoadedLocal {
    branch: String,
    files: Vec<FileDiff>,
    stats: Vec<(u64, u64)>,
    branch_pr: Option<(github_types::RepoId, u64, Option<String>)>,
    /// The branch PR's node id (for viewed-state mutations), when it has a PR.
    pr_node: Option<NodeId>,
    /// Paths already marked "viewed" on the branch's PR (empty when no PR).
    viewed: HashSet<String>,
}

pub struct LocalSessionView {
    clone: PathBuf,
    repo_label: String,
    branch: String,
    base_input: Entity<InputState>,
    mode: LocalMode,
    load: LocalLoad,
    files: Vec<FileDiff>,
    stats: Vec<(u64, u64)>,
    branch_pr: Option<(github_types::RepoId, u64, Option<String>)>,
    branch_pr_node: Option<NodeId>,
    /// Files marked "viewed" (backed by the branch's PR on GitHub when it has
    /// one; session-local otherwise).
    viewed: HashSet<String>,
    collapsed_folders: HashSet<String>,
    tree_mode: bool,
    wrap: bool,
    sidebar_rows: Vec<SidebarRow>,
    rows: Vec<DiffRow>,
    split_rows: Vec<SplitRow>,
    view_mode: ViewMode,
    selected_file: Option<usize>,
    header_rows: Vec<usize>,
    list_state: ListState,
    sidebar_scroll: UniformListScrollHandle,
    /// Open "create PR" form: (title input, body input).
    create_form: Option<(Entity<InputState>, Entity<InputState>)>,
    creating_pr: bool,
    create_error: Option<String>,
    generation: u64,
    _subscription: gpui::Subscription,
    pub focus_handle: FocusHandle,
}

impl LocalSessionView {
    pub fn new(clone: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let repo_label = repo_local::origin_repo(&clone)
            .map(|r| r.slug())
            .or_else(|| clone.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| clone.display().to_string());
        let base_input = cx.new(|cx| InputState::new(window, cx).placeholder("base branch"));
        // Refetch when the user presses Enter in the base field.
        let subscription = cx.subscribe(&base_input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.refresh(cx);
            }
        });
        let mut this = Self {
            clone,
            repo_label,
            branch: String::new(),
            branch_pr: None,
            branch_pr_node: None,
            viewed: HashSet::new(),
            collapsed_folders: HashSet::new(),
            tree_mode: true,
            wrap: true,
            sidebar_rows: Vec::new(),
            base_input,
            mode: LocalMode::BranchVsBase,
            load: LocalLoad::Loading,
            files: Vec::new(),
            stats: Vec::new(),
            rows: Vec::new(),
            split_rows: Vec::new(),
            view_mode: ViewMode::Unified,
            selected_file: None,
            header_rows: Vec::new(),
            list_state: ListState::new(0, ListAlignment::Top, px(512.)),
            sidebar_scroll: UniformListScrollHandle::new(),
            create_form: None,
            creating_pr: false,
            create_error: None,
            generation: 0,
            _subscription: subscription,
            focus_handle: cx.focus_handle(),
        };
        this.detect_base_then_load(window, cx);
        this
    }

    fn open_create_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let clone = self.clone.clone();
        let default_title = repo_local::last_commit_subject(&clone);
        let title = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Pull request title")
        });
        if !default_title.is_empty() {
            title.update(cx, |s, cx| s.set_value(default_title, window, cx));
        }
        let body = cx.new(|cx| {
            InputState::new(window, cx).multi_line(true).placeholder("Description (optional)…")
        });
        window.focus(&title.focus_handle(cx));
        self.create_form = Some((title, body));
        self.create_error = None;
        cx.notify();
    }

    fn submit_create_pr(&mut self, cx: &mut Context<Self>) {
        if self.creating_pr {
            return;
        }
        let Some((title_input, body_input)) = &self.create_form else { return };
        let title = title_input.read(cx).value().trim().to_string();
        if title.is_empty() {
            self.create_error = Some("A title is required.".into());
            cx.notify();
            return;
        }
        let body = body_input.read(cx).value().to_string();
        let base = self.base_input.read(cx).value().trim().to_string();
        self.creating_pr = true;
        self.create_error = None;
        cx.notify();
        let clone = self.clone.clone();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_spawn(async move {
                    repo_local::create_pr(&clone, &title, &body, &base)
                })
                .await;
            view.update(cx, |this, cx| {
                this.creating_pr = false;
                match result {
                    Ok((repo, number)) => {
                        this.create_form = None;
                        // create_pr used the active account, which has access.
                        cx.emit(LocalSessionEvent::OpenPr { repo, number, account: None });
                    }
                    Err(err) => this.create_error = Some(format!("{err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Auto-detects the base branch, fills the input, then loads the diff.
    fn detect_base_then_load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let clone = self.clone.clone();
        let base_input = self.base_input.clone();
        cx.spawn_in(window, async move |view, cx| {
            let base = cx
                .background_spawn(async move {
                    repo_local::default_base(&clone).unwrap_or_else(|_| "main".to_string())
                })
                .await;
            view.update_in(cx, |this, window, cx| {
                if this.base_input.read(cx).value().is_empty() {
                    base_input.update(cx, |s, cx| s.set_value(base, window, cx));
                }
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.load = LocalLoad::Loading;
        let clone = self.clone.clone();
        let mode = self.mode;
        let base = self.base_input.read(cx).value().trim().to_string();
        cx.spawn(async move |view, cx| {
            let loaded = cx
                .background_spawn(async move { fetch_local(&clone, mode, &base) })
                .await;
            view.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match loaded {
                    Ok(loaded) => {
                        this.branch = loaded.branch;
                        this.files = loaded.files;
                        this.stats = loaded.stats;
                        this.branch_pr = loaded.branch_pr;
                        this.branch_pr_node = loaded.pr_node;
                        this.viewed = loaded.viewed;
                        this.load = LocalLoad::Ready;
                        this.relayout();
                    }
                    Err(err) => this.load = LocalLoad::Failed(err),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn relayout(&mut self) {
        let opts = LayoutOptions { show_resolved: true, show_outdated: true };
        self.rows = layout_unified(&self.files, &[], opts);
        self.split_rows = layout_split(&self.files, &[], opts);
        self.recompute_header_rows();
        self.rebuild_sidebar();
        self.list_state.reset(self.active_len());
    }

    fn rebuild_sidebar(&mut self) {
        self.sidebar_rows = file_tree::build_rows(&self.files, &self.collapsed_folders, self.tree_mode);
    }

    /// Toggles a file's "viewed" checkbox. When this branch maps to a PR the
    /// change is persisted to GitHub (so it syncs with the web UI / VS Code);
    /// otherwise it is session-local. Reverts on failure.
    fn toggle_viewed(&mut self, file_ix: usize, cx: &mut Context<Self>) {
        let Some(file) = self.files.get(file_ix) else {
            return;
        };
        let path = file.path.clone();
        let now_viewed = !self.viewed.contains(&path);
        if now_viewed {
            self.viewed.insert(path.clone());
        } else {
            self.viewed.remove(&path);
        }
        cx.notify();
        let (Some(node), Some((_, _, account))) =
            (self.branch_pr_node.clone(), self.branch_pr.clone())
        else {
            return; // No PR backing this branch; keep the local toggle only.
        };
        let client = match account {
            Some(a) => GithubClient::gh_cli_as(a),
            None => GithubClient::gh_cli(),
        };
        let req_path = path.clone();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_spawn(async move { client.set_file_viewed(&node, &req_path, now_viewed) })
                .await;
            if result.is_err() {
                view.update(cx, |this, cx| {
                    if now_viewed {
                        this.viewed.remove(&path);
                    } else {
                        this.viewed.insert(path);
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn toggle_folder(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.collapsed_folders.remove(&path) {
            self.collapsed_folders.insert(path);
        }
        self.rebuild_sidebar();
        cx.notify();
    }

    fn toggle_tree_mode(&mut self, cx: &mut Context<Self>) {
        self.tree_mode = !self.tree_mode;
        self.rebuild_sidebar();
        cx.notify();
    }

    fn toggle_wrap(&mut self, cx: &mut Context<Self>) {
        self.wrap = !self.wrap;
        self.list_state.reset(self.active_len());
        cx.notify();
    }

    fn sidebar_callbacks(&self, entity: &Entity<Self>) -> SidebarCallbacks {
        let e1 = entity.clone();
        let on_select: diff_pane::SelectFileFn = Rc::new(move |ix, _w, cx| {
            e1.update(cx, |this, cx| this.select_file(ix, cx));
        });
        let e2 = entity.clone();
        let on_toggle_viewed: diff_pane::ToggleViewedFn = Rc::new(move |ix, _w, cx| {
            e2.update(cx, |this, cx| this.toggle_viewed(ix, cx));
        });
        let e3 = entity.clone();
        let on_toggle_folder: diff_pane::ToggleFolderFn = Rc::new(move |path, _w, cx| {
            e3.update(cx, |this, cx| this.toggle_folder(path, cx));
        });
        SidebarCallbacks { on_select, on_toggle_viewed, on_toggle_folder }
    }

    fn render_sidebar_item(
        &self,
        ix: usize,
        cb: &SidebarCallbacks,
        cx: &App,
    ) -> gpui::AnyElement {
        let Some(row) = self.sidebar_rows.get(ix) else {
            return div().into_any_element();
        };
        let pane = self.pane();
        match row {
            SidebarRow::Folder { depth, name, path, file_count, collapsed } => pane
                .render_folder_row(
                    ix,
                    *depth,
                    name,
                    path.clone(),
                    *file_count,
                    *collapsed,
                    &cb.on_toggle_folder,
                    cx,
                ),
            SidebarRow::File { depth, file_ix } => {
                let (adds, dels) = self.stats.get(*file_ix).copied().unwrap_or((0, 0));
                let viewed =
                    self.files.get(*file_ix).map(|f| self.viewed.contains(&f.path)).unwrap_or(false);
                pane.render_file_row(
                    *file_ix,
                    ix,
                    *depth,
                    adds,
                    dels,
                    self.selected_file == Some(*file_ix),
                    viewed,
                    None,
                    cb,
                    cx,
                )
            }
        }
    }

    fn render_sidebar_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let reviewed = self.files.iter().filter(|f| self.viewed.contains(&f.path)).count();
        let total = self.files.len();
        let tree_mode = self.tree_mode;
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(format!("{reviewed}/{total} reviewed"))),
            )
            .child(
                Button::new("local-sidebar-tree")
                    .label(if tree_mode { "Tree" } else { "Flat" })
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_tree_mode(cx))),
            )
    }

    fn active_len(&self) -> usize {
        match self.view_mode {
            ViewMode::Unified => self.rows.len(),
            ViewMode::Split => self.split_rows.len(),
        }
    }

    fn recompute_header_rows(&mut self) {
        self.header_rows = (0..self.files.len())
            .map(|file_ix| match self.view_mode {
                ViewMode::Unified => self
                    .rows
                    .iter()
                    .position(|r| matches!(r, DiffRow::FileHeader { file_ix: fx } if *fx == file_ix))
                    .unwrap_or(0),
                ViewMode::Split => self
                    .split_rows
                    .iter()
                    .position(|r| matches!(r, SplitRow::FileHeader { file_ix: fx } if *fx == file_ix))
                    .unwrap_or(0),
            })
            .collect();
    }

    fn set_mode(&mut self, mode: LocalMode, cx: &mut Context<Self>) {
        if self.mode != mode {
            self.mode = mode;
            self.refresh(cx);
        }
    }

    fn toggle_view_mode(&mut self, cx: &mut Context<Self>) {
        self.view_mode = match self.view_mode {
            ViewMode::Unified => ViewMode::Split,
            ViewMode::Split => ViewMode::Unified,
        };
        self.recompute_header_rows();
        self.list_state.reset(self.active_len());
        cx.notify();
    }

    fn select_file(&mut self, file_ix: usize, cx: &mut Context<Self>) {
        self.selected_file = Some(file_ix);
        if let Some(&row_ix) = self.header_rows.get(file_ix) {
            self.list_state.scroll_to(ListOffset { item_ix: row_ix, offset_in_item: px(0.) });
        }
        cx.notify();
    }

    fn step_file(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.files.is_empty() {
            return;
        }
        let current = self.selected_file.unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(self.files.len() as isize) as usize;
        self.select_file(next, cx);
    }

    /// The pane over local data: clone is on head (line numbers trustworthy),
    /// and no comment hook (read-only).
    fn pane(&self) -> PaneData<'_> {
        PaneData {
            files: &self.files,
            zed_target: Some((self.clone.clone(), true)),
            add_comment: None,
            wrap: self.wrap,
        }
    }

    fn render_diff_row(&self, ix: usize, cx: &App) -> gpui::AnyElement {
        let Some(row) = self.rows.get(ix) else {
            return div().into_any_element();
        };
        let pane = self.pane();
        match row {
            DiffRow::FileHeader { file_ix } => pane.render_file_header(*file_ix, ix, cx),
            DiffRow::HunkHeader { file_ix, hunk_ix } => pane.render_hunk_header(*file_ix, *hunk_ix, cx),
            DiffRow::Line { line, in_comment_range, file_ix } => {
                pane.render_line(line, *in_comment_range, ix, *file_ix, cx)
            }
            // Local rows never contain threads.
            _ => div().into_any_element(),
        }
    }

    fn render_split_row(&self, ix: usize, cx: &App) -> gpui::AnyElement {
        let Some(row) = self.split_rows.get(ix) else {
            return div().into_any_element();
        };
        let pane = self.pane();
        match row {
            SplitRow::FileHeader { file_ix } => pane.render_file_header(*file_ix, ix, cx),
            SplitRow::HunkHeader { file_ix, hunk_ix } => pane.render_hunk_header(*file_ix, *hunk_ix, cx),
            SplitRow::Pair { file_ix, left, right, in_comment_range } => {
                pane.render_pair(left.as_ref(), right.as_ref(), *in_comment_range, *file_ix, ix, cx)
            }
            _ => div().into_any_element(),
        }
    }

    fn render_create_form(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let (title, body) = self.create_form.as_ref().expect("form present");
        let creating = self.creating_pr;
        let mut panel = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary.opacity(0.5))
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(SharedString::from(format!(
                        "Create pull request — {} ← base",
                        self.branch
                    ))),
            )
            .child(
                div()
                    .h(px(28.))
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded_md()
                    .child(Input::new(title).small()),
            )
            .child(
                div()
                    .h(px(64.))
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded_md()
                    .child(Input::new(body)),
            );
        if let Some(err) = &self.create_error {
            panel = panel.child(
                div()
                    .text_sm()
                    .text_color(rgb(0xe06c75))
                    .child(SharedString::from(err.clone())),
            );
        }
        panel.child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(
                    Button::new("create-cancel")
                        .label("Cancel")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.create_form = None;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("create-submit")
                        .label(if creating { "Creating…" } else { "Create PR" })
                        .primary()
                        .small()
                        .disabled(creating)
                        .on_click(cx.listener(|this, _, _, cx| this.submit_create_pr(cx))),
                ),
        )
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let clone = self.clone.clone();
        let is_branch = self.mode == LocalMode::BranchVsBase;
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("local-back")
                    .label("‹ Back")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(LocalSessionEvent::Close))),
            )
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .whitespace_nowrap()
                    .child(SharedString::from(self.repo_label.clone())),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .whitespace_nowrap()
                    .when(!self.branch.is_empty(), |el| {
                        el.child(SharedString::from(if is_branch {
                            format!("{} ← base", self.branch)
                        } else {
                            format!("{} · uncommitted", self.branch)
                        }))
                    }),
            )
            // Mode toggle.
            .child(
                Button::new("mode-branch")
                    .label("Branch")
                    .xsmall()
                    .when(is_branch, |b| b.primary())
                    .when(!is_branch, |b| b.ghost())
                    .on_click(cx.listener(|this, _, _, cx| this.set_mode(LocalMode::BranchVsBase, cx))),
            )
            .child(
                Button::new("mode-uncommitted")
                    .label("Uncommitted")
                    .xsmall()
                    .when(!is_branch, |b| b.primary())
                    .when(is_branch, |b| b.ghost())
                    .on_click(cx.listener(|this, _, _, cx| this.set_mode(LocalMode::Uncommitted, cx))),
            )
            // Base branch field (only meaningful for branch-vs-base).
            .when(is_branch, |row| {
                row.child(
                    div()
                        .w(px(160.))
                        .h(px(24.))
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded_md()
                        .child(Input::new(&self.base_input).small()),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new("local-view")
                    .label(match self.view_mode {
                        ViewMode::Unified => "Split",
                        ViewMode::Split => "Unified",
                    })
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_view_mode(cx))),
            )
            .child(
                Button::new("local-wrap")
                    .label("Wrap")
                    .xsmall()
                    .when(self.wrap, |b| b.primary())
                    .when(!self.wrap, |b| b.ghost())
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_wrap(cx))),
            )
            .child(
                Button::new("local-refresh")
                    .label("Refresh")
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            // If this branch has an open PR, offer to review/comment on it;
            // otherwise offer to create one.
            .when_some(self.branch_pr.clone(), |row, (repo, number, account)| {
                row.child(
                    Button::new("local-review-pr")
                        .label(format!("Review PR #{number}"))
                        .primary()
                        .xsmall()
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(LocalSessionEvent::OpenPr {
                                repo: repo.clone(),
                                number,
                                account: account.clone(),
                            })
                        })),
                )
            })
            .when(self.branch_pr.is_none() && !self.files.is_empty(), |row| {
                row.child(
                    Button::new("local-create-pr")
                        .label("Create PR")
                        .primary()
                        .xsmall()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_create_form(window, cx)
                        })),
                )
            })
            .child(
                Button::new("local-open-zed")
                    .label("Open repo in Zed")
                    .ghost()
                    .xsmall()
                    .on_click(move |_, _, _| {
                        let _ = zed_bridge::open_in_zed(&clone, None);
                    }),
            )
    }
}

/// Blocking: runs on the background executor.
fn fetch_local(clone: &std::path::Path, mode: LocalMode, base: &str) -> Result<LoadedLocal, String> {
    let branch = repo_local::current_branch(clone).unwrap_or_default();
    let raw = match mode {
        LocalMode::BranchVsBase => {
            let base = if base.is_empty() {
                repo_local::default_base(clone).map_err(|e| e.to_string())?
            } else {
                base.to_string()
            };
            repo_local::branch_diff(clone, &base).map_err(|e| e.to_string())?
        }
        LocalMode::Uncommitted => {
            repo_local::uncommitted_diff(clone).map_err(|e| e.to_string())?
        }
    };
    let files = diff_kit::parse_git_diff(&raw).map_err(|e| e.to_string())?;
    let stats = files.iter().map(file_stats).collect();
    // Detect the branch's PR using whichever gh account can access this repo
    // (personal vs work), so work-repo PRs resolve even when the app's default
    // account can't see them.
    let branch_pr = repo_local::origin_repo(clone).and_then(|repo| {
        let account = github_client::account_for_repo(&repo, None);
        let token = account.as_deref().and_then(github_client::account_token);
        repo_local::branch_pr(clone, token.as_deref())
            .map(|(pr_repo, number)| (pr_repo, number, account))
    });
    // When the branch has a PR, pull the PR's node id + per-file viewed state so
    // the "reviewed" checkboxes match github.com and the VS Code extension.
    let mut pr_node = None;
    let mut viewed = HashSet::new();
    if let Some((pr_repo, number, account)) = &branch_pr {
        let client = match account {
            Some(a) => GithubClient::gh_cli_as(a.clone()),
            None => GithubClient::gh_cli(),
        };
        let pr_number = github_types::PrNumber(*number);
        if let Ok(files) = client.pr_viewed_files(pr_repo, pr_number) {
            viewed = files.into_iter().filter(|(_, v)| *v).map(|(p, _)| p).collect();
        }
        pr_node = client.pr_review_state(pr_repo, pr_number).ok().map(|s| s.node_id);
    }
    Ok(LoadedLocal { branch, files, stats, branch_pr, pr_node, viewed })
}

fn file_stats(f: &FileDiff) -> (u64, u64) {
    let mut adds = 0;
    let mut dels = 0;
    for hunk in &f.hunks {
        for line in &hunk.lines {
            match line {
                DiffLine::Added { .. } => adds += 1,
                DiffLine::Removed { .. } => dels += 1,
                DiffLine::Context { .. } => {}
            }
        }
    }
    (adds, dels)
}

impl gpui::EventEmitter<LocalSessionEvent> for LocalSessionView {}

impl Focusable for LocalSessionView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for LocalSessionView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body: gpui::AnyElement = match &self.load {
            LocalLoad::Loading => centered("Loading diff…", cx),
            LocalLoad::Failed(err) => centered(&format!("Failed to load diff: {err}"), cx),
            LocalLoad::Ready if self.files.is_empty() => {
                centered("No changes to review.", cx)
            }
            LocalLoad::Ready => {
                let entity = cx.entity();
                let sidebar_entity = entity.clone();
                let row_entity = entity.clone();
                let sidebar_len = self.sidebar_rows.len();
                let callbacks = self.sidebar_callbacks(&entity);
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .w(px(280.))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .border_r_1()
                            .border_color(cx.theme().border)
                            .child(self.render_sidebar_header(cx))
                            .child(
                                uniform_list("local-sidebar", sidebar_len, move |range, _w, cx| {
                                    let this = sidebar_entity.read(cx);
                                    range
                                        .map(|ix| this.render_sidebar_item(ix, &callbacks, cx))
                                        .collect()
                                })
                                .track_scroll(self.sidebar_scroll.clone())
                                .flex_1(),
                            ),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            list(self.list_state.clone(), move |ix, _w, cx| {
                                let this = row_entity.read(cx);
                                match this.view_mode {
                                    ViewMode::Unified => this.render_diff_row(ix, cx),
                                    ViewMode::Split => this.render_split_row(ix, cx),
                                }
                            })
                            .size_full(),
                        ),
                    )
                    .into_any_element()
            }
        };
        div()
            .track_focus(&self.focus_handle)
            .key_context(SESSION_KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &NextFile, _, cx| this.step_file(1, cx)))
            .on_action(cx.listener(|this, _: &PrevFile, _, cx| this.step_file(-1, cx)))
            .on_action(cx.listener(|this, _: &RefreshSession, _, cx| this.refresh(cx)))
            .on_action(cx.listener(|_this, _: &DismissOverlay, _, cx| {
                cx.emit(LocalSessionEvent::Close)
            }))
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_header(cx))
            .child(body)
            .when(self.create_form.is_some(), |root| {
                root.child(self.render_create_form(cx))
            })
    }
}

fn centered(text: &str, cx: &App) -> gpui::AnyElement {
    div()
        .flex()
        .flex_1()
        .items_center()
        .justify_center()
        .text_color(cx.theme().muted_foreground)
        .child(SharedString::from(text.to_string()))
        .into_any_element()
}
