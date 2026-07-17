//! Turns parsed file diffs + review threads into the flat row list the UI
//! renders 1:1. All thread-placement rules live here, UI-free and testable.

use crate::{DiffLine, FileDiff};
use github_types::{DiffSide, NodeId, ReviewThread};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum DiffRow {
    FileHeader { file_ix: usize },
    HunkHeader { file_ix: usize, hunk_ix: usize },
    Line {
        file_ix: usize,
        line: DiffLine,
        /// Inside some thread's multi-line range — the UI highlights it.
        in_comment_range: bool,
    },
    /// An inline review thread, directly after its anchor line.
    Thread { thread_id: NodeId },
    /// Collapsible group header for a file's outdated threads.
    OutdatedThreadsHeader { file_ix: usize, count: usize },
    /// An outdated thread inside the group (renders its own diff_hunk).
    OutdatedThread { thread_id: NodeId },
}

#[derive(Debug, Clone, Copy)]
pub struct LayoutOptions {
    pub show_resolved: bool,
    pub show_outdated: bool,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self { show_resolved: true, show_outdated: true }
    }
}

fn line_anchor(line: &DiffLine) -> Vec<(DiffSide, u32)> {
    match line {
        // A context line is addressable from both sides; GitHub anchors
        // context comments as Right, but tolerate Left anchors too.
        DiffLine::Context { old, new, .. } => {
            vec![(DiffSide::Right, *new), (DiffSide::Left, *old)]
        }
        DiffLine::Added { new, .. } => vec![(DiffSide::Right, *new)],
        DiffLine::Removed { old, .. } => vec![(DiffSide::Left, *old)],
    }
}

/// Lays out all files as one flat unified-diff row list with threads inline.
///
/// Rules (see plan):
/// - A live thread sits directly after the line matching `(side, line)` in
///   the PR's current diff. Multi-line threads anchor at the end line and
///   mark `start_line..=line` as `in_comment_range`.
/// - Resolved threads are only included when `show_resolved`.
/// - Outdated threads (`line == None`) never guess a position: they group
///   under the file header behind `OutdatedThreadsHeader`.
/// - Threads whose `(side, line)` doesn't exist in the current diff (e.g.
///   anchored outside the hunk context GitHub returns) are treated as
///   outdated rather than dropped.
pub fn layout_unified(
    files: &[FileDiff],
    threads: &[ReviewThread],
    opts: LayoutOptions,
) -> Vec<DiffRow> {
    let mut rows = Vec::new();

    // Threads keyed by the path they anchor to (renames key by new path,
    // which is what GitHub reports in `thread.path`).
    let mut by_path: HashMap<&str, Vec<&ReviewThread>> = HashMap::new();
    for thread in threads {
        if !opts.show_resolved && thread.is_resolved {
            continue;
        }
        by_path.entry(thread.path.as_str()).or_default().push(thread);
    }

    for (file_ix, file) in files.iter().enumerate() {
        let file_threads = by_path.remove(file.path.as_str()).unwrap_or_default();

        // Split live vs outdated, resolving each live thread to its anchor.
        let mut anchored: HashMap<(DiffSide, u32), Vec<&ReviewThread>> = HashMap::new();
        let mut outdated: Vec<&ReviewThread> = Vec::new();
        let addressable: std::collections::HashSet<(DiffSide, u32)> = file
            .hunks
            .iter()
            .flat_map(|h| h.lines.iter())
            .flat_map(|l| line_anchor(l))
            .collect();
        for thread in file_threads {
            match thread.line {
                Some(line) if !thread.is_outdated && addressable.contains(&(thread.side, line)) => {
                    anchored.entry((thread.side, line)).or_default().push(thread);
                }
                _ => outdated.push(thread),
            }
        }

        // Ranges to highlight: (side, start..=end) per multi-line thread.
        let ranges: Vec<(DiffSide, u32, u32)> = anchored
            .values()
            .flatten()
            .filter_map(|t| {
                let end = t.line?;
                let start = t.start_line?;
                Some((t.start_side.unwrap_or(t.side), start, end))
            })
            .collect();

        rows.push(DiffRow::FileHeader { file_ix });

        if opts.show_outdated && !outdated.is_empty() {
            rows.push(DiffRow::OutdatedThreadsHeader { file_ix, count: outdated.len() });
            for thread in &outdated {
                rows.push(DiffRow::OutdatedThread { thread_id: thread.id.clone() });
            }
        }

        for (hunk_ix, hunk) in file.hunks.iter().enumerate() {
            rows.push(DiffRow::HunkHeader { file_ix, hunk_ix });
            for line in &hunk.lines {
                let in_comment_range = line_anchor(line).iter().any(|(side, n)| {
                    ranges
                        .iter()
                        .any(|(rside, start, end)| rside == side && (start..=end).contains(&n))
                });
                rows.push(DiffRow::Line { file_ix, line: line.clone(), in_comment_range });
                // Threads whose anchor matches this row, in input order.
                for (side, n) in line_anchor(line) {
                    if let Some(list) = anchored.remove(&(side, n)) {
                        rows.extend(
                            list.into_iter()
                                .map(|t| DiffRow::Thread { thread_id: t.id.clone() }),
                        );
                    }
                }
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileStatus, parse::parse_patch};
    use github_types::{Actor, ReviewComment};

    fn thread(id: &str, path: &str, side: DiffSide, line: Option<u32>) -> ReviewThread {
        ReviewThread {
            id: NodeId(id.to_string()),
            path: path.to_string(),
            is_resolved: false,
            is_outdated: false,
            line,
            start_line: None,
            side,
            start_side: None,
            original_line: line,
            comments: vec![ReviewComment {
                id: NodeId(format!("{id}-c1")),
                database_id: None,
                author: Actor { login: "reviewer".into(), avatar_url: None },
                body_markdown: "nit".into(),
                created_at: chrono::Utc::now(),
                is_pending: false,
                diff_hunk: String::new(),
            }],
        }
    }

    fn file(path: &str, patch: &str) -> FileDiff {
        FileDiff {
            path: path.to_string(),
            old_path: None,
            status: FileStatus::Modified,
            is_binary_or_too_large: false,
            hunks: parse_patch(patch).unwrap(),
        }
    }

    const PATCH: &str = "@@ -1,4 +1,5 @@\n context one\n-removed two\n+added two\n+added three\n context last";

    fn row_kinds(rows: &[DiffRow]) -> Vec<&'static str> {
        rows.iter()
            .map(|r| match r {
                DiffRow::FileHeader { .. } => "file",
                DiffRow::HunkHeader { .. } => "hunk",
                DiffRow::Line { .. } => "line",
                DiffRow::Thread { .. } => "thread",
                DiffRow::OutdatedThreadsHeader { .. } => "outdated-header",
                DiffRow::OutdatedThread { .. } => "outdated-thread",
            })
            .collect()
    }

    #[test]
    fn thread_sits_after_its_anchor_line() {
        let files = [file("src/a.rs", PATCH)];
        // Anchor on Right line 3 = "added three".
        let threads = [thread("t1", "src/a.rs", DiffSide::Right, Some(3))];
        let rows = layout_unified(&files, &threads, LayoutOptions::default());
        assert_eq!(
            row_kinds(&rows),
            ["file", "hunk", "line", "line", "line", "line", "thread", "line"]
        );
        let DiffRow::Line { line, .. } = &rows[5] else { panic!() };
        assert_eq!(*line, DiffLine::Added { new: 3, text: "added three".into() });
    }

    #[test]
    fn left_side_thread_anchors_on_removed_line() {
        let files = [file("src/a.rs", PATCH)];
        let threads = [thread("t1", "src/a.rs", DiffSide::Left, Some(2))];
        let rows = layout_unified(&files, &threads, LayoutOptions::default());
        assert_eq!(
            row_kinds(&rows),
            ["file", "hunk", "line", "line", "thread", "line", "line", "line"]
        );
        let DiffRow::Line { line, .. } = &rows[3] else { panic!() };
        assert_eq!(*line, DiffLine::Removed { old: 2, text: "removed two".into() });
    }

    #[test]
    fn multi_line_thread_highlights_range() {
        let files = [file("src/a.rs", PATCH)];
        let mut t = thread("t1", "src/a.rs", DiffSide::Right, Some(3));
        t.start_line = Some(2);
        t.start_side = Some(DiffSide::Right);
        let rows = layout_unified(&files, &[t], LayoutOptions::default());
        let flagged: Vec<bool> = rows
            .iter()
            .filter_map(|r| match r {
                DiffRow::Line { in_comment_range, .. } => Some(*in_comment_range),
                _ => None,
            })
            .collect();
        // Right lines 2 ("added two") and 3 ("added three") are in range;
        // context line 1 and 4 are not... but context "context one" is
        // Right line 1 (false) and "context last" Right line 4 (false).
        assert_eq!(flagged, [false, false, true, true, false]);
    }

    #[test]
    fn outdated_threads_group_under_file_header() {
        let files = [file("src/a.rs", PATCH)];
        let mut gone = thread("t1", "src/a.rs", DiffSide::Right, None);
        gone.is_outdated = true;
        // A thread whose anchor line isn't in the current diff is treated
        // as outdated too, never guessed.
        let unmappable = thread("t2", "src/a.rs", DiffSide::Right, Some(999));
        let rows = layout_unified(&files, &[gone, unmappable], LayoutOptions::default());
        assert_eq!(
            row_kinds(&rows)[..4],
            ["file", "outdated-header", "outdated-thread", "outdated-thread"]
        );
        let DiffRow::OutdatedThreadsHeader { count, .. } = rows[1] else { panic!() };
        assert_eq!(count, 2);
    }

    #[test]
    fn resolved_threads_can_be_hidden() {
        let files = [file("src/a.rs", PATCH)];
        let mut t = thread("t1", "src/a.rs", DiffSide::Right, Some(3));
        t.is_resolved = true;
        let shown = layout_unified(&files, std::slice::from_ref(&t), LayoutOptions::default());
        assert!(shown.iter().any(|r| matches!(r, DiffRow::Thread { .. })));
        let hidden = layout_unified(
            &files,
            &[t],
            LayoutOptions { show_resolved: false, show_outdated: true },
        );
        assert!(!hidden.iter().any(|r| matches!(r, DiffRow::Thread { .. })));
    }

    #[test]
    fn threads_for_other_files_do_not_leak() {
        let files = [file("src/a.rs", PATCH), file("src/b.rs", PATCH)];
        let threads = [thread("t1", "src/b.rs", DiffSide::Right, Some(3))];
        let rows = layout_unified(&files, &threads, LayoutOptions::default());
        let thread_pos = rows
            .iter()
            .position(|r| matches!(r, DiffRow::Thread { .. }))
            .unwrap();
        let b_header = rows
            .iter()
            .position(|r| matches!(r, DiffRow::FileHeader { file_ix: 1 }))
            .unwrap();
        assert!(thread_pos > b_header, "thread must be inside b.rs's section");
    }

    #[test]
    fn binary_file_renders_header_only() {
        let f = FileDiff {
            path: "img.png".into(),
            old_path: None,
            status: FileStatus::Modified,
            is_binary_or_too_large: true,
            hunks: vec![],
        };
        let rows = layout_unified(&[f], &[], LayoutOptions::default());
        assert_eq!(row_kinds(&rows), ["file"]);
    }
}
