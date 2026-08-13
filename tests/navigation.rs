use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn trench_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_trench"))
}

fn trench(dir: &Path) -> Command {
    let xdg = dir.join(".xdg");
    let mut command = Command::new(trench_bin());
    command
        .current_dir(dir)
        .env("XDG_CONFIG_HOME", xdg.join("config"))
        .env("XDG_DATA_HOME", xdg.join("data"))
        .env("XDG_STATE_HOME", xdg.join("state"))
        .env("XDG_CACHE_HOME", xdg.join("cache"));
    command
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_repo(path: &Path) {
    git(path, &["init", "-b", "main"]);
    git(path, &["config", "user.name", "Test"]);
    git(path, &["config", "user.email", "test@example.com"]);
    fs::write(path.join("README.md"), "test\n").unwrap();
    git(path, &["add", "README.md"]);
    git(path, &["commit", "-m", "init"]);
}

fn add_worktree(repository: &Path, root: &Path, branch: &str) -> PathBuf {
    let path = root.join(branch.replace('/', "-"));
    git(
        repository,
        &["worktree", "add", "-b", branch, path.to_str().unwrap()],
    );
    path.canonicalize().unwrap()
}

fn executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn direct_switch_prints_a_composable_path_and_parent_shell_hint_without_state() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repository");
    let worktrees = root.path().join("worktrees");
    fs::create_dir_all(&repository).unwrap();
    fs::create_dir_all(&worktrees).unwrap();
    init_repo(&repository);
    let path = add_worktree(&repository, &worktrees, "feature/auth");

    let output = trench(&repository)
        .args(["switch", "feature/auth"])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}\n", path.display())
    );
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "hint: a child process cannot change its parent shell; use `tn switch <worktree>` to cd\n"
    );
    assert!(!repository.join(".xdg/data/trench/trench.db").exists());
}

#[test]
fn internal_switch_path_mode_suppresses_the_direct_use_hint() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repository");
    let worktrees = root.path().join("worktrees");
    fs::create_dir_all(&repository).unwrap();
    fs::create_dir_all(&worktrees).unwrap();
    init_repo(&repository);
    let path = add_worktree(&repository, &worktrees, "feature/auth");

    let output = trench(&repository)
        .args(["switch", "feature-auth", "--print-path"])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}\n", path.display())
    );
    assert!(output.stderr.is_empty());

    let help = trench(&repository)
        .args(["switch", "--help"])
        .output()
        .unwrap();
    assert!(!String::from_utf8(help.stdout)
        .unwrap()
        .contains("--print-path"));
}

fn configured_editor_fixture(exit_code: i32) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("work tree");
    fs::create_dir(&repository).unwrap();
    init_repo(&repository);
    let editor = root.path().join("fake-editor");
    let argv = root.path().join("argv");
    executable(
        &editor,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit {exit_code}\n",
            argv.display()
        ),
    );
    fs::write(
        repository.join(".trench.toml"),
        format!(
            "[editor]\ncommand = \"{} --profile 'Work Trees' --literal='two words'\"\n",
            editor.display()
        ),
    )
    .unwrap();
    (root, repository, argv)
}

#[test]
fn open_preserves_configured_argv_appends_the_path_and_waits() {
    let (_root, repository, argv) = configured_editor_fixture(0);
    let selector = repository.file_name().unwrap().to_str().unwrap();

    let output = trench(&repository)
        .args(["open", selector])
        .env("EDITOR", "must-not-run")
        .env("VISUAL", "must-not-run")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "open failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read_to_string(argv).unwrap(),
        format!(
            "--profile\nWork Trees\n--literal=two words\n{}\n",
            repository.canonicalize().unwrap().display()
        )
    );
    assert!(!repository.join(".xdg/data/trench/trench.db").exists());
}

#[test]
fn open_propagates_a_nonzero_editor_exit() {
    let (_root, repository, argv) = configured_editor_fixture(17);
    let selector = repository.file_name().unwrap().to_str().unwrap();

    let output = trench(&repository)
        .args(["open", selector])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(argv.exists(), "the editor must run before open returns");
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("editor exited with status exit status: 17"));
}

fn shell_init(shell: &str, repository: &Path) -> Output {
    trench(repository)
        .args(["shell-init", shell])
        .output()
        .unwrap()
}

fn available(program: &str) -> bool {
    Command::new(program).arg("--version").output().is_ok()
}

#[test]
fn generated_wrappers_cd_in_the_parent_and_preserve_arguments_and_status() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repository");
    let target = root.path().join("target worktree");
    let bin = root.path().join("bin");
    fs::create_dir(&repository).unwrap();
    fs::create_dir(&target).unwrap();
    fs::create_dir(&bin).unwrap();
    init_repo(&repository);
    executable(
        &bin.join("trench"),
        "#!/bin/sh\n[ \"$1\" = switch ] && [ \"$2\" = --print-path ] && [ \"$3\" = 'feature name' ] || exit 41\nprintf '%s\\n' \"$FAKE_SWITCH_TARGET\"\nexit \"${FAKE_SWITCH_STATUS:-0}\"\n",
    );

    for shell in ["bash", "zsh", "fish"] {
        if !available(shell) {
            continue;
        }
        let definition = shell_init(shell, &repository);
        assert!(definition.status.success());
        let definition = String::from_utf8(definition.stdout).unwrap();
        let quoted_repository = shell_words::quote(repository.to_str().unwrap());
        let script = if shell == "fish" {
            format!(
                "{definition}\ncd -- {quoted_repository}\ntn switch 'feature name'\nset wrapper_status $status\nprintf '%s\\n%s\\n' $wrapper_status \"$PWD\""
            )
        } else {
            format!(
                "{definition}\ncd -- {quoted_repository}\ntn switch 'feature name'\nwrapper_status=$?\nprintf '%s\\n%s\\n' \"$wrapper_status\" \"$PWD\""
            )
        };
        let output = Command::new(shell)
            .args(["-c", &script])
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("FAKE_SWITCH_TARGET", &target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{shell} wrapper failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("0\n{}\n", target.display()),
            "{shell} did not cd in its parent function"
        );

        let failure_script = if shell == "fish" {
            format!(
                "{definition}\ncd -- {quoted_repository}\ntn switch 'feature name'\nset wrapper_status $status\nprintf '%s\\n%s\\n' $wrapper_status \"$PWD\""
            )
        } else {
            format!(
                "{definition}\ncd -- {quoted_repository}\ntn switch 'feature name'\nwrapper_status=$?\nprintf '%s\\n%s\\n' \"$wrapper_status\" \"$PWD\""
            )
        };
        let failure = Command::new(shell)
            .args(["-c", &failure_script])
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("FAKE_SWITCH_TARGET", &target)
            .env("FAKE_SWITCH_STATUS", "23")
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8(failure.stdout).unwrap(),
            format!("23\n{}\n", repository.display()),
            "{shell} did not preserve switch failure status and directory"
        );
    }
}
