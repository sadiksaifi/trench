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

#[test]
fn log_output_replays_hook_stdout_stderr() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    // Write .trench.toml with post_create hooks that produce output
    std::fs::write(
        tmp.path().join(".trench.toml"),
        r#"
[hooks.post_create]
run = ["echo hook_run_output"]
shell = "echo hook_shell_output >&2"
timeout_secs = 30
"#,
    )
    .unwrap();

    // Create a worktree — triggers post_create hook
    let create = Command::new(trench_bin())
        .args(["create", "output-test"])
        .current_dir(tmp.path())
        .output()
        .expect("create");
    assert!(
        create.status.success(),
        "trench create should succeed, stderr: {}",
        String::from_utf8_lossy(&create.stderr)
    );

    // Replay hook output via --output (table mode)
    let output = Command::new(trench_bin())
        .args(["log", "output-test", "--output", "--no-color"])
        .current_dir(tmp.path())
        .output()
        .expect("trench log --output");

    assert!(
        output.status.success(),
        "trench log --output should exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should contain actual hook output
    assert!(
        stdout.contains("hook_run_output"),
        "should contain run output, got: {stdout}"
    );
    assert!(
        stdout.contains("hook_shell_output"),
        "should contain shell output, got: {stdout}"
    );

    // Should contain step labels
    assert!(
        stdout.contains("[run]"),
        "should contain [run] step label, got: {stdout}"
    );
    assert!(
        stdout.contains("[shell]"),
        "should contain [shell] step label, got: {stdout}"
    );

    // Should contain event type header
    assert!(
        stdout.contains("hook:post_create"),
        "should contain event type, got: {stdout}"
    );
}

#[test]
fn log_output_json_returns_structured_output() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    // Write .trench.toml with hooks that produce output
    std::fs::write(
        tmp.path().join(".trench.toml"),
        r#"
[hooks.post_create]
run = ["echo json_test_output"]
timeout_secs = 30
"#,
    )
    .unwrap();

    // Create a worktree
    let create = Command::new(trench_bin())
        .args(["create", "json-output-test"])
        .current_dir(tmp.path())
        .output()
        .expect("create");
    assert!(
        create.status.success(),
        "trench create should succeed, stderr: {}",
        String::from_utf8_lossy(&create.stderr)
    );

    // Replay hook output via --output --json
    let output = Command::new(trench_bin())
        .args(["log", "json-output-test", "--output", "--json"])
        .current_dir(tmp.path())
        .output()
        .expect("trench log --output --json");

    assert!(
        output.status.success(),
        "trench log --output --json should exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");

    assert_eq!(parsed["event_type"], "hook:post_create");
    assert_eq!(parsed["exit_code"], 0);
    assert!(parsed["duration_secs"].is_number());
    assert!(parsed["timestamp"].is_string());

    let lines = parsed["lines"].as_array().expect("lines array");
    assert!(!lines.is_empty(), "should have at least one output line");

    // Find the line containing our output
    let has_output = lines.iter().any(|l| l["line"].as_str() == Some("json_test_output"));
    assert!(has_output, "should contain our hook output, got: {parsed}");

    // Check step label
    let run_line = lines.iter().find(|l| l["line"].as_str() == Some("json_test_output")).unwrap();
    assert_eq!(run_line["step"], "run");
    assert_eq!(run_line["stream"], "stdout");
}

#[test]
fn log_output_without_branch_exits_8() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    let output = Command::new(trench_bin())
        .args(["log", "--output"])
        .current_dir(tmp.path())
        .output()
        .expect("trench log --output");

    assert_eq!(
        exit_code(output.status),
        8,
        "should exit 8 when --output used without branch, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn log_output_no_hooks_exits_2() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    // Create a worktree without hooks
    let create = Command::new(trench_bin())
        .args(["create", "no-hooks-test", "--no-hooks"])
        .current_dir(tmp.path())
        .output()
        .expect("create");
    assert!(create.status.success());

    // Try to replay output — should fail since no hook events
    let output = Command::new(trench_bin())
        .args(["log", "no-hooks-test", "--output"])
        .current_dir(tmp.path())
        .output()
        .expect("trench log --output");

    assert_eq!(
        exit_code(output.status),
        2,
        "should exit 2 when no hook output, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn log_summary_empty_state_shows_no_events() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    let output = Command::new(trench_bin())
        .args(["log", "--summary"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench log --summary");

    assert!(
        output.status.success(),
        "trench log --summary should exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("No events"),
        "should show empty state message, got: {stdout}"
    );
}

#[test]
fn log_summary_json_empty_state_returns_zeroed_stats() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    let output = Command::new(trench_bin())
        .args(["log", "--summary", "--json"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run trench log --summary --json");

    assert!(
        output.status.success(),
        "should exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("should be valid JSON");

    assert_eq!(parsed["total_events"], 0);
    assert_eq!(parsed["hook_runs"], 0);
    assert_eq!(parsed["avg_hook_duration_secs"], 0.0);
    assert_eq!(parsed["successes"], 0);
    assert_eq!(parsed["failures"], 0);
    assert!(parsed["most_active_worktree"].is_null());
}

#[test]
fn log_summary_shows_accurate_stats_after_events() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    // Create a .trench.toml with a post_create hook so hook events get recorded
    std::fs::write(
        tmp.path().join(".trench.toml"),
        r#"
[hooks.post_create]
run = ["echo hello"]
"#,
    )
    .unwrap();
    git(tmp.path(), &["add", "."]);
    git(tmp.path(), &["commit", "-m", "add trench config"]);

    // Create two worktrees (each generates "created" + "hook:post_create" events)
    let create1 = Command::new(trench_bin())
        .args(["create", "summary-feat-1"])
        .current_dir(tmp.path())
        .output()
        .expect("create 1");
    assert!(
        create1.status.success(),
        "create 1 failed: {}",
        String::from_utf8_lossy(&create1.stderr)
    );

    let create2 = Command::new(trench_bin())
        .args(["create", "summary-feat-2"])
        .current_dir(tmp.path())
        .output()
        .expect("create 2");
    assert!(
        create2.status.success(),
        "create 2 failed: {}",
        String::from_utf8_lossy(&create2.stderr)
    );

    // Get JSON summary
    let summary_output = Command::new(trench_bin())
        .args(["log", "--summary", "--json"])
        .current_dir(tmp.path())
        .output()
        .expect("summary");
    assert!(
        summary_output.status.success(),
        "summary failed: {}",
        String::from_utf8_lossy(&summary_output.stderr)
    );

    let stdout = String::from_utf8_lossy(&summary_output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("should be valid JSON");

    // Should have at least 4 events (2 created + 2 hook:post_create)
    let total = parsed["total_events"].as_u64().unwrap();
    assert!(
        total >= 4,
        "should have at least 4 events, got {total}"
    );

    // Should have at least 2 hook runs
    let hooks = parsed["hook_runs"].as_u64().unwrap();
    assert!(
        hooks >= 2,
        "should have at least 2 hook runs, got {hooks}"
    );

    // Hook duration should be > 0
    let avg = parsed["avg_hook_duration_secs"].as_f64().unwrap();
    assert!(
        avg >= 0.0,
        "avg_hook_duration_secs should be non-negative, got {avg}"
    );

    // Successes should be >= 2 (both hooks succeeded)
    let successes = parsed["successes"].as_u64().unwrap();
    assert!(
        successes >= 2,
        "should have at least 2 successes, got {successes}"
    );

    assert_eq!(parsed["failures"], 0, "no hook failures expected");

    // Most active worktree should be present
    assert!(
        parsed["most_active_worktree"].is_object(),
        "most_active_worktree should be an object"
    );
    assert!(
        parsed["most_active_worktree"]["name"].is_string(),
        "most_active_worktree.name should be a string"
    );
    assert!(
        parsed["most_active_worktree"]["event_count"].as_u64().unwrap() >= 2,
        "most_active_worktree should have at least 2 events"
    );

    // Also verify human-readable output has the expected labels
    let human_output = Command::new(trench_bin())
        .args(["log", "--summary"])
        .current_dir(tmp.path())
        .output()
        .expect("human summary");
    assert!(human_output.status.success());

    let human_stdout = String::from_utf8_lossy(&human_output.stdout);
    assert!(human_stdout.contains("Total events:"), "should have Total events label");
    assert!(human_stdout.contains("Hook runs:"), "should have Hook runs label");
    assert!(human_stdout.contains("Avg hook duration:"), "should have Avg hook duration label");
    assert!(human_stdout.contains("Successes:"), "should have Successes label");
    assert!(human_stdout.contains("Failures:"), "should have Failures label");
    assert!(human_stdout.contains("Most active:"), "should have Most active label");
}

#[test]
fn log_summary_and_output_conflict() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    let output = Command::new(trench_bin())
        .args(["log", "--summary", "--output"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run");

    assert_eq!(
        exit_code(output.status),
        9,
        "should exit 9 for conflicting flags, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--summary") && stderr.contains("--output"),
        "should mention both flags in error: {stderr}"
    );
}
