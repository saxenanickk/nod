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
    let tmp = std::env::temp_dir().join(format!("prdesk-test-{}", std::process::id()));
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
