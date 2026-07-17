//! Parser for the per-file `patch` strings returned by GitHub's
//! `GET /pulls/{n}/files` endpoint: unified-diff hunks without file headers.

use crate::{DiffLine, Hunk};
use std::sync::Arc;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum PatchParseError {
    #[error("line {0}: expected hunk header, got {1:?}")]
    ExpectedHunkHeader(usize, String),
    #[error("line {0}: malformed hunk header {1:?}")]
    BadHunkHeader(usize, String),
    #[error("line {0}: unknown line prefix {1:?}")]
    UnknownPrefix(usize, String),
}

/// Parses a GitHub `patch` string into hunks. Empty input (binary or
/// too-large files omit `patch`) yields no hunks.
pub fn parse_patch(patch: &str) -> Result<Vec<Hunk>, PatchParseError> {
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut old_line = 0u32;
    let mut new_line = 0u32;

    for (ix, raw) in patch.lines().enumerate() {
        if raw.starts_with("@@") {
            let (old_start, new_start) = parse_hunk_header(raw)
                .ok_or_else(|| PatchParseError::BadHunkHeader(ix + 1, raw.to_string()))?;
            old_line = old_start;
            new_line = new_start;
            hunks.push(Hunk {
                old_start,
                new_start,
                header: raw.to_string(),
                lines: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = hunks.last_mut() else {
            return Err(PatchParseError::ExpectedHunkHeader(ix + 1, raw.to_string()));
        };
        let text = || -> Arc<str> { raw[1..].into() };
        match raw.as_bytes().first() {
            Some(b' ') => {
                hunk.lines.push(DiffLine::Context {
                    old: old_line,
                    new: new_line,
                    text: text(),
                });
                old_line += 1;
                new_line += 1;
            }
            Some(b'+') => {
                hunk.lines.push(DiffLine::Added {
                    new: new_line,
                    text: text(),
                });
                new_line += 1;
            }
            Some(b'-') => {
                hunk.lines.push(DiffLine::Removed {
                    old: old_line,
                    text: text(),
                });
                old_line += 1;
            }
            // "\ No newline at end of file" — metadata, not a diff row.
            Some(b'\\') => {}
            // A fully empty line inside a hunk is a context line whose text
            // is empty (the leading space got trimmed somewhere upstream).
            None => {
                hunk.lines.push(DiffLine::Context {
                    old: old_line,
                    new: new_line,
                    text: "".into(),
                });
                old_line += 1;
                new_line += 1;
            }
            _ => return Err(PatchParseError::UnknownPrefix(ix + 1, raw.to_string())),
        }
    }
    Ok(hunks)
}

/// `@@ -12,5 +14,8 @@ optional section` → (12, 14). Counts are optional
/// (`@@ -1 +1 @@`).
fn parse_hunk_header(header: &str) -> Option<(u32, u32)> {
    let rest = header.strip_prefix("@@ -")?;
    let (old_part, rest) = rest.split_once(" +")?;
    let (new_part, _) = rest.split_once(" @@")?;
    let old_start = old_part.split(',').next()?.parse().ok()?;
    let new_start = new_part.split(',').next()?.parse().ok()?;
    Some((old_start, new_start))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "@@ -1,4 +1,5 @@\n context one\n-removed two\n+added two\n+added three\n context last\n@@ -10,2 +11,2 @@ fn section()\n-old ten\n+new eleven\n context\n\\ No newline at end of file";

    #[test]
    fn parses_two_hunks_with_numbering() {
        let hunks = parse_patch(PATCH).unwrap();
        assert_eq!(hunks.len(), 2);

        let first = &hunks[0];
        assert_eq!((first.old_start, first.new_start), (1, 1));
        assert_eq!(first.lines.len(), 5);
        assert_eq!(
            first.lines[0],
            DiffLine::Context { old: 1, new: 1, text: "context one".into() }
        );
        assert_eq!(first.lines[1], DiffLine::Removed { old: 2, text: "removed two".into() });
        assert_eq!(first.lines[2], DiffLine::Added { new: 2, text: "added two".into() });
        assert_eq!(first.lines[3], DiffLine::Added { new: 3, text: "added three".into() });
        assert_eq!(
            first.lines[4],
            DiffLine::Context { old: 3, new: 4, text: "context last".into() }
        );

        let second = &hunks[1];
        assert_eq!((second.old_start, second.new_start), (10, 11));
        assert_eq!(second.header, "@@ -10,2 +11,2 @@ fn section()");
        assert_eq!(second.lines.len(), 3, "no-newline marker is not a row");
        assert_eq!(second.lines[1], DiffLine::Added { new: 11, text: "new eleven".into() });
    }

    #[test]
    fn empty_patch_is_no_hunks() {
        assert_eq!(parse_patch("").unwrap(), vec![]);
    }

    #[test]
    fn header_without_counts() {
        let hunks = parse_patch("@@ -1 +1 @@\n-a\n+b").unwrap();
        assert_eq!((hunks[0].old_start, hunks[0].new_start), (1, 1));
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(matches!(
            parse_patch("not a patch"),
            Err(PatchParseError::ExpectedHunkHeader(1, _))
        ));
        assert!(matches!(
            parse_patch("@@ -1,1 +1,1 @@\n?weird"),
            Err(PatchParseError::UnknownPrefix(2, _))
        ));
    }
}
