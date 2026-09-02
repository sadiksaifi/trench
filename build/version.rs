use std::path::Path;
use std::process::Command;

#[derive(Debug, PartialEq, Eq)]
pub struct BuildInfo {
    pub version: String,
    pub commit: Option<String>,
    pub exact_tag: Option<String>,
    pub dirty: Option<bool>,
    pub official: bool,
}

pub fn derive(repo: &Path, official: bool) -> Result<BuildInfo, String> {
    let commit = match git(repo, &["rev-parse", "--verify", "HEAD"]) {
        Ok(commit) => commit,
        Err(error) if official => {
            return Err(format!(
                "official release builds require usable Git metadata: {error}"
            ));
        }
        Err(_) => return Ok(BuildInfo::unknown()),
    };
    let dirty = match git(repo, &["status", "--porcelain", "--untracked-files=normal"]) {
        Ok(status) => !status.is_empty(),
        Err(error) if official => {
            return Err(format!(
                "official release builds require usable Git metadata: {error}"
            ));
        }
        Err(_) => return Ok(BuildInfo::unknown()),
    };
    let exact_tags = git(repo, &["tag", "--points-at", "HEAD"])?
        .lines()
        .filter_map(|tag| {
            canonical_version(tag).and_then(|version| {
                let object_type = git(repo, &["cat-file", "-t", &format!("refs/tags/{tag}")]);
                matches!(object_type.as_deref(), Ok("tag"))
                    .then(|| (tag.to_owned(), version.to_owned()))
            })
        })
        .collect::<Vec<_>>();

    if official {
        if dirty {
            return Err("official release builds require a clean checkout".to_owned());
        }
        if exact_tags.len() != 1 {
            return Err(
                "official release builds require exactly one canonical annotated tag at HEAD"
                    .to_owned(),
            );
        }
    }

    let (exact_tag, base_version) = exact_tags
        .into_iter()
        .next()
        .map_or((None, "0.0.0".to_owned()), |(tag, version)| {
            (Some(tag), version)
        });
    let mut version = if official {
        base_version
    } else {
        format!(
            "{base_version}-dev.g{}",
            commit.chars().take(12).collect::<String>()
        )
    };
    if dirty {
        version.push_str(".dirty");
    }

    Ok(BuildInfo {
        version,
        commit: Some(commit),
        exact_tag,
        dirty: Some(dirty),
        official,
    })
}

impl BuildInfo {
    fn unknown() -> Self {
        Self {
            version: "0.0.0-dev.unknown".to_owned(),
            commit: None,
            exact_tag: None,
            dirty: None,
            official: false,
        }
    }
}

fn canonical_version(tag: &str) -> Option<&str> {
    let version = tag.strip_prefix('v')?;
    let mut components = version.split('.');
    let major = components.next()?;
    let minor = components.next()?;
    let patch = components.next()?;
    if components.next().is_some()
        || !valid_component(major)
        || !valid_component(minor)
        || !valid_component(patch)
    {
        return None;
    }
    Some(version)
}

fn valid_component(component: &str) -> bool {
    !component.is_empty()
        && component.bytes().all(|byte| byte.is_ascii_digit())
        && (component == "0" || !component.starts_with('0'))
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .map_err(|error| format!("could not run git: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    String::from_utf8(output.stdout)
        .map(|output| output.trim().to_owned())
        .map_err(|error| format!("git output was not UTF-8: {error}"))
}
