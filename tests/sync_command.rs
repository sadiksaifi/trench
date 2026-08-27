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

#[test]
fn dry_run_human_output_is_stdout_only_and_never_prompts() {
    let (root, worktree) = repository();
    let output = trench(
        &worktree,
        root.path(),
        &[
            "sync",
            "feature/topic",
            "--strategy",
            "merge",
            "--base",
            "main",
            "--dry-run",
        ],
    );

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!(
            "Dry run - no changes will be made\n  Worktree: feature-topic\n  Branch:   feature/topic\n  Path:     {}\n  Base:     main\n  Strategy: merge\n  Hooks:    enabled\n",
            worktree.display()
        )
    );
}

#[test]
fn real_rebase_json_reports_the_atomic_outcome_and_stages() {
    let (root, worktree) = repository();
    let repository = root.path().join("repository");
    commit(&repository, "main-only", "main\n", "advance main");
    commit(&worktree, "feature-only", "feature\n", "advance feature");

    let output = trench(
        &worktree,
        root.path(),
        &[
            "sync",
            "feature/topic",
            "--strategy",
            "rebase",
            "--no-hooks",
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
            "ok": true,
            "target": "feature-topic",
            "branch": "feature/topic",
            "path": worktree,
            "base": "main",
            "strategy": "rebase",
            "before": { "ahead": 1, "behind": 1 },
            "after": { "ahead": 1, "behind": 0 },
            "mutation_state": "applied",
            "stages": [
                { "stage": "validate", "success": true },
                { "stage": "validate", "success": true },
                { "stage": "sync", "success": true }
            ]
        })
    );
}

#[test]
fn dirty_target_json_is_structured_and_reports_no_mutation() {
    let (root, worktree) = repository();
    fs::write(worktree.join("dirty"), "local\n").unwrap();

    let output = trench(
        &worktree,
        root.path(),
        &[
            "sync",
            "feature/topic",
            "--strategy",
            "merge",
            "--no-hooks",
            "--json",
        ],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        serde_json::json!({
            "ok": false,
            "failure": {
                "stage": "validate",
                "mutation_state": "not_started",
                "class": "dirty",
                "message": "worktree 'feature-topic' has uncommitted changes; commit or stash them before syncing"
            },
            "stages": []
        })
    );
}

#[test]
fn invalid_dry_run_base_uses_the_same_structured_failure_contract() {
    let (root, worktree) = repository();
    let output = trench(
        &worktree,
        root.path(),
        &[
            "sync",
            "feature/topic",
            "--strategy",
            "rebase",
            "--base",
            "missing",
            "--dry-run",
            "--json",
        ],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        serde_json::json!({
            "ok": false,
            "failure": {
                "stage": "validate",
                "mutation_state": "not_started",
                "class": "invalid_base",
                "message": "explicit base not found: missing"
            },
            "stages": []
        })
    );
}

#[test]
fn dry_run_does_not_fetch_run_hooks_or_create_runtime_state() {
    let (root, worktree) = repository();
    let repository = root.path().join("repository");
    let remote = root.path().join("remote.git");
    git2::Repository::init_bare(&remote).unwrap();
    git(
        &repository,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&repository, &["push", "-u", "origin", "main"]);
    let bare = git2::Repository::open_bare(&remote).unwrap();
    let head = bare.refname_to_id("refs/heads/main").unwrap();
    bare.reference("refs/heads/unfetched", head, false, "test")
        .unwrap();
    let marker = root.path().join("hook-ran");
    fs::write(
        repository.join(".trench.toml"),
        format!(
            "[hooks.pre_sync]\nshell = {:?}\n",
            format!("touch {}", marker.display())
        ),
    )
    .unwrap();

    let refs_before = git(
        &repository,
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    );
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

    assert!(output.status.success());
    assert_eq!(
        git(
            &repository,
            &["for-each-ref", "--format=%(refname) %(objectname)"]
        ),
        refs_before
    );
    assert!(!marker.exists());
    for directory in ["config", "data", "state", "cache"] {
        assert!(!root.path().join(directory).exists(), "created {directory}");
    }
}

#[test]
fn pre_sync_timeout_has_stable_exit_seven_and_structured_truth() {
    let (root, worktree) = repository();
    fs::write(
        root.path().join("repository/.trench.toml"),
        "[hooks.pre_sync]\nshell = \"sleep 2\"\ntimeout_secs = 1\n",
    )
    .unwrap();

    let output = trench(
        &worktree,
        root.path(),
        &["sync", "feature/topic", "--strategy", "merge", "--json"],
    );

    assert_eq!(output.status.code(), Some(7));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["failure"]["stage"], "pre_hook");
    assert_eq!(value["failure"]["mutation_state"], "not_started");
    assert_eq!(value["failure"]["class"], "hook_timeout");
}

#[test]
fn merge_conflict_reports_no_mutation_and_leaves_no_operation_state() {
    let (root, worktree) = repository();
    let repository = root.path().join("repository");
    commit(&repository, "README.md", "main\n", "main conflict");
    commit(&worktree, "README.md", "feature\n", "feature conflict");
    let head = git(&worktree, &["rev-parse", "HEAD"]);

    let output = trench(
        &worktree,
        root.path(),
        &[
            "sync",
            "feature/topic",
            "--strategy",
            "merge",
            "--no-hooks",
            "--json",
        ],
    );

    assert_eq!(output.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["failure"]["class"], "conflict");
    assert_eq!(value["failure"]["mutation_state"], "not_started");
    assert_eq!(git(&worktree, &["rev-parse", "HEAD"]), head);
    assert!(git(&worktree, &["status", "--porcelain"]).is_empty());
    let git_dir = PathBuf::from(git(&worktree, &["rev-parse", "--git-dir"]).trim());
    assert!(!git_dir.join("MERGE_HEAD").exists());
    assert!(!git_dir.join("rebase-merge").exists());
    assert!(!git_dir.join("rebase-apply").exists());
}

#[test]
fn typed_sync_diagnostics_never_retain_hook_output() {
    let (root, worktree) = repository();
    fs::write(
        root.path().join("repository/.trench.toml"),
        "[hooks.pre_sync]\nshell = \"printf 'top-secret\\n'\"\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_trench"))
        .current_dir(&worktree)
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env("TRENCH_LOG", "debug")
        .args(["sync", "feature/topic", "--strategy", "rebase"])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("top-secret"));
    let diagnostics = fs::read_to_string(root.path().join("state/trench/trench.log")).unwrap();
    assert!(diagnostics.contains("operation=sync"));
    assert!(diagnostics.contains("stage=hook"));
    assert!(!diagnostics.contains("top-secret"));
}

#[test]
fn sync_help_exposes_only_the_single_target_contract() {
    let root = tempfile::tempdir().unwrap();
    let output = trench(root.path(), root.path(), &["sync", "--help"]);
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for option in [
        "<BRANCH>",
        "--strategy",
        "--base",
        "--no-hooks",
        "--dry-run",
        "--json",
    ] {
        assert!(help.contains(option), "missing {option}: {help}");
    }
    assert!(
        !help.contains("--all"),
        "legacy --all leaked into help: {help}"
    );
}
