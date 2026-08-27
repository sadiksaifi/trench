use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn commit(repo: &Path, path: &str, contents: &str, message: &str) {
    fs::write(repo.join(path), contents).unwrap();
    git(repo, &["add", path]);
    git(repo, &["commit", "-m", message]);
}

fn repository() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repository");
    let worktree = root.path().join("feature-topic");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    commit(&repo, "README.md", "initial\n", "initial");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "feature/topic",
            worktree.to_str().unwrap(),
        ],
    );
    (root, worktree.canonicalize().unwrap())
}

fn trench(repo: &Path, root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trench"))
        .current_dir(repo)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .args(args)
        .output()
        .expect("trench should run")
}

#[test]
fn dry_run_json_uses_the_stateless_single_target_contract() {
    let (root, worktree) = repository();
    let output = trench(
        &worktree,
        root.path(),
        &[
            "sync",
            "feature/topic",
            "--strategy",
            "rebase",
            "--dry-run",
            "--json",
        ],
    );

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "dry_run": true,
            "target": "feature-topic",
            "branch": "feature/topic",
            "path": worktree,
            "base": "main",
            "strategy": "rebase",
            "hook_policy": "run",
            "before": { "ahead": 0, "behind": 0 }
        })
    );
    assert!(!root.path().join("data/trench/trench.db").exists());
}
