use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

#[path = "../build/version.rs"]
mod build_version;

fn git(repo: &Path, args: &[&str]) {
    git_output(repo, args);
}

fn git_output(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("UTF-8 git output")
        .trim()
        .to_owned()
}

fn short_commit(repo: &Path) -> String {
    git_output(repo, &["rev-parse", "HEAD"])
        .chars()
        .take(12)
        .collect()
}

fn repository() -> TempDir {
    let repo = tempfile::tempdir().expect("temporary repository");
    git(repo.path(), &["init", "--quiet"]);
    git(repo.path(), &["config", "user.name", "Trench Tests"]);
    git(repo.path(), &["config", "user.email", "trench@example.com"]);
    std::fs::write(repo.path().join("tracked"), "initial\n").expect("write fixture");
    git(repo.path(), &["add", "tracked"]);
    git(repo.path(), &["commit", "--quiet", "-m", "initial"]);
    repo
}

#[test]
fn official_build_uses_the_exact_annotated_release_tag() {
    let repo = repository();
    git(repo.path(), &["tag", "-a", "v1.2.3", "-m", "release"]);

    let info = build_version::derive(repo.path(), true).expect("valid release build");

    assert_eq!(info.version, "1.2.3");
    assert_eq!(info.exact_tag.as_deref(), Some("v1.2.3"));
    assert!(info.official);
    assert_eq!(info.dirty, Some(false));
}

#[test]
fn local_build_at_an_exact_release_tag_keeps_a_development_identity() {
    let repo = repository();
    git(repo.path(), &["tag", "-a", "v1.2.3", "-m", "release"]);
    let commit = short_commit(repo.path());

    let info = build_version::derive(repo.path(), false).expect("local build");

    assert_eq!(info.version, format!("1.2.3-dev.g{commit}"));
}

#[test]
fn dirty_local_build_marks_its_version_dirty() {
    let repo = repository();
    git(repo.path(), &["tag", "-a", "v1.2.3", "-m", "release"]);
    std::fs::write(repo.path().join("tracked"), "changed\n").expect("dirty fixture");
    let commit = short_commit(repo.path());

    let info = build_version::derive(repo.path(), false).expect("dirty local build");

    assert_eq!(info.version, format!("1.2.3-dev.g{commit}.dirty"));
    assert_eq!(info.dirty, Some(true));
}

#[test]
fn local_build_without_git_metadata_has_an_unknown_development_identity() {
    let directory = tempfile::tempdir().expect("temporary directory");

    let info = build_version::derive(directory.path(), false).expect("metadata-free local build");

    assert_eq!(info.version, "0.0.0-dev.unknown");
    assert_eq!(info.commit, None);
    assert_eq!(info.dirty, None);
}

#[test]
fn untagged_local_build_uses_the_placeholder_base_version() {
    let repo = repository();
    let commit = short_commit(repo.path());

    let info = build_version::derive(repo.path(), false).expect("untagged local build");

    assert_eq!(info.version, format!("0.0.0-dev.g{commit}"));
    assert_eq!(info.exact_tag, None);
}

#[test]
fn lightweight_and_noncanonical_tags_do_not_supply_a_version() {
    let repo = repository();
    git(repo.path(), &["tag", "v1.2.3"]);
    git(
        repo.path(),
        &["tag", "-a", "release-2.0.0", "-m", "not canonical"],
    );
    let commit = short_commit(repo.path());

    let info = build_version::derive(repo.path(), false).expect("local build");

    assert_eq!(info.version, format!("0.0.0-dev.g{commit}"));
    assert_eq!(info.exact_tag, None);
}

#[test]
fn official_build_rejects_a_lightweight_release_tag() {
    let repo = repository();
    git(repo.path(), &["tag", "v1.2.3"]);

    let error = build_version::derive(repo.path(), true).expect_err("invalid release build");

    assert!(error.contains("canonical annotated tag"), "{error}");
}

#[test]
fn official_build_rejects_a_dirty_checkout() {
    let repo = repository();
    git(repo.path(), &["tag", "-a", "v1.2.3", "-m", "release"]);
    std::fs::write(repo.path().join("tracked"), "changed\n").expect("dirty fixture");

    let error = build_version::derive(repo.path(), true).expect_err("dirty release build");

    assert!(error.contains("clean checkout"), "{error}");
}

#[test]
fn official_build_rejects_missing_git_metadata() {
    let directory = tempfile::tempdir().expect("temporary directory");

    let error =
        build_version::derive(directory.path(), true).expect_err("metadata-free release build");

    assert!(error.contains("usable Git metadata"), "{error}");
}

#[test]
fn dirty_untagged_build_reports_the_exact_development_version() {
    let repo = repository();
    std::fs::write(repo.path().join("tracked"), "changed\n").expect("dirty fixture");
    let commit = short_commit(repo.path());

    let info = build_version::derive(repo.path(), false).expect("dirty untagged build");

    assert_eq!(info.version, format!("0.0.0-dev.g{commit}.dirty"));
}
