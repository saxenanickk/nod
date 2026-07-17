//! Serde mirrors of the GraphQL responses we consume, plus conversion into
//! `github_types` domain models. One struct set per query, tested against
//! recorded fixtures.

use chrono::{DateTime, Utc};
use github_types::{
    Actor, CheckState, NodeId, PrNumber, PrState, PrSummary, RepoId, ReviewDecision,
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

#[cfg(test)]
mod tests {
    use super::*;

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
