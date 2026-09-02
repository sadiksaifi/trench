use std::fs;
use std::path::PathBuf;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn tagsmith_config_declares_the_single_stable_trench_release_target() {
    let config = fs::read_to_string(repository_root().join(".tagsmith.jsonc"))
        .expect("repository should contain a Tagsmith configuration");

    for required in [
        r#""baseBranch": "main""#,
        r#""tagPattern": "v{version}""#,
        r#""initialVersion": "0.0.0""#,
        r#""trench": {"#,
        r#""path": ".""#,
        r#""name": "stable""#,
        r#""strategy": "stable""#,
    ] {
        assert!(config.contains(required), "missing `{required}`");
    }
}

#[test]
fn shared_ci_runs_every_required_make_gate_for_prs_main_and_releases() {
    let workflow = fs::read_to_string(repository_root().join(".github/workflows/ci.yml"))
        .expect("repository should contain the shared CI workflow");

    for required in [
        "pull_request:",
        "branches: [main]",
        "workflow_call:",
        "make fmt-check",
        "make lint-strict",
        "make check",
        "make test",
    ] {
        assert!(workflow.contains(required), "missing `{required}`");
    }
}

#[test]
fn release_workflow_validates_builds_attests_and_publishes_both_macos_targets() {
    let workflow = fs::read_to_string(repository_root().join(".github/workflows/release.yml"))
        .expect("repository should contain the release workflow");

    for required in [
        "tags: [\"v*\"]",
        "cancel-in-progress: false",
        "uses: ./.github/workflows/ci.yml",
        "fetch-depth: 0",
        "pnpm dlx tagsmith@latest validate",
        "git merge-base --is-ancestor",
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "MACOSX_DEPLOYMENT_TARGET: \"11.0\"",
        "TRENCH_RELEASE_BUILD: \"true\"",
        "trench-checksums.txt",
        "trench-release.json",
        "trench-installer.sh",
        "actions/attest-build-provenance@",
        "--draft",
        "--draft=false",
        "trench-release-workflow:v1",
    ] {
        assert!(workflow.contains(required), "missing `{required}`");
    }
    assert!(workflow.contains("scripts/package-release.sh"));
    assert!(workflow.contains("permissions:\n  contents: read"));
    assert!(workflow.contains("contents: write"));
    assert!(workflow.contains("id-token: write"));
    assert!(workflow.contains("attestations: write"));
    let packaging = fs::read_to_string(repository_root().join("scripts/package-release.sh"))
        .expect("release packaging should be a tested repository script");
    assert!(packaging.contains("otool -L"));

    assert!(
        !workflow
            .lines()
            .any(|line| line.trim_start().starts_with("git tag")),
        "release automation must never create tags"
    );
    assert!(!workflow.contains("tagsmith@latest tag"));
    assert!(!workflow.contains("git push refs/tags"));
    assert!(
        workflow
            .find("Reject mutation of a published release")
            .unwrap()
            < workflow
                .find("Attest every published release asset")
                .unwrap(),
        "release ownership must be checked before publishing attestations"
    );
    for required in [
        "Verify release tag still points to validated commit",
        "refs/tags/$RELEASE_TAG:refs/tags/$RELEASE_TAG",
        "git cat-file -t \"refs/tags/$RELEASE_TAG\"",
        "git rev-parse \"refs/tags/$RELEASE_TAG^{commit}\"",
    ] {
        assert!(workflow.contains(required), "missing `{required}`");
    }
    assert!(
        workflow
            .find("Verify release tag still points to validated commit")
            .unwrap()
            < workflow
                .find("Attest every published release asset")
                .unwrap(),
        "the remote tag must be revalidated before attestation"
    );
    assert!(
        workflow
            .find("Reverify release tag before publication")
            .unwrap()
            < workflow
                .find("Publish only after all assets and attestations succeed")
                .unwrap(),
        "the remote tag must be revalidated after staging and before publication"
    );
}

#[test]
fn release_notes_are_transient_user_facing_and_repository_aware() {
    let config = fs::read_to_string(repository_root().join("cliff.toml"))
        .expect("repository should contain git-cliff configuration");

    for required in [
        "Breaking Changes",
        "Features",
        "Fixes",
        "Performance",
        "^fix/tui remove dialog actions",
        "owner = \"sadiksaifi\"",
        "repo = \"trench\"",
        "releases/latest/download/trench-installer.sh",
    ] {
        assert!(config.contains(required), "missing `{required}`");
    }
    for excluded_group in ["Documentation", "Chores", "Tests", "CI", "Build"] {
        assert!(
            !config.contains(&format!("group = \"{excluded_group}\"")),
            "non-user-facing group `{excluded_group}` must be excluded"
        );
    }
    assert!(!repository_root().join("CHANGELOG.md").exists());
}

#[test]
fn repository_governance_requires_tagsmith_and_conventional_pull_requests() {
    let agents = fs::read_to_string(repository_root().join("AGENTS.md")).unwrap();
    for required in [
        "Git tags are the sole release-version source of truth",
        "pnpm dlx tagsmith@latest",
        "Raw or manual `git tag` commands",
        "CI validates release tags but never creates them",
        "Generated changelogs and release notes are not committed",
    ] {
        assert!(agents.contains(required), "missing `{required}`");
    }

    let workflow = fs::read_to_string(repository_root().join(".github/workflows/pr-title.yml"))
        .expect("repository should enforce conventional pull-request titles");
    assert!(workflow.contains("pull_request_target:"));
    assert!(workflow.contains("amannn/action-semantic-pull-request@"));
}
