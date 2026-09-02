use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::installation::{self, InstallationManager};

const RELEASE_BASE_URL: &str = "https://github.com/sadiksaifi/trench/releases/latest/download";
const MANIFEST_NAME: &str = "trench-release.json";
const CHECKSUMS_NAME: &str = "trench-checksums.txt";

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    fn parse(value: &str) -> Option<Self> {
        let mut components = value.split('.');
        let major = parse_component(components.next()?)?;
        let minor = parse_component(components.next()?)?;
        let patch = parse_component(components.next()?)?;
        (components.next().is_none()).then_some(Self {
            major,
            minor,
            patch,
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

fn parse_component(value: &str) -> Option<u64> {
    if value.is_empty() || (value.len() > 1 && value.starts_with('0')) {
        return None;
    }
    value.parse().ok()
}

fn validate_current_version(version: &str, development: bool) -> Result<Version> {
    if development {
        bail!(
            "cannot upgrade a development build ({version}); reinstall with the installation method that owns this executable"
        );
    }
    Version::parse(version).ok_or_else(|| {
        anyhow::anyhow!(
            "cannot upgrade an unknown version ({version}); reinstall with the installation method that owns this executable"
        )
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseManifest {
    schema: u8,
    tag: String,
    version: String,
    commit: String,
    assets: Vec<ReleaseAsset>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseAsset {
    name: String,
    sha256: String,
}

struct ParsedRelease {
    version: Version,
    archive_sha256: String,
}

fn parse_release(contents: &str, archive_name: &str) -> Result<ParsedRelease> {
    let manifest: ReleaseManifest = serde_json::from_str(contents)?;
    if manifest.schema != 1 {
        bail!("unsupported release manifest schema {}", manifest.schema);
    }
    let version = Version::parse(&manifest.version)
        .ok_or_else(|| anyhow::anyhow!("release manifest contains an invalid stable version"))?;
    if manifest.tag != format!("v{}", manifest.version) {
        bail!("release manifest tag and version do not match");
    }
    if !is_hex_digest(&manifest.commit, 40) {
        bail!("release manifest contains an invalid commit");
    }

    let matching_assets = manifest
        .assets
        .iter()
        .filter(|asset| asset.name == archive_name)
        .collect::<Vec<_>>();
    if matching_assets.len() != 1 || !is_hex_digest(&matching_assets[0].sha256, 64) {
        bail!("release manifest does not contain one valid digest for {archive_name}");
    }

    Ok(ParsedRelease {
        version,
        archive_sha256: matching_assets[0].sha256.to_ascii_lowercase(),
    })
}

fn is_hex_digest(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug)]
pub enum UpgradeOutcome {
    Updated { from: String, to: String },
    AlreadyCurrent { version: String },
    Homebrew(ExitStatus),
}

trait ReleaseSource {
    fn fetch(&self, asset: &str, destination: &Path) -> Result<()>;
}

struct GithubRelease;

impl ReleaseSource for GithubRelease {
    fn fetch(&self, asset: &str, destination: &Path) -> Result<()> {
        let url = format!("{RELEASE_BASE_URL}/{asset}");
        let output = Command::new("curl")
            .args(["--proto", "=https", "--tlsv1.2", "-fLsS", &url, "-o"])
            .arg(destination)
            .output()
            .with_context(|| format!("could not run curl while downloading {asset}"))?;
        if !output.status.success() {
            bail!(
                "failed to download {asset}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }
}

pub fn execute() -> Result<UpgradeOutcome> {
    let current = validate_current_version(
        crate::build_info::VERSION,
        crate::build_info::is_development(),
    )?;
    match installation::detect_current()? {
        InstallationManager::Standalone => {
            let executable = std::env::current_exe()?.canonicalize()?;
            let target = current_target()?;
            standalone_upgrade(&GithubRelease, &executable, current, target)
        }
        InstallationManager::Homebrew => {
            let status = Command::new("brew")
                .args(["upgrade", "trench"])
                .status()
                .context("could not run `brew upgrade trench`")?;
            Ok(UpgradeOutcome::Homebrew(status))
        }
        InstallationManager::ManualOrUnknown => bail!(
            "cannot determine who owns {}; reinstall or upgrade it with the installation method that placed it there",
            std::env::current_exe()?.display()
        ),
    }
}

fn current_target() -> Result<&'static str> {
    if std::env::consts::OS != "macos" {
        bail!("trench upgrade supports macOS only");
    }
    match std::env::consts::ARCH {
        "aarch64" => Ok("aarch64-apple-darwin"),
        "x86_64" => Ok("x86_64-apple-darwin"),
        architecture => bail!("unsupported macOS architecture: {architecture}"),
    }
}

fn standalone_upgrade(
    source: &dyn ReleaseSource,
    executable: &Path,
    current: Version,
    target: &str,
) -> Result<UpgradeOutcome> {
    let workspace = Workspace::create()?;
    let manifest_path = workspace.path.join(MANIFEST_NAME);
    let checksums_path = workspace.path.join(CHECKSUMS_NAME);
    source.fetch(MANIFEST_NAME, &manifest_path)?;
    source.fetch(CHECKSUMS_NAME, &checksums_path)?;

    let archive_name = format!("trench-{target}.tar.gz");
    let manifest_contents = fs::read_to_string(&manifest_path)
        .with_context(|| format!("could not read {MANIFEST_NAME}"))?;
    let release = parse_release(&manifest_contents, &archive_name)?;
    let checksums = parse_checksums(&fs::read_to_string(&checksums_path)?)?;
    verify_file_digest(
        &manifest_path,
        required_digest(&checksums, MANIFEST_NAME)?,
        MANIFEST_NAME,
    )?;
    let checksum_digest = required_digest(&checksums, &archive_name)?;
    if !checksum_digest.eq_ignore_ascii_case(&release.archive_sha256) {
        bail!("manifest and checksum file disagree for {archive_name}");
    }

    if release.version <= current {
        return Ok(UpgradeOutcome::AlreadyCurrent {
            version: current.to_string(),
        });
    }

    let archive_path = workspace.path.join(&archive_name);
    source.fetch(&archive_name, &archive_path)?;
    verify_file_digest(&archive_path, checksum_digest, &archive_name)?;

    let extracted = workspace.path.join("extracted");
    fs::create_dir(&extracted)?;
    extract_archive(&archive_path, &extracted)?;
    let candidate = extracted.join("trench");
    let candidate_version = candidate_version(&candidate)?;
    let expected_version = format!("trench {}", release.version);
    if candidate_version != expected_version {
        bail!("downloaded trench reports `{candidate_version}`, expected `{expected_version}`");
    }

    atomic_replace(&candidate, executable)?;
    Ok(UpgradeOutcome::Updated {
        from: current.to_string(),
        to: release.version.to_string(),
    })
}

fn parse_checksums(contents: &str) -> Result<HashMap<String, String>> {
    let mut checksums = HashMap::new();
    for (index, line) in contents.lines().enumerate() {
        let mut fields = line.split_whitespace();
        let digest = fields
            .next()
            .ok_or_else(|| anyhow::anyhow!("invalid checksum line {}", index + 1))?;
        let name = fields
            .next()
            .ok_or_else(|| anyhow::anyhow!("invalid checksum line {}", index + 1))?
            .trim_start_matches('*');
        if fields.next().is_some()
            || !is_hex_digest(digest, 64)
            || Path::new(name).file_name().and_then(|value| value.to_str()) != Some(name)
            || checksums
                .insert(name.to_string(), digest.to_ascii_lowercase())
                .is_some()
        {
            bail!("invalid checksum line {}", index + 1);
        }
    }
    Ok(checksums)
}

fn required_digest<'a>(checksums: &'a HashMap<String, String>, name: &str) -> Result<&'a str> {
    checksums
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| anyhow::anyhow!("checksum is missing for {name}"))
}

fn sha256(path: &Path) -> Result<String> {
    let output = Command::new("shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .context("could not run shasum")?;
    if !output.status.success() {
        bail!(
            "could not checksum {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let digest = String::from_utf8(output.stdout)?
        .split_whitespace()
        .next()
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("shasum returned no digest for {}", path.display()))?;
    if !is_hex_digest(&digest, 64) {
        bail!("shasum returned an invalid digest for {}", path.display());
    }
    Ok(digest.to_ascii_lowercase())
}

fn verify_file_digest(path: &Path, expected: &str, name: &str) -> Result<()> {
    let actual = sha256(path)?;
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("checksum verification failed for {name}");
    }
    Ok(())
}

fn extract_archive(archive: &Path, destination: &Path) -> Result<()> {
    let listing = Command::new("tar")
        .args(["-tzf"])
        .arg(archive)
        .output()
        .context("could not run tar while inspecting upgrade archive")?;
    if !listing.status.success() {
        bail!(
            "failed to inspect upgrade archive: {}",
            String::from_utf8_lossy(&listing.stderr).trim()
        );
    }
    let entries = String::from_utf8(listing.stdout)?;
    if entries.lines().collect::<Vec<_>>() != ["trench", "LICENSE", "README.md"] {
        bail!("upgrade archive contains an unsafe or unexpected layout");
    }

    let output = Command::new("tar")
        .args(["-xzf"])
        .arg(archive)
        .arg("-C")
        .arg(destination)
        .output()
        .context("could not run tar")?;
    if !output.status.success() {
        bail!(
            "failed to extract upgrade archive: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn candidate_version(candidate: &Path) -> Result<String> {
    let output = Command::new(candidate)
        .arg("--version")
        .output()
        .with_context(|| format!("could not run downloaded trench at {}", candidate.display()))?;
    if !output.status.success() {
        bail!("downloaded trench failed its version check");
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn atomic_replace(candidate: &Path, executable: &Path) -> Result<()> {
    let parent = executable
        .parent()
        .ok_or_else(|| anyhow::anyhow!("running executable has no parent directory"))?;
    let staged = parent.join(format!(".trench-upgrade-{}", std::process::id()));
    let mut staged_created = false;
    let result = (|| -> Result<()> {
        let mut source = File::open(candidate)?;
        let mut destination = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
            .with_context(|| format!("could not stage upgrade beside {}", executable.display()))?;
        staged_created = true;
        io::copy(&mut source, &mut destination)?;
        destination.set_permissions(source.metadata()?.permissions())?;
        destination.sync_all()?;
        fs::rename(&staged, executable).with_context(|| {
            format!(
                "could not atomically replace {}; the existing executable was left unchanged",
                executable.display()
            )
        })?;
        // The rename is the atomic commit point. Directory fsync is not
        // supported by every macOS filesystem, so durability is best effort
        // and must not report a failed upgrade after replacement succeeded.
        let _ = File::open(parent).and_then(|directory| directory.sync_all());
        Ok(())
    })();
    if result.is_err() && staged_created {
        let _ = fs::remove_file(&staged);
    }
    result
}

struct Workspace {
    path: PathBuf,
}

impl Workspace {
    fn create() -> Result<Self> {
        let base = std::env::temp_dir();
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        for attempt in 0..10 {
            let path = base.join(format!(
                "trench-upgrade-{}-{timestamp}-{attempt}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        bail!("could not create a unique temporary upgrade directory")
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    struct LocalRelease(PathBuf);

    impl ReleaseSource for LocalRelease {
        fn fetch(&self, asset: &str, destination: &Path) -> Result<()> {
            fs::copy(self.0.join(asset), destination)?;
            Ok(())
        }
    }

    #[cfg(unix)]
    fn release_fixture(root: &Path, target: &str, version: &str) {
        use std::os::unix::fs::PermissionsExt;

        let payload = root.join("payload");
        fs::create_dir(&payload).unwrap();
        let candidate = payload.join("trench");
        fs::write(
            &candidate,
            format!("#!/bin/sh\nprintf 'trench {version}\\n'\n"),
        )
        .unwrap();
        fs::set_permissions(&candidate, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(payload.join("LICENSE"), "license").unwrap();
        fs::write(payload.join("README.md"), "readme").unwrap();

        let archive_name = format!("trench-{target}.tar.gz");
        let archive = root.join(&archive_name);
        let status = Command::new("tar")
            .args(["-czf"])
            .arg(&archive)
            .arg("-C")
            .arg(&payload)
            .args(["trench", "LICENSE", "README.md"])
            .status()
            .unwrap();
        assert!(status.success());

        let archive_digest = sha256(&archive).unwrap();
        let manifest = format!(
            r#"{{"schema":1,"tag":"v{version}","version":"{version}","commit":"0123456789abcdef0123456789abcdef01234567","assets":[{{"name":"{archive_name}","sha256":"{archive_digest}"}}]}}"#
        );
        let manifest_path = root.join(MANIFEST_NAME);
        fs::write(&manifest_path, manifest).unwrap();
        let manifest_digest = sha256(&manifest_path).unwrap();
        fs::write(
            root.join(CHECKSUMS_NAME),
            format!("{archive_digest}  {archive_name}\n{manifest_digest}  {MANIFEST_NAME}\n"),
        )
        .unwrap();
    }

    #[test]
    fn development_and_unknown_versions_are_refused() {
        let development = validate_current_version("0.1.0-dev.g123456789abc", true)
            .expect_err("development build must be refused");
        assert!(development.to_string().contains("development build"));

        let unknown = validate_current_version("0.0.0-dev.unknown", true)
            .expect_err("unknown build must be refused");
        assert!(unknown.to_string().contains("development build"));

        let malformed = validate_current_version("not-a-version", false)
            .expect_err("unknown version must be refused");
        assert!(malformed.to_string().contains("unknown version"));
    }

    #[test]
    fn release_manifest_requires_consistent_stable_identity_and_archive_digest() {
        let manifest = r#"{
            "schema": 1,
            "tag": "v1.2.3",
            "version": "1.2.3",
            "commit": "0123456789abcdef0123456789abcdef01234567",
            "assets": [
                {"name": "trench-aarch64-apple-darwin.tar.gz", "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
            ]
        }"#;

        let release = parse_release(manifest, "trench-aarch64-apple-darwin.tar.gz")
            .expect("valid release manifest");
        assert_eq!(release.version, Version::parse("1.2.3").unwrap());
        assert_eq!(
            release.archive_sha256,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );

        let inconsistent = manifest.replace("\"tag\": \"v1.2.3\"", "\"tag\": \"v1.2.4\"");
        assert!(parse_release(&inconsistent, "trench-aarch64-apple-darwin.tar.gz").is_err());
        assert!(parse_release(manifest, "trench-x86_64-apple-darwin.tar.gz").is_err());
        let invalid_commit = manifest.replace(
            "0123456789abcdef0123456789abcdef01234567",
            "z123456789abcdef0123456789abcdef01234567",
        );
        assert!(parse_release(&invalid_commit, "trench-aarch64-apple-darwin.tar.gz").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn standalone_upgrade_verifies_and_atomically_replaces_the_executable() {
        let root = TempDir::new().unwrap();
        let release = root.path().join("release");
        fs::create_dir(&release).unwrap();
        release_fixture(&release, "aarch64-apple-darwin", "1.1.0");
        let executable = root.path().join("bin/trench");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, "old binary").unwrap();

        let outcome = standalone_upgrade(
            &LocalRelease(release),
            &executable,
            Version::parse("1.0.0").unwrap(),
            "aarch64-apple-darwin",
        )
        .expect("verified upgrade should succeed");

        assert!(matches!(
            outcome,
            UpgradeOutcome::Updated { ref from, ref to }
                if from == "1.0.0" && to == "1.1.0"
        ));
        assert_eq!(candidate_version(&executable).unwrap(), "trench 1.1.0");
        assert!(!executable
            .with_file_name(format!(".trench-upgrade-{}", std::process::id()))
            .exists());
    }

    #[cfg(unix)]
    #[test]
    fn current_installation_stops_before_downloading_the_archive() {
        let root = TempDir::new().unwrap();
        let release = root.path().join("release");
        fs::create_dir(&release).unwrap();
        release_fixture(&release, "aarch64-apple-darwin", "1.1.0");
        fs::remove_file(release.join("trench-aarch64-apple-darwin.tar.gz")).unwrap();
        let executable = root.path().join("trench");
        fs::write(&executable, "current").unwrap();

        let outcome = standalone_upgrade(
            &LocalRelease(release),
            &executable,
            Version::parse("1.1.0").unwrap(),
            "aarch64-apple-darwin",
        )
        .unwrap();

        assert!(matches!(
            outcome,
            UpgradeOutcome::AlreadyCurrent { ref version } if version == "1.1.0"
        ));
        assert_eq!(fs::read(&executable).unwrap(), b"current");
    }

    #[cfg(unix)]
    #[test]
    fn checksum_failure_preserves_the_old_executable() {
        let root = TempDir::new().unwrap();
        let release = root.path().join("release");
        fs::create_dir(&release).unwrap();
        release_fixture(&release, "aarch64-apple-darwin", "1.1.0");
        let archive = release.join("trench-aarch64-apple-darwin.tar.gz");
        fs::write(&archive, "corrupt archive").unwrap();
        let executable = root.path().join("trench");
        fs::write(&executable, "old binary").unwrap();

        let error = standalone_upgrade(
            &LocalRelease(release),
            &executable,
            Version::parse("1.0.0").unwrap(),
            "aarch64-apple-darwin",
        )
        .expect_err("corrupt archive must fail");

        assert!(error.to_string().contains("checksum verification failed"));
        assert_eq!(fs::read(&executable).unwrap(), b"old binary");
    }

    #[cfg(unix)]
    #[test]
    fn candidate_version_mismatch_preserves_the_old_executable() {
        let root = TempDir::new().unwrap();
        let release = root.path().join("release");
        fs::create_dir(&release).unwrap();
        release_fixture(&release, "aarch64-apple-darwin", "1.1.0");
        let manifest_path = release.join(MANIFEST_NAME);
        let manifest = fs::read_to_string(&manifest_path)
            .unwrap()
            .replace("\"tag\":\"v1.1.0\"", "\"tag\":\"v1.2.0\"")
            .replace("\"version\":\"1.1.0\"", "\"version\":\"1.2.0\"");
        fs::write(&manifest_path, manifest).unwrap();
        let manifest_digest = sha256(&manifest_path).unwrap();
        let archive_name = "trench-aarch64-apple-darwin.tar.gz";
        let archive_digest = sha256(&release.join(archive_name)).unwrap();
        fs::write(
            release.join(CHECKSUMS_NAME),
            format!("{archive_digest}  {archive_name}\n{manifest_digest}  {MANIFEST_NAME}\n"),
        )
        .unwrap();
        let executable = root.path().join("trench");
        fs::write(&executable, "old binary").unwrap();

        let error = standalone_upgrade(
            &LocalRelease(release),
            &executable,
            Version::parse("1.0.0").unwrap(),
            "aarch64-apple-darwin",
        )
        .expect_err("candidate version mismatch must fail");

        assert!(error.to_string().contains("expected `trench 1.2.0`"));
        assert_eq!(fs::read(&executable).unwrap(), b"old binary");
    }

    #[cfg(unix)]
    #[test]
    fn interrupted_asset_download_preserves_the_old_executable() {
        let root = TempDir::new().unwrap();
        let release = root.path().join("release");
        fs::create_dir(&release).unwrap();
        release_fixture(&release, "aarch64-apple-darwin", "1.1.0");
        fs::remove_file(release.join("trench-aarch64-apple-darwin.tar.gz")).unwrap();
        let executable = root.path().join("trench");
        fs::write(&executable, "old binary").unwrap();

        assert!(standalone_upgrade(
            &LocalRelease(release),
            &executable,
            Version::parse("1.0.0").unwrap(),
            "aarch64-apple-darwin",
        )
        .is_err());
        assert_eq!(fs::read(&executable).unwrap(), b"old binary");
    }

    #[cfg(unix)]
    #[test]
    fn failed_atomic_staging_preserves_the_old_executable() {
        let root = TempDir::new().unwrap();
        let candidate = root.path().join("candidate");
        let executable = root.path().join("trench");
        fs::write(&candidate, "new binary").unwrap();
        fs::write(&executable, "old binary").unwrap();
        let staged = root
            .path()
            .join(format!(".trench-upgrade-{}", std::process::id()));
        fs::write(&staged, "occupied").unwrap();

        let error = atomic_replace(&candidate, &executable)
            .expect_err("occupied staging path must fail safely");

        assert!(error.to_string().contains("could not stage upgrade"));
        assert_eq!(fs::read(&executable).unwrap(), b"old binary");
        assert_eq!(fs::read(&staged).unwrap(), b"occupied");
    }

    #[test]
    fn checksums_reject_duplicate_and_traversing_asset_names() {
        let digest = "a".repeat(64);
        assert!(parse_checksums(&format!(
            "{digest}  trench-release.json\n{digest}  trench-release.json\n"
        ))
        .is_err());
        assert!(parse_checksums(&format!("{digest}  ../trench-release.json\n")).is_err());
    }

    #[test]
    fn archive_extraction_refuses_unexpected_layout() {
        let root = TempDir::new().unwrap();
        let payload = root.path().join("payload");
        let destination = root.path().join("destination");
        let archive = root.path().join("release.tar.gz");
        fs::create_dir(&payload).unwrap();
        fs::create_dir(&destination).unwrap();
        for name in ["trench", "LICENSE", "README.md", "unexpected"] {
            fs::write(payload.join(name), name).unwrap();
        }
        assert!(Command::new("tar")
            .args(["-czf"])
            .arg(&archive)
            .arg("-C")
            .arg(&payload)
            .args(["trench", "LICENSE", "README.md", "unexpected"])
            .status()
            .unwrap()
            .success());

        let error = extract_archive(&archive, &destination)
            .expect_err("unexpected archive entries must be refused");
        assert!(error.to_string().contains("unsafe or unexpected layout"));
        assert!(fs::read_dir(&destination).unwrap().next().is_none());
    }
}
