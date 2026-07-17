//! GitHub API access for prdesk. Reads and writes go through a pluggable
//! [`GithubTransport`]; all calls are blocking — run them on a background
//! executor.

mod parse;
mod transport;

pub use transport::{GhCliTransport, GithubTransport, TransportError};

use github_types::{PrFile, PrNumber, PrSummary, RepoId};
use parse::{RestPrFile, SearchNode, SearchPrsData};
use serde_json::json;
use std::sync::Arc;

const SEARCH_PRS: &str = include_str!("queries/search_prs.graphql");
const VIEWER: &str = "query { viewer { login avatarUrl } }";

/// The three PR-list tabs. Each maps to a GitHub search qualifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueueTab {
    ReviewRequested,
    Created,
    Assigned,
}

impl QueueTab {
    pub const ALL: [QueueTab; 3] = [
        QueueTab::ReviewRequested,
        QueueTab::Created,
        QueueTab::Assigned,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            QueueTab::ReviewRequested => "To review",
            QueueTab::Created => "Created",
            QueueTab::Assigned => "Assigned",
        }
    }

    fn qualifier(&self) -> &'static str {
        match self {
            QueueTab::ReviewRequested => "review-requested:@me",
            QueueTab::Created => "author:@me",
            QueueTab::Assigned => "assignee:@me",
        }
    }

    /// Builds the search string. `scope` narrows results, e.g. `org:foo` or
    /// `repo:foo/bar`; empty means all of GitHub.
    pub fn search_query(&self, scope: &str) -> String {
        let mut q = format!("is:open is:pr archived:false {}", self.qualifier());
        let scope = scope.trim();
        if !scope.is_empty() {
            q.push(' ');
            q.push_str(scope);
        }
        q
    }
}

#[derive(Clone)]
pub struct GithubClient {
    transport: Arc<dyn GithubTransport>,
}

impl GithubClient {
    pub fn new(transport: Arc<dyn GithubTransport>) -> Self {
        Self { transport }
    }

    pub fn gh_cli() -> Self {
        Self::new(Arc::new(GhCliTransport::default()))
    }

    /// A client pinned to a specific gh account (gh supports multiple logins).
    pub fn gh_cli_as(account: impl Into<String>) -> Self {
        Self::new(Arc::new(GhCliTransport::for_account(account)))
    }

    /// The authenticated user's login.
    pub fn viewer_login(&self) -> Result<String, TransportError> {
        let data = self.transport.graphql(VIEWER, &json!({}))?;
        data.pointer("/viewer/login")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| TransportError::Api(format!("unexpected viewer response: {data}")))
    }

    /// All changed files of a PR, with per-file patches, across all pages.
    pub fn pr_files(&self, repo: &RepoId, number: PrNumber) -> Result<Vec<PrFile>, TransportError> {
        let path = format!(
            "repos/{}/{}/pulls/{}/files?per_page=100",
            repo.owner, repo.name, number.0
        );
        let items = self.transport.rest_paginated(&path)?;
        items
            .into_iter()
            .map(|item| {
                serde_json::from_value::<RestPrFile>(item)
                    .map(PrFile::from)
                    .map_err(|e| TransportError::Other(format!("bad files response: {e}")))
            })
            .collect()
    }

    pub fn search_prs(&self, query: &str, first: u32) -> Result<Vec<PrSummary>, TransportError> {
        let data = self
            .transport
            .graphql(SEARCH_PRS, &json!({ "q": query, "first": first }))?;
        let parsed: SearchPrsData = serde_json::from_value(data)
            .map_err(|e| TransportError::Other(format!("bad search response: {e}")))?;
        Ok(parsed
            .search
            .nodes
            .into_iter()
            .filter_map(|node| match node {
                SearchNode::Pr(pr) => Some(PrSummary::from(*pr)),
                SearchNode::Other(_) => None,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_queries() {
        assert_eq!(
            QueueTab::ReviewRequested.search_query(""),
            "is:open is:pr archived:false review-requested:@me"
        );
        assert_eq!(
            QueueTab::Created.search_query("org:octo-org"),
            "is:open is:pr archived:false author:@me org:octo-org"
        );
    }
}
