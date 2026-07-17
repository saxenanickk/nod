//! Diff parsing, row layout, and review-comment anchor mapping — the
//! correctness core of prdesk. UI-free by design.

mod layout;
mod parse;

pub use layout::{DiffRow, LayoutOptions, layout_unified};
pub use parse::{PatchParseError, parse_patch};

use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Removed,
    Renamed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileDiff {
    /// Repo-relative path (the *new* path for renames — matches how GitHub
    /// keys review threads).
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    /// GitHub omits `patch` for binary and very large files.
    pub is_binary_or_too_large: bool,
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    pub old_start: u32,
    pub new_start: u32,
    /// The raw `@@ … @@ section` header line.
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DiffLine {
    Context { old: u32, new: u32, text: Arc<str> },
    Added { new: u32, text: Arc<str> },
    Removed { old: u32, text: Arc<str> },
}

impl DiffLine {
    pub fn text(&self) -> &str {
        match self {
            DiffLine::Context { text, .. }
            | DiffLine::Added { text, .. }
            | DiffLine::Removed { text, .. } => text,
        }
    }
}
