use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

fn git(repo: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git should run")
}

fn commit(repo: &Path, message: &str, sequence: usize) {
    std::fs::write(repo.join("change.txt"), sequence.to_string()).unwrap();
    assert!(git(repo, &["add", "change.txt"]).status.success());
    assert!(git(repo, &["commit", "-m", message]).status.success());
}

fn commit_with_body(repo: &Path, message: &str, body: &str, sequence: usize) {
    std::fs::write(repo.join("change.txt"), sequence.to_string()).unwrap();
    assert!(git(repo, &["add", "change.txt"]).status.success());
    assert!(git(repo, &["commit", "-m", message, "-m", body])
        .status
        .success());
}

#[test]
fn git_cliff_snapshots_user_facing_first_release_notes() {
    let root = TempDir::new().unwrap();
    assert!(git(root.path(), &["init", "--initial-branch=main"])
        .status
        .success());
    assert!(git(root.path(), &["config", "user.name", "Release Test"])
        .status
        .success());
    assert!(git(
        root.path(),
        &["config", "user.email", "release@example.com"]
    )
    .status
    .success());

    for (index, message) in [
        "feat(cli): add portable install (#12)",
        "fix/tui remove dialog actions (#170)",
        "perf: speed up catalog discovery",
        "docs: rewrite internal notes",
        "chore: update development tooling",
        "feat!: remove an obsolete interface",
    ]
    .iter()
    .enumerate()
    {
        commit(root.path(), message, index);
    }
    commit_with_body(
        root.path(),
        "fix: revise configuration semantics",
        "BREAKING CHANGE: legacy configuration is no longer accepted",
        6,
    );
    assert!(
        git(root.path(), &["tag", "-a", "v0.1.0", "-m", "Release 0.1.0"])
            .status
            .success()
    );

    let output = Command::new("git-cliff")
        .args([
            "--config",
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("cliff.toml")
                .to_str()
                .unwrap(),
            "--current",
            "--offline",
        ])
        .current_dir(root.path())
        .output()
        .expect("git-cliff must be installed for the release-notes contract test");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let notes = String::from_utf8(output.stdout).unwrap();

    for expected in [
        "releases/latest/download/trench-installer.sh",
        "## 0.1.0",
        "### Breaking Changes",
        "### Features",
        "### Fixes",
        "### Performance",
        "Add portable install",
        "Remove dialog actions",
        "Speed up catalog discovery",
        "https://github.com/sadiksaifi/trench/pull/12",
        "https://github.com/sadiksaifi/trench/pull/170",
    ] {
        assert!(
            notes.contains(expected),
            "missing `{expected}` in:\n{notes}"
        );
    }
    for excluded in ["rewrite internal notes", "update development tooling"] {
        assert!(
            !notes.to_lowercase().contains(excluded),
            "unexpected `{excluded}`"
        );
    }
    let breaking = notes
        .split("### Breaking Changes")
        .nth(1)
        .unwrap()
        .split("### Features")
        .next()
        .unwrap();
    assert!(breaking.contains("Revise configuration semantics"));
}
