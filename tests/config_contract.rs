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
    std::fs::write(repo.join("README.md"), "trench config test\n").unwrap();
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

fn trench(repo: &Path, xdg_root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trench"))
        .current_dir(repo)
        .env("XDG_CONFIG_HOME", xdg_root.join("config"))
        .env("XDG_DATA_HOME", xdg_root.join("data"))
        .env("XDG_STATE_HOME", xdg_root.join("state"))
        .env("XDG_CACHE_HOME", xdg_root.join("cache"))
        .args(args)
        .output()
        .expect("trench should run")
}

fn global_config_path(xdg_root: &Path) -> PathBuf {
    xdg_root.join("config/trench/config.toml")
}

#[test]
fn global_config_defaults_to_home_dot_config_when_xdg_home_is_unset() {
    let repo = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    init_repo(repo.path());

    let config_path = home.path().join(".config/trench/config.toml");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, "[git]\nauto_prune = true\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_trench"))
        .current_dir(repo.path())
        .env("HOME", home.path())
        .env_remove("XDG_CONFIG_HOME")
        .env("XDG_STATE_HOME", home.path().join(".local/state"))
        .args(["list", "--json"])
        .output()
        .expect("trench should run");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(6), "stderr: {stderr}");
    assert!(
        stderr.contains(&config_path.display().to_string()),
        "{stderr}"
    );
    assert!(stderr.contains("auto_prune"), "{stderr}");
}

#[test]
fn valid_minimal_global_and_project_configs_are_accepted() {
    let repo = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    init_repo(repo.path());

    let global_path = global_config_path(xdg.path());
    std::fs::create_dir_all(global_path.parent().unwrap()).unwrap();
    std::fs::write(
        &global_path,
        r#"
[ui]
theme = "ops"

[git]
default_base = "main"

[editor]
command = "vi"

[worktrees]
root = "~/.worktrees"

[hooks.post_create]
run = ["make setup"]
"#,
    )
    .unwrap();
    std::fs::write(
        repo.path().join(".trench.toml"),
        r#"
[ui]
theme = "gruvbox"

[hooks.pre_remove]
shell = "true"
"#,
    )
    .unwrap();

    let output = trench(repo.path(), xdg.path(), &["list", "--json"]);

    assert!(
        output.status.success(),
        "valid config should be accepted: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn unknown_global_key_exits_with_config_error_and_exact_diagnostic() {
    let repo = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    init_repo(repo.path());

    let global_path = global_config_path(xdg.path());
    std::fs::create_dir_all(global_path.parent().unwrap()).unwrap();
    std::fs::write(&global_path, "[git]\nauto_prune = true\n").unwrap();

    let output = trench(repo.path(), xdg.path(), &["list", "--json"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(6), "stderr: {stderr}");
    assert!(
        stderr.contains(&global_path.display().to_string()),
        "{stderr}"
    );
    assert!(stderr.contains("auto_prune"), "{stderr}");
}

#[test]
fn former_project_path_template_exits_with_exact_file_and_key() {
    let repo = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    init_repo(repo.path());

    let project_path = repo.path().join(".trench.toml");
    std::fs::write(
        &project_path,
        r#"[worktrees]
root = "{{ repo }}/{{ branch | sanitize }}"
"#,
    )
    .unwrap();

    let output = trench(repo.path(), xdg.path(), &["list", "--json"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(6), "stderr: {stderr}");
    assert!(
        stderr.contains(&project_path.display().to_string()),
        "{stderr}"
    );
    assert!(stderr.contains("worktrees.root"), "{stderr}");
}

#[test]
fn removed_config_controls_do_not_appear_in_cli_help() {
    let repo = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    init_repo(repo.path());

    for args in [&["--help"][..], &["switch", "--help"], &["open", "--help"]] {
        let output = trench(repo.path(), xdg.path(), args);
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.contains("tmux"), "help {args:?}: {stdout}");
        assert!(!stdout.contains("template"), "help {args:?}: {stdout}");
        assert!(!stdout.contains("auto-refresh"), "help {args:?}: {stdout}");
    }
}
