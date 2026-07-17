//! GitHub API access for prdesk. Reads and writes go through a pluggable
//! [`GithubTransport`]; all calls are blocking — run them on a background
//! executor.

mod parse;
mod transport;

pub use transport::{GhCliTransport, GithubTransport, TransportError};

use github_types::{
    CommentAnchor, DiffSide, NodeId, PrFile, PrNumber, PrSummary, RepoId, ReviewThread,
};
use parse::{RestPrFile, SearchNode, SearchPrsData, ThreadsData};
use serde_json::json;
use std::sync::Arc;

const SEARCH_PRS: &str = include_str!("queries/search_prs.graphql");
const PR_REVIEW_THREADS: &str = include_str!("queries/pr_review_threads.graphql");
const ADD_LINE_COMMENT: &str = include_str!("queries/add_line_comment.graphql");
const REPLY_TO_THREAD: &str = include_str!("queries/reply_to_thread.graphql");
const RESOLVE_THREAD: &str = include_str!("queries/resolve_thread.graphql");
const UNRESOLVE_THREAD: &str = include_str!("queries/unresolve_thread.graphql");
const VIEWER: &str = "query { viewer { login avatarUrl } }";

fn side_str(side: DiffSide) -> &'static str {
    match side {
        DiffSide::Left => "LEFT",
        DiffSide::Right => "RIGHT",
    }
}

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

    /// All review threads of a PR, following reviewThreads pagination.
    /// Note: comments within a thread are capped at the first 50.
    pub fn pr_review_threads(
        &self,
        repo: &RepoId,
        number: PrNumber,
    ) -> Result<Vec<ReviewThread>, TransportError> {
        let mut threads = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut vars = json!({
                "owner": repo.owner,
                "name": repo.name,
                "number": number.0,
            });
            if let Some(c) = &cursor {
                vars["cursor"] = json!(c);
            }
            let data = self.transport.graphql(PR_REVIEW_THREADS, &vars)?;
            let parsed: ThreadsData = serde_json::from_value(data)
                .map_err(|e| TransportError::Other(format!("bad threads response: {e}")))?;
            let connection = parsed.repository.pull_request.review_threads;
            threads.extend(connection.nodes.into_iter().map(ReviewThread::from));
            if connection.page_info.has_next_page {
                cursor = connection.page_info.end_cursor;
                if cursor.is_none() {
                    break;
                }
            } else {
                break;
            }
        }
        Ok(threads)
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

    // ---- writes (all immediate, i.e. not batched into a pending review) ----

    /// Posts a standalone line comment as a one-comment COMMENT review, which
    /// GitHub submits immediately (no pending draft). `commit` is the PR head
    /// sha; passing it pins the comment to the current diff.
    pub fn add_line_comment(
        &self,
        pr_node_id: &NodeId,
        commit: Option<&str>,
        anchor: &CommentAnchor,
        body: &str,
    ) -> Result<(), TransportError> {
        let mut vars = json!({
            "prId": pr_node_id.0,
            "path": anchor.path,
            "line": anchor.line,
            "side": side_str(anchor.side),
            "body": body,
        });
        if let Some(commit) = commit {
            vars["commit"] = json!(commit);
        }
        if let Some(start) = anchor.start_line {
            vars["startLine"] = json!(start);
            vars["startSide"] = json!(side_str(anchor.start_side.unwrap_or(anchor.side)));
        }
        self.transport.graphql(ADD_LINE_COMMENT, &vars)?;
        Ok(())
    }

    /// Replies to an existing review thread (posted immediately).
    pub fn reply_to_thread(&self, thread_id: &NodeId, body: &str) -> Result<(), TransportError> {
        self.transport.graphql(
            REPLY_TO_THREAD,
            &json!({ "threadId": thread_id.0, "body": body }),
        )?;
        Ok(())
    }

    pub fn resolve_thread(&self, thread_id: &NodeId) -> Result<(), TransportError> {
        self.transport
            .graphql(RESOLVE_THREAD, &json!({ "threadId": thread_id.0 }))?;
        Ok(())
    }

    pub fn unresolve_thread(&self, thread_id: &NodeId) -> Result<(), TransportError> {
        self.transport
            .graphql(UNRESOLVE_THREAD, &json!({ "threadId": thread_id.0 }))?;
        Ok(())
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
