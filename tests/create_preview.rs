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
    std::fs::write(repo.join("README.md"), "create preview test\n").unwrap();
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
fn json_preview_exposes_the_flat_create_contract() {
    let repo = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let root = outside.path().join("worktrees");
    init_repo(repo.path());
    write_project_config(repo.path(), &root, "");

    let output = trench(
        repo.path(),
        xdg.path(),
        &["create", "feature/auth", "--dry-run", "--json"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let repository = repo.path().file_name().unwrap().to_string_lossy();
    let expected_path = root.join(repository.as_ref()).join("feature-auth");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        json,
        serde_json::json!({
            "dry_run": true,
            "action": "new_branch",
            "branch": "feature/auth",
            "worktree": "feature-auth",
            "path": expected_path,
            "base": "main",
            "tracking": null,
            "hook_policy": "run"
        })
    );
}

#[test]
fn preview_does_not_create_state_run_hooks_mutate_git_or_fetch() {
    let repo = tempfile::tempdir().unwrap();
    let remote_dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let root = outside.path().join("missing-worktrees");
    let marker = outside.path().join("hook-ran");
    init_repo(repo.path());

    git2::Repository::init_bare(remote_dir.path()).unwrap();
    let local = git2::Repository::open(repo.path()).unwrap();
    local
        .remote("origin", remote_dir.path().to_str().unwrap())
        .unwrap();
    let head = local.head().unwrap().target().unwrap();
    drop(local);
    git(repo.path(), &["push", "-u", "origin", "main"]);
    let remote = git2::Repository::open_bare(remote_dir.path()).unwrap();
    remote
        .reference("refs/heads/unfetched", head, false, "preview test")
        .unwrap();
    drop(remote);

    write_project_config(
        repo.path(),
        &root,
        &format!(
            "\n[hooks.pre_create]\nshell = {:?}\n\n[hooks.post_create]\nshell = {:?}\n",
            format!("touch {}", marker.display()),
            format!("touch {}", marker.display())
        ),
    );
    let refs_before = git_stdout(
        repo.path(),
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    );
    let worktrees_before = git_stdout(repo.path(), &["worktree", "list", "--porcelain"]);

    let output = trench(
        repo.path(),
        xdg.path(),
        &["create", "feature/new", "--dry-run", "--json"],
    );

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        git_stdout(
            repo.path(),
            &["for-each-ref", "--format=%(refname) %(objectname)"],
        ),
        refs_before
    );
    assert_eq!(
        git_stdout(repo.path(), &["worktree", "list", "--porcelain"]),
        worktrees_before
    );
    assert!(!root.exists(), "preview created the worktree root");
    assert!(!marker.exists(), "preview ran a lifecycle hook");
    for directory in ["config", "data", "state", "cache"] {
        assert!(
            !xdg.path().join(directory).exists(),
            "preview created the XDG {directory} directory"
        );
    }
    assert!(
        !repo
            .path()
            .join(".git/refs/remotes/origin/unfetched")
            .exists(),
        "preview fetched origin"
    );
}

#[test]
fn preview_classifies_local_remote_and_checked_out_refs() {
    let repo = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let root = outside.path().join("worktrees");
    init_repo(repo.path());
    write_project_config(repo.path(), &root, "");
    git(repo.path(), &["branch", "release"]);
    let head = git_stdout(repo.path(), &["rev-parse", "HEAD"]);
    git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/topic", head.trim()],
    );

    let local = trench(
        repo.path(),
        xdg.path(),
        &["create", "release", "--dry-run", "--json"],
    );
    let remote = trench(
        repo.path(),
        xdg.path(),
        &["create", "origin/topic", "--dry-run", "--json"],
    );
    assert!(local.status.success());
    assert!(remote.status.success());
    let local: serde_json::Value = serde_json::from_slice(&local.stdout).unwrap();
    let remote: serde_json::Value = serde_json::from_slice(&remote.stdout).unwrap();
    assert_eq!(local["action"], "existing_local");
    assert_eq!(local["base"], serde_json::Value::Null);
    assert_eq!(remote["action"], "track_remote");
    assert_eq!(remote["branch"], "topic");
    assert_eq!(remote["tracking"], "origin/topic");

    let repository = repo.path().file_name().unwrap().to_string_lossy();
    let target = root.join(repository.as_ref()).join("feature-auth");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    git(
        repo.path(),
        &[
            "worktree",
            "add",
            "-b",
            "feature/auth",
            target.to_str().unwrap(),
        ],
    );
    let navigate = trench(
        repo.path(),
        xdg.path(),
        &["create", "feature/auth", "--dry-run", "--json"],
    );
    assert!(
        navigate.status.success(),
        "{}",
        String::from_utf8_lossy(&navigate.stderr)
    );
    let navigate: serde_json::Value = serde_json::from_slice(&navigate.stdout).unwrap();
    assert_eq!(navigate["action"], "navigate");
    assert_eq!(navigate["worktree"], "feature-auth");
}
