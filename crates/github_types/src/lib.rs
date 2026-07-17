//! Pure GitHub PR/review domain models shared by every prdesk crate.
//!
//! This crate must stay free of I/O, HTTP, and UI dependencies — it is the
//! layer that would port into another host (e.g. Zed core) verbatim.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoId {
    pub owner: String,
    pub name: String,
}

impl RepoId {
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// Parses `owner/name`.
    pub fn parse(slug: &str) -> Option<Self> {
        let (owner, name) = slug.split_once('/')?;
        if owner.is_empty() || name.is_empty() || name.contains('/') {
            return None;
        }
        Some(Self {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }
}

impl std::fmt::Display for RepoId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PrNumber(pub u64);

impl std::fmt::Display for PrNumber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// GraphQL global node ID — the primary key for mutations.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub login: String,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrState {
    Open,
    Merged,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeableState {
    Mergeable,
    Conflicting,
    Unknown,
}

/// Combined commit-status/check-run rollup for a PR's head commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckState {
    Success,
    Failure,
    Pending,
    Error,
}

/// A row in the PR list: everything the list UI needs, nothing more.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrSummary {
    pub repo: RepoId,
    pub number: PrNumber,
    pub node_id: NodeId,
    pub title: String,
    pub author: Actor,
    pub is_draft: bool,
    pub state: PrState,
    pub review_decision: Option<ReviewDecision>,
    pub checks: Option<CheckState>,
    pub head_ref: String,
    pub head_sha: String,
    pub updated_at: DateTime<Utc>,
    pub url: String,
}

/// Full PR detail for a review session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullRequest {
    pub repo: RepoId,
    pub number: PrNumber,
    pub node_id: NodeId,
    pub title: String,
    pub body_markdown: String,
    pub author: Actor,
    pub state: PrState,
    pub is_draft: bool,
    pub base_ref: String,
    pub head_ref: String,
    /// `Some(owner)` when the head branch lives in a fork.
    pub head_repo_owner: Option<String>,
    pub head_sha: String,
    pub mergeable: MergeableState,
    pub review_decision: Option<ReviewDecision>,
    pub updated_at: DateTime<Utc>,
    pub url: String,
}

/// One entry from `GET /pulls/{n}/files`: the file-level change plus the
/// raw patch text (absent for binary/huge files).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrFile {
    pub path: String,
    pub previous_path: Option<String>,
    pub status: FileChangeStatus,
    pub additions: u64,
    pub deletions: u64,
    pub patch: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileChangeStatus {
    Added,
    Modified,
    Removed,
    Renamed,
}

/// Which side of the diff a comment anchors to. `Left` = base (deletions),
/// `Right` = head (additions and context).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiffSide {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewComment {
    pub id: NodeId,
    /// REST id, needed for the REST reply endpoint fallback.
    pub database_id: Option<u64>,
    pub author: Actor,
    pub body_markdown: String,
    pub created_at: DateTime<Utc>,
    /// Part of the viewer's not-yet-submitted pending review.
    pub is_pending: bool,
    /// The excerpt of the diff this comment was written against, as GitHub
    /// stores it. Used to render outdated threads.
    pub diff_hunk: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewThread {
    pub id: NodeId,
    pub path: String,
    pub is_resolved: bool,
    pub is_outdated: bool,
    /// Anchor line in the PR's *current* diff; `None` for outdated threads.
    pub line: Option<u32>,
    /// Set for multi-line threads: the range is `start_line..=line`.
    pub start_line: Option<u32>,
    pub side: DiffSide,
    pub start_side: Option<DiffSide>,
    /// Anchor at the time the thread was written (fallback display for
    /// outdated threads).
    pub original_line: Option<u32>,
    pub comments: Vec<ReviewComment>,
}

/// Where a new comment will be anchored.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CommentAnchor {
    pub path: String,
    pub side: DiffSide,
    pub line: u32,
    pub start_line: Option<u32>,
    pub start_side: Option<DiffSide>,
}

/// The viewer's server-side PENDING review on a PR, if any.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingReview {
    pub id: NodeId,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewVerdict {
    Approve,
    RequestChanges,
    Comment,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_id_parse() {
        assert_eq!(
            RepoId::parse("octo-org/octo-app"),
            Some(RepoId {
                owner: "octo-org".into(),
                name: "octo-app".into()
            })
        );
        assert_eq!(RepoId::parse("nope"), None);
        assert_eq!(RepoId::parse("a/b/c"), None);
        assert_eq!(RepoId::parse("/x"), None);
    }
}
