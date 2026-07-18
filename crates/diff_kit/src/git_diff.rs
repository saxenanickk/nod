//! Parser for full `git diff` output (as opposed to GitHub's headerless
//! per-file patches). Splits into files, classifies each, and reuses
//! [`parse_patch`](crate::parse_patch) on each file's hunk region.

use crate::{FileDiff, FileStatus, PatchParseError, parse_patch};

/// Parses `git diff` output into per-file diffs. Files with no textual hunks
/// (pure renames, binary, mode-only changes) come back with empty `hunks`.
pub fn parse_git_diff(diff: &str) -> Result<Vec<FileDiff>, PatchParseError> {
    let mut files = Vec::new();
    // Each file section starts at a `diff --git` line.
    let lines: Vec<&str> = diff.lines().collect();
    let mut starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.starts_with("diff --git "))
        .map(|(i, _)| i)
        .collect();
    starts.push(lines.len());

    for window in starts.windows(2) {
        let (start, end) = (window[0], window[1]);
        let block = &lines[start..end];
        if let Some(file) = parse_block(block)? {
            files.push(file);
        }
    }
    Ok(files)
}

fn strip_prefix_path(s: &str) -> Option<String> {
    // `a/path`, `b/path`, or `/dev/null`.
    if s == "/dev/null" {
        return None;
    }
    let s = s.strip_prefix("a/").or_else(|| s.strip_prefix("b/")).unwrap_or(s);
    // Git quotes paths with special chars; drop surrounding quotes.
    let s = s.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(s);
    Some(s.to_string())
}

fn parse_block(block: &[&str]) -> Result<Option<FileDiff>, PatchParseError> {
    let mut is_new = false;
    let mut is_deleted = false;
    let mut is_binary = false;
    let mut rename_from: Option<String> = None;
    let mut rename_to: Option<String> = None;
    let mut minus_path: Option<String> = None; // from `--- a/…`
    let mut plus_path: Option<String> = None; // from `+++ b/…`
    let mut hunk_start: Option<usize> = None;

    for (i, line) in block.iter().enumerate() {
        if line.starts_with("@@") {
            hunk_start = Some(i);
            break; // everything from here on is hunks
        } else if line.starts_with("new file mode") {
            is_new = true;
        } else if line.starts_with("deleted file mode") {
            is_deleted = true;
        } else if let Some(rest) = line.strip_prefix("rename from ") {
            rename_from = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("rename to ") {
            rename_to = Some(rest.to_string());
        } else if line.starts_with("Binary files ") {
            is_binary = true;
        } else if let Some(rest) = line.strip_prefix("--- ") {
            minus_path = strip_prefix_path(rest);
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            plus_path = strip_prefix_path(rest);
        }
    }

    // Resolve status, new path, and old path.
    let (status, path, old_path) = if rename_from.is_some() || rename_to.is_some() {
        let to = rename_to
            .clone()
            .or_else(|| plus_path.clone())
            .or_else(|| header_paths(block).map(|(_, b)| b))
            .unwrap_or_default();
        (FileStatus::Renamed, to, rename_from)
    } else if is_new {
        let p = plus_path
            .clone()
            .or_else(|| header_paths(block).map(|(_, b)| b))
            .unwrap_or_default();
        (FileStatus::Added, p, None)
    } else if is_deleted {
        let p = minus_path
            .clone()
            .or_else(|| header_paths(block).map(|(a, _)| a))
            .unwrap_or_default();
        (FileStatus::Removed, p, None)
    } else {
        let p = plus_path
            .clone()
            .or_else(|| minus_path.clone())
            .or_else(|| header_paths(block).map(|(_, b)| b))
            .unwrap_or_default();
        (FileStatus::Modified, p, None)
    };

    if path.is_empty() {
        return Ok(None);
    }

    let hunks = match hunk_start {
        Some(h) => parse_patch(&block[h..].join("\n"))?,
        None => Vec::new(),
    };

    Ok(Some(FileDiff {
        path,
        old_path,
        status,
        is_binary_or_too_large: is_binary,
        hunks,
    }))
}

/// Extracts `(a_path, b_path)` from a `diff --git a/… b/…` header line, as a
/// fallback when there are no `---`/`+++` lines (e.g. pure rename or binary).
fn header_paths(block: &[&str]) -> Option<(String, String)> {
    let header = block.first()?;
    let rest = header.strip_prefix("diff --git ")?;
    // Split at " b/" — works for the common unquoted case.
    let (a, b) = rest.split_once(" b/")?;
    let a = a.strip_prefix("a/").unwrap_or(a).to_string();
    Some((a, b.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DiffLine;

    const DIFF: &str = "diff --git a/src/a.rs b/src/a.rs\nindex 111..222 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,3 +1,3 @@\n ctx\n-old line\n+new line\n ctx2\ndiff --git a/newfile.txt b/newfile.txt\nnew file mode 100644\nindex 000..333\n--- /dev/null\n+++ b/newfile.txt\n@@ -0,0 +1,2 @@\n+hello\n+world\ndiff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\nindex 444..000\n--- a/gone.txt\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-bye\ndiff --git a/old/name.rs b/new/name.rs\nsimilarity index 100%\nrename from old/name.rs\nrename to new/name.rs\ndiff --git a/logo.png b/logo.png\nindex 555..666 100644\nBinary files a/logo.png and b/logo.png differ";

    #[test]
    fn parses_all_change_kinds() {
        let files = parse_git_diff(DIFF).unwrap();
        assert_eq!(files.len(), 5);

        let modified = &files[0];
        assert_eq!(modified.path, "src/a.rs");
        assert_eq!(modified.status, FileStatus::Modified);
        assert_eq!(modified.old_path, None);
        assert_eq!(modified.hunks.len(), 1);
        assert_eq!(
            modified.hunks[0].lines[1],
            DiffLine::Removed { old: 2, text: "old line".into() }
        );

        let added = &files[1];
        assert_eq!(added.path, "newfile.txt");
        assert_eq!(added.status, FileStatus::Added);
        assert_eq!(added.hunks[0].lines.len(), 2);

        let deleted = &files[2];
        assert_eq!(deleted.path, "gone.txt");
        assert_eq!(deleted.status, FileStatus::Removed);

        let renamed = &files[3];
        assert_eq!(renamed.status, FileStatus::Renamed);
        assert_eq!(renamed.path, "new/name.rs");
        assert_eq!(renamed.old_path.as_deref(), Some("old/name.rs"));
        assert!(renamed.hunks.is_empty(), "pure rename has no hunks");

        let binary = &files[4];
        assert_eq!(binary.path, "logo.png");
        assert!(binary.is_binary_or_too_large);
        assert!(binary.hunks.is_empty());
    }

    #[test]
    fn empty_diff_is_no_files() {
        assert_eq!(parse_git_diff("").unwrap(), vec![]);
    }
}
