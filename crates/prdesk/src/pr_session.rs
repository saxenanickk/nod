//! One open PR: changed-files sidebar + scrollable unified diff.

use diff_kit::{DiffLine, DiffRow, FileDiff, FileStatus, LayoutOptions, layout_unified};
use github_client::GithubClient;
use github_types::PrSummary;
use gpui::{
    App, Context, FocusHandle, Focusable, ScrollStrategy, SharedString,
    UniformListScrollHandle, Window, div, prelude::*, px, rgb, uniform_list,
};
use gpui_component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
};

const MONO: &str = "Menlo";
const ROW_HEIGHT: f32 = 22.0;

pub enum SessionEvent {
    Close,
}

enum SessionLoad {
    Loading,
    Failed(String),
    Ready,
}

/// Everything derived off-thread from the files response.
struct LoadedDiff {
    files: Vec<FileDiff>,
    /// (additions, deletions) parallel to `files`.
    stats: Vec<(u64, u64)>,
    rows: Vec<DiffRow>,
    /// file_ix → index of its `FileHeader` row.
    header_rows: Vec<usize>,
}

pub struct PrSessionView {
    pr: PrSummary,
    load: SessionLoad,
    files: Vec<FileDiff>,
    stats: Vec<(u64, u64)>,
    rows: Vec<DiffRow>,
    header_rows: Vec<usize>,
    selected_file: Option<usize>,
    diff_scroll: UniformListScrollHandle,
    pub focus_handle: FocusHandle,
}

impl PrSessionView {
    pub fn new(client: GithubClient, pr: PrSummary, cx: &mut Context<Self>) -> Self {
        let this = Self {
            pr: pr.clone(),
            load: SessionLoad::Loading,
            files: Vec::new(),
            stats: Vec::new(),
            rows: Vec::new(),
            header_rows: Vec::new(),
            selected_file: None,
            diff_scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
        };
        cx.spawn(async move |view, cx| {
            let loaded = cx
                .background_spawn(async move { fetch_diff(&client, &pr) })
                .await;
            view.update(cx, |this, cx| {
                match loaded {
                    Ok(loaded) => {
                        this.files = loaded.files;
                        this.stats = loaded.stats;
                        this.rows = loaded.rows;
                        this.header_rows = loaded.header_rows;
                        this.load = SessionLoad::Ready;
                    }
                    Err(err) => this.load = SessionLoad::Failed(err),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        this
    }

    fn select_file(&mut self, file_ix: usize, cx: &mut Context<Self>) {
        self.selected_file = Some(file_ix);
        if let Some(&row_ix) = self.header_rows.get(file_ix) {
            self.diff_scroll.scroll_to_item(row_ix, ScrollStrategy::Top);
        }
        cx.notify();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let url = self.pr.url.clone();
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
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .whitespace_nowrap()
                    .child(SharedString::from(format!(
                        "{} {}",
                        self.pr.repo.slug(),
                        self.pr.number
                    ))),
            )
            .child(
                Button::new("open-browser")
                    .label("Open on GitHub")
                    .ghost()
                    .xsmall()
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            )
    }

    fn render_sidebar_row(&self, file_ix: usize, cx: &App) -> gpui::AnyElement {
        let Some(file) = self.files.get(file_ix) else {
            return div().into_any_element();
        };
        let (adds, dels) = self.stats.get(file_ix).copied().unwrap_or((0, 0));
        let selected = self.selected_file == Some(file_ix);
        let status_color = match file.status {
            FileStatus::Added => rgb(0x3fb950),
            FileStatus::Removed => rgb(0xf85149),
            FileStatus::Renamed => rgb(0xd29922),
            FileStatus::Modified => rgb(0xd29922),
        };
        let marker = match file.status {
            FileStatus::Added => "A",
            FileStatus::Modified => "M",
            FileStatus::Removed => "D",
            FileStatus::Renamed => "R",
        };
        div()
            .id(file_ix)
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
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .whitespace_nowrap()
                    .child(SharedString::from(format!("+{adds} −{dels}"))),
            )
            .into_any_element()
    }

    fn render_diff_row(&self, ix: usize, cx: &App) -> gpui::AnyElement {
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
                base.bg(cx.theme().secondary)
                    .px_3()
                    .gap_2()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(SharedString::from(label))
                    .into_any_element()
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
            DiffRow::Line { line, .. } => render_diff_line(line, cx),
            // Threads arrive in M2.
            DiffRow::Thread { .. }
            | DiffRow::OutdatedThreadsHeader { .. }
            | DiffRow::OutdatedThread { .. } => div().h(px(ROW_HEIGHT)).into_any_element(),
        }
    }
}

fn render_diff_line(line: &DiffLine, cx: &App) -> gpui::AnyElement {
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
        .h(px(ROW_HEIGHT))
        .w_full()
        .flex()
        .items_center()
        .font_family(MONO)
        .text_sm();
    if let Some(bg) = bg {
        el = el.bg(bg);
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

/// Blocking: runs on the background executor.
fn fetch_diff(client: &GithubClient, pr: &PrSummary) -> Result<LoadedDiff, String> {
    let pr_files = client
        .pr_files(&pr.repo, pr.number)
        .map_err(|e| e.to_string())?;
    let mut files = Vec::with_capacity(pr_files.len());
    let mut stats = Vec::with_capacity(pr_files.len());
    for f in &pr_files {
        files.push(diff_kit::file_diff_from_pr_file(f).map_err(|e| format!("{}: {e}", f.path))?);
        stats.push((f.additions, f.deletions));
    }
    let rows = layout_unified(&files, &[], LayoutOptions::default());
    let header_rows = files
        .iter()
        .enumerate()
        .map(|(file_ix, _)| {
            rows.iter()
                .position(|r| matches!(r, DiffRow::FileHeader { file_ix: fx } if *fx == file_ix))
                .unwrap_or(0)
        })
        .collect();
    Ok(LoadedDiff { files, stats, rows, header_rows })
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
                let file_count = self.files.len();
                let row_count = self.rows.len();
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
                                .h_full(),
                            ),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            uniform_list("diff-rows", row_count, move |range, _window, cx| {
                                let this = entity.read(cx);
                                range.map(|ix| this.render_diff_row(ix, cx)).collect()
                            })
                            .track_scroll(self.diff_scroll.clone())
                            .h_full(),
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
            .child(body)
    }
}
