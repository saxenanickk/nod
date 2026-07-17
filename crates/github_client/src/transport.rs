//! Pluggable transport for GitHub API calls.
//!
//! v1 ships a `gh` CLI shell-out (piggybacks on the user's `gh auth`, zero
//! HTTP/auth code). The trait keeps the door open for an in-process
//! `http_client` implementation later without touching any caller.
//!
//! Calls are blocking; run them on a background executor.

use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("`gh` CLI not found; install it and run `gh auth login`")]
    GhMissing,
    #[error("not authenticated; run `gh auth login`")]
    NotAuthenticated,
    #[error("account `{0}` is not logged in to gh; run `gh auth login` to add it")]
    AccountMissing(String),
    #[error("GitHub API error: {0}")]
    Api(String),
    #[error("{0}")]
    Other(String),
}

pub trait GithubTransport: Send + Sync + 'static {
    /// Executes a GraphQL query/mutation and returns the `data` object.
    fn graphql(&self, query: &str, variables: &Value) -> Result<Value, TransportError>;

    /// Executes a REST call, returning the raw response body. `path` is
    /// relative to the API root, e.g. `repos/o/r/pulls/1/files`.
    fn rest(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, TransportError>;

    /// GETs a paginated list endpoint, following `Link` headers, and returns
    /// the concatenated items across all pages.
    fn rest_paginated(&self, path: &str) -> Result<Vec<Value>, TransportError>;
}

pub struct GhCliTransport {
    program: PathBuf,
    /// GitHub login to act as (gh supports multiple accounts). `None` uses
    /// gh's active account. The resolved token is cached in memory only.
    account: Option<String>,
    token: Mutex<Option<String>>,
}

impl Default for GhCliTransport {
    fn default() -> Self {
        Self {
            program: PathBuf::from("gh"),
            account: None,
            token: Mutex::new(None),
        }
    }
}

impl GhCliTransport {
    pub fn for_account(account: impl Into<String>) -> Self {
        Self {
            account: Some(account.into()),
            ..Self::default()
        }
    }

    /// Cheap preflight: is `gh` installed and is the configured account
    /// authenticated?
    pub fn auth_status(&self) -> Result<(), TransportError> {
        let out = Command::new(&self.program)
            .args(["auth", "status"])
            .output()
            .map_err(|_| TransportError::GhMissing)?;
        if !out.status.success() {
            return Err(TransportError::NotAuthenticated);
        }
        if self.account.is_some() {
            self.account_token()?;
        }
        Ok(())
    }

    /// Resolves and caches the token for the configured account, so every
    /// API call is pinned to it regardless of gh's globally active account.
    fn account_token(&self) -> Result<Option<String>, TransportError> {
        let Some(account) = &self.account else {
            return Ok(None);
        };
        if let Some(token) = self.token.lock().unwrap().clone() {
            return Ok(Some(token));
        }
        let out = Command::new(&self.program)
            .args(["auth", "token", "--user", account])
            .output()
            .map_err(|_| TransportError::GhMissing)?;
        if !out.status.success() {
            return Err(TransportError::AccountMissing(account.clone()));
        }
        let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if token.is_empty() {
            return Err(TransportError::AccountMissing(account.clone()));
        }
        *self.token.lock().unwrap() = Some(token.clone());
        Ok(Some(token))
    }

    fn run(&self, args: &[String]) -> Result<Value, TransportError> {
        let mut cmd = Command::new(&self.program);
        cmd.args(args);
        if let Some(token) = self.account_token()? {
            cmd.env("GH_TOKEN", token);
        }
        let out = cmd
            .output()
            .map_err(|_| TransportError::GhMissing)?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if stderr.contains("gh auth login") || stderr.contains("authentication") {
                // Drop the cached token so a fresh one is resolved after the
                // user re-authenticates.
                *self.token.lock().unwrap() = None;
                return Err(TransportError::NotAuthenticated);
            }
            // gh prints API errors to stdout as JSON when possible.
            let detail = if stderr.trim().is_empty() { &stdout } else { &stderr };
            return Err(TransportError::Api(detail.trim().to_string()));
        }
        if stdout.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&stdout)
            .map_err(|e| TransportError::Other(format!("bad JSON from gh: {e}")))
    }
}

impl GithubTransport for GhCliTransport {
    fn graphql(&self, query: &str, variables: &Value) -> Result<Value, TransportError> {
        let mut args = vec![
            "api".to_string(),
            "graphql".to_string(),
            "-f".to_string(),
            format!("query={query}"),
        ];
        if let Some(vars) = variables.as_object() {
            for (key, value) in vars {
                match value {
                    Value::String(s) => args.extend(["-f".into(), format!("{key}={s}")]),
                    // -F parses numbers/booleans as typed values.
                    other => args.extend(["-F".into(), format!("{key}={other}")]),
                }
            }
        }
        let response = self.run(&args)?;
        match response.get("data") {
            Some(data) if !data.is_null() => Ok(data.clone()),
            _ => Err(TransportError::Api(format!(
                "GraphQL response had no data: {response}"
            ))),
        }
    }

    fn rest(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, TransportError> {
        let mut args = vec![
            "api".to_string(),
            "-X".to_string(),
            method.to_string(),
            path.to_string(),
        ];
        if let Some(body) = body.and_then(|b| b.as_object()) {
            for (key, value) in body {
                match value {
                    Value::String(s) => args.extend(["-f".into(), format!("{key}={s}")]),
                    other => args.extend(["-F".into(), format!("{key}={other}")]),
                }
            }
        }
        self.run(&args)
    }

    fn rest_paginated(&self, path: &str) -> Result<Vec<Value>, TransportError> {
        // --slurp wraps each page's array in an outer array; flatten.
        let args = vec![
            "api".to_string(),
            "--paginate".to_string(),
            "--slurp".to_string(),
            path.to_string(),
        ];
        let pages = self.run(&args)?;
        let Some(pages) = pages.as_array() else {
            return Err(TransportError::Api(format!(
                "expected paginated array from {path}, got: {pages}"
            )));
        };
        Ok(pages
            .iter()
            .flat_map(|page| page.as_array().cloned().unwrap_or_default())
            .collect())
    }
}
