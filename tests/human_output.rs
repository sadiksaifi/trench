//! Process tests for human-readable CLI output: help text, the single error
//! format, and what each command says on success and failure.
//!
//! Structured output (`--json`, `--porcelain`) is covered by the per-command
//! test files and must not change when this wording does.

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

struct Sandbox {
    root: tempfile::TempDir,
}

impl Sandbox {
    /// A repository with one commit and a worktree root outside it.
    fn new(extra_config: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let sandbox = Self { root };
        std::fs::create_dir(sandbox.repo()).unwrap();
        git(&sandbox.repo(), &["init", "-b", "main"]);
        git(&sandbox.repo(), &["config", "user.name", "Test"]);
        git(
            &sandbox.repo(),
            &["config", "user.email", "test@example.com"],
        );
        std::fs::write(sandbox.repo().join("README.md"), "human output\n").unwrap();
        std::fs::write(
            sandbox.repo().join(".trench.toml"),
            format!(
                "[worktrees]\nroot = {:?}\n{extra_config}",
                sandbox.root.path().join("worktrees").to_string_lossy()
            ),
        )
        .unwrap();
        git(&sandbox.repo(), &["add", "."]);
        git(&sandbox.repo(), &["commit", "-m", "init"]);
        sandbox
    }

    fn repo(&self) -> PathBuf {
        self.root.path().join("acme")
    }

    /// The path `trench create` plans and prints: the configured root as
    /// written, joined with the repository and worktree names.
    fn planned(&self, name: &str) -> PathBuf {
        self.root.path().join("worktrees/acme").join(name)
    }

    /// The same directory as Git and `trench list` report it.
    fn worktree(&self, name: &str) -> PathBuf {
        self.root
            .path()
            .canonicalize()
            .unwrap()
            .join("worktrees/acme")
            .join(name)
    }

    fn command(&self, cwd: &Path) -> Command {
        let home = self.root.path().join("home");
        let mut command = Command::new(env!("CARGO_BIN_EXE_trench"));
        command
            .current_dir(cwd)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_STATE_HOME", home.join("state"))
            .env("XDG_CACHE_HOME", home.join("cache"))
            .env_remove("EDITOR")
            .env_remove("VISUAL")
            .env_remove("NO_COLOR");
        command
    }

    fn trench(&self, args: &[&str]) -> Output {
        self.command(&self.repo())
            .args(args)
            .output()
            .expect("trench should run")
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[test]
fn top_level_help_introduces_the_tui_and_shows_examples() {
    let sandbox = Sandbox::new("");
    let help = stdout(&sandbox.trench(&["--help"]));

    assert!(
        help.starts_with("Manage Git worktrees: create, switch, sync and remove them\n\nUsage:"),
        "{help}"
    );
    assert!(
        help.contains("Run `trench` with no command in a terminal to open the interactive list."),
        "{help}"
    );
    assert!(
        help.contains("Examples:\n  trench create feature/login\n"),
        "{help}"
    );
    assert!(
        help.contains("  create       Create a worktree for a new or existing branch\n"),
        "{help}"
    );
}

#[test]
fn commands_that_take_a_worktree_say_so_and_shell_init_keeps_its_snippets() {
    let sandbox = Sandbox::new("");
    for command in ["switch", "open", "sync", "remove"] {
        let help = stdout(&sandbox.trench(&[command, "--help"]));
        assert!(
            help.contains("<WORKTREE>  Branch name, worktree directory name, or path"),
            "{command}: {help}"
        );
    }

    let create = stdout(&sandbox.trench(&["create", "--help"]));
    assert!(create.contains("--from <BASE>"), "{create}");
    assert!(
        create.contains("branch from BASE. Prints the worktree path on stdout.\n"),
        "{create}"
    );

    let shell_init = stdout(&sandbox.trench(&["shell-init", "--help"]));
    assert!(
        shell_init.contains("  # ~/.zshrc\n  eval \"$(trench shell-init zsh)\"\n"),
        "{shell_init}"
    );
}

#[test]
fn every_failure_uses_one_lowercase_error_prefix_without_escape_bytes() {
    let sandbox = Sandbox::new("");
    let outside = tempfile::tempdir().unwrap();
    let cases: [(&Path, &[&str], i32); 7] = [
        (outside.path(), &["list"], 5),
        (outside.path(), &["create", "feature/x"], 5),
        (&sandbox.repo(), &["switch", "nope"], 2),
        (&sandbox.repo(), &["open", "nope"], 2),
        (&sandbox.repo(), &["remove", "nope", "--yes"], 2),
        (&sandbox.repo(), &["create", "bad name"], 1),
        (
            &sandbox.repo(),
            &["create", "feature/x", "--from", "nope"],
            1,
        ),
    ];
    for (cwd, args, code) in cases {
        let output = sandbox.command(cwd).args(args).output().unwrap();
        let stderr = stderr(&output);
        assert_eq!(output.status.code(), Some(code), "{args:?}: {stderr}");
        assert!(stderr.starts_with("error: "), "{args:?}: {stderr}");
        assert!(!stderr.contains("Error:"), "{args:?}: {stderr}");
        assert!(!stderr.contains('\x1b'), "{args:?}: {stderr}");
        assert!(output.stdout.is_empty(), "{args:?}");
    }
}

#[test]
fn bare_trench_without_a_terminal_names_a_command_to_run_instead() {
    let sandbox = Sandbox::new("");
    let output = sandbox.trench(&[]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "error: the TUI needs an interactive terminal\n\
         hint: run a command instead, for example `trench list`\n"
    );
}

#[test]
fn unknown_worktree_points_at_list() {
    let sandbox = Sandbox::new("");
    let output = sandbox.trench(&["switch", "nope"]);

    assert_eq!(
        stderr(&output),
        "error: worktree not found: nope\n\
         hint: run `trench list` to see worktrees\n\
         hint: a detached worktree is named detached@<commit>\n"
    );
}

#[test]
fn switch_and_open_refuse_a_worktree_whose_directory_is_gone() {
    let sandbox = Sandbox::new("");
    assert!(sandbox.trench(&["create", "stale/gone"]).status.success());
    let path = sandbox.worktree("stale-gone");
    std::fs::remove_dir_all(&path).unwrap();

    for args in [
        &["switch", "stale/gone"][..],
        &["switch", "stale/gone", "--print-path"],
        &["open", "stale/gone"],
        &["sync", "stale/gone", "--strategy", "rebase"],
    ] {
        let output = sandbox.trench(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        let stderr = stderr(&output);
        assert!(
            stderr.starts_with(&format!(
                "error: the directory of worktree stale-gone is missing: {}\n",
                path.display()
            )),
            "{args:?}: {stderr}"
        );
        assert!(
            stderr.contains("hint: run `trench remove stale-gone` to clean up its Git entry"),
            "{args:?}: {stderr}"
        );
    }
}

#[test]
fn create_for_a_branch_whose_worktree_directory_is_gone_warns_instead_of_silently_succeeding() {
    let sandbox = Sandbox::new("");
    assert!(sandbox.trench(&["create", "stale/gone"]).status.success());
    std::fs::remove_dir_all(sandbox.worktree("stale-gone")).unwrap();

    let output = sandbox.trench(&["create", "stale/gone"]);

    let stderr = stderr(&output);
    assert!(
        stderr.contains("stale/gone already has a worktree; nothing created"),
        "{stderr}"
    );
    assert!(
        stderr.contains("warning: the directory of worktree stale-gone is missing"),
        "{stderr}"
    );
    assert!(
        stderr.contains("hint: run `trench remove stale-gone` to clean up its Git entry"),
        "{stderr}"
    );
}

#[test]
fn create_labels_hook_output_and_summarizes_each_outcome() {
    let sandbox = Sandbox::new(
        "[hooks.pre_create]\nshell = \"echo preparing\"\n\n[hooks.post_create]\nshell = \"echo installing; echo done\"\n",
    );

    let created = sandbox.trench(&["create", "feature/login"]);
    assert_eq!(
        stderr(&created),
        "pre_create hook:\n\
         \x20 preparing\n\
         post_create hook:\n\
         \x20 installing\n\
         \x20 done\n\
         Created feature/login (new branch from main)\n"
    );
    assert_eq!(
        stdout(&created),
        format!("{}\n", sandbox.planned("feature-login").display())
    );

    let again = sandbox.trench(&["create", "feature/login"]);
    assert_eq!(
        stderr(&again),
        "feature/login already has a worktree; nothing created\n"
    );

    git(&sandbox.repo(), &["branch", "release"]);
    let existing = sandbox.trench(&["create", "release", "--no-hooks"]);
    assert_eq!(
        stderr(&existing),
        "Created worktree for existing branch release\n"
    );
}

#[test]
fn json_mode_keeps_hook_output_raw() {
    let sandbox = Sandbox::new("[hooks.post_create]\nshell = \"echo installing\"\n");

    let created = sandbox.trench(&["create", "feature/json", "--json"]);

    assert!(created.status.success());
    assert_eq!(stderr(&created), "installing\n");
    let document: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    assert_eq!(document["branch"], "feature/json");
}

#[test]
fn hook_failure_names_the_hook_and_says_what_state_things_are_in() {
    let sandbox = Sandbox::new("[hooks.post_create]\nrun = [\"false\"]\n");

    let output = sandbox.trench(&["create", "feature/broken"]);

    assert_eq!(output.status.code(), Some(4));
    assert_eq!(
        stderr(&output),
        "error: post_create hook failed: `false` exited with code 1\n\
         hint: nothing was created; the worktree and branch were rolled back\n"
    );
    assert!(!sandbox.worktree("feature-broken").exists());
}

#[test]
fn create_refuses_a_branch_nested_in_an_existing_branch_before_touching_git() {
    let sandbox = Sandbox::new("");
    git(&sandbox.repo(), &["branch", "feat"]);

    for args in [
        &["create", "feat/login"][..],
        &["create", "feat/login", "--dry-run"],
    ] {
        let output = sandbox.trench(args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(
            stderr(&output),
            "error: branch 'feat/login' cannot be created: branch 'feat' exists, and Git cannot have both\n",
            "{args:?}"
        );
    }
    assert!(!sandbox.planned("feat-login").exists());

    git(&sandbox.repo(), &["branch", "topic/a"]);
    let above = sandbox.trench(&["create", "topic"]);
    assert!(
        stderr(&above).contains("branch 'topic/a' exists, and Git cannot have both"),
        "{}",
        stderr(&above)
    );
}

#[test]
fn create_preview_reads_as_sentences_and_names_configured_hooks() {
    let sandbox = Sandbox::new("[hooks.post_create]\nshell = \"true\"\n");

    let preview = sandbox.trench(&["create", "feature/preview", "--dry-run"]);

    assert!(preview.stderr.is_empty());
    assert_eq!(
        stdout(&preview),
        format!(
            "Dry run: nothing was changed.\n\n\
             Would create worktree feature-preview\n\
             \x20 Branch  feature/preview (new, from main)\n\
             \x20 Path    {}\n\
             \x20 Hooks   post_create\n",
            sandbox.planned("feature-preview").display()
        )
    );
}

#[test]
fn remove_reports_one_fact_per_line_and_never_echoes_flags() {
    let sandbox = Sandbox::new("");
    assert!(sandbox.trench(&["create", "feature/done"]).status.success());
    let path = sandbox.worktree("feature-done");

    let preview = sandbox.trench(&["remove", "feature/done", "--dry-run", "--delete-branch"]);
    assert_eq!(
        stdout(&preview),
        format!(
            "Dry run: nothing was changed.\n\n\
             Would remove worktree feature-done\n\
             \x20 Branch  feature/done (would be deleted)\n\
             \x20 Path    {}\n\
             \x20 Dirty   no\n\
             \x20 Hooks   none configured\n",
            path.display()
        )
    );
    assert!(path.exists());

    let removed = sandbox.trench(&["remove", "feature/done", "--yes", "--delete-branch"]);
    assert!(removed.status.success());
    assert!(removed.stdout.is_empty());
    assert_eq!(
        stderr(&removed),
        "Removed worktree feature-done\nDeleted branch feature/done\n"
    );
}

#[test]
fn remove_refusals_are_plain_sentences() {
    let sandbox = Sandbox::new("");
    assert!(sandbox
        .trench(&["create", "feature/dirty"])
        .status
        .success());
    std::fs::write(sandbox.worktree("feature-dirty").join("untracked"), "x").unwrap();

    let dirty = sandbox.trench(&["remove", "feature/dirty", "--yes"]);
    assert_eq!(dirty.status.code(), Some(1));
    assert_eq!(
        stderr(&dirty),
        "error: worktree feature-dirty has uncommitted changes\n\
         hint: commit or stash them, or pass --force-worktree to discard them\n"
    );

    let unconfirmed = sandbox.trench(&["remove", "feature/dirty"]);
    assert_eq!(unconfirmed.status.code(), Some(8));
    assert_eq!(
        stderr(&unconfirmed),
        "error: removing a worktree needs confirmation\n\
         hint: pass --yes when not running in a terminal\n"
    );

    let forced = sandbox.trench(&["remove", "feature/dirty", "--yes", "--force-worktree"]);
    assert_eq!(
        stderr(&forced),
        "Removed worktree feature-dirty\n\
         Kept branch feature/dirty\n\
         Discarded uncommitted changes\n"
    );
}

#[test]
fn sync_reports_one_sentence_on_stderr_and_nothing_on_stdout() {
    let sandbox = Sandbox::new("");
    assert!(sandbox.trench(&["create", "feature/sync"]).status.success());
    std::fs::write(sandbox.repo().join("main-only"), "main\n").unwrap();
    git(&sandbox.repo(), &["add", "main-only"]);
    git(&sandbox.repo(), &["commit", "-m", "advance main"]);

    let synced = sandbox.trench(&["sync", "feature/sync", "--strategy", "rebase"]);

    assert!(synced.status.success(), "{}", stderr(&synced));
    assert!(synced.stdout.is_empty());
    assert_eq!(
        stderr(&synced),
        "Rebased feature/sync onto main. Was 0 ahead, 1 behind; now 0 ahead, 0 behind.\n"
    );

    let again = sandbox.trench(&["sync", "feature/sync", "--strategy", "rebase"]);
    assert_eq!(
        stderr(&again),
        "feature/sync is already up to date with main\n"
    );
}
