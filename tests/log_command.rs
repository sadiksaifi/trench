//! Integration tests for `trench log` command.

use std::path::PathBuf;
use std::process::{Command, ExitStatus};

fn trench_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_trench"))
}

/// Helper to get the exit code from a status.
fn exit_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or(-1)
}

/// Run a git command in `dir`, panicking with stderr on failure.
fn git(dir: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("failed to run git {}: {e}", args[0]));
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args[0],
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Initialize a temporary git repo with an initial commit.
fn init_git_repo(dir: &std::path::Path) {
    git(dir, &["init", "-b", "main"]);
    git(dir, &["config", "user.email", "test@test.com"]);
    git(dir, &["config", "user.name", "Test"]);
    std::fs::write(dir.join("README.md"), "# test\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", "init"]);
}

#[test]
fn log_empty_state_shows_no_events() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    let output = Command::new(trench_bin())
        .args(["log"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench log");

    assert!(
        output.status.success(),
        "trench log should exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("No events"),
        "should show empty state message, got: {stdout}"
    );
}

#[test]
fn log_json_empty_state_shows_empty_array() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    let output = Command::new(trench_bin())
        .args(["log", "--json"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench log --json");

    assert!(
        output.status.success(),
        "trench log --json should exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "[]", "should output empty JSON array");
}

#[test]
fn log_shows_events_after_create_and_remove() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    // Create a worktree
    let create_output = Command::new(trench_bin())
        .args(["create", "log-test-feature", "--no-hooks"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench create");
    assert!(
        create_output.status.success(),
        "trench create should succeed, stderr: {}",
        String::from_utf8_lossy(&create_output.stderr)
    );

    // Remove the worktree
    let remove_output = Command::new(trench_bin())
        .args(["remove", "log-test-feature", "--force", "--no-hooks"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench remove");
    assert!(
        remove_output.status.success(),
        "trench remove should succeed, stderr: {}",
        String::from_utf8_lossy(&remove_output.stderr)
    );

    // Run trench log --json to get structured output
    let log_output = Command::new(trench_bin())
        .args(["log", "--json"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench log --json");
    assert!(
        log_output.status.success(),
        "trench log --json should exit 0, stderr: {}",
        String::from_utf8_lossy(&log_output.stderr)
    );

    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("should be valid JSON");
    let arr = parsed.as_array().expect("should be a JSON array");

    // Should have at least a "created" and "removed" event
    assert!(
        arr.len() >= 2,
        "should have at least 2 events (created + removed), got {}",
        arr.len()
    );

    let event_types: Vec<&str> = arr
        .iter()
        .filter_map(|e| e["event_type"].as_str())
        .collect();

    assert!(
        event_types.contains(&"created"),
        "should contain 'created' event, got: {:?}",
        event_types
    );
    assert!(
        event_types.contains(&"removed"),
        "should contain 'removed' event, got: {:?}",
        event_types
    );

    // Most recent first — "removed" should be before "created"
    let removed_idx = event_types.iter().position(|&t| t == "removed").unwrap();
    let created_idx = event_types.iter().position(|&t| t == "created").unwrap();
    assert!(
        removed_idx < created_idx,
        "removed should be before created (most recent first)"
    );

    // Each event should have a worktree (string or null for repo-level events)
    for event in arr {
        assert!(
            event["worktree"].is_string() || event["worktree"].is_null(),
            "worktree should be string or null, got: {}",
            event
        );
        assert!(
            event["timestamp"].is_string(),
            "each event should have a timestamp"
        );
        assert!(
            event["created_at"].is_number(),
            "each event should have created_at"
        );
    }
}

#[test]
fn log_table_output_after_create() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    // Create a worktree
    let create_output = Command::new(trench_bin())
        .args(["create", "log-table-test", "--no-hooks"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench create");
    assert!(
        create_output.status.success(),
        "trench create should succeed, stderr: {}",
        String::from_utf8_lossy(&create_output.stderr)
    );

    // Run trench log (table output, with --no-color to avoid ANSI)
    let log_output = Command::new(trench_bin())
        .args(["log", "--no-color"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench log");
    assert!(
        log_output.status.success(),
        "trench log should exit 0, stderr: {}",
        String::from_utf8_lossy(&log_output.stderr)
    );

    let stdout = String::from_utf8_lossy(&log_output.stdout);

    // Should have table headers
    assert!(stdout.contains("Timestamp"), "should have Timestamp header");
    assert!(stdout.contains("Type"), "should have Type header");
    assert!(stdout.contains("Worktree"), "should have Worktree header");

    // Should show the created event
    assert!(
        stdout.contains("created"),
        "should show created event, got: {stdout}"
    );
    assert!(
        stdout.contains("log-table-test"),
        "should show worktree name, got: {stdout}"
    );
}

#[test]
fn log_nonexistent_worktree_exits_2() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    // Create at least one worktree so the repo is tracked
    let create = Command::new(trench_bin())
        .args(["create", "real-branch", "--no-hooks"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench create");
    assert!(create.status.success());

    let output = Command::new(trench_bin())
        .args(["log", "nonexistent-branch"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench log");

    assert_eq!(
        exit_code(output.status),
        2,
        "should exit 2 for unknown worktree, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn log_scoped_to_worktree_filters_events() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    // Create two worktrees
    let out = Command::new(trench_bin())
        .args(["create", "alpha-branch", "--no-hooks"])
        .current_dir(tmp.path())
        .output()
        .expect("create alpha");
    assert!(out.status.success(), "trench create alpha failed: {}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(trench_bin())
        .args(["create", "beta-branch", "--no-hooks"])
        .current_dir(tmp.path())
        .output()
        .expect("create beta");
    assert!(out.status.success(), "trench create beta failed: {}", String::from_utf8_lossy(&out.stderr));

    // Log scoped to alpha — JSON output
    let output = Command::new(trench_bin())
        .args(["log", "alpha-branch", "--json"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench log alpha-branch --json");

    assert!(
        output.status.success(),
        "trench log alpha-branch --json should exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let arr = parsed.as_array().expect("array");

    // All events should be for alpha-branch
    for event in arr {
        assert_eq!(
            event["worktree"].as_str().unwrap_or(""),
            "alpha-branch",
            "scoped log should only contain alpha events"
        );
    }
}

#[test]
fn log_tail_limits_output() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    // Create and remove to generate multiple events
    let out = Command::new(trench_bin())
        .args(["create", "tail-test", "--no-hooks"])
        .current_dir(tmp.path())
        .output()
        .expect("create");
    assert!(out.status.success(), "trench create failed: {}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(trench_bin())
        .args(["remove", "tail-test", "--force", "--no-hooks"])
        .current_dir(tmp.path())
        .output()
        .expect("remove");
    assert!(out.status.success(), "trench remove failed: {}", String::from_utf8_lossy(&out.stderr));

    // tail 1 — JSON
    let output = Command::new(trench_bin())
        .args(["log", "--tail", "1", "--json"])
        .current_dir(tmp.path())
        .output()
        .expect("trench log --tail 1 --json");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let arr = parsed.as_array().expect("array");
    assert_eq!(arr.len(), 1, "tail 1 should return exactly 1 event");
}

#[test]
fn log_scoped_and_tail_combined() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    // Create two worktrees
    let out = Command::new(trench_bin())
        .args(["create", "combo-a", "--no-hooks"])
        .current_dir(tmp.path())
        .output()
        .expect("create combo-a");
    assert!(out.status.success(), "trench create combo-a failed: {}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(trench_bin())
        .args(["create", "combo-b", "--no-hooks"])
        .current_dir(tmp.path())
        .output()
        .expect("create combo-b");
    assert!(out.status.success(), "trench create combo-b failed: {}", String::from_utf8_lossy(&out.stderr));

    // Remove combo-a to generate more events for it
    let out = Command::new(trench_bin())
        .args(["remove", "combo-a", "--force", "--no-hooks"])
        .current_dir(tmp.path())
        .output()
        .expect("remove combo-a");
    assert!(out.status.success(), "trench remove combo-a failed: {}", String::from_utf8_lossy(&out.stderr));

    // combo-a should have at least 2 events (created + removed), tail to 1
    let output = Command::new(trench_bin())
        .args(["log", "combo-a", "--tail", "1", "--json"])
        .current_dir(tmp.path())
        .output()
        .expect("trench log combo-a --tail 1 --json");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let arr = parsed.as_array().expect("array");
    assert_eq!(arr.len(), 1, "combined filter should return 1 event");
    assert_eq!(
        arr[0]["worktree"].as_str().unwrap_or(""),
        "combo-a",
        "event should be for combo-a"
    );
}
