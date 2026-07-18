//! One open PR: changed-files sidebar + scrollable unified diff with review
//! comment threads rendered inline at their anchor lines.

use crate::config::{self, Config};
use crate::diff_pane::{self, CODE_INDENT, MONO, PaneData, ViewMode};
use crate::util::relative_time;
use diff_kit::{
    DiffRow, FileDiff, LayoutOptions, SplitRow, layout_split, layout_unified,
};
use github_client::GithubClient;
use github_types::{
    CommentAnchor, ConversationKind, DiffSide, MergeMethod, MergeableState, NodeId, PrConversation,
    PrReviewState, PrSummary, ReviewThread, ReviewVerdict,
};
use std::path::PathBuf;
use std::rc::Rc;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, ListAlignment, ListOffset, ListState,
    SharedString, UniformListScrollHandle, Window, div, list, prelude::*, px, rgb, uniform_list,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
    tag::Tag,
};
use std::collections::{HashMap, HashSet};

pub enum SessionEvent {
    Close,
}

gpui::actions!(
    pr_session,
    [NextFile, PrevFile, NextThread, PrevThread, RefreshSession, DismissOverlay]
);

/// The key context the session's bindings are scoped to (see main.rs).
pub const SESSION_KEY_CONTEXT: &str = "PrSession";

enum SessionLoad {
    Loading,
    Failed(String),
    Ready,
}

#[derive(Clone)]
enum CheckoutState {
    Resolving,
    NoClone,
    Ready {
        path: PathBuf,
        branch: String,
        dirty: bool,
        /// Local HEAD equals the PR's head sha — line-precise Zed jumps are
        /// safe.
        on_pr: bool,
    },
    CheckingOut,
    Failed(String),
}

/// Everything derived off-thread from the files + threads responses.
struct LoadedDiff {
    files: Vec<FileDiff>,
    /// (additions, deletions) parallel to `files`.
    stats: Vec<(u64, u64)>,
    threads: Vec<ReviewThread>,
    review_state: Option<PrReviewState>,
    conversation: Option<PrConversation>,
}

/// The finish-review drawer: a summary box + verdict choice.
struct ReviewDrawer {
    summary: Entity<InputState>,
    submitting: bool,
    error: Option<String>,
}

/// What an open composer will post.
#[derive(Clone)]
enum ComposerTarget {
    /// Reply to an existing thread.
    Reply { thread_id: NodeId, path: String },
    /// A brand-new line comment.
    NewComment(CommentAnchor),
}

#[derive(Clone, PartialEq)]
enum ComposerState {
    Editing,
    Sending,
    Failed(String),
}

struct Composer {
    target: ComposerTarget,
    input: Entity<InputState>,
    state: ComposerState,
}

pub struct PrSessionView {
    client: GithubClient,
    pr: PrSummary,
    load: SessionLoad,
    files: Vec<FileDiff>,
    stats: Vec<(u64, u64)>,
    threads: Vec<ReviewThread>,
    /// thread node id → index into `threads`.
    thread_ix: HashMap<String, usize>,
    rows: Vec<DiffRow>,
    split_rows: Vec<SplitRow>,
    view_mode: ViewMode,
    /// file_ix → index of its `FileHeader` row (for the active view mode).
    header_rows: Vec<usize>,
    show_resolved: bool,
    /// Resolved threads the user expanded back open.
    expanded: HashSet<String>,
    selected_file: Option<usize>,
    list_state: ListState,
    sidebar_scroll: UniformListScrollHandle,
    checkout: CheckoutState,
    clone_hint: Option<PathBuf>,
    clone_roots: Vec<PathBuf>,
    /// Thread ids with an in-flight resolve/unresolve toggle.
    resolving: HashSet<String>,
    composer: Option<Composer>,
    /// Live review/merge state (pending review, mergeability).
    review_state: Option<PrReviewState>,
    review_drawer: Option<ReviewDrawer>,
    merging: bool,
    /// PR conversation (description + comments + review summaries).
    conversation: Option<PrConversation>,
    /// The right-side conversation panel is open.
    show_conversation: bool,
    /// Composer for posting a conversation comment (created lazily).
    conv_input: Option<Entity<InputState>>,
    posting_comment: bool,
    conv_error: Option<String>,
    /// Cursor into thread rows for n/p navigation.
    thread_nav: usize,
    generation: u64,
    pub focus_handle: FocusHandle,
}

impl PrSessionView {
    pub fn new(
        client: GithubClient,
        pr: PrSummary,
        config: &Config,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            client,
            clone_hint: config.clones.get(&pr.repo.slug()).cloned(),
            clone_roots: config.clone_roots.clone(),
            pr,
            load: SessionLoad::Loading,
            files: Vec::new(),
            stats: Vec::new(),
            threads: Vec::new(),
            thread_ix: HashMap::new(),
            rows: Vec::new(),
            split_rows: Vec::new(),
            view_mode: ViewMode::Unified,
            header_rows: Vec::new(),
            show_resolved: true,
            expanded: HashSet::new(),
            selected_file: None,
            list_state: ListState::new(0, ListAlignment::Top, px(512.)),
            sidebar_scroll: UniformListScrollHandle::new(),
            checkout: CheckoutState::Resolving,
            resolving: HashSet::new(),
            composer: None,
            review_state: None,
            review_drawer: None,
            merging: false,
            conversation: None,
            show_conversation: false,
            conv_input: None,
            posting_comment: false,
            conv_error: None,
            thread_nav: 0,
            generation: 0,
            focus_handle: cx.focus_handle(),
        };
        this.refresh(cx);
        this.resolve_clone(cx);
        this
    }

    /// Finds the local clone (config hint, else filesystem scan) and reads
    /// its git state.
    fn resolve_clone(&mut self, cx: &mut Context<Self>) {
        let repo = self.pr.repo.clone();
        let head_sha = self.pr.head_sha.clone();
        let hint = self.clone_hint.clone();
        let roots = self.clone_roots.clone();
        cx.spawn(async move |view, cx| {
            let state = cx
                .background_spawn(async move {
                    let path = hint
                        .filter(|p| p.join(".git").exists())
                        .or_else(|| repo_local::find_clone(&roots, &repo, 3));
                    let Some(path) = path else {
                        return CheckoutState::NoClone;
                    };
                    config::record_clone(&repo.slug(), &path);
                    match (
                        repo_local::current_branch(&path),
                        repo_local::working_tree_dirty(&path),
                        repo_local::head_sha(&path),
                    ) {
                        (Ok(branch), Ok(dirty), Ok(sha)) => CheckoutState::Ready {
                            path,
                            branch,
                            dirty,
                            on_pr: !head_sha.is_empty() && sha == head_sha,
                        },
                        (b, d, s) => CheckoutState::Failed(
                            [b.err(), d.err(), s.err()]
                                .into_iter()
                                .flatten()
                                .map(|e| e.to_string())
                                .collect::<Vec<_>>()
                                .join("; "),
                        ),
                    }
                })
                .await;
            view.update(cx, |this, cx| {
                this.checkout = state;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn do_checkout(&mut self, cx: &mut Context<Self>) {
        let CheckoutState::Ready { path, dirty, .. } = self.checkout.clone() else {
            return;
        };
        self.checkout = CheckoutState::CheckingOut;
        cx.notify();
        let repo = self.pr.repo.clone();
        let number = self.pr.number.0;
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_spawn(async move {
                    if dirty {
                        repo_local::stash_all(&path, &format!("prdesk: before PR #{number}"))?;
                    }
                    repo_local::checkout_pr(&path, &repo, number)
                })
                .await;
            view.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.resolve_clone(cx),
                    Err(err) => this.checkout = CheckoutState::Failed(format!("{err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The clone path, when line-precise "open in Zed" is safe.
    fn zed_target(&self) -> Option<(PathBuf, bool)> {
        match &self.checkout {
            CheckoutState::Ready { path, on_pr, .. } => Some((path.clone(), *on_pr)),
            _ => None,
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let client = self.client.clone();
        let pr = self.pr.clone();
        cx.spawn(async move |view, cx| {
            let loaded = cx
                .background_spawn(async move { fetch_diff(&client, &pr) })
                .await;
            view.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match loaded {
                    Ok(loaded) => {
                        this.files = loaded.files;
                        this.stats = loaded.stats;
                        this.thread_ix = loaded
                            .threads
                            .iter()
                            .enumerate()
                            .map(|(ix, t)| (t.id.0.clone(), ix))
                            .collect();
                        this.threads = loaded.threads;
                        this.review_state = loaded.review_state;
                        this.conversation = loaded.conversation;
                        this.load = SessionLoad::Ready;
                        this.relayout();
                    }
                    Err(err) => this.load = SessionLoad::Failed(err),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Recomputes both row lists from stored files/threads and current
    /// options, then refreshes the list length for the active view.
    fn relayout(&mut self) {
        let opts = LayoutOptions {
            show_resolved: self.show_resolved,
            show_outdated: true,
        };
        self.rows = layout_unified(&self.files, &self.threads, opts);
        self.split_rows = layout_split(&self.files, &self.threads, opts);
        self.recompute_header_rows();
        self.list_state.reset(self.active_len());
    }

    fn active_len(&self) -> usize {
        match self.view_mode {
            ViewMode::Unified => self.rows.len(),
            ViewMode::Split => self.split_rows.len(),
        }
    }

    fn recompute_header_rows(&mut self) {
        self.header_rows = self
            .files
            .iter()
            .enumerate()
            .map(|(file_ix, _)| match self.view_mode {
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

    fn toggle_resolved(&mut self, cx: &mut Context<Self>) {
        self.show_resolved = !self.show_resolved;
        self.relayout();
        cx.notify();
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
            self.list_state.scroll_to(ListOffset {
                item_ix: row_ix,
                offset_in_item: px(0.),
            });
        }
        cx.notify();
    }

    fn thread(&self, id: &NodeId) -> Option<&ReviewThread> {
        self.thread_ix.get(&id.0).and_then(|&ix| self.threads.get(ix))
    }

    // ---- keyboard navigation ----

    fn scroll_to_row(&mut self, row_ix: usize) {
        self.list_state.scroll_to(ListOffset {
            item_ix: row_ix,
            offset_in_item: px(0.),
        });
    }

    fn step_file(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.files.is_empty() {
            return;
        }
        let current = self.selected_file.unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(self.files.len() as isize) as usize;
        self.select_file(next, cx);
    }

    fn step_thread(&mut self, delta: isize, cx: &mut Context<Self>) {
        let thread_rows: Vec<usize> = match self.view_mode {
            ViewMode::Unified => self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, r)| {
                    matches!(r, DiffRow::Thread { .. } | DiffRow::OutdatedThread { .. })
                })
                .map(|(ix, _)| ix)
                .collect(),
            ViewMode::Split => self
                .split_rows
                .iter()
                .enumerate()
                .filter(|(_, r)| {
                    matches!(r, SplitRow::Thread { .. } | SplitRow::OutdatedThread { .. })
                })
                .map(|(ix, _)| ix)
                .collect(),
        };
        if thread_rows.is_empty() {
            return;
        }
        let cursor = (self.thread_nav as isize + delta).rem_euclid(thread_rows.len() as isize)
            as usize;
        self.thread_nav = cursor;
        self.scroll_to_row(thread_rows[cursor]);
        cx.notify();
    }

    /// Closes whichever bottom overlay is open (composer or review drawer);
    /// returns whether anything was dismissed.
    fn dismiss_overlay(&mut self, cx: &mut Context<Self>) -> bool {
        if self.review_drawer.take().is_some() || self.composer.take().is_some() {
            cx.notify();
            true
        } else {
            false
        }
    }

    // ---- writes: resolve/unresolve and the comment composer ----

    /// Optimistically flips a thread's resolved flag, then reconciles with the
    /// server; reverts on failure.
    fn toggle_thread_resolved(&mut self, thread_id: NodeId, cx: &mut Context<Self>) {
        let Some(&ix) = self.thread_ix.get(&thread_id.0) else {
            return;
        };
        if self.resolving.contains(&thread_id.0) {
            return;
        }
        let want_resolved = !self.threads[ix].is_resolved;
        // Optimistic flip.
        self.threads[ix].is_resolved = want_resolved;
        self.resolving.insert(thread_id.0.clone());
        self.relayout();
        cx.notify();

        let client = self.client.clone();
        let id = thread_id.clone();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_spawn(async move {
                    if want_resolved {
                        client.resolve_thread(&id)
                    } else {
                        client.unresolve_thread(&id)
                    }
                })
                .await;
            view.update(cx, |this, cx| {
                this.resolving.remove(&thread_id.0);
                if let Err(err) = result {
                    // Revert the optimistic flip.
                    if let Some(&ix) = this.thread_ix.get(&thread_id.0) {
                        this.threads[ix].is_resolved = !want_resolved;
                    }
                    this.relayout();
                    this.flash_error(format!("Couldn't update thread: {err}"), cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_reply(&mut self, thread_id: NodeId, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder("Write a reply…")
        });
        window.focus(&input.focus_handle(cx));
        self.composer = Some(Composer {
            target: ComposerTarget::Reply { thread_id, path },
            input,
            state: ComposerState::Editing,
        });
        cx.notify();
    }

    fn open_new_comment(&mut self, anchor: CommentAnchor, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder("Leave a comment on this line…")
        });
        window.focus(&input.focus_handle(cx));
        self.composer = Some(Composer {
            target: ComposerTarget::NewComment(anchor),
            input,
            state: ComposerState::Editing,
        });
        cx.notify();
    }

    fn cancel_composer(&mut self, cx: &mut Context<Self>) {
        self.composer = None;
        cx.notify();
    }

    /// Posts the composer's text. `pending` batches a new comment into a
    /// pending review instead of posting it immediately (ignored for replies,
    /// which are always immediate).
    fn submit_composer(&mut self, pending: bool, cx: &mut Context<Self>) {
        let Some(composer) = &self.composer else { return };
        if composer.state == ComposerState::Sending {
            return;
        }
        let body = composer.input.read(cx).value().trim().to_string();
        if body.is_empty() {
            return;
        }
        let target = composer.target.clone();
        if let Some(composer) = &mut self.composer {
            composer.state = ComposerState::Sending;
        }
        cx.notify();

        let client = self.client.clone();
        let pr_id = self.pr.node_id.clone();
        let head_sha = self.pr.head_sha.clone();
        // For pending mode, reuse an existing pending review or create one.
        let existing_review = self
            .review_state
            .as_ref()
            .and_then(|s| s.pending_review.as_ref().map(|r| r.id.clone()));
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_spawn(async move {
                    match &target {
                        ComposerTarget::Reply { thread_id, .. } => {
                            client.reply_to_thread(thread_id, &body)
                        }
                        ComposerTarget::NewComment(anchor) if !pending => client.add_line_comment(
                            &pr_id,
                            (!head_sha.is_empty()).then_some(head_sha.as_str()),
                            anchor,
                            &body,
                        ),
                        ComposerTarget::NewComment(anchor) => {
                            let review_id = match existing_review {
                                Some(id) => id,
                                None => client.create_pending_review(&pr_id)?,
                            };
                            client.add_comment_to_review(&review_id, anchor, &body)
                        }
                    }
                })
                .await;
            view.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.composer = None;
                        // Pull authoritative state so the new comment/reply
                        // shows with its real id, author, and timestamp.
                        this.refresh(cx);
                    }
                    Err(err) => {
                        if let Some(composer) = &mut this.composer {
                            composer.state = ComposerState::Failed(format!("{err}"));
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- finish-review drawer + merge ----

    fn open_review_drawer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let seed = self
            .review_state
            .as_ref()
            .and_then(|s| s.pending_review.as_ref())
            .map(|r| r.body.clone())
            .unwrap_or_default();
        let summary = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder("Review summary (optional)…")
        });
        if !seed.is_empty() {
            summary.update(cx, |s, cx| s.set_value(seed, window, cx));
        }
        window.focus(&summary.focus_handle(cx));
        self.review_drawer = Some(ReviewDrawer { summary, submitting: false, error: None });
        cx.notify();
    }

    fn submit_review(&mut self, verdict: ReviewVerdict, cx: &mut Context<Self>) {
        let Some(drawer) = &self.review_drawer else { return };
        if drawer.submitting {
            return;
        }
        let Some(review_id) = self
            .review_state
            .as_ref()
            .and_then(|s| s.pending_review.as_ref().map(|r| r.id.clone()))
        else {
            if let Some(drawer) = &mut self.review_drawer {
                drawer.error = Some("No pending review to submit.".into());
            }
            cx.notify();
            return;
        };
        let body = drawer.summary.read(cx).value().to_string();
        if let Some(drawer) = &mut self.review_drawer {
            drawer.submitting = true;
            drawer.error = None;
        }
        cx.notify();

        let client = self.client.clone();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_spawn(async move { client.submit_review(&review_id, verdict, &body) })
                .await;
            view.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.review_drawer = None;
                        this.refresh(cx);
                    }
                    Err(err) => {
                        if let Some(drawer) = &mut this.review_drawer {
                            drawer.submitting = false;
                            drawer.error = Some(format!("{err}"));
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn discard_pending_review(&mut self, cx: &mut Context<Self>) {
        let Some(review_id) = self
            .review_state
            .as_ref()
            .and_then(|s| s.pending_review.as_ref().map(|r| r.id.clone()))
        else {
            return;
        };
        let client = self.client.clone();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_spawn(async move { client.delete_pending_review(&review_id) })
                .await;
            view.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.review_drawer = None;
                        this.refresh(cx);
                    }
                    Err(err) => this.flash_error(format!("Couldn't discard review: {err}"), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn merge(&mut self, method: MergeMethod, cx: &mut Context<Self>) {
        if self.merging {
            return;
        }
        self.merging = true;
        cx.notify();
        let client = self.client.clone();
        let repo = self.pr.repo.clone();
        let number = self.pr.number;
        let sha = self.pr.head_sha.clone();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_spawn(async move { client.merge_pr(&repo, number, method, &sha) })
                .await;
            view.update(cx, |this, cx| {
                this.merging = false;
                match result {
                    Ok(()) => this.refresh(cx),
                    Err(err) => this.flash_error(format!("Merge failed: {err}"), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn flash_error(&mut self, message: String, cx: &mut Context<Self>) {
        // For now surface transient errors via stderr; a toast layer is a
        // polish item. Kept as a single channel so callers stay uniform.
        eprintln!("prdesk: {message}");
        cx.notify();
    }

    // ---- conversation panel ----

    fn toggle_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_conversation = !self.show_conversation;
        if self.show_conversation && self.conv_input.is_none() {
            self.conv_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .multi_line(true)
                    .placeholder("Comment on this pull request…")
            }));
        }
        cx.notify();
    }

    fn post_conversation_comment(&mut self, cx: &mut Context<Self>) {
        if self.posting_comment {
            return;
        }
        let Some(input) = &self.conv_input else { return };
        let body = input.read(cx).value().trim().to_string();
        if body.is_empty() {
            return;
        }
        self.posting_comment = true;
        self.conv_error = None;
        cx.notify();
        let client = self.client.clone();
        let repo = self.pr.repo.clone();
        let number = self.pr.number;
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_spawn(async move { client.add_issue_comment(&repo, number, &body) })
                .await;
            view.update(cx, |this, cx| {
                this.posting_comment = false;
                match result {
                    Ok(()) => {
                        // Clear the box and pull the updated timeline.
                        this.conv_input = None;
                        this.show_conversation = true;
                        this.refresh(cx);
                    }
                    Err(err) => this.conv_error = Some(format!("{err}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Scrolls the diff to a thread's inline row.
    fn jump_to_thread(&mut self, thread_id: &NodeId, cx: &mut Context<Self>) {
        let target = match self.view_mode {
            ViewMode::Unified => self.rows.iter().position(|r| {
                matches!(r, DiffRow::Thread { thread_id: t } | DiffRow::OutdatedThread { thread_id: t } if t == thread_id)
            }),
            ViewMode::Split => self.split_rows.iter().position(|r| {
                matches!(r, SplitRow::Thread { thread_id: t } | SplitRow::OutdatedThread { thread_id: t } if t == thread_id)
            }),
        };
        if let Some(row_ix) = target {
            self.list_state.scroll_to(ListOffset { item_ix: row_ix, offset_in_item: px(0.) });
            cx.notify();
        }
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let composer = self.composer.as_ref().expect("composer present");
        let (title, send_label) = match &composer.target {
            ComposerTarget::Reply { path, .. } => {
                (format!("Reply to thread in {path}"), "Reply")
            }
            ComposerTarget::NewComment(anchor) => (
                format!(
                    "Comment on {}:{} ({})",
                    anchor.path,
                    anchor.line,
                    match anchor.side {
                        DiffSide::Left => "old",
                        DiffSide::Right => "new",
                    }
                ),
                "Comment",
            ),
        };
        let sending = composer.state == ComposerState::Sending;
        let mut panel = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary.opacity(0.4))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(SharedString::from(title)),
                    ),
            )
            .child(
                div()
                    .h(px(72.))
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded_md()
                    .child(Input::new(&composer.input).h_full()),
            );

        if let ComposerState::Failed(err) = &composer.state {
            panel = panel.child(
                div()
                    .text_sm()
                    .text_color(rgb(0xe06c75))
                    .child(SharedString::from(format!(
                        "Failed to post: {err} — your text is preserved; try again."
                    ))),
            );
        }

        let is_new_comment = matches!(composer.target, ComposerTarget::NewComment(_));
        let has_pending = self
            .review_state
            .as_ref()
            .is_some_and(|s| s.pending_review.is_some());
        let mut actions = div()
            .flex()
            .items_center()
            .gap_2()
            .justify_end()
            .child(
                Button::new("composer-cancel")
                    .label("Cancel")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_composer(cx))),
            );
        // New comments can be batched into a pending review; replies post now.
        if is_new_comment {
            actions = actions.child(
                Button::new("composer-add-review")
                    .label(if has_pending { "Add to review" } else { "Start a review" })
                    .ghost()
                    .small()
                    .disabled(sending)
                    .on_click(cx.listener(|this, _, _, cx| this.submit_composer(true, cx))),
            );
        }
        panel.child(
            actions.child(
                Button::new("composer-send")
                    .label(if sending { "Sending…" } else { send_label })
                    .primary()
                    .small()
                    .disabled(sending)
                    .on_click(cx.listener(|this, _, _, cx| this.submit_composer(false, cx))),
            ),
        )
    }

    fn render_review_drawer(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let drawer = self.review_drawer.as_ref().expect("drawer present");
        let submitting = drawer.submitting;
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
                    .child("Finish your review"),
            )
            .child(
                div()
                    .h(px(64.))
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded_md()
                    .child(Input::new(&drawer.summary).h_full()),
            );
        if let Some(err) = &drawer.error {
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
                .items_center()
                .gap_2()
                .child(
                    Button::new("review-discard")
                        .label("Discard review")
                        .ghost()
                        .small()
                        .disabled(submitting)
                        .on_click(cx.listener(|this, _, _, cx| this.discard_pending_review(cx))),
                )
                .child(div().flex_1())
                .child(
                    Button::new("review-cancel")
                        .label("Cancel")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.review_drawer = None;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("review-comment")
                        .label("Comment")
                        .ghost()
                        .small()
                        .disabled(submitting)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.submit_review(ReviewVerdict::Comment, cx)
                        })),
                )
                .child(
                    Button::new("review-request-changes")
                        .label("Request changes")
                        .ghost()
                        .small()
                        .disabled(submitting)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.submit_review(ReviewVerdict::RequestChanges, cx)
                        })),
                )
                .child(
                    Button::new("review-approve")
                        .label(if submitting { "Submitting…" } else { "Approve" })
                        .primary()
                        .small()
                        .disabled(submitting)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.submit_review(ReviewVerdict::Approve, cx)
                        })),
                ),
        )
    }

    /// The right-side conversation panel: the PR timeline (description, issue
    /// comments, review summaries), a composer to post, and a jump-list of the
    /// line-comment threads.
    fn render_conversation_panel(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut timeline = div().flex().flex_col().gap_2().p_3();
        timeline = timeline.child(
            div().font_weight(gpui::FontWeight::BOLD).child("Conversation"),
        );
        match &self.conversation {
            None => {
                timeline = timeline.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Loading conversation…"),
                );
            }
            Some(conv) => {
                for item in &conv.items {
                    let (label, color) = match item.kind {
                        ConversationKind::Description => ("described", rgb(0x828997)),
                        ConversationKind::Comment => ("commented", rgb(0x74ade8)),
                        ConversationKind::Review(ReviewVerdict::Approve) => {
                            ("approved", rgb(0x98c379))
                        }
                        ConversationKind::Review(ReviewVerdict::RequestChanges) => {
                            ("requested changes", rgb(0xe06c75))
                        }
                        ConversationKind::Review(ReviewVerdict::Comment) => {
                            ("reviewed", rgb(0x74ade8))
                        }
                    };
                    let meta = div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(diff_pane::avatar(&item.author.login))
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_sm()
                                .child(SharedString::from(item.author.login.clone())),
                        )
                        .child(small_tag(label, color.into()))
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(SharedString::from(relative_time(item.created_at))),
                        );
                    let body = if item.body_markdown.trim().is_empty() {
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .italic()
                            .child("(no description)")
                            .into_any_element()
                    } else {
                        render_comment_body(&item.body_markdown, cx)
                    };
                    timeline = timeline.child(
                        div()
                            .border_1()
                            .border_color(cx.theme().border)
                            .rounded_md()
                            .bg(cx.theme().secondary.opacity(0.5))
                            .p_2()
                            .child(meta)
                            .child(body),
                    );
                }
            }
        }

        // Composer to post a new conversation comment.
        if let Some(input) = &self.conv_input {
            let posting = self.posting_comment;
            let mut composer = div().flex().flex_col().gap_2().child(
                div()
                    .h(px(64.))
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded_md()
                    .child(Input::new(input)),
            );
            if let Some(err) = &self.conv_error {
                composer = composer.child(
                    div()
                        .text_sm()
                        .text_color(rgb(0xe06c75))
                        .child(SharedString::from(err.clone())),
                );
            }
            timeline = timeline.child(composer.child(
                div().flex().justify_end().child(
                    Button::new("post-comment")
                        .label(if posting { "Posting…" } else { "Comment" })
                        .primary()
                        .small()
                        .disabled(posting)
                        .on_click(cx.listener(|this, _, _, cx| this.post_conversation_comment(cx))),
                ),
            ));
        }

        // Jump-list of line-comment threads.
        let live_threads: Vec<&ReviewThread> =
            self.threads.iter().filter(|t| !t.is_resolved).collect();
        if !live_threads.is_empty() {
            timeline = timeline.child(
                div()
                    .mt_2()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(SharedString::from(format!(
                        "Review comments ({})",
                        live_threads.len()
                    ))),
            );
            for thread in live_threads {
                let thread_id = thread.id.clone();
                let entity = cx.entity();
                let first = thread
                    .comments
                    .first()
                    .map(|c| c.body_markdown.lines().next().unwrap_or("").to_string())
                    .unwrap_or_default();
                let loc = match thread.line {
                    Some(l) => format!("{}:{}", thread.path, l),
                    None => format!("{} (outdated)", thread.path),
                };
                timeline = timeline.child(
                    div()
                        .id(SharedString::from(format!("jump-{}", thread.id.0)))
                        .flex()
                        .flex_col()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|s| s.bg(cx.theme().accent))
                        .on_click(move |_, _, cx| {
                            let thread_id = thread_id.clone();
                            entity.update(cx, |this, cx| this.jump_to_thread(&thread_id, cx));
                        })
                        .child(
                            div()
                                .text_sm()
                                .font_family(MONO)
                                .text_color(cx.theme().muted_foreground)
                                .truncate()
                                .child(SharedString::from(loc)),
                        )
                        .child(div().text_sm().truncate().child(SharedString::from(first))),
                );
            }
        }

        div()
            .w(px(360.))
            .flex_shrink_0()
            .border_l_1()
            .border_color(cx.theme().border)
            .id("conversation-scroll")
            .overflow_y_scroll()
            .child(timeline)
            .into_any_element()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let url = self.pr.url.clone();
        let thread_count = self.threads.len();
        let unresolved = self.threads.iter().filter(|t| !t.is_resolved).count();
        let mut header = div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("back")
                    .label("‹ Back")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SessionEvent::Close))),
            )
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .truncate()
                    .flex_1()
                    .child(SharedString::from(self.pr.title.clone())),
            )
            .when(thread_count > 0, |el| {
                el.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .whitespace_nowrap()
                        .child(SharedString::from(format!(
                            "{unresolved}/{thread_count} threads open"
                        ))),
                )
            })
            .child({
                let conv_count = self.conversation.as_ref().map(|c| c.items.len()).unwrap_or(0);
                Button::new("toggle-conversation")
                    .label(if conv_count > 0 {
                        format!("Conversation ({conv_count})")
                    } else {
                        "Conversation".to_string()
                    })
                    .xsmall()
                    .when(self.show_conversation, |b| b.primary())
                    .when(!self.show_conversation, |b| b.ghost())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_conversation(window, cx)
                    }))
            })
            .child(
                Button::new("toggle-view")
                    .label(match self.view_mode {
                        ViewMode::Unified => "Split",
                        ViewMode::Split => "Unified",
                    })
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_view_mode(cx))),
            )
            .child(
                Button::new("toggle-resolved")
                    .label(if self.show_resolved {
                        "Hide resolved"
                    } else {
                        "Show resolved"
                    })
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_resolved(cx))),
            )
            .child(
                Button::new("refresh-session")
                    .label("Refresh")
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            );

        // Finish-review button appears once a pending review has drafts.
        let pending_count = self
            .review_state
            .as_ref()
            .and_then(|s| s.pending_review.as_ref())
            .map(|_| {
                self.threads
                    .iter()
                    .flat_map(|t| &t.comments)
                    .filter(|c| c.is_pending)
                    .count()
            });
        if let Some(count) = pending_count {
            header = header.child(
                Button::new("finish-review")
                    .label(format!("Finish review ({count})"))
                    .primary()
                    .xsmall()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_review_drawer(window, cx)
                    })),
            );
        }

        // Merge appears when GitHub reports the PR mergeable.
        let mergeable = self
            .review_state
            .as_ref()
            .is_some_and(|s| !s.merged && s.mergeable == MergeableState::Mergeable);
        if mergeable {
            header = header.child(
                Button::new("merge-squash")
                    .label(if self.merging { "Merging…" } else { "Squash & merge" })
                    .primary()
                    .xsmall()
                    .disabled(self.merging)
                    .on_click(cx.listener(|this, _, _, cx| this.merge(MergeMethod::Squash, cx))),
            );
        }

        header.child(
            Button::new("open-browser")
                .label("Open on GitHub")
                .ghost()
                .xsmall()
                .on_click(move |_, _, cx| cx.open_url(&url)),
        )
    }

    fn render_checkout_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut bar = div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .text_sm()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary.opacity(0.4));
        match &self.checkout {
            CheckoutState::Resolving => {
                bar = bar.child(
                    div()
                        .text_color(cx.theme().muted_foreground)
                        .child("Locating local clone…"),
                );
            }
            CheckoutState::NoClone => {
                bar = bar.child(div().text_color(cx.theme().muted_foreground).child(
                    SharedString::from(format!(
                        "No local clone of {} found — add it to config.json → clones",
                        self.pr.repo.slug()
                    )),
                ));
            }
            CheckoutState::CheckingOut => {
                bar = bar.child(
                    div()
                        .text_color(rgb(0xe5c07b))
                        .child("Checking out PR branch…"),
                );
            }
            CheckoutState::Failed(err) => {
                bar = bar
                    .child(
                        div()
                            .text_color(rgb(0xe06c75))
                            .truncate()
                            .flex_1()
                            .child(SharedString::from(format!("Checkout failed: {err}"))),
                    )
                    .child(
                        Button::new("retry-clone")
                            .label("Retry")
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(|this, _, _, cx| this.resolve_clone(cx))),
                    );
            }
            CheckoutState::Ready { path, branch, dirty, on_pr } => {
                bar = bar.child(
                    div()
                        .text_color(cx.theme().muted_foreground)
                        .font_family(MONO)
                        .truncate()
                        .child(SharedString::from(format!(
                            "{} · {branch}",
                            path.display()
                        ))),
                );
                if *on_pr {
                    bar = bar
                        .child(
                            div()
                                .text_color(rgb(0x98c379))
                                .whitespace_nowrap()
                                .child("✓ on PR head — ⌘-click any line to open in Zed"),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("open-repo-zed")
                                .label("Open repo in Zed")
                                .ghost()
                                .xsmall()
                                .on_click({
                                    let path = path.clone();
                                    move |_, _, _| {
                                        let _ = zed_bridge::open_in_zed(&path, None);
                                    }
                                }),
                        );
                } else {
                    if *dirty {
                        bar = bar.child(
                            div()
                                .text_color(rgb(0xe5c07b))
                                .whitespace_nowrap()
                                .child("working tree has changes"),
                        );
                    }
                    bar = bar.child(div().flex_1()).child(
                        Button::new("checkout")
                            .label(if *dirty { "Stash & checkout PR" } else { "Checkout PR" })
                            .primary()
                            .xsmall()
                            .on_click(cx.listener(|this, _, _, cx| this.do_checkout(cx))),
                    );
                }
            }
        }
        bar
    }

    /// A sidebar file row with the PR-specific unresolved-comment badge.
    fn render_sidebar_row(&self, file_ix: usize, entity: &Entity<Self>, cx: &App) -> gpui::AnyElement {
        let (adds, dels) = self.stats.get(file_ix).copied().unwrap_or((0, 0));
        let badge = self.files.get(file_ix).map(|file| {
            self.threads
                .iter()
                .filter(|t| t.path == file.path && !t.is_resolved)
                .count()
        });
        self.pane(entity).render_sidebar_row(
            file_ix,
            adds,
            dels,
            self.selected_file == Some(file_ix),
            badge,
            cx,
        )
    }

    /// Builds the shared diff pane view over this session's data, wired to
    /// open the comment composer via the `+` affordance.
    fn pane<'a>(&'a self, entity: &Entity<Self>) -> PaneData<'a> {
        let entity = entity.clone();
        let add_comment: crate::diff_pane::AddCommentFn = Rc::new(move |anchor, window, cx| {
            entity.update(cx, |this, cx| this.open_new_comment(anchor, window, cx));
        });
        PaneData {
            files: &self.files,
            zed_target: self.zed_target(),
            add_comment: Some(add_comment),
        }
    }

    fn render_thread_row(
        &self,
        thread_id: &NodeId,
        ix: usize,
        outdated: bool,
        entity: &Entity<Self>,
        cx: &App,
    ) -> gpui::AnyElement {
        let Some(thread) = self.thread(thread_id) else {
            return div().into_any_element();
        };
        render_thread_card(thread, ix, outdated, &self.expanded, entity, cx)
    }

    fn render_diff_row(
        &self,
        ix: usize,
        entity: &Entity<Self>,
        cx: &App,
    ) -> gpui::AnyElement {
        let Some(row) = self.rows.get(ix) else {
            return div().into_any_element();
        };
        let pane = self.pane(entity);
        match row {
            DiffRow::FileHeader { file_ix } => pane.render_file_header(*file_ix, ix, cx),
            DiffRow::HunkHeader { file_ix, hunk_ix } => {
                pane.render_hunk_header(*file_ix, *hunk_ix, cx)
            }
            DiffRow::Line { line, in_comment_range, file_ix } => {
                pane.render_line(line, *in_comment_range, ix, *file_ix, cx)
            }
            DiffRow::Thread { thread_id } => self.render_thread_row(thread_id, ix, false, entity, cx),
            DiffRow::OutdatedThreadsHeader { count, .. } => diff_pane::outdated_header_el(*count),
            DiffRow::OutdatedThread { thread_id } => {
                self.render_thread_row(thread_id, ix, true, entity, cx)
            }
        }
    }

    fn render_split_row(
        &self,
        ix: usize,
        entity: &Entity<Self>,
        cx: &App,
    ) -> gpui::AnyElement {
        let Some(row) = self.split_rows.get(ix) else {
            return div().into_any_element();
        };
        let pane = self.pane(entity);
        match row {
            SplitRow::FileHeader { file_ix } => pane.render_file_header(*file_ix, ix, cx),
            SplitRow::HunkHeader { file_ix, hunk_ix } => {
                pane.render_hunk_header(*file_ix, *hunk_ix, cx)
            }
            SplitRow::Pair { file_ix, left, right, in_comment_range } => {
                pane.render_pair(left.as_ref(), right.as_ref(), *in_comment_range, *file_ix, ix, cx)
            }
            SplitRow::Thread { thread_id } => self.render_thread_row(thread_id, ix, false, entity, cx),
            SplitRow::OutdatedThreadsHeader { count, .. } => diff_pane::outdated_header_el(*count),
            SplitRow::OutdatedThread { thread_id } => {
                self.render_thread_row(thread_id, ix, true, entity, cx)
            }
        }
    }
}

fn render_thread_card(
    thread: &ReviewThread,
    row_ix: usize,
    outdated: bool,
    expanded: &HashSet<String>,
    entity: &Entity<PrSessionView>,
    cx: &App,
) -> gpui::AnyElement {
    let collapsed = thread.is_resolved && !expanded.contains(&thread.id.0);

    let mut card = div()
        .my_1()
        .border_1()
        .border_color(cx.theme().border)
        .rounded_md()
        .bg(cx.theme().secondary.opacity(0.6))
        .overflow_hidden();

    if collapsed {
        let first = thread.comments.first();
        let summary = format!(
            "✓ Resolved — {}: {} · {} comment{}",
            first.map(|c| c.author.login.as_str()).unwrap_or("?"),
            first
                .map(|c| c.body_markdown.lines().next().unwrap_or("").to_string())
                .unwrap_or_default(),
            thread.comments.len(),
            if thread.comments.len() == 1 { "" } else { "s" },
        );
        let thread_id = thread.id.0.clone();
        let entity = entity.clone();
        card = card.child(
            div()
                .id(("resolved-toggle", row_ix))
                .px_2()
                .py_1()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .truncate()
                .cursor_pointer()
                .hover(|el| el.bg(cx.theme().accent))
                .on_click(move |_, _, cx| {
                    let thread_id = thread_id.clone();
                    entity.update(cx, |this, cx| {
                        this.expanded.insert(thread_id);
                        cx.notify();
                    });
                })
                .child(SharedString::from(summary)),
        );
    } else {
        let mut badges = div().flex().items_center().gap_1();
        if thread.is_resolved {
            badges = badges.child(small_tag("Resolved", rgb(0x98c379).into()));
        }
        if outdated {
            badges = badges.child(small_tag("Outdated", rgb(0xe5c07b).into()));
        }
        let mut header = div()
            .flex()
            .items_center()
            .justify_between()
            .px_2()
            .py_1()
            .child(badges);
        if thread.is_resolved {
            let thread_id = thread.id.0.clone();
            let entity = entity.clone();
            header = header.child(
                div()
                    .id(("resolved-collapse", row_ix))
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        let thread_id = thread_id.clone();
                        entity.update(cx, |this, cx| {
                            this.expanded.remove(&thread_id);
                            cx.notify();
                        });
                    })
                    .child("collapse"),
            );
        }
        card = card.child(header);

        // Outdated threads carry their original diff excerpt.
        if outdated {
            if let Some(hunk) = thread
                .comments
                .first()
                .map(|c| c.diff_hunk.as_str())
                .filter(|h| !h.is_empty())
            {
                let excerpt: Vec<String> = hunk
                    .lines()
                    .rev()
                    .take(4)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .map(str::to_string)
                    .collect();
                card = card.child(
                    div()
                        .px_2()
                        .py_1()
                        .font_family(MONO)
                        .text_sm()
                        .bg(cx.theme().background.opacity(0.6))
                        .text_color(cx.theme().muted_foreground)
                        .children(
                            excerpt
                                .into_iter()
                                .map(|l| div().whitespace_nowrap().overflow_hidden().child(SharedString::from(l))),
                        ),
                );
            }
        }

        for comment in &thread.comments {
            let mut meta = div()
                .flex()
                .items_center()
                .gap_2()
                .child(diff_pane::avatar(&comment.author.login))
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_sm()
                        .child(SharedString::from(comment.author.login.clone())),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(relative_time(comment.created_at))),
                );
            if comment.is_pending {
                meta = meta.child(small_tag("Pending", rgb(0xe5c07b).into()));
            }
            card = card.child(
                div()
                    .px_2()
                    .py_1()
                    .border_t_1()
                    .border_color(cx.theme().border.opacity(0.5))
                    .child(meta)
                    .child(render_comment_body(&comment.body_markdown, cx)),
            );
        }

        // Action row: Reply + Resolve/Unresolve.
        let thread_id = thread.id.clone();
        let path = thread.path.clone();
        let resolved = thread.is_resolved;
        let reply_entity = entity.clone();
        let resolve_entity = entity.clone();
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .border_t_1()
                .border_color(cx.theme().border.opacity(0.5))
                .child(
                    Button::new(("reply", row_ix))
                        .label("Reply")
                        .ghost()
                        .xsmall()
                        .on_click(move |_, window, cx| {
                            let (thread_id, path) = (thread_id.clone(), path.clone());
                            reply_entity.update(cx, |this, cx| {
                                this.open_reply(thread_id, path, window, cx);
                            });
                        }),
                )
                .child({
                    let thread_id2 = thread.id.clone();
                    Button::new(("resolve", row_ix))
                        .label(if resolved { "Unresolve" } else { "Resolve" })
                        .ghost()
                        .xsmall()
                        .on_click(move |_, _, cx| {
                            let thread_id2 = thread_id2.clone();
                            resolve_entity.update(cx, |this, cx| {
                                this.toggle_thread_resolved(thread_id2, cx);
                            });
                        })
                }),
        );
    }

    div()
        .w_full()
        .pl(px(CODE_INDENT))
        .pr_3()
        .child(card)
        .into_any_element()
}

fn small_tag(label: &'static str, color: gpui::Hsla) -> impl IntoElement {
    Tag::custom(color.opacity(0.15), color, color.opacity(0.4))
        .small()
        .child(SharedString::from(label))
}

/// Lightweight markdown for comment bodies: preserves line breaks and renders
/// ``` fenced blocks in monospace. Full markdown (via a text engine) is a
/// later enhancement.
fn render_comment_body(body: &str, cx: &App) -> gpui::AnyElement {
    let mut container = div().flex().flex_col().gap_1().text_sm();
    let mut in_fence = false;
    let mut code_buf: Vec<String> = Vec::new();
    let mut text_buf: Vec<String> = Vec::new();

    let flush_text = |container: gpui::Div, buf: &mut Vec<String>| -> gpui::Div {
        if buf.is_empty() {
            return container;
        }
        let lines = std::mem::take(buf);
        container.child(
            div().flex().flex_col().children(
                lines
                    .into_iter()
                    .map(|l| div().child(SharedString::from(l))),
            ),
        )
    };
    let flush_code = |container: gpui::Div, buf: &mut Vec<String>, cx: &App| -> gpui::Div {
        if buf.is_empty() {
            return container;
        }
        let lines = std::mem::take(buf);
        container.child(
            div()
                .font_family(MONO)
                .px_2()
                .py_1()
                .rounded_md()
                .bg(cx.theme().background.opacity(0.6))
                .children(
                    lines
                        .into_iter()
                        .map(|l| div().whitespace_nowrap().overflow_hidden().child(SharedString::from(l))),
                ),
        )
    };

    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            if in_fence {
                container = flush_code(container, &mut code_buf, cx);
                in_fence = false;
            } else {
                container = flush_text(container, &mut text_buf);
                in_fence = true;
            }
            continue;
        }
        if in_fence {
            code_buf.push(line.to_string());
        } else {
            text_buf.push(line.to_string());
        }
    }
    // Unclosed fence: render what we gathered as code anyway.
    container = flush_text(container, &mut text_buf);
    container = flush_code(container, &mut code_buf, cx);
    container.into_any_element()
}

/// Blocking: runs on the background executor.
fn fetch_diff(client: &GithubClient, pr: &PrSummary) -> Result<LoadedDiff, String> {
    let pr_files = client
        .pr_files(&pr.repo, pr.number)
        .map_err(|e| e.to_string())?;
    let threads = client
        .pr_review_threads(&pr.repo, pr.number)
        .map_err(|e| e.to_string())?;
    // The review/merge state and conversation are best-effort: a failure
    // here shouldn't block reading the diff.
    let review_state = client.pr_review_state(&pr.repo, pr.number).ok();
    let conversation = client.pr_conversation(&pr.repo, pr.number).ok();
    let mut files = Vec::with_capacity(pr_files.len());
    let mut stats = Vec::with_capacity(pr_files.len());
    for f in &pr_files {
        files.push(diff_kit::file_diff_from_pr_file(f).map_err(|e| format!("{}: {e}", f.path))?);
        stats.push((f.additions, f.deletions));
    }
    Ok(LoadedDiff { files, stats, threads, review_state, conversation })
}

impl gpui::EventEmitter<SessionEvent> for PrSessionView {}

impl Focusable for PrSessionView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PrSessionView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body: gpui::AnyElement = match &self.load {
            SessionLoad::Loading => div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child("Loading diff…")
                .into_any_element(),
            SessionLoad::Failed(err) => div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(format!("Failed to load diff: {err}")))
                .into_any_element(),
            SessionLoad::Ready => {
                let entity = cx.entity();
                let sidebar_entity = entity.clone();
                let row_entity = entity.clone();
                let file_count = self.files.len();
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .w(px(280.))
                            .flex_shrink_0()
                            .border_r_1()
                            .border_color(cx.theme().border)
                            .child(
                                uniform_list(
                                    "file-sidebar",
                                    file_count,
                                    move |range, _window, cx| {
                                        let this = sidebar_entity.read(cx);
                                        range
                                            .map(|ix| {
                                                let row =
                                                    this.render_sidebar_row(ix, &sidebar_entity, cx);
                                                div()
                                                    .id(("file", ix))
                                                    .on_click({
                                                        let entity = sidebar_entity.clone();
                                                        move |_, _, cx| {
                                                            entity.update(cx, |this, cx| {
                                                                this.select_file(ix, cx)
                                                            });
                                                        }
                                                    })
                                                    .child(row)
                                                    .into_any_element()
                                            })
                                            .collect()
                                    },
                                )
                                .track_scroll(self.sidebar_scroll.clone())
                                .h_full(),
                            ),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            list(self.list_state.clone(), move |ix, _window, cx| {
                                let entity = row_entity.clone();
                                let this = entity.read(cx);
                                match this.view_mode {
                                    ViewMode::Unified => this.render_diff_row(ix, &row_entity, cx),
                                    ViewMode::Split => this.render_split_row(ix, &row_entity, cx),
                                }
                            })
                            .size_full(),
                        ),
                    )
                    .when(self.show_conversation, |row| {
                        row.child(self.render_conversation_panel(cx))
                    })
                    .into_any_element()
            }
        };
        let mut root = div()
            .track_focus(&self.focus_handle)
            .key_context(SESSION_KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &NextFile, _, cx| this.step_file(1, cx)))
            .on_action(cx.listener(|this, _: &PrevFile, _, cx| this.step_file(-1, cx)))
            .on_action(cx.listener(|this, _: &NextThread, _, cx| this.step_thread(1, cx)))
            .on_action(cx.listener(|this, _: &PrevThread, _, cx| this.step_thread(-1, cx)))
            .on_action(cx.listener(|this, _: &RefreshSession, _, cx| this.refresh(cx)))
            .on_action(cx.listener(|this, _: &DismissOverlay, _, cx| {
                if !this.dismiss_overlay(cx) {
                    cx.emit(SessionEvent::Close);
                }
            }))
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_header(cx))
            .child(self.render_checkout_bar(cx))
            .child(body);
        if self.review_drawer.is_some() {
            root = root.child(self.render_review_drawer(cx));
        } else if self.composer.is_some() {
            root = root.child(self.render_composer(cx));
        }
        root
    }
}
