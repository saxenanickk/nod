//! Parses a recorded real `git diff` (a slice of nod's own history) and
//! checks structural invariants across every file.

use diff_kit::{DiffLine, FileStatus, parse_git_diff};

#[test]
fn parses_real_multi_file_git_diff() {
    let raw = include_str!("../fixtures/real_git_diff.txt");
    let files = parse_git_diff(raw).expect("parses");
    assert!(files.len() >= 8, "expected many files, got {}", files.len());

    for file in &files {
        assert!(!file.path.is_empty(), "every file has a path");
        if file.status == FileStatus::Renamed {
            assert!(file.old_path.is_some(), "rename records its old path");
        }
        // Line numbering must advance consistently within each hunk.
        for hunk in &file.hunks {
            let (mut old, mut new) = (hunk.old_start, hunk.new_start);
            for line in &hunk.lines {
                match line {
                    DiffLine::Context { old: o, new: n, .. } => {
                        assert_eq!((*o, *n), (old, new), "{}: context drift", file.path);
                        old += 1;
                        new += 1;
                    }
                    DiffLine::Added { new: n, .. } => {
                        assert_eq!(*n, new, "{}: added drift", file.path);
                        new += 1;
                    }
                    DiffLine::Removed { old: o, .. } => {
                        assert_eq!(*o, old, "{}: removed drift", file.path);
                        old += 1;
                    }
                }
            }
        }
    }

    // At least one file should have real hunks (not all renames/binaries).
    assert!(files.iter().any(|f| !f.hunks.is_empty()));
}
