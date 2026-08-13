use std::path::Path;
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

fn git_stdout(repo: &Path, args: &[&str]) -> String {
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

fn init_repo(repo: &Path) {
    git(repo, &["init", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "create execution test\n").unwrap();
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

fn write_project_config(repo: &Path, root: &Path, extra: &str) {
    std::fs::write(
        repo.join(".trench.toml"),
        format!("[worktrees]\nroot = {:?}\n{extra}", root.to_string_lossy()),
    )
    .unwrap();
}

fn trench(repo: &Path, xdg: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trench"))
        .current_dir(repo)
        .env("XDG_CONFIG_HOME", xdg.join("config"))
        .env("XDG_DATA_HOME", xdg.join("data"))
        .env("XDG_STATE_HOME", xdg.join("state"))
        .env("XDG_CACHE_HOME", xdg.join("cache"))
        .args(args)
        .output()
        .expect("trench should run")
}

#[test]
fn creates_the_exact_planned_new_branch_without_product_state() {
    let repo = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let root = outside.path().join("worktrees");
    init_repo(repo.path());
    write_project_config(repo.path(), &root, "");

    let preview = trench(
        repo.path(),
        xdg.path(),
        &["create", "feature/auth", "--dry-run", "--json"],
    );
    assert!(preview.status.success());
    let preview: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();

    let output = trench(
        repo.path(),
        xdg.path(),
        &["create", "feature/auth", "--json"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(result["dry_run"], false);
    assert_eq!(result["action"], preview["action"]);
    assert_eq!(result["branch"], preview["branch"]);
    assert_eq!(result["worktree"], preview["worktree"]);
    assert_eq!(result["path"], preview["path"]);
    assert_eq!(result["base"], preview["base"]);
    assert_eq!(result["tracking"], preview["tracking"]);
    assert_eq!(result["hook_policy"], preview["hook_policy"]);
    assert_eq!(result["mutation_state"], "applied");
    assert!(Path::new(result["path"].as_str().unwrap()).is_dir());
    assert_eq!(
        String::from_utf8_lossy(
            &Command::new("git")
                .current_dir(repo.path())
                .args(["branch", "--show-current"])
                .output()
                .unwrap()
                .stdout
        )
        .trim(),
        "main"
    );
    assert!(!xdg.path().join("data/trench/trench.db").exists());
}

#[test]
fn creates_a_worktree_for_an_existing_local_branch() {
    let repo = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let root = outside.path().join("worktrees");
    init_repo(repo.path());
    write_project_config(repo.path(), &root, "");
    git(repo.path(), &["branch", "release"]);

    let output = trench(repo.path(), xdg.path(), &["create", "release", "--json"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["action"], "existing_local");
    assert_eq!(result["branch"], "release");
    assert_eq!(result["mutation_state"], "applied");
    assert!(Path::new(result["path"].as_str().unwrap()).is_dir());
    assert_eq!(
        Command::new("git")
            .current_dir(result["path"].as_str().unwrap())
            .args(["branch", "--show-current"])
            .output()
            .unwrap()
            .stdout,
        b"release\n"
    );
}

#[test]
fn creates_a_local_tracking_branch_for_a_remote_only_ref() {
    let repo = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let root = outside.path().join("worktrees");
    init_repo(repo.path());
    write_project_config(repo.path(), &root, "");
    git(
        repo.path(),
        &["remote", "add", "origin", "unused-test-remote"],
    );
    git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/topic", "HEAD"],
    );

    let output = trench(
        repo.path(),
        xdg.path(),
        &["create", "origin/topic", "--json"],
    );

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["action"], "track_remote");
    assert_eq!(result["branch"], "topic");
    assert_eq!(result["tracking"], "origin/topic");
    assert_eq!(result["mutation_state"], "applied");
    assert_eq!(
        String::from_utf8_lossy(
            &Command::new("git")
                .current_dir(repo.path())
                .args([
                    "for-each-ref",
                    "--format=%(upstream:short)",
                    "refs/heads/topic"
                ])
                .output()
                .unwrap()
                .stdout
        )
        .trim(),
        "origin/topic"
    );
}

#[test]
fn already_checked_out_branch_navigates_without_mutation_or_hooks() {
    let repo = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let root = outside.path().join("worktrees");
    let hook_marker = outside.path().join("hook-ran");
    init_repo(repo.path());
    write_project_config(
        repo.path(),
        &root,
        &format!(
            "\n[hooks.pre_create]\nshell = {:?}\n\n[hooks.post_create]\nshell = {:?}\n",
            format!("touch {}", hook_marker.display()),
            format!("touch {}", hook_marker.display())
        ),
    );
    let refs_before = git_stdout(repo.path(), &["show-ref"]);
    let worktrees_before = git_stdout(repo.path(), &["worktree", "list", "--porcelain"]);

    let output = trench(repo.path(), xdg.path(), &["create", "main", "--json"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["action"], "navigate");
    let canonical_repo = repo.path().canonicalize().unwrap();
    assert_eq!(result["path"].as_str(), canonical_repo.to_str());
    assert_eq!(result["mutation_state"], "not_started");
    assert_eq!(git_stdout(repo.path(), &["show-ref"]), refs_before);
    assert_eq!(
        git_stdout(repo.path(), &["worktree", "list", "--porcelain"]),
        worktrees_before
    );
    assert!(!root.exists());
    assert!(!hook_marker.exists());
}
