//! Exercises clone discovery against a real on-disk git repo.

use github_types::RepoId;
use std::process::Command;

fn git(dir: &std::path::Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git runs")
        .status
        .success();
    assert!(ok, "git {args:?} failed");
}

#[test]
fn finds_and_reads_a_real_clone() {
    let tmp = std::env::temp_dir().join(format!("nod-test-{}", std::process::id()));
    let nested = tmp.join("code").join("acme-app");
    std::fs::create_dir_all(&nested).unwrap();

    git(&nested, &["init", "-q", "-b", "main"]);
    git(&nested, &["remote", "add", "origin", "git@github.com:acme/acme-app.git"]);
    git(&nested, &["config", "user.email", "t@example.com"]);
    git(&nested, &["config", "user.name", "T"]);
    std::fs::write(nested.join("README.md"), "hi").unwrap();
    git(&nested, &["add", "-A"]);
    git(&nested, &["commit", "-q", "-m", "init"]);

    let repo = RepoId { owner: "acme".into(), name: "acme-app".into() };

    // origin_repo reads the remote.
    assert_eq!(repo_local::origin_repo(&nested), Some(repo.clone()));

    // find_clone locates it two levels below the scan root.
    let found = repo_local::find_clone(&[tmp.clone()], &repo, 3);
    assert_eq!(found.as_deref(), Some(nested.as_path()));

    // git-state readers work.
    assert_eq!(repo_local::current_branch(&nested).unwrap(), "main");
    assert!(!repo_local::working_tree_dirty(&nested).unwrap());
    assert_eq!(repo_local::head_sha(&nested).unwrap().len(), 40);

    // A repo we don't have isn't falsely matched.
    let other = RepoId { owner: "acme".into(), name: "other".into() };
    assert_eq!(repo_local::find_clone(&[tmp.clone()], &other, 3), None);

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn local_diffs() {
    let clone = std::env::temp_dir().join(format!("nod-diff-{}", std::process::id()));
    std::fs::create_dir_all(&clone).unwrap();
    git(&clone, &["init", "-q", "-b", "main"]);
    git(&clone, &["config", "user.email", "t@example.com"]);
    git(&clone, &["config", "user.name", "T"]);
    std::fs::write(clone.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    git(&clone, &["add", "-A"]);
    git(&clone, &["commit", "-q", "-m", "init"]);

    // No origin remote → default_base falls back to the existing `main`.
    assert_eq!(repo_local::default_base(&clone).unwrap(), "main");

    // Branch with a committed change.
    git(&clone, &["switch", "-q", "-c", "feature"]);
    std::fs::write(clone.join("b.txt"), "new file\n").unwrap();
    git(&clone, &["add", "-A"]);
    git(&clone, &["commit", "-q", "-m", "add b"]);

    let branch = repo_local::branch_diff(&clone, "main").unwrap();
    assert!(branch.contains("diff --git"), "branch diff has file headers");
    assert!(branch.contains("b.txt"), "branch diff includes the new file");

    // Uncommitted edit shows only in the uncommitted diff.
    std::fs::write(clone.join("a.txt"), "one\nCHANGED\nthree\n").unwrap();
    let uncommitted = repo_local::uncommitted_diff(&clone).unwrap();
    assert!(uncommitted.contains("a.txt"), "uncommitted diff includes edit");
    assert!(uncommitted.contains("+CHANGED"), "shows the changed line");
    assert!(
        !repo_local::branch_diff(&clone, "main").unwrap().contains("CHANGED"),
        "branch diff excludes uncommitted work"
    );

    std::fs::remove_dir_all(&clone).ok();
}
