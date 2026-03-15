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

use std::path::PathBuf;
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
