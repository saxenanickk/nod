//! Shared diff rendering used by both the PR review session and the local
//! review session: file sidebar rows, file/hunk headers, unified lines and
//! side-by-side pairs, syntax highlighting, and ⌘-click-to-Zed.
//!
//! PR-only interactivity is passed in as hooks: [`PaneData::add_comment`]
//! drives the `+`-to-comment affordance (local mode leaves it `None`), and
//! thread rows are rendered by the caller (local rows never contain them).

use diff_kit::{DiffLine, FileDiff, FileStatus, HalfKind, HalfRow};
use github_types::{CommentAnchor, DiffSide};
use gpui::{App, SharedString, Window, div, prelude::*, px, rgb};
use gpui_component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
};
use std::path::PathBuf;
use std::rc::Rc;

pub const MONO: &str = "Menlo";
pub const ROW_HEIGHT: f32 = 22.0;
/// Left offset that aligns thread cards with the code column (add-comment
/// column + two gutters + marker).
pub const CODE_INDENT: f32 = 18. + 44. + 44. + 20.;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Unified,
    Split,
}

/// Invoked when the `+` affordance is clicked to start a comment on a line.
pub type AddCommentFn = Rc<dyn Fn(CommentAnchor, &mut Window, &mut App)>;

/// The borrowed state the shared renderers need, plus optional PR-only hooks.
pub struct PaneData<'a> {
    pub files: &'a [FileDiff],
    /// `(clone_path, on_head)` — enables ⌘-click-to-Zed when `on_head`.
    pub zed_target: Option<(PathBuf, bool)>,
    /// `None` disables the `+`-to-comment affordance (read-only local mode).
    pub add_comment: Option<AddCommentFn>,
}

impl PaneData<'_> {
    fn lang_for(&self, file_ix: usize) -> Option<syntax::Lang> {
        self.files.get(file_ix).and_then(|f| syntax::lang_for_path(&f.path))
    }

    /// The Zed target for a specific new/old line, if the clone is on head.
    fn zed_for(&self, file_ix: usize, line_no: Option<u32>) -> Option<(PathBuf, Option<u32>)> {
        let (clone, on_head) = self.zed_target.as_ref()?;
        if !on_head {
            return None;
        }
        let file = self.files.get(file_ix)?;
        Some((clone.join(&file.path), line_no))
    }

    pub fn render_file_header(&self, file_ix: usize, row_ix: usize, cx: &App) -> gpui::AnyElement {
        let Some(file) = self.files.get(file_ix) else {
            return div().into_any_element();
        };
        let label = match (&file.old_path, file.is_binary_or_too_large) {
            (Some(old), _) => format!("{} → {}", old, file.path),
            (None, true) => format!("{} (no preview: binary or too large)", file.path),
            (None, false) => file.path.clone(),
        };
        let mut header = div()
            .h(px(ROW_HEIGHT))
            .w_full()
            .flex()
            .items_center()
            .bg(cx.theme().secondary)
            .px_3()
            .gap_2()
            .font_weight(gpui::FontWeight::BOLD)
            .child(div().flex_1().min_w_0().truncate().child(SharedString::from(label)));
        if let Some((clone_path, _)) = &self.zed_target {
            let file_path = clone_path.join(&file.path);
            header = header.child(
                Button::new(("open-zed", row_ix))
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

    pub fn render_hunk_header(&self, file_ix: usize, hunk_ix: usize, cx: &App) -> gpui::AnyElement {
        let header = self
            .files
            .get(file_ix)
            .and_then(|f| f.hunks.get(hunk_ix))
            .map(|h| h.header.clone())
            .unwrap_or_default();
        div()
            .h(px(ROW_HEIGHT))
            .w_full()
            .flex()
            .items_center()
            .px_3()
            .font_family(MONO)
            .text_sm()
            .text_color(rgb(0x6a9fb5))
            .bg(cx.theme().secondary.opacity(0.5))
            .child(SharedString::from(header))
            .into_any_element()
    }

    pub fn render_line(
        &self,
        line: &DiffLine,
        in_comment_range: bool,
        row_ix: usize,
        file_ix: usize,
        cx: &App,
    ) -> gpui::AnyElement {
        let lang = self.lang_for(file_ix);
        let (old, new, marker, bg): (Option<u32>, Option<u32>, &str, Option<gpui::Hsla>) =
            match line {
                DiffLine::Context { old, new, .. } => (Some(*old), Some(*new), " ", None),
                DiffLine::Added { new, .. } => {
                    (None, Some(*new), "+", Some(gpui::Hsla::from(rgb(0x2ea043)).opacity(0.13)))
                }
                DiffLine::Removed { old, .. } => {
                    (Some(*old), None, "-", Some(gpui::Hsla::from(rgb(0xf85149)).opacity(0.13)))
                }
            };
        let zed_line = match line {
            DiffLine::Context { new, .. } | DiffLine::Added { new, .. } => Some(*new),
            DiffLine::Removed { .. } => None,
        };
        let zed = self.zed_for(file_ix, zed_line);
        let anchor = self
            .files
            .get(file_ix)
            .map(|f| anchor_for_line(&f.path, line));

        let gutter = |n: Option<u32>| {
            div()
                .w(px(44.))
                .flex_shrink_0()
                .text_right()
                .pr_1()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(n.map(|n| n.to_string()).unwrap_or_default()))
        };
        let group_name = SharedString::from(format!("line-{row_ix}"));
        let mut el = div()
            .id(("line", row_ix))
            .group(group_name.clone())
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
        el.child(self.add_affordance(("add-comment", row_ix), group_name, anchor, 18.))
            .child(gutter(old))
            .child(gutter(new))
            .child(
                div()
                    .w(px(20.))
                    .flex_shrink_0()
                    .text_center()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(marker)),
            )
            .child(code_cell(line.text(), lang, cx))
            .into_any_element()
    }

    pub fn render_pair(
        &self,
        left: Option<&HalfRow>,
        right: Option<&HalfRow>,
        in_comment_range: bool,
        file_ix: usize,
        row_ix: usize,
        cx: &App,
    ) -> gpui::AnyElement {
        let lang = self.lang_for(file_ix);
        let path = self.files.get(file_ix).map(|f| f.path.clone());
        let mut container = div().w_full().flex().items_center();
        if in_comment_range {
            container = container.border_l_2().border_color(rgb(0xd29922));
        }
        // Comment + Zed anchor only on the right (head) column.
        let right_anchor = path.clone().map(|p| CommentAnchor {
            path: p,
            side: DiffSide::Right,
            line: right.map(|h| h.line_no).unwrap_or(0),
            start_line: None,
            start_side: None,
        });
        let right_zed = self.zed_for(file_ix, right.map(|h| h.line_no));
        container
            .child(self.render_half(left, lang, ("half-l", row_ix), None, None, cx))
            .child(
                div()
                    .w(px(1.))
                    .h(px(ROW_HEIGHT))
                    .bg(cx.theme().border)
                    .flex_shrink_0(),
            )
            .child(self.render_half(right, lang, ("half-r", row_ix), right_anchor, right_zed, cx))
            .into_any_element()
    }

    /// One column of a side-by-side pair.
    fn render_half(
        &self,
        half: Option<&HalfRow>,
        lang: Option<syntax::Lang>,
        id: (&'static str, usize),
        anchor: Option<CommentAnchor>,
        zed: Option<(PathBuf, Option<u32>)>,
        cx: &App,
    ) -> gpui::AnyElement {
        let Some(half) = half else {
            return div()
                .w_1_2()
                .h(px(ROW_HEIGHT))
                .flex_shrink_0()
                .bg(cx.theme().secondary.opacity(0.25))
                .into_any_element();
        };
        let bg = match half.kind {
            HalfKind::Added => Some(gpui::Hsla::from(rgb(0x2ea043)).opacity(0.13)),
            HalfKind::Removed => Some(gpui::Hsla::from(rgb(0xf85149)).opacity(0.13)),
            HalfKind::Context => None,
        };
        let group_name = SharedString::from(format!("{}-{}", id.0, id.1));
        let mut cell = div()
            .id(id)
            .group(group_name.clone())
            .w_1_2()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .font_family(MONO)
            .text_sm();
        if let Some(bg) = bg {
            cell = cell.bg(bg);
        }
        if let Some((path, line_no)) = zed {
            cell = cell.on_click(move |event: &gpui::ClickEvent, _, _| {
                if event.modifiers().platform {
                    let _ = zed_bridge::open_in_zed(&path, line_no);
                }
            });
        }
        cell.child(self.add_affordance((id.0, id.1), group_name, anchor, 16.))
            .child(
                div()
                    .w(px(40.))
                    .flex_shrink_0()
                    .text_right()
                    .pr_1()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(half.line_no.to_string())),
            )
            .child(code_cell(&half.text, lang, cx))
            .into_any_element()
    }

    /// The hover-revealed `+` cell. Renders empty (just spacing) when there's
    /// no comment hook or anchor.
    fn add_affordance(
        &self,
        id: (&'static str, usize),
        group_name: SharedString,
        anchor: Option<CommentAnchor>,
        width: f32,
    ) -> gpui::Div {
        let mut cell = div().w(px(width)).flex_shrink_0().flex().justify_center();
        if let (Some(cb), Some(anchor)) = (&self.add_comment, anchor) {
            let cb = cb.clone();
            cell = cell.child(
                div()
                    // Offset the id so line and half affordances never collide.
                    .id((id.0, id.1 + 2_000_000))
                    .invisible()
                    .group_hover(group_name, |s| s.visible())
                    .text_color(rgb(0x539bf5))
                    .cursor_pointer()
                    .child("+")
                    .on_click(move |_, window, cx| cb(anchor.clone(), window, cx)),
            );
        }
        cell
    }

    /// A file row for the changed-files sidebar. `badge` is an optional
    /// unresolved-comment count (PR mode).
    pub fn render_sidebar_row(
        &self,
        file_ix: usize,
        adds: u64,
        dels: u64,
        selected: bool,
        badge: Option<usize>,
        cx: &App,
    ) -> gpui::AnyElement {
        let Some(file) = self.files.get(file_ix) else {
            return div().into_any_element();
        };
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
            .child(div().flex_1().min_w_0().truncate().child(SharedString::from(file.path.clone())))
            .when_some(badge.filter(|c| *c > 0), |el, count| {
                el.child(
                    div()
                        .text_color(rgb(0xd29922))
                        .whitespace_nowrap()
                        .child(SharedString::from(format!("💬{count}"))),
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
}

/// The anchor a new comment on `line` would use.
pub fn anchor_for_line(path: &str, line: &DiffLine) -> CommentAnchor {
    let (side, line_no) = match line {
        DiffLine::Context { new, .. } | DiffLine::Added { new, .. } => (DiffSide::Right, *new),
        DiffLine::Removed { old, .. } => (DiffSide::Left, *old),
    };
    CommentAnchor { path: path.to_string(), side, line: line_no, start_line: None, start_side: None }
}

pub fn outdated_header_el(count: usize) -> gpui::AnyElement {
    div()
        .w_full()
        .px_3()
        .py_1()
        .text_sm()
        .text_color(rgb(0xd29922))
        .child(SharedString::from(format!(
            "⚠ {count} outdated thread{} (code has changed since)",
            if count == 1 { "" } else { "s" }
        )))
        .into_any_element()
}

/// Maps a semantic highlight kind to a color (catppuccin-mocha-ish).
fn highlight_style(kind: syntax::Highlight) -> gpui::HighlightStyle {
    use syntax::Highlight as H;
    let color = match kind {
        H::Keyword => rgb(0xcba6f7),
        H::Function => rgb(0x89b4fa),
        H::Type => rgb(0xf9e2af),
        H::String => rgb(0xa6e3a1),
        H::Number | H::Constant => rgb(0xfab387),
        H::Comment => rgb(0x7f849c),
        H::Operator => rgb(0x89dceb),
        H::Punctuation => rgb(0xbac2de),
        H::Property => rgb(0x89b4fa),
        H::Variable => rgb(0xcdd6f4),
    };
    gpui::HighlightStyle { color: Some(color.into()), ..Default::default() }
}

/// Builds the code cell for a line, syntax-highlighted when a language is known.
pub fn code_cell(text: &str, lang: Option<syntax::Lang>, _cx: &App) -> gpui::AnyElement {
    let spans = lang.map(|l| syntax::highlight_line(l, text)).unwrap_or_default();
    let cell = div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap();
    if spans.is_empty() {
        return cell.child(SharedString::from(text.to_string())).into_any_element();
    }
    let highlights: Vec<(std::ops::Range<usize>, gpui::HighlightStyle)> = spans
        .iter()
        .map(|s| (s.start..s.end, highlight_style(s.kind)))
        .collect();
    cell.child(
        gpui::StyledText::new(SharedString::from(text.to_string())).with_highlights(highlights),
    )
    .into_any_element()
}
