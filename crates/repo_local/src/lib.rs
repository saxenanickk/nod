//! Local clone discovery and PR checkout. Shells out to `git` and `gh` so
//! the user's existing credentials, hooks, and remotes are respected.

use anyhow::{Context, Result, bail};
use github_types::RepoId;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(clone: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(clone)
        .args(args)
        .output()
        .context("failed to run git")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn working_tree_dirty(clone: &Path) -> Result<bool> {
    Ok(!git(clone, &["status", "--porcelain"])?.is_empty())
}

pub fn current_branch(clone: &Path) -> Result<String> {
    git(clone, &["rev-parse", "--abbrev-ref", "HEAD"])
}

pub fn head_sha(clone: &Path) -> Result<String> {
    git(clone, &["rev-parse", "HEAD"])
}

/// Stashes everything (including untracked) with a recognizable message.
pub fn stash_all(clone: &Path, label: &str) -> Result<()> {
    git(clone, &["stash", "push", "-u", "-m", label])?;
    Ok(())
}

/// Checks out a PR branch via `gh pr checkout` (handles forks and upstream
/// tracking). Runs with the user's active gh account, whose git credentials
/// the clone already uses.
pub fn checkout_pr(clone: &Path, repo: &RepoId, number: u64) -> Result<()> {
    let out = Command::new("gh")
        .current_dir(clone)
        .args([
            "pr",
            "checkout",
            &number.to_string(),
            "--repo",
            &repo.slug(),
        ])
        .output()
        .context("failed to run gh")?;
    if !out.status.success() {
        bail!(
            "gh pr checkout failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Reads the GitHub `owner/name` a clone's `origin` remote points at.
pub fn origin_repo(clone: &Path) -> Option<RepoId> {
    let url = git(clone, &["remote", "get-url", "origin"]).ok()?;
    parse_github_remote(&url)
}

/// Handles `git@github.com:o/r.git`, `https://github.com/o/r.git`,
/// `ssh://git@github.com/o/r`.
pub fn parse_github_remote(url: &str) -> Option<RepoId> {
    let rest = url
        .split_once("github.com")
        .map(|(_, rest)| rest.trim_start_matches([':', '/']))?;
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let mut parts = rest.split('/');
    let owner = parts.next()?.to_string();
    let name = parts.next()?.to_string();
    if owner.is_empty() || name.is_empty() {
        return None;
    }
    Some(RepoId { owner, name })
}

const SKIP_DIRS: &[&str] = &[
    "Library",
    "Applications",
    "Movies",
    "Music",
    "Pictures",
    "Downloads",
    "node_modules",
    "target",
];

/// Scans `roots` (up to `depth` levels of directories) for a clone whose
/// origin matches `repo`. Skips hidden and well-known non-code directories.
pub fn find_clone(roots: &[PathBuf], repo: &RepoId, depth: usize) -> Option<PathBuf> {
    for root in roots {
        if let Some(found) = scan_dir(root, repo, depth) {
            return Some(found);
        }
    }
    None
}

fn scan_dir(dir: &Path, repo: &RepoId, depth: usize) -> Option<PathBuf> {
    if dir.join(".git").exists() {
        return (origin_repo(dir).as_ref() == Some(repo)).then(|| dir.to_path_buf());
    }
    if depth == 0 {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
            continue;
        }
        if let Some(found) = scan_dir(&path, repo, depth - 1) {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_remote_urls() {
        for url in [
            "git@github.com:octo-org/octo-app.git",
            "https://github.com/octo-org/octo-app.git",
            "https://github.com/octo-org/octo-app",
            "ssh://git@github.com/octo-org/octo-app.git",
        ] {
            assert_eq!(
                parse_github_remote(url),
                Some(RepoId { owner: "octo-org".into(), name: "octo-app".into() }),
                "failed for {url}"
            );
        }
        assert_eq!(parse_github_remote("https://gitlab.com/x/y.git"), None);
    }
}
