//! Serde mirrors of the GraphQL responses we consume, plus conversion into
//! `github_types` domain models. One struct set per query, tested against
//! recorded fixtures.

use chrono::{DateTime, Utc};
use github_types::{
    Actor, CheckState, DiffSide, FileChangeStatus, NodeId, PrFile, PrNumber, PrState, PrSummary,
    RepoId, ReviewComment, ReviewDecision, ReviewThread,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPrsData {
    pub search: SearchConnection,
}

#[derive(Deserialize)]
pub struct SearchConnection {
    pub nodes: Vec<SearchNode>,
}

/// Search can return non-PR nodes (empty objects for issues); tolerate them.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum SearchNode {
    Pr(Box<PrNode>),
    Other(#[allow(dead_code)] serde_json::Value),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrNode {
    pub id: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub is_draft: bool,
    pub state: String,
    pub updated_at: DateTime<Utc>,
    pub review_decision: Option<String>,
    pub author: Option<GqlActor>,
    pub repository: GqlRepo,
    pub commits: CommitsConnection,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlActor {
    pub login: String,
    pub avatar_url: Option<String>,
}

#[derive(Deserialize)]
pub struct GqlRepo {
    pub name: String,
    pub owner: GqlOwner,
}

#[derive(Deserialize)]
pub struct GqlOwner {
    pub login: String,
}

#[derive(Deserialize)]
pub struct CommitsConnection {
    pub nodes: Vec<CommitNode>,
}

#[derive(Deserialize)]
pub struct CommitNode {
    pub commit: Commit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub status_check_rollup: Option<Rollup>,
}

#[derive(Deserialize)]
pub struct Rollup {
    pub state: String,
}

pub fn parse_pr_state(s: &str) -> PrState {
    match s {
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        _ => PrState::Open,
    }
}

pub fn parse_review_decision(s: &str) -> Option<ReviewDecision> {
    match s {
        "APPROVED" => Some(ReviewDecision::Approved),
        "CHANGES_REQUESTED" => Some(ReviewDecision::ChangesRequested),
        "REVIEW_REQUIRED" => Some(ReviewDecision::ReviewRequired),
        _ => None,
    }
}

pub fn parse_check_state(s: &str) -> Option<CheckState> {
    match s {
        "SUCCESS" => Some(CheckState::Success),
        "FAILURE" => Some(CheckState::Failure),
        "ERROR" => Some(CheckState::Error),
        "PENDING" | "EXPECTED" => Some(CheckState::Pending),
        _ => None,
    }
}

impl From<PrNode> for PrSummary {
    fn from(node: PrNode) -> Self {
        let checks = node
            .commits
            .nodes
            .first()
            .and_then(|c| c.commit.status_check_rollup.as_ref())
            .and_then(|r| parse_check_state(&r.state));
        PrSummary {
            repo: RepoId {
                owner: node.repository.owner.login,
                name: node.repository.name,
            },
            number: PrNumber(node.number),
            node_id: NodeId(node.id),
            title: node.title,
            author: node
                .author
                .map(|a| Actor {
                    login: a.login,
                    avatar_url: a.avatar_url,
                })
                // Deleted accounts come back as null authors.
                .unwrap_or(Actor {
                    login: "ghost".into(),
                    avatar_url: None,
                }),
            is_draft: node.is_draft,
            state: parse_pr_state(&node.state),
            review_decision: node.review_decision.as_deref().and_then(parse_review_decision),
            checks,
            updated_at: node.updated_at,
            url: node.url,
        }
    }
}

// ---- reviewThreads query ----

#[derive(Deserialize)]
pub struct ThreadsData {
    pub repository: ThreadsRepo,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadsRepo {
    pub pull_request: ThreadsPr,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadsPr {
    pub review_threads: ThreadsConnection,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadsConnection {
    pub page_info: PageInfo,
    pub nodes: Vec<GqlThread>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    pub has_next_page: bool,
    pub end_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlThread {
    pub id: String,
    pub is_resolved: bool,
    pub is_outdated: bool,
    pub path: String,
    pub line: Option<u32>,
    pub start_line: Option<u32>,
    pub original_line: Option<u32>,
    pub diff_side: Option<String>,
    pub start_diff_side: Option<String>,
    pub comments: GqlCommentsConnection,
}

#[derive(Deserialize)]
pub struct GqlCommentsConnection {
    pub nodes: Vec<GqlComment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlComment {
    pub id: String,
    pub database_id: Option<u64>,
    pub body: String,
    pub created_at: DateTime<Utc>,
    /// PENDING while part of the viewer's unsubmitted review.
    pub state: Option<String>,
    #[serde(default)]
    pub diff_hunk: String,
    pub author: Option<GqlActor>,
}

fn parse_side(s: Option<&str>) -> Option<DiffSide> {
    match s {
        Some("LEFT") => Some(DiffSide::Left),
        Some("RIGHT") => Some(DiffSide::Right),
        _ => None,
    }
}

impl From<GqlThread> for ReviewThread {
    fn from(t: GqlThread) -> Self {
        ReviewThread {
            id: NodeId(t.id),
            path: t.path,
            is_resolved: t.is_resolved,
            is_outdated: t.is_outdated,
            line: t.line,
            start_line: t.start_line,
            side: parse_side(t.diff_side.as_deref()).unwrap_or(DiffSide::Right),
            start_side: parse_side(t.start_diff_side.as_deref()),
            original_line: t.original_line,
            comments: t.comments.nodes.into_iter().map(ReviewComment::from).collect(),
        }
    }
}

impl From<GqlComment> for ReviewComment {
    fn from(c: GqlComment) -> Self {
        ReviewComment {
            id: NodeId(c.id),
            database_id: c.database_id,
            author: c
                .author
                .map(|a| Actor { login: a.login, avatar_url: a.avatar_url })
                .unwrap_or(Actor { login: "ghost".into(), avatar_url: None }),
            body_markdown: c.body,
            created_at: c.created_at,
            is_pending: c.state.as_deref() == Some("PENDING"),
            diff_hunk: c.diff_hunk,
        }
    }
}

/// One entry of the REST `GET /pulls/{n}/files` response.
#[derive(Deserialize)]
pub struct RestPrFile {
    pub filename: String,
    pub previous_filename: Option<String>,
    pub status: String,
    #[serde(default)]
    pub additions: u64,
    #[serde(default)]
    pub deletions: u64,
    /// Omitted by GitHub for binary and very large files.
    pub patch: Option<String>,
}

impl From<RestPrFile> for PrFile {
    fn from(f: RestPrFile) -> Self {
        let status = match f.status.as_str() {
            "added" | "copied" => FileChangeStatus::Added,
            "removed" => FileChangeStatus::Removed,
            "renamed" => FileChangeStatus::Renamed,
            _ => FileChangeStatus::Modified,
        };
        PrFile {
            path: f.filename,
            previous_path: f.previous_filename,
            status,
            additions: f.additions,
            deletions: f.deletions,
            patch: f.patch,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_threads_fixture() {
        // Recorded from zed-industries/zed PR #59596, hand-extended with a
        // resolved flip and a synthetic live multi-line pending thread.
        let raw = include_str!("../fixtures/pr_review_threads.json");
        let data: ThreadsData = serde_json::from_str(raw).expect("fixture parses");
        let conn = data.repository.pull_request.review_threads;
        assert!(!conn.page_info.has_next_page);
        let threads: Vec<ReviewThread> =
            conn.nodes.into_iter().map(ReviewThread::from).collect();
        assert_eq!(threads.len(), 4);
        assert_eq!(threads.iter().filter(|t| t.is_resolved).count(), 1);
        assert_eq!(threads.iter().filter(|t| t.is_outdated).count(), 3);

        let live = threads.iter().find(|t| !t.is_outdated).expect("live thread");
        assert_eq!(live.line, Some(42));
        assert_eq!(live.start_line, Some(40));
        assert_eq!(live.side, DiffSide::Right);
        assert_eq!(live.start_side, Some(DiffSide::Right));
        assert!(live.comments[0].is_pending);
        assert!(!live.comments[0].author.login.is_empty());

        let outdated = threads.iter().find(|t| t.is_outdated).unwrap();
        assert!(!outdated.comments.is_empty());
        assert!(!outdated.comments[0].diff_hunk.is_empty(), "diffHunk recorded");
    }

    #[test]
    fn parses_search_fixture() {
        let raw = include_str!("../fixtures/search_prs.json");
        let data: SearchPrsData = serde_json::from_str(raw).expect("fixture parses");
        let prs: Vec<PrSummary> = data
            .search
            .nodes
            .into_iter()
            .filter_map(|n| match n {
                SearchNode::Pr(pr) => Some(PrSummary::from(*pr)),
                SearchNode::Other(_) => None,
            })
            .collect();
        assert_eq!(prs.len(), 2);

        let first = &prs[0];
        assert_eq!(first.repo.slug(), "octo-org/octo-app");
        assert_eq!(first.number, PrNumber(1938));
        assert_eq!(first.author.login, "octocat");
        assert_eq!(first.review_decision, Some(ReviewDecision::ChangesRequested));
        assert_eq!(first.checks, Some(CheckState::Failure));
        assert!(!first.is_draft);

        let second = &prs[1];
        assert_eq!(second.author.login, "ghost", "null author becomes ghost");
        assert_eq!(second.review_decision, None);
        assert_eq!(second.checks, None, "missing rollup tolerated");
    }
}
