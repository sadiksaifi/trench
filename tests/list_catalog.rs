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

fn output_json(output: &Output) -> Vec<serde_json::Value> {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("list should return JSON records")
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
    let records = output_json(&output);
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

#[test]
fn list_json_reports_only_identity_fields() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("status-repo");
    let linked = root.path().join("feature-status");
    let xdg = root.path().join("xdg");
    std::fs::create_dir(&repo).unwrap();
    init_repo(&repo);
    add_worktree(&repo, &linked, "feature/status");
    std::fs::write(linked.join("README.md"), "worktree change\n").unwrap();
    std::fs::write(linked.join("staged.txt"), "staged\n").unwrap();
    git(&linked, &["add", "staged.txt"]);
    std::fs::write(linked.join("untracked.txt"), "untracked\n").unwrap();
    git(
        &linked,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "feature commit",
            "staged.txt",
        ],
    );
    std::fs::write(linked.join("staged-two.txt"), "staged\n").unwrap();
    git(&linked, &["add", "staged-two.txt"]);
    std::fs::write(
        repo.join(".trench.toml"),
        "[git]\ndefault_base = \"main\"\n",
    )
    .unwrap();

    let records = output_json(&trench(&linked, &xdg, &["list", "--json"]));
    let linked_record = records
        .iter()
        .find(|record| record["worktree"] == "feature-status")
        .unwrap();
    let expected_keys = [
        "branch",
        "detached",
        "is_current",
        "is_main",
        "path",
        "worktree",
    ];
    let mut actual_keys: Vec<_> = linked_record.as_object().unwrap().keys().cloned().collect();
    actual_keys.sort();
    assert_eq!(actual_keys, expected_keys);
}

#[test]
fn list_reports_detached_worktree_with_stable_identity_and_porcelain_shape() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("detached-repo");
    let detached = root.path().join("detached-checkout");
    let xdg = root.path().join("xdg");
    std::fs::create_dir(&repo).unwrap();
    init_repo(&repo);
    git(
        &repo,
        &[
            "worktree",
            "add",
            "--detach",
            detached.to_str().unwrap(),
            "HEAD",
        ],
    );
    let head = String::from_utf8(
        Command::new("git")
            .current_dir(&detached)
            .args(["rev-parse", "--short=7", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let records = output_json(&trench(&detached, &xdg, &["list", "--json"]));
    let record = &records[0];
    assert_eq!(record["worktree"], format!("detached@{head}"));
    assert!(record["branch"].is_null());
    assert_eq!(record["detached"], true);
    assert_eq!(record["is_current"], true);

    let porcelain = trench(&detached, &xdg, &["list", "--porcelain"]);
    assert!(porcelain.status.success());
    let stdout = String::from_utf8(porcelain.stdout).unwrap();
    let line = stdout.lines().next().unwrap();
    assert_eq!(line.split(':').count(), 3);
    assert!(
        line.starts_with(&format!("detached@{head}:(detached):")),
        "{line}"
    );
}

#[test]
fn piped_human_list_prints_branch_and_absolute_path_without_reshaping() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("human-repo");
    let linked = root
        .path()
        .join("feature-with-a-long-name-that-would-not-fit-in-a-narrow-terminal");
    let gone = root.path().join("deleted-by-hand");
    let xdg = root.path().join("xdg");
    std::fs::create_dir(&repo).unwrap();
    init_repo(&repo);
    add_worktree(&repo, &linked, "feature/with-a-long-name");
    add_worktree(&repo, &gone, "stale/gone");
    std::fs::remove_dir_all(&gone).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_trench"))
        .current_dir(&repo)
        .env("HOME", root.path())
        .env("XDG_CONFIG_HOME", xdg.join("config"))
        .env("XDG_DATA_HOME", xdg.join("data"))
        .env("XDG_STATE_HOME", xdg.join("state"))
        .env("XDG_CACHE_HOME", xdg.join("cache"))
        .env("COLUMNS", "40")
        .arg("list")
        .output()
        .expect("trench should run");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let gone = canonical(root.path()).join("deleted-by-hand");
    assert_eq!(
        stdout,
        format!(
            "  BRANCH                    PATH\n\
             * main                      {}\n\
             \x20 stale/gone                {}  (missing)\n\
             \x20 feature/with-a-long-name  {}\n",
            canonical(&repo).display(),
            gone.display(),
            canonical(&linked).display(),
        )
    );
    assert!(!stdout.contains('\x1b'));
    assert!(output.stderr.is_empty());
}

#[test]
fn list_outside_repository_has_direct_error_and_actionable_hint() {
    let root = tempfile::tempdir().unwrap();
    let xdg = root.path().join("xdg");

    let output = trench(root.path(), &xdg, &["list"]);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!output.status.success());
    assert!(stderr.contains("not a git repository"), "{stderr}");
    assert!(
        stderr.contains("hint: run trench inside a Git repository"),
        "{stderr}"
    );
    assert!(!stderr.contains("select"), "{stderr}");
}

#[test]
fn list_ignores_invalid_configuration() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let xdg = root.path().join("xdg");
    std::fs::create_dir(&repo).unwrap();
    init_repo(&repo);
    std::fs::write(repo.join(".trench.toml"), "invalid = [").unwrap();
    std::fs::create_dir_all(xdg.join("config/trench")).unwrap();
    std::fs::write(xdg.join("config/trench/config.toml"), "invalid = [").unwrap();
    for args in [
        vec!["list"],
        vec!["list", "--json"],
        vec!["list", "--porcelain"],
    ] {
        let output = trench(&repo, &xdg, &args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
