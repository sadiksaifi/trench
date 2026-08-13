use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn git(repo: &Path, args: &[&str]) {
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
}

fn init_repo(repo: &Path) {
    git(repo, &["init", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "live catalog\n").unwrap();
    git(repo, &["add", "README.md"]);
    git(
        repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "init",
        ],
    );
}

fn add_worktree(repo: &Path, path: &Path, branch: &str) {
    git(
        repo,
        &[
            "worktree",
            "add",
            "-b",
            branch,
            path.to_str().unwrap(),
            "main",
        ],
    );
}

fn trench(cwd: &Path, xdg: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trench"))
        .current_dir(cwd)
        .env("XDG_CONFIG_HOME", xdg.join("config"))
        .env("XDG_DATA_HOME", xdg.join("data"))
        .env("XDG_STATE_HOME", xdg.join("state"))
        .env("XDG_CACHE_HOME", xdg.join("cache"))
        .args(args)
        .output()
        .expect("trench should run")
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap()
}

#[test]
fn list_json_uses_live_git_worktrees_without_creating_a_database() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("catalog-repo");
    let linked = root.path().join("feature-live");
    let xdg = root.path().join("xdg");
    std::fs::create_dir(&repo).unwrap();
    init_repo(&repo);
    add_worktree(&repo, &linked, "feature/live");

    let output = trench(&linked, &xdg, &["list", "--json"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["worktree"], "feature-live");
    assert_eq!(records[0]["branch"], "feature/live");
    assert_eq!(
        records[0]["path"],
        canonical(&linked).to_string_lossy().as_ref()
    );
    assert_eq!(records[0]["is_current"], true);
    assert_eq!(
        records[1]["path"],
        canonical(&repo).to_string_lossy().as_ref()
    );
    assert!(!xdg.join("data/trench/trench.db").exists());
}
