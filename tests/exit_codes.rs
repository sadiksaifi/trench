//! Integration tests verifying exit codes (FR-37) for each error scenario.
//!
//! Exit code map:
//!   0: Success
//!   1: General error
//!   2: Not found
//!   3: Branch exists
//!   4: Hook failed
//!   5: Git error
//!   6: Config error
//!   7: Hook timeout
//!   8: Missing required flag

use std::path::{Path, PathBuf};
use std::process::Command;

fn trench_bin() -> PathBuf {
    // CARGO_BIN_EXE_<name> is set by cargo for integration tests
    PathBuf::from(env!("CARGO_BIN_EXE_trench"))
}

/// Initialize a temporary git repo with an initial commit.
fn init_git_repo(dir: &std::path::Path) {
    Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(dir)
        .output()
        .expect("git init failed");
    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir)
        .output()
        .expect("git config email failed");
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir)
        .output()
        .expect("git config name failed");
    std::fs::write(dir.join("README.md"), "# test\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(dir)
        .output()
        .expect("git add failed");
    Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(dir)
        .output()
        .expect("git commit failed");
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git command failed");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

// ── Exit code 8: Missing required flag ─────────────────────────────────

#[test]
fn exit_code_8_sync_all_without_strategy() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    let output = Command::new(trench_bin())
        .args(["sync", "--all"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench");

    assert_eq!(
        output.status.code(),
        Some(8),
        "sync --all without --strategy should exit 8, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn exit_code_8_remove_json_without_force() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    create_worktree(tmp.path(), "json-needs-force");

    let output = Command::new(trench_bin())
        .args(["remove", "json-needs-force", "--json"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench remove --json");

    assert_eq!(
        output.status.code(),
        Some(8),
        "remove --json without --force should exit 8, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ── Exit code 6: Config error ──────────────────────────────────────────

#[test]
fn exit_code_6_init_when_file_exists() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    // Create .trench.toml first
    std::fs::write(tmp.path().join(".trench.toml"), "[hooks]\n").unwrap();

    let output = Command::new(trench_bin())
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench");

    assert_eq!(
        output.status.code(),
        Some(6),
        "init when .trench.toml exists should exit 6, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ── Exit code 2: Not found ────────────────────────────────────────────

#[test]
fn exit_code_2_switch_nonexistent() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    let output = Command::new(trench_bin())
        .args(["switch", "nonexistent-branch-xyz"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench");

    assert_eq!(
        output.status.code(),
        Some(2),
        "switch nonexistent should exit 2, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ── Exit code 3: Branch exists ─────────────────────────────────────────

#[test]
fn exit_code_3_create_existing_branch() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    // Create a branch that already exists
    Command::new("git")
        .args(["branch", "existing-feature"])
        .current_dir(tmp.path())
        .output()
        .expect("git branch failed");

    let output = Command::new(trench_bin())
        .args(["create", "existing-feature"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench");

    assert_eq!(
        output.status.code(),
        Some(3),
        "create existing branch should exit 3, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ── Exit code 4: Hook failed ──────────────────────────────────────────

#[test]
fn exit_code_4_pre_create_hook_fails() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    // Write .trench.toml with a pre_create hook that fails
    std::fs::write(
        tmp.path().join(".trench.toml"),
        r#"
[hooks.pre_create]
run = ["false"]
timeout_secs = 10
"#,
    )
    .unwrap();

    let output = Command::new(trench_bin())
        .args(["create", "hook-fail-test"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench");

    assert_eq!(
        output.status.code(),
        Some(4),
        "pre_create hook failure should exit 4, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ── Exit code 7: Hook timeout ─────────────────────────────────────────

#[test]
fn exit_code_7_hook_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    // Write .trench.toml with a pre_create hook that times out
    std::fs::write(
        tmp.path().join(".trench.toml"),
        r#"
[hooks.pre_create]
run = ["sleep 5"]
timeout_secs = 1
"#,
    )
    .unwrap();

    let output = Command::new(trench_bin())
        .args(["create", "timeout-test"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench");

    assert_eq!(
        output.status.code(),
        Some(7),
        "hook timeout should exit 7, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ── Exit code 5: Git error ────────────────────────────────────────────

#[test]
fn exit_code_5_git_error_not_a_repo() {
    let tmp = tempfile::tempdir().unwrap();
    // Do NOT init git — tmp is not a git repo

    let output = Command::new(trench_bin())
        .args(["list"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench");

    assert_eq!(
        output.status.code(),
        Some(5),
        "git error (not a repo) should exit 5, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ── Exit code 1: General error ─────────────────────────────────────────

#[test]
fn exit_code_1_sync_branch_with_all_flag() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    let output = Command::new(trench_bin())
        .args(["sync", "--all", "some-branch"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench");

    assert_eq!(
        output.status.code(),
        Some(1),
        "sync with both --all and <BRANCH> should exit 1, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ── Dry-run tests ─────────────────────────────────────────────────────

/// Helper: create a worktree via trench so we can test dry-run removal.
fn create_worktree(repo_dir: &Path, branch: &str) {
    let output = Command::new(trench_bin())
        .args(["create", branch])
        .current_dir(repo_dir)
        .output()
        .expect("failed to run trench create");
    assert!(
        output.status.success(),
        "trench create should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn dry_run_remove_does_not_delete_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    // Create a worktree first
    create_worktree(tmp.path(), "dry-run-integ");

    // Get the worktree path from list
    let list_output = Command::new(trench_bin())
        .args(["list", "--json"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench list");
    assert!(
        list_output.status.success(),
        "trench list --json should succeed, stderr: {}",
        String::from_utf8_lossy(&list_output.stderr)
    );
    let list_json: serde_json::Value =
        serde_json::from_slice(&list_output.stdout).expect("list should output valid JSON");
    let wt_path = list_json[0]["path"]
        .as_str()
        .expect("should have worktree path");
    assert!(
        Path::new(wt_path).exists(),
        "worktree should exist before dry-run"
    );

    // Run remove with --dry-run
    let output = Command::new(trench_bin())
        .args(["remove", "dry-run-integ", "--force", "--dry-run"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench remove --dry-run");

    assert!(
        output.status.success(),
        "dry-run remove should exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify worktree still exists (no side effects)
    assert!(
        Path::new(wt_path).exists(),
        "worktree should still exist after dry-run"
    );

    // Verify stdout contains plan info
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Dry run"),
        "stdout should contain dry-run plan, got: {stdout}"
    );
    assert!(
        stdout.contains("dry-run-integ"),
        "stdout should mention the worktree name"
    );
}

#[test]
fn dry_run_remove_with_json_outputs_valid_json() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    create_worktree(tmp.path(), "json-dry-integ");

    // Run remove with --dry-run --json
    let output = Command::new(trench_bin())
        .args(["remove", "json-dry-integ", "--force", "--dry-run", "--json"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench remove --dry-run --json");

    assert!(
        output.status.success(),
        "dry-run --json should exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Parse JSON output
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be valid JSON");

    assert_eq!(json["dry_run"], true);
    assert_eq!(json["name"], "json-dry-integ");
    assert_eq!(json["branch"], "json-dry-integ");
    assert_eq!(json["delete_branch_requested"], false);
    assert_eq!(json["force"], true);
    assert!(json["path"].is_string(), "path should be a string");

    // Verify worktree still exists
    let wt_path = json["path"].as_str().unwrap();
    assert!(
        Path::new(wt_path).exists(),
        "worktree should still exist after dry-run --json"
    );
}

#[test]
fn dry_run_remove_with_delete_branch_shows_requested_true() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    create_worktree(tmp.path(), "delete-branch-dry-integ");

    let output = Command::new(trench_bin())
        .args([
            "remove",
            "delete-branch-dry-integ",
            "--force",
            "--delete-branch",
            "--dry-run",
            "--json",
        ])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench remove --dry-run --delete-branch --json");

    assert!(
        output.status.success(),
        "dry-run --delete-branch --json should exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be valid JSON");

    assert_eq!(json["dry_run"], true);
    assert_eq!(
        json["delete_branch_requested"], true,
        "delete_branch_requested should be true in JSON output"
    );
}

#[test]
fn remove_live_json_with_delete_branch_outputs_json() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    create_worktree(tmp.path(), "json-delete-branch");

    let output = Command::new(trench_bin())
        .args([
            "--json",
            "remove",
            "json-delete-branch",
            "--force",
            "--delete-branch",
            "--no-hooks",
        ])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench remove --json --delete-branch");

    assert!(
        output.status.success(),
        "live remove --json should exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be valid JSON");
    assert_eq!(json["worktree"], "json-delete-branch");
    assert_eq!(json["branch"], "json-delete-branch");
    assert_eq!(json["delete_branch_requested"], true);
    assert_eq!(json["branch_deleted"], true);
    assert_eq!(json["branch_delete_forced"], true);
    assert!(json["branch_delete_error"].is_null());
}

#[test]
fn exit_code_8_remove_without_force_outside_interactive_terminal() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    create_worktree(tmp.path(), "needs-force");

    let output = Command::new(trench_bin())
        .args(["remove", "needs-force"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench remove without force");

    assert_eq!(
        output.status.code(),
        Some(8),
        "remove without --force outside interactive terminal should exit 8, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn switch_print_path_keeps_stdout_raw_and_reports_path_on_stderr() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    create_worktree(tmp.path(), "switch-print-path");

    let list_output = Command::new(trench_bin())
        .args(["list", "--json"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench list");
    assert!(
        list_output.status.success(),
        "trench list --json should succeed, stderr: {}",
        String::from_utf8_lossy(&list_output.stderr)
    );
    let list_json: serde_json::Value =
        serde_json::from_slice(&list_output.stdout).expect("list should output valid JSON");
    let wt_path = list_json
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == "switch-print-path")
        .and_then(|item| item["path"].as_str())
        .expect("should find worktree path")
        .to_string();

    let output = Command::new(trench_bin())
        .args(["switch", "switch-print-path", "--print-path"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench switch --print-path");

    assert!(
        output.status.success(),
        "switch --print-path should exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim_end(),
        wt_path,
        "stdout must stay raw path for shell integration"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!("Switched to {}", wt_path)),
        "stderr should report switched absolute path, got: {stderr}"
    );
}
