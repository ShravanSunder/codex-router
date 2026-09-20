//! Repository identity discovered from the invoking checkout and Git metadata.
//!
//! Discovery shells out to Git and touches the filesystem, so it belongs beside the
//! caller. The membership predicate it feeds lives in `codex-native-integration`, next
//! to the SQL clause it has to agree with, and is re-exported here.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Command,
};

use codex_native_integration::non_empty_trimmed;
pub use codex_native_integration::{
    RepositoryIdentity, normalize_path, normalized_paths_resolve_to_same_location,
    path_identity_candidates, paths_resolve_to_same_location, repository_contains_session,
};

pub(super) fn find_worktree_root(current_dir: &Path) -> Option<PathBuf> {
    for ancestor in current_dir.ancestors() {
        if ancestor.join(".git").exists() {
            return Some(normalize_path(ancestor));
        }
    }
    None
}

pub fn checkout_root(current_dir: &Path) -> PathBuf {
    find_worktree_root(current_dir).unwrap_or_else(|| normalize_path(current_dir))
}

/// Discovers repository identity from the invoking directory and Git metadata.
#[must_use]
pub fn discover_repository_identity(current_dir: &Path) -> RepositoryIdentity {
    let current_checkout = checkout_root(current_dir);
    let discovered_live_roots = repo_roots(current_dir);
    let raw_origin = git_stdout(current_dir, &["remote", "get-url", "origin"]);
    let normalized_origin = raw_origin.as_deref().and_then(normalize_git_origin_url);
    let git_common_dir = git_stdout(
        current_dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .map(PathBuf::from);
    let has_git_repository_evidence = normalized_origin.is_some()
        || git_common_dir.is_some()
        || !discovered_live_roots.is_empty();
    let live_roots = live_roots_with_current_checkout_fallback(
        discovered_live_roots,
        &current_checkout,
        has_git_repository_evidence,
    );
    let primary_worktree = live_roots.first().map(PathBuf::as_path);
    let repository_basename = if has_git_repository_evidence {
        repository_basename_from_evidence(
            normalized_origin.as_deref(),
            git_common_dir.as_deref(),
            primary_worktree,
            &current_checkout,
        )
    } else {
        String::new()
    };
    RepositoryIdentity {
        normalized_origin,
        live_roots,
        repository_basename,
        fallback_cwd: (!has_git_repository_evidence).then(|| normalize_path(current_dir)),
    }
}

pub(super) fn live_roots_with_current_checkout_fallback(
    mut live_roots: Vec<PathBuf>,
    current_checkout: &Path,
    has_git_repository_evidence: bool,
) -> Vec<PathBuf> {
    if has_git_repository_evidence && live_roots.is_empty() {
        live_roots.push(current_checkout.to_path_buf());
    }
    live_roots
}

fn git_stdout(current_dir: &Path, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(current_dir)
        .args(arguments)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    non_empty_trimmed(&value).map(str::to_owned)
}

pub(super) use codex_native_integration::normalize_git_origin_url;

pub(super) fn repository_basename_from_evidence(
    normalized_origin: Option<&str>,
    git_common_dir: Option<&Path>,
    primary_worktree: Option<&Path>,
    current_checkout: &Path,
) -> String {
    normalized_origin
        .and_then(|origin| origin.rsplit('/').next())
        .or_else(|| {
            git_common_dir.and_then(|common_dir| {
                if common_dir.file_name() == Some(OsStr::new(".git")) {
                    common_dir.parent().and_then(Path::file_name)
                } else {
                    common_dir.file_name()
                }
                .and_then(OsStr::to_str)
            })
        })
        .or_else(|| {
            primary_worktree
                .and_then(Path::file_name)
                .and_then(OsStr::to_str)
        })
        .or_else(|| current_checkout.file_name().and_then(OsStr::to_str))
        .unwrap_or_default()
        .to_owned()
}

fn repo_roots(current_dir: &Path) -> Vec<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(current_dir)
        .arg("worktree")
        .arg("list")
        .arg("--porcelain")
        .output();
    if let Ok(output) = output
        && output.status.success()
        && let Ok(stdout) = String::from_utf8(output.stdout)
    {
        let roots = parse_git_worktree_roots(&stdout);
        if !roots.is_empty() {
            return roots;
        }
    }
    Vec::new()
}

fn parse_git_worktree_roots(output: &str) -> Vec<PathBuf> {
    output
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
        .map(|path| normalize_path(&path))
        .collect()
}

#[cfg(test)]
#[path = "tests/repository_tests.rs"]
mod tests;
