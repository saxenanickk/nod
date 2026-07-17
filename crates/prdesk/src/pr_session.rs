//! One open PR: changed-files sidebar + scrollable unified diff with review
//! comment threads rendered inline at their anchor lines.

use crate::config::{self, Config};
use crate::util::relative_time;
use diff_kit::{DiffLine, DiffRow, FileDiff, FileStatus, LayoutOptions, layout_unified};
use github_client::GithubClient;
use github_types::{PrSummary, ReviewThread};
use std::path::PathBuf;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, ListAlignment, ListOffset, ListState,
    SharedString, UniformListScrollHandle, Window, div, list, prelude::*, px, rgb, uniform_list,
};
use gpui_component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    tag::Tag,
};
use std::collections::{HashMap, HashSet};

const MONO: &str = "Menlo";
const ROW_HEIGHT: f32 = 22.0;
/// Left offset that aligns thread cards with the code column (two gutters +
/// marker).
const CODE_INDENT: f32 = 44. + 44. + 20.;

pub enum SessionEvent {
    Close,
}

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
    /// file_ix → index of its `FileHeader` row.
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
            header_rows: Vec::new(),
            show_resolved: true,
            expanded: HashSet::new(),
            selected_file: None,
            list_state: ListState::new(0, ListAlignment::Top, px(512.)),
            sidebar_scroll: UniformListScrollHandle::new(),
            checkout: CheckoutState::Resolving,
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

    /// Recomputes the row list from stored files/threads and current options.
    fn relayout(&mut self) {
        let opts = LayoutOptions {
            show_resolved: self.show_resolved,
            show_outdated: true,
        };
        self.rows = layout_unified(&self.files, &self.threads, opts);
        self.header_rows = self
            .files
            .iter()
            .enumerate()
            .map(|(file_ix, _)| {
                self.rows
                    .iter()
                    .position(
                        |r| matches!(r, DiffRow::FileHeader { file_ix: fx } if *fx == file_ix),
                    )
                    .unwrap_or(0)
            })
            .collect();
        self.list_state.reset(self.rows.len());
    }

    fn toggle_resolved(&mut self, cx: &mut Context<Self>) {
        self.show_resolved = !self.show_resolved;
        self.relayout();
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

    fn thread(&self, id: &github_types::NodeId) -> Option<&ReviewThread> {
        self.thread_ix.get(&id.0).and_then(|&ix| self.threads.get(ix))
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let url = self.pr.url.clone();
        let thread_count = self.threads.len();
        let unresolved = self.threads.iter().filter(|t| !t.is_resolved).count();
        div()
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
            )
            .child(
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
                        .text_color(rgb(0xd29922))
                        .child("Checking out PR branch…"),
                );
            }
            CheckoutState::Failed(err) => {
                bar = bar
                    .child(
                        div()
                            .text_color(rgb(0xf85149))
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
                                .text_color(rgb(0x3fb950))
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
                                .text_color(rgb(0xd29922))
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

    fn render_sidebar_row(&self, file_ix: usize, cx: &App) -> gpui::AnyElement {
        let Some(file) = self.files.get(file_ix) else {
            return div().into_any_element();
        };
        let (adds, dels) = self.stats.get(file_ix).copied().unwrap_or((0, 0));
        let selected = self.selected_file == Some(file_ix);
        let comment_count = self
            .threads
            .iter()
            .filter(|t| t.path == file.path && !t.is_resolved)
            .count();
        let status_color = match file.status {
            FileStatus::Added => rgb(0x3fb950),
            FileStatus::Removed => rgb(0xf85149),
            FileStatus::Renamed | FileStatus::Modified => rgb(0xd29922),
        };
        let marker = match file.status {
            FileStatus::Added => "A",
            FileStatus::Modified => "M",
            FileStatus::Removed => "D",
            FileStatus::Renamed => "R",
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(ROW_HEIGHT + 4.))
            .px_2()
            .text_sm()
            .cursor_pointer()
            .when(selected, |el| el.bg(cx.theme().accent))
            .hover(|el| el.bg(cx.theme().accent))
            .child(
                div()
                    .text_color(status_color)
                    .font_family(MONO)
                    .child(SharedString::from(marker)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(file.path.clone())),
            )
            .when(comment_count > 0, |el| {
                el.child(
                    div()
                        .text_color(rgb(0xd29922))
                        .whitespace_nowrap()
                        .child(SharedString::from(format!("💬{comment_count}"))),
                )
            })
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .whitespace_nowrap()
                    .child(SharedString::from(format!("+{adds} −{dels}"))),
            )
            .into_any_element()
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
        let base = div().h(px(ROW_HEIGHT)).w_full().flex().items_center();
        match row {
            DiffRow::FileHeader { file_ix } => {
                let Some(file) = self.files.get(*file_ix) else {
                    return div().into_any_element();
                };
                let label = match (&file.old_path, file.is_binary_or_too_large) {
                    (Some(old), _) => format!("{} → {}", old, file.path),
                    (None, true) => format!("{} (no preview: binary or too large)", file.path),
                    (None, false) => file.path.clone(),
                };
                let mut header = base
                    .bg(cx.theme().secondary)
                    .px_3()
                    .gap_2()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(SharedString::from(label)),
                    );
                if let Some((clone_path, _)) = self.zed_target() {
                    let file_path = clone_path.join(&file.path);
                    header = header.child(
                        Button::new(("open-zed", ix))
                            .label("Open in Zed")
                            .ghost()
                            .xsmall()
                            .on_click(move |_, _, _| {
                                let _ = zed_bridge::open_in_zed(&file_path, None);
                            }),
                    );
                }
                header.into_any_element()
            }
            DiffRow::HunkHeader { file_ix, hunk_ix } => {
                let header = self
                    .files
                    .get(*file_ix)
                    .and_then(|f| f.hunks.get(*hunk_ix))
                    .map(|h| h.header.clone())
                    .unwrap_or_default();
                base.px_3()
                    .font_family(MONO)
                    .text_sm()
                    .text_color(rgb(0x6a9fb5))
                    .bg(cx.theme().secondary.opacity(0.5))
                    .child(SharedString::from(header))
                    .into_any_element()
            }
            DiffRow::Line { line, in_comment_range, file_ix } => {
                // ⌘-click opens the line in Zed once the clone sits on the
                // PR head (line numbers are only trustworthy then).
                let zed = self.zed_target().and_then(|(clone, on_pr)| {
                    if !on_pr {
                        return None;
                    }
                    let file = self.files.get(*file_ix)?;
                    let line_no = match line {
                        DiffLine::Context { new, .. } | DiffLine::Added { new, .. } => Some(*new),
                        DiffLine::Removed { .. } => None,
                    };
                    Some((clone.join(&file.path), line_no))
                });
                render_diff_line(line, *in_comment_range, ix, zed, cx)
            }
            DiffRow::Thread { thread_id } => {
                let Some(thread) = self.thread(thread_id) else {
                    return div().into_any_element();
                };
                render_thread_card(thread, ix, false, &self.expanded, entity, cx)
            }
            DiffRow::OutdatedThreadsHeader { count, .. } => div()
                .w_full()
                .px_3()
                .py_1()
                .text_sm()
                .text_color(rgb(0xd29922))
                .child(SharedString::from(format!(
                    "⚠ {count} outdated thread{} (code has changed since)",
                    if *count == 1 { "" } else { "s" }
                )))
                .into_any_element(),
            DiffRow::OutdatedThread { thread_id } => {
                let Some(thread) = self.thread(thread_id) else {
                    return div().into_any_element();
                };
                render_thread_card(thread, ix, true, &self.expanded, entity, cx)
            }
        }
    }
}

fn render_diff_line(
    line: &DiffLine,
    in_comment_range: bool,
    row_ix: usize,
    zed: Option<(PathBuf, Option<u32>)>,
    cx: &App,
) -> gpui::AnyElement {
    let (old, new, marker, bg): (Option<u32>, Option<u32>, &str, Option<gpui::Hsla>) = match line {
        DiffLine::Context { old, new, .. } => (Some(*old), Some(*new), " ", None),
        DiffLine::Added { new, .. } => {
            (None, Some(*new), "+", Some(gpui::Hsla::from(rgb(0x2ea043)).opacity(0.13)))
        }
        DiffLine::Removed { old, .. } => {
            (Some(*old), None, "-", Some(gpui::Hsla::from(rgb(0xf85149)).opacity(0.13)))
        }
    };
    let gutter = |n: Option<u32>| {
        div()
            .w(px(44.))
            .flex_shrink_0()
            .text_right()
            .pr_1()
            .text_color(cx.theme().muted_foreground)
            .child(SharedString::from(
                n.map(|n| n.to_string()).unwrap_or_default(),
            ))
    };
    let mut el = div()
        .id(("line", row_ix))
        .h(px(ROW_HEIGHT))
        .w_full()
        .flex()
        .items_center()
        .font_family(MONO)
        .text_sm();
    if let Some(bg) = bg {
        el = el.bg(bg);
    }
    if in_comment_range {
        el = el.border_l_2().border_color(rgb(0xd29922));
    }
    if let Some((path, line_no)) = zed {
        el = el.on_click(move |event: &gpui::ClickEvent, _, _| {
            if event.modifiers().platform {
                let _ = zed_bridge::open_in_zed(&path, line_no);
            }
        });
    }
    el.child(gutter(old))
        .child(gutter(new))
        .child(
            div()
                .w(px(20.))
                .flex_shrink_0()
                .text_center()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(marker)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(SharedString::from(line.text().to_string())),
        )
        .into_any_element()
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
            badges = badges.child(small_tag("Resolved", rgb(0x3fb950).into()));
        }
        if outdated {
            badges = badges.child(small_tag("Outdated", rgb(0xd29922).into()));
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
                meta = meta.child(small_tag("Pending", rgb(0xd29922).into()));
            }
            card = card.child(
                div()
                    .px_2()
                    .py_1()
                    .border_t_1()
                    .border_color(cx.theme().border.opacity(0.5))
                    .child(meta)
                    .child(
                        div()
                            .text_sm()
                            .child(SharedString::from(comment.body_markdown.clone())),
                    ),
            );
        }
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

/// Blocking: runs on the background executor.
fn fetch_diff(client: &GithubClient, pr: &PrSummary) -> Result<LoadedDiff, String> {
    let pr_files = client
        .pr_files(&pr.repo, pr.number)
        .map_err(|e| e.to_string())?;
    let threads = client
        .pr_review_threads(&pr.repo, pr.number)
        .map_err(|e| e.to_string())?;
    let mut files = Vec::with_capacity(pr_files.len());
    let mut stats = Vec::with_capacity(pr_files.len());
    for f in &pr_files {
        files.push(diff_kit::file_diff_from_pr_file(f).map_err(|e| format!("{}: {e}", f.path))?);
        stats.push((f.additions, f.deletions));
    }
    Ok(LoadedDiff { files, stats, threads })
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
                                                let row = this.render_sidebar_row(ix, cx);
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
                                this.render_diff_row(ix, &row_entity, cx)
                            })
                            .size_full(),
                        ),
                    )
                    .into_any_element()
            }
        };
        div()
            .track_focus(&self.focus_handle)
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_header(cx))
            .child(self.render_checkout_bar(cx))
            .child(body)
    }
}
