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

fn trench_cmd(dir: &Path) -> Command {
    let xdg_root = dir.join(".test-xdg");
    let mut cmd = Command::new(trench_bin());
    cmd.current_dir(dir)
        .env("HOME", xdg_root.join("home"))
        .env("XDG_CONFIG_HOME", xdg_root.join("config"))
        .env("XDG_DATA_HOME", xdg_root.join("data"))
        .env("XDG_STATE_HOME", xdg_root.join("state"))
        .env("XDG_CACHE_HOME", xdg_root.join("cache"));
    cmd
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

#[test]
fn clap_exit_2_rejects_removed_sync_all_without_strategy() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    let output = trench_cmd(tmp.path())
        .args(["sync", "--all"])
        .output()
        .expect("failed to run trench");

    assert_eq!(
        output.status.code(),
        Some(2),
        "removed sync --all should be rejected by clap, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn exit_code_8_remove_json_without_yes() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    create_worktree(tmp.path(), "json-needs-force");

    let output = trench_cmd(tmp.path())
        .args(["remove", "json-needs-force", "--json"])
        .output()
        .expect("failed to run trench remove --json");

    assert_eq!(
        output.status.code(),
        Some(8),
        "remove --json without --yes should exit 8, stderr: {}",
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

    let output = trench_cmd(tmp.path())
        .args(["init"])
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

    let output = trench_cmd(tmp.path())
        .args(["switch", "nonexistent-branch-xyz"])
        .output()
        .expect("failed to run trench");

    assert_eq!(
        output.status.code(),
        Some(2),
        "switch nonexistent should exit 2, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ── Existing local branch: create its worktree ─────────────────────────

#[test]
fn create_existing_local_branch_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    // Create a branch that already exists
    Command::new("git")
        .args(["branch", "existing-feature"])
        .current_dir(tmp.path())
        .output()
        .expect("git branch failed");

    let output = trench_cmd(tmp.path())
        .args(["create", "existing-feature"])
        .output()
        .expect("failed to run trench");

    assert!(
        output.status.success(),
        "creating an existing local branch should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let path = String::from_utf8(output.stdout).unwrap();
    assert!(Path::new(path.trim()).is_dir());
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

    let output = trench_cmd(tmp.path())
        .args(["create", "hook-fail-test"])
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

    let output = trench_cmd(tmp.path())
        .args(["create", "timeout-test"])
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

    let output = trench_cmd(tmp.path())
        .args(["list"])
        .output()
        .expect("failed to run trench");

    assert_eq!(
        output.status.code(),
        Some(5),
        "git error (not a repo) should exit 5, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn clap_exit_2_rejects_removed_sync_all_with_target() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    let output = trench_cmd(tmp.path())
        .args(["sync", "--all", "some-branch"])
        .output()
        .expect("failed to run trench");

    assert_eq!(
        output.status.code(),
        Some(2),
        "removed sync --all should be rejected by clap, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ── Dry-run tests ─────────────────────────────────────────────────────

/// Helper: create a worktree via trench so we can test dry-run removal.
fn create_worktree(repo_dir: &Path, branch: &str) {
    let output = trench_cmd(repo_dir)
        .args(["create", branch])
        .output()
        .expect("failed to run trench create");
    assert!(
        output.status.success(),
        "trench create should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn worktree_path(repo_dir: &Path, branch: &str) -> PathBuf {
    let output = trench_cmd(repo_dir)
        .args(["list", "--json"])
        .output()
        .expect("failed to list worktrees");
    let records: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    PathBuf::from(
        records
            .as_array()
            .unwrap()
            .iter()
            .find(|record| record["branch"] == branch)
            .and_then(|record| record["path"].as_str())
            .expect("worktree should be listed"),
    )
}

#[test]
fn dry_run_remove_does_not_delete_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    // Create a worktree first
    create_worktree(tmp.path(), "dry-run-integ");

    // Get the worktree path from list
    let list_output = trench_cmd(tmp.path())
        .args(["list", "--json"])
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
    let output = trench_cmd(tmp.path())
        .args(["remove", "dry-run-integ", "--dry-run"])
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
        stdout.contains("Would remove"),
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
    let output = trench_cmd(tmp.path())
        .args(["remove", "json-dry-integ", "--dry-run", "--json"])
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
    assert_eq!(json["worktree"], "json-dry-integ");
    assert_eq!(json["branch"], "json-dry-integ");
    assert_eq!(json["delete_branch"], false);
    assert_eq!(json["confirmation"], "dry_run");
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

    let output = trench_cmd(tmp.path())
        .args([
            "remove",
            "delete-branch-dry-integ",
            "--delete-branch",
            "--dry-run",
            "--json",
        ])
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
        json["delete_branch"], true,
        "delete_branch should be true in JSON output"
    );
}

#[test]
fn remove_live_json_with_delete_branch_outputs_json() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    create_worktree(tmp.path(), "json-delete-branch");

    let output = trench_cmd(tmp.path())
        .args([
            "remove",
            "json-delete-branch",
            "--json",
            "--yes",
            "--delete-branch",
            "--no-hooks",
        ])
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
    assert_eq!(json["delete_branch"], true);
    assert_eq!(json["branch_deleted"], true);
    assert_eq!(json["force_branch"], false);
    assert_eq!(json["confirmation"], "flag");
    assert_eq!(json["no_hooks"], true);
}

#[test]
fn remove_worktree_after_its_local_branch_was_deleted() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    create_worktree(tmp.path(), "missing-local-branch");
    let path = worktree_path(tmp.path(), "missing-local-branch");
    let deleted = Command::new("git")
        .args(["update-ref", "-d", "refs/heads/missing-local-branch"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(deleted.status.success());

    let list = trench_cmd(tmp.path())
        .args(["list", "--json"])
        .output()
        .unwrap();
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    let records: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    assert!(records
        .as_array()
        .unwrap()
        .iter()
        .any(|record| { record["branch"] == "missing-local-branch" && record["ahead"].is_null() }));

    let preview = trench_cmd(tmp.path())
        .args([
            "remove",
            "missing-local-branch",
            "--dry-run",
            "--force-worktree",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(plan["branch"], "missing-local-branch");
    assert_eq!(plan["merged"], serde_json::Value::Null);
    assert_eq!(plan["branch_present"], false);
    assert!(path.exists());

    let without_dirty_override = trench_cmd(tmp.path())
        .args(["remove", "missing-local-branch", "--yes"])
        .output()
        .unwrap();
    assert!(!without_dirty_override.status.success());
    let refusal = String::from_utf8_lossy(&without_dirty_override.stderr);
    assert!(refusal.contains("no longer exists"), "{refusal}");
    assert!(refusal.contains("--force-worktree"), "{refusal}");
    assert!(!refusal.contains("is dirty"), "{refusal}");
    assert!(path.exists());
    let without_branch = trench_cmd(tmp.path())
        .args([
            "remove",
            "missing-local-branch",
            "--yes",
            "--force-worktree",
            "--delete-branch",
        ])
        .output()
        .unwrap();
    assert!(!without_branch.status.success());
    assert!(String::from_utf8_lossy(&without_branch.stderr).contains("omit --delete-branch"));
    assert!(path.exists());

    let removed = trench_cmd(tmp.path())
        .args([
            "remove",
            "missing-local-branch",
            "--yes",
            "--force-worktree",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    let outcome: serde_json::Value = serde_json::from_slice(&removed.stdout).unwrap();
    assert_eq!(outcome["branch_deleted"], false);
    assert!(!path.exists());
}

fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn git_lists_worktree(repo_dir: &Path, path: &Path) -> bool {
    let listed = git(repo_dir, &["worktree", "list", "--porcelain"]);
    let expected = format!("worktree {}", path.display());
    String::from_utf8_lossy(&listed.stdout)
        .lines()
        .any(|line| line == expected)
}

fn branch_exists(repo_dir: &Path, branch: &str) -> bool {
    Command::new("git")
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .current_dir(repo_dir)
        .output()
        .unwrap()
        .status
        .success()
}

#[test]
fn remove_worktree_after_its_directory_was_deleted() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    create_worktree(tmp.path(), "directory-deleted-by-hand");
    let path = worktree_path(tmp.path(), "directory-deleted-by-hand");
    std::fs::remove_dir_all(&path).unwrap();
    assert!(git_lists_worktree(tmp.path(), &path));

    let preview = trench_cmd(tmp.path())
        .args(["remove", "directory-deleted-by-hand", "--dry-run", "--json"])
        .output()
        .unwrap();
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(plan["directory_missing"], true);
    assert_eq!(plan["dirty"], false);
    assert!(git_lists_worktree(tmp.path(), &path));

    let removed = trench_cmd(tmp.path())
        .args(["remove", "directory-deleted-by-hand", "--yes", "--json"])
        .output()
        .unwrap();
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    let outcome: serde_json::Value = serde_json::from_slice(&removed.stdout).unwrap();
    assert_eq!(outcome["directory_missing"], true);
    assert_eq!(outcome["mutation_state"], "applied");
    assert_eq!(outcome["branch_deleted"], false);
    assert!(!git_lists_worktree(tmp.path(), &path));
    assert!(branch_exists(tmp.path(), "directory-deleted-by-hand"));
}

#[test]
fn remove_missing_directory_can_delete_the_local_branch() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    create_worktree(tmp.path(), "directory-and-branch-cleanup");
    let path = worktree_path(tmp.path(), "directory-and-branch-cleanup");
    std::fs::remove_dir_all(&path).unwrap();

    let removed = trench_cmd(tmp.path())
        .args([
            "remove",
            "directory-and-branch-cleanup",
            "--yes",
            "--delete-branch",
        ])
        .output()
        .unwrap();
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(!git_lists_worktree(tmp.path(), &path));
    assert!(!branch_exists(tmp.path(), "directory-and-branch-cleanup"));
}

#[test]
fn remove_missing_directory_skips_pre_remove_and_runs_post_remove() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    std::fs::write(
        tmp.path().join(".trench.toml"),
        "[hooks.pre_remove]\nrun = [\"touch pre-remove-ran\"]\n\n[hooks.post_remove]\nrun = [\"touch post-remove-ran\"]\n",
    )
    .unwrap();
    create_worktree(tmp.path(), "missing-directory-hooks");
    let path = worktree_path(tmp.path(), "missing-directory-hooks");
    std::fs::remove_dir_all(&path).unwrap();

    let removed = trench_cmd(tmp.path())
        .args(["remove", "missing-directory-hooks", "--yes", "--json"])
        .output()
        .unwrap();
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    let outcome: serde_json::Value = serde_json::from_slice(&removed.stdout).unwrap();
    assert!(outcome["warning"]
        .as_str()
        .is_some_and(|warning| warning.contains("pre_remove")));
    assert!(!tmp.path().join("pre-remove-ran").exists());
    assert!(!path.exists());
    assert!(tmp.path().join("post-remove-ran").exists());
    assert!(!git_lists_worktree(tmp.path(), &path));
}

#[test]
fn removing_a_missing_worktree_leaves_other_stale_entries_alone() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    create_worktree(tmp.path(), "stale-target");
    create_worktree(tmp.path(), "stale-bystander");
    let target = worktree_path(tmp.path(), "stale-target");
    let bystander = worktree_path(tmp.path(), "stale-bystander");
    std::fs::remove_dir_all(&target).unwrap();
    // The bystander is only away, as on an unmounted disk. It comes back.
    let parked = tmp.path().join("parked");
    std::fs::rename(&bystander, &parked).unwrap();

    let removed = trench_cmd(tmp.path())
        .args(["remove", "stale-target", "--yes"])
        .output()
        .unwrap();
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(!git_lists_worktree(tmp.path(), &target));
    assert!(
        git_lists_worktree(tmp.path(), &bystander),
        "an entry the user did not name was pruned"
    );

    std::fs::rename(&parked, &bystander).unwrap();
    git(&bystander, &["status"]);
}

#[test]
fn an_ambiguous_remove_selector_keeps_its_general_error_exit_code() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    let first = tmp.path().join("one").join("shared");
    let second = tmp.path().join("two").join("shared");
    for (path, branch) in [(&first, "first"), (&second, "second")] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        git(
            tmp.path(),
            &["worktree", "add", "-b", branch, path.to_str().unwrap()],
        );
    }

    let refused = trench_cmd(tmp.path())
        .args(["remove", "shared", "--yes"])
        .output()
        .unwrap();

    assert_eq!(refused.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.starts_with("error: more than one worktree matches 'shared'"),
        "{stderr}"
    );
    assert!(first.exists() && second.exists());
}

#[test]
fn a_failing_post_remove_hook_is_reported_once() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    std::fs::write(
        tmp.path().join(".trench.toml"),
        "[hooks.post_remove]\nrun = [\"false\"]\n",
    )
    .unwrap();
    create_worktree(tmp.path(), "hook-warns-once");

    let removed = trench_cmd(tmp.path())
        .args(["remove", "hook-warns-once", "--yes", "--force-worktree"])
        .output()
        .unwrap();

    assert!(removed.status.success());
    let stderr = String::from_utf8_lossy(&removed.stderr);
    assert_eq!(
        stderr.matches("warning: post_remove hook failed").count(),
        1,
        "{stderr}"
    );
}

#[test]
fn remove_refuses_locked_worktree_without_touching_it() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    create_worktree(tmp.path(), "locked-stays-intact");
    let path = worktree_path(tmp.path(), "locked-stays-intact");
    std::fs::write(path.join("ignored-work"), "keep").unwrap();
    git(tmp.path(), &["worktree", "lock", path.to_str().unwrap()]);

    for arguments in [
        vec!["remove", "locked-stays-intact", "--yes", "--force-worktree"],
        vec![
            "remove",
            "locked-stays-intact",
            "--dry-run",
            "--force-worktree",
        ],
    ] {
        let refused = trench_cmd(tmp.path()).args(&arguments).output().unwrap();
        assert_eq!(refused.status.code(), Some(1), "{arguments:?}");
        let stderr = String::from_utf8_lossy(&refused.stderr);
        assert!(stderr.contains("locked"), "{stderr}");
        assert!(stderr.contains("git worktree unlock"), "{stderr}");
        assert!(path.join("ignored-work").exists());
        assert!(git_lists_worktree(tmp.path(), &path));
    }

    git(tmp.path(), &["worktree", "unlock", path.to_str().unwrap()]);
    let removed = trench_cmd(tmp.path())
        .args(["remove", "locked-stays-intact", "--yes", "--force-worktree"])
        .output()
        .unwrap();
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(!path.exists());
}

#[test]
fn remove_refuses_locked_worktree_whose_directory_is_missing() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    create_worktree(tmp.path(), "locked-and-missing");
    let path = worktree_path(tmp.path(), "locked-and-missing");
    git(tmp.path(), &["worktree", "lock", path.to_str().unwrap()]);
    std::fs::remove_dir_all(&path).unwrap();

    let refused = trench_cmd(tmp.path())
        .args(["remove", "locked-and-missing", "--yes"])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("locked"), "{stderr}");
    assert!(git_lists_worktree(tmp.path(), &path));

    git(tmp.path(), &["worktree", "unlock", path.to_str().unwrap()]);
    let removed = trench_cmd(tmp.path())
        .args(["remove", "locked-and-missing", "--yes"])
        .output()
        .unwrap();
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(!git_lists_worktree(tmp.path(), &path));
}

#[test]
fn delete_branch_is_refused_while_another_worktree_has_the_branch_checked_out() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    create_worktree(tmp.path(), "shared-branch");
    let path = worktree_path(tmp.path(), "shared-branch");
    let other = tempfile::tempdir().unwrap();
    let other_path = other.path().join("second-checkout");
    git(
        tmp.path(),
        &[
            "worktree",
            "add",
            "--force",
            other_path.to_str().unwrap(),
            "shared-branch",
        ],
    );
    let target = path.to_str().unwrap();

    for arguments in [
        vec!["remove", target, "--dry-run", "--delete-branch"],
        vec!["remove", target, "--yes", "--delete-branch"],
    ] {
        let refused = trench_cmd(tmp.path()).args(&arguments).output().unwrap();
        assert_eq!(refused.status.code(), Some(1), "{arguments:?}");
        let stderr = String::from_utf8_lossy(&refused.stderr);
        assert!(stderr.contains("also checked out"), "{stderr}");
        assert!(stderr.contains("second-checkout"), "{stderr}");
        assert!(path.exists());
    }

    let removed = trench_cmd(tmp.path())
        .args(["remove", target, "--yes"])
        .output()
        .unwrap();
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(!path.exists());
    assert!(branch_exists(tmp.path(), "shared-branch"));
}

#[test]
fn remove_keeps_the_repository_readable_when_relative_worktree_paths_are_enabled() {
    let version = Command::new("git").arg("--version").output().unwrap();
    let version = String::from_utf8_lossy(&version.stdout);
    let mut numbers = version
        .split_whitespace()
        .nth(2)
        .unwrap_or_default()
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or(0));
    let (major, minor) = (numbers.next().unwrap_or(0), numbers.next().unwrap_or(0));
    if (major, minor) < (2, 48) {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    create_worktree(tmp.path(), "relative-paths-config");
    let path = worktree_path(tmp.path(), "relative-paths-config");
    git(tmp.path(), &["config", "worktree.useRelativePaths", "true"]);

    let removed = trench_cmd(tmp.path())
        .args(["remove", "relative-paths-config", "--yes"])
        .output()
        .unwrap();
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(!path.exists());
    let extension = Command::new("git")
        .args(["config", "--get", "extensions.relativeworktrees"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(!extension.status.success());
    let list = trench_cmd(tmp.path()).arg("list").output().unwrap();
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
}

#[test]
fn missing_remote_base_requires_force_to_delete_local_branch() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    create_worktree(tmp.path(), "missing-remote-base");
    let path = worktree_path(tmp.path(), "missing-remote-base");
    std::fs::write(
        tmp.path().join(".trench.toml"),
        "[git]\ndefault_base = \"origin/main\"\n",
    )
    .unwrap();

    let refused = trench_cmd(tmp.path())
        .args(["remove", "missing-remote-base", "--yes", "--delete-branch"])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(path.exists());

    let removed = trench_cmd(tmp.path())
        .args([
            "remove",
            "missing-remote-base",
            "--yes",
            "--delete-branch",
            "--force-branch",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    let outcome: serde_json::Value = serde_json::from_slice(&removed.stdout).unwrap();
    assert_eq!(outcome["merged"], serde_json::Value::Null);
    assert_eq!(outcome["force_branch_applied"], true);
    assert_eq!(outcome["branch_deleted"], true);
    assert!(!path.exists());
    let branch = Command::new("git")
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/missing-remote-base",
        ])
        .current_dir(tmp.path())
        .status()
        .unwrap();
    assert!(!branch.success());
}

#[test]
fn exit_code_8_remove_without_yes_outside_interactive_terminal() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    create_worktree(tmp.path(), "needs-force");

    let output = trench_cmd(tmp.path())
        .args(["remove", "needs-force"])
        .output()
        .expect("failed to run trench remove without force");

    assert_eq!(
        output.status.code(),
        Some(8),
        "remove without --yes outside interactive terminal should exit 8, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn yes_does_not_authorize_dirty_worktree_removal() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    create_worktree(tmp.path(), "dirty-needs-force-worktree");
    let path = worktree_path(tmp.path(), "dirty-needs-force-worktree");
    std::fs::write(path.join("untracked"), "preserve").unwrap();

    let refused = trench_cmd(tmp.path())
        .args(["remove", "dirty-needs-force-worktree", "--yes"])
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(1));
    assert!(path.join("untracked").exists());

    let removed = trench_cmd(tmp.path())
        .args([
            "remove",
            "dirty-needs-force-worktree",
            "--yes",
            "--force-worktree",
        ])
        .output()
        .unwrap();
    assert!(removed.status.success());
    assert!(!path.exists());
}

#[test]
fn force_branch_without_delete_branch_is_a_flag_conflict() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    create_worktree(tmp.path(), "force-branch-conflict");

    let output = trench_cmd(tmp.path())
        .args(["remove", "force-branch-conflict", "--yes", "--force-branch"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(9));
}

#[test]
fn switch_print_path_keeps_stdout_raw_and_suppresses_the_direct_use_hint() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    create_worktree(tmp.path(), "switch-print-path");

    let list_output = trench_cmd(tmp.path())
        .args(["list", "--json"])
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
        .find(|item| item["worktree"] == "switch-print-path")
        .and_then(|item| item["path"].as_str())
        .expect("should find worktree path")
        .to_string();

    let output = trench_cmd(tmp.path())
        .args(["switch", "switch-print-path", "--print-path"])
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

    assert!(output.stderr.is_empty());
}
