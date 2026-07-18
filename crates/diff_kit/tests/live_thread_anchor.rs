//! Repro from a real PR (saxenanickk/custom-expo-updates-server#1): a live,
//! non-outdated review thread at RIGHT line 71 must anchor inline.

use diff_kit::{DiffRow, FileDiff, FileStatus, LayoutOptions, layout_unified, parse_patch};
use github_types::{Actor, DiffSide, NodeId, ReviewComment, ReviewThread};

#[test]
fn live_thread_anchors_inline() {
    let patch = include_str!("../fixtures/live_thread_patch.txt");
    let file = FileDiff {
        path: "expo-updates-server/__tests__/manifest.test.ts".into(),
        old_path: None,
        status: FileStatus::Modified,
        is_binary_or_too_large: false,
        hunks: parse_patch(patch).unwrap(),
    };
    let thread = ReviewThread {
        id: NodeId("PRRT_live".into()),
        path: "expo-updates-server/__tests__/manifest.test.ts".into(),
        is_resolved: false,
        is_outdated: false,
        line: Some(71),
        start_line: None,
        side: DiffSide::Right,
        start_side: None,
        original_line: Some(71),
        comments: vec![ReviewComment {
            id: NodeId("c1".into()),
            database_id: None,
            author: Actor { login: "reviewer".into(), avatar_url: None },
            body_markdown: "please fix".into(),
            created_at: chrono::Utc::now(),
            is_pending: false,
            diff_hunk: String::new(),
        }],
    };
    let rows = layout_unified(&[file], &[thread], LayoutOptions::default());
    let has_inline_thread = rows.iter().any(|r| matches!(r, DiffRow::Thread { .. }));
    let has_outdated = rows.iter().any(|r| matches!(r, DiffRow::OutdatedThread { .. }));
    assert!(has_inline_thread, "live thread must render inline, rows: {rows:?}");
    assert!(!has_outdated, "live thread must NOT be treated as outdated");
}
