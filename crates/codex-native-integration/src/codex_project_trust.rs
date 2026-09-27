//! Read-only projection of Codex's project trust keys for the provider TUI face.
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectTrustMatchKind {
    WorkingDirectory,
    ProjectRoot,
    MainRepository,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectTrustAnswer {
    Trusted {
        matched_key: String,
        match_kind: ProjectTrustMatchKind,
    },
    Untrusted {
        trust_target: String,
        explicitly_untrusted: bool,
    },
    ConfigUnavailable {
        reason: String,
        trust_target: String,
    },
}

pub trait CodexProjectTrustLookup: Send + Sync {
    fn project_trust(&self, cwd: &Path) -> ProjectTrustAnswer;
}

pub struct CodexHomeProjectTrust {
    codex_home: PathBuf,
}

impl CodexHomeProjectTrust {
    #[must_use]
    pub fn new(codex_home: PathBuf) -> Self {
        Self { codex_home }
    }
}

impl CodexProjectTrustLookup for CodexHomeProjectTrust {
    fn project_trust(&self, cwd: &Path) -> ProjectTrustAnswer {
        let fallback = cwd.to_string_lossy().into_owned();
        let config = match fs::read_to_string(self.codex_home.join("config.toml")) {
            Ok(config) => config,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return ProjectTrustAnswer::Untrusted {
                    trust_target: fallback,
                    explicitly_untrusted: false,
                };
            }
            Err(error) => {
                return ProjectTrustAnswer::ConfigUnavailable {
                    reason: error.kind().to_string(),
                    trust_target: fallback,
                };
            }
        };
        let config: toml::Value = match toml::from_str(&config) {
            Ok(config) => config,
            Err(error) => {
                return ProjectTrustAnswer::ConfigUnavailable {
                    reason: format!("invalid Codex config: {error}"),
                    trust_target: fallback,
                };
            }
        };
        let markers = match config.get("project_root_markers") {
            None => vec![".git".to_string()],
            Some(toml::Value::Array(entries)) => {
                let Some(markers) = entries
                    .iter()
                    .map(|entry| entry.as_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>()
                else {
                    return ProjectTrustAnswer::ConfigUnavailable {
                        reason: "invalid project root markers".into(),
                        trust_target: fallback,
                    };
                };
                markers
            }
            Some(_) => {
                return ProjectTrustAnswer::ConfigUnavailable {
                    reason: "invalid project root markers".into(),
                    trust_target: fallback,
                };
            }
        };
        let projects = config.get("projects").and_then(toml::Value::as_table);
        let mut candidates = vec![(cwd.to_path_buf(), ProjectTrustMatchKind::WorkingDirectory)];
        if let Some(root) = project_root(cwd, &markers) {
            candidates.push((root, ProjectTrustMatchKind::ProjectRoot));
        }
        if let Some(root) = main_repository_root(cwd) {
            candidates.push((root, ProjectTrustMatchKind::MainRepository));
        }
        let trust_target = candidates.last().map_or_else(
            || fallback.clone(),
            |(path, _)| path.to_string_lossy().into_owned(),
        );
        for (path, kind) in candidates {
            for key in path_keys(&path) {
                let Some(entry) = projects.and_then(|projects| projects.get(&key)) else {
                    continue;
                };
                let Some(raw_level) = entry.get("trust_level") else {
                    continue;
                };
                return match raw_level.as_str() {
                    Some("trusted") => ProjectTrustAnswer::Trusted {
                        matched_key: key,
                        match_kind: kind,
                    },
                    Some("untrusted") => ProjectTrustAnswer::Untrusted {
                        trust_target: key,
                        explicitly_untrusted: true,
                    },
                    _ => ProjectTrustAnswer::ConfigUnavailable {
                        reason: "unknown Codex trust level".into(),
                        trust_target: key,
                    },
                };
            }
        }
        ProjectTrustAnswer::Untrusted {
            trust_target,
            explicitly_untrusted: false,
        }
    }
}

fn path_keys(path: &Path) -> Vec<String> {
    let raw_key = path.to_string_lossy().into_owned();
    let mut keys = Vec::with_capacity(2);
    if let Ok(canonical) = fs::canonicalize(path) {
        let key = canonical.to_string_lossy().into_owned();
        keys.push(key);
    }
    if !keys.contains(&raw_key) {
        keys.push(raw_key);
    }
    keys
}

fn project_root(cwd: &Path, markers: &[String]) -> Option<PathBuf> {
    for ancestor in cwd.ancestors() {
        for marker in markers {
            let marker_path = ancestor.join(marker);
            if marker_path.exists()
                && (marker != ".git" || !marker_path.is_dir() || marker_path.join("HEAD").exists())
            {
                return Some(ancestor.to_path_buf());
            }
        }
    }
    None
}

// Mirrors Codex git-utils trust.rs: a linked checkout must point back to its
// registration and the main checkout must own the common Git directory.
fn main_repository_root(cwd: &Path) -> Option<PathBuf> {
    let checkout = project_root(cwd, &[".git".into()])?;
    let dot_git = checkout.join(".git");
    if dot_git.is_dir() {
        return Some(checkout);
    }
    let pointer = fs::read_to_string(&dot_git).ok()?;
    let git_dir = dot_git
        .parent()?
        .join(pointer.trim().strip_prefix("gitdir:")?.trim());
    let git_dir = fs::canonicalize(git_dir).ok()?;
    if git_dir.parent()?.file_name()? != "worktrees" {
        return None;
    }
    let common_dir = git_dir.parent()?.parent()?;
    let registered = fs::read_to_string(git_dir.join("gitdir")).ok()?;
    let registered_dot_git = git_dir.join(registered.trim());
    if registered_dot_git.file_name()? != ".git"
        || fs::canonicalize(registered_dot_git.parent()?).ok()?
            != fs::canonicalize(&checkout).ok()?
    {
        return None;
    }
    let commondir = fs::read_to_string(git_dir.join("commondir")).ok()?;
    if fs::canonicalize(git_dir.join(commondir.trim())).ok()?
        != fs::canonicalize(common_dir).ok()?
    {
        return None;
    }
    let main_root = common_dir.parent()?;
    let main_git = main_root.join(".git");
    let main_git_dir = if main_git.is_dir() {
        main_git
    } else {
        let pointer = fs::read_to_string(&main_git).ok()?;
        main_git
            .parent()?
            .join(pointer.trim().strip_prefix("gitdir:")?.trim())
    };
    (fs::canonicalize(main_git_dir).ok()? == fs::canonicalize(common_dir).ok()?)
        .then(|| main_root.to_path_buf())
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;
    use std::{error::Error, process::Command};

    fn write_config(home: &Path, body: &str) -> Result<(), Box<dyn Error>> {
        fs::create_dir_all(home)?;
        fs::write(home.join("config.toml"), body)?;
        Ok(())
    }

    fn trust_entry(path: &Path) -> String {
        format!(
            "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            path.display()
        )
    }

    /// Oracle: Codex config/src/loader/mod.rs:1027-1041 and 1366-1382.
    #[test]
    fn exact_and_canonical_working_directory_keys() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let home = root.path().join("home");
        let project = root.path().join("project");
        let alias = root.path().join("alias");
        fs::create_dir(&project)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&project, &alias)?;
        write_config(&home, &trust_entry(&alias))?;
        let lookup = CodexHomeProjectTrust::new(home.clone());
        assert!(
            matches!(lookup.project_trust(&alias), ProjectTrustAnswer::Trusted { matched_key, match_kind: ProjectTrustMatchKind::WorkingDirectory } if matched_key == alias.to_string_lossy()),
            "{:?}",
            lookup.project_trust(&alias)
        );
        let canonical_project = fs::canonicalize(&project)?;
        write_config(&home, &trust_entry(&canonical_project))?;
        assert!(
            matches!(lookup.project_trust(&alias), ProjectTrustAnswer::Trusted { matched_key, match_kind: ProjectTrustMatchKind::WorkingDirectory } if matched_key == canonical_project.to_string_lossy())
        );
        Ok(())
    }

    /// Oracle: Codex config/src/loader/mod.rs:1359-1371.
    #[test]
    fn canonical_key_precedes_the_raw_alias_when_both_have_trust() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let home = root.path().join("home");
        let project = root.path().join("project");
        let alias = root.path().join("alias");
        fs::create_dir(&project)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&project, &alias)?;
        let canonical = fs::canonicalize(&project)?;
        write_config(
            &home,
            &format!(
                "[projects.\"{}\"]\ntrust_level = \"untrusted\"\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
                alias.display(),
                canonical.display()
            ),
        )?;
        let lookup = CodexHomeProjectTrust::new(home);
        assert!(
            matches!(lookup.project_trust(&alias), ProjectTrustAnswer::Trusted { matched_key, .. } if matched_key == canonical.to_string_lossy())
        );
        Ok(())
    }

    /// Oracle: Codex config/src/loader/mod.rs:1041-1054,1464-1490.
    #[test]
    fn marker_root_matches_but_arbitrary_parent_does_not() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let home = root.path().join("home");
        let repo = root.path().join("repo");
        let child = repo.join("child");
        fs::create_dir_all(&child)?;
        fs::create_dir(repo.join(".git"))?;
        fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n")?;
        write_config(&home, &trust_entry(&repo))?;
        let lookup = CodexHomeProjectTrust::new(home.clone());
        assert!(
            matches!(
                lookup.project_trust(&child),
                ProjectTrustAnswer::Trusted {
                    match_kind: ProjectTrustMatchKind::ProjectRoot,
                    ..
                }
            ),
            "{:?}",
            lookup.project_trust(&child)
        );
        fs::remove_dir_all(repo.join(".git"))?;
        assert!(matches!(
            lookup.project_trust(&child),
            ProjectTrustAnswer::Untrusted { .. }
        ));
        write_config(
            &home,
            &format!(
                "project_root_markers = [\"marker.txt\"]\n{}",
                trust_entry(&repo)
            ),
        )?;
        fs::write(repo.join("marker.txt"), "")?;
        assert!(matches!(
            lookup.project_trust(&child),
            ProjectTrustAnswer::Trusted {
                match_kind: ProjectTrustMatchKind::ProjectRoot,
                ..
            }
        ));
        Ok(())
    }

    /// Oracle: Codex git-utils/src/trust.rs:9-140; config/src/loader/mod.rs:1054-1067.
    #[test]
    fn linked_worktree_inherits_verified_main_repository_trust() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let home = root.path().join("home");
        let main = root.path().join("main");
        let linked = root.path().join("linked");
        let git = |args: &[&str]| -> Result<(), Box<dyn Error>> {
            let output = Command::new("git").args(args).current_dir(&main).output()?;
            if !output.status.success() {
                return Err(
                    format!("git failed: {}", String::from_utf8_lossy(&output.stderr)).into(),
                );
            }
            Ok(())
        };
        fs::create_dir(&main)?;
        git(&["init"])?;
        fs::write(main.join("README"), "fixture")?;
        git(&["add", "README"])?;
        git(&[
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "fixture",
        ])?;
        git(&[
            "worktree",
            "add",
            "--detach",
            linked.to_str().ok_or("non-UTF8 path")?,
        ])?;
        write_config(&home, &trust_entry(&fs::canonicalize(&main)?))?;
        let lookup = CodexHomeProjectTrust::new(home);
        assert!(matches!(
            lookup.project_trust(&linked),
            ProjectTrustAnswer::Trusted {
                match_kind: ProjectTrustMatchKind::MainRepository,
                ..
            }
        ));
        Ok(())
    }

    /// Oracle: Codex config/src/loader/mod.rs:1027-1072.
    #[test]
    fn missing_or_invalid_config_fails_closed() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let home = root.path().join("home");
        let project = root.path().join("project");
        fs::create_dir(&project)?;
        let lookup = CodexHomeProjectTrust::new(home.clone());
        assert!(matches!(
            lookup.project_trust(&project),
            ProjectTrustAnswer::Untrusted { .. }
        ));
        write_config(&home, "not valid = [")?;
        assert!(matches!(
            lookup.project_trust(&project),
            ProjectTrustAnswer::ConfigUnavailable { .. }
        ));
        write_config(
            &home,
            &format!(
                "[projects.\"{}\"]\ntrust_level = \"perhaps\"\n",
                project.display()
            ),
        )?;
        assert!(matches!(
            lookup.project_trust(&project),
            ProjectTrustAnswer::ConfigUnavailable { .. }
        ));
        Ok(())
    }
}
