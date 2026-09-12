//! Repository identity from origins, live worktrees and bounded historical fallbacks.
use super::non_empty_trimmed;
use codex_native_integration::path_sql_values;
use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub(super) fn path_identity_candidates(path: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![path.to_path_buf(), normalize_path(path)];
    candidates.sort();
    candidates.dedup();
    candidates
}

pub(super) fn normalize_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_error| path.to_path_buf())
}

pub(super) fn find_worktree_root(current_dir: &Path) -> Option<PathBuf> {
    for ancestor in current_dir.ancestors() {
        if ancestor.join(".git").exists() {
            return Some(normalize_path(ancestor));
        }
    }
    None
}

fn checkout_root(current_dir: &Path) -> PathBuf {
    find_worktree_root(current_dir).unwrap_or_else(|| normalize_path(current_dir))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RepositoryIdentity {
    pub(crate) normalized_origin: Option<String>,
    pub(crate) live_roots: Vec<PathBuf>,
    pub(crate) repository_basename: String,
    pub(crate) fallback_cwd: Option<PathBuf>,
}

impl RepositoryIdentity {
    pub(super) fn discover(current_dir: &Path) -> Self {
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
        Self {
            normalized_origin,
            live_roots,
            repository_basename,
            fallback_cwd: (!has_git_repository_evidence).then(|| normalize_path(current_dir)),
        }
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

pub(super) fn normalize_git_origin_url(origin: &str) -> Option<String> {
    project_board::normalize_git_origin_url(origin)
}

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

pub(crate) fn session_belongs_to_repository(
    identity: &RepositoryIdentity,
    row_origin: Option<&str>,
    cwd: &Path,
) -> bool {
    if let Some(fallback_cwd) = &identity.fallback_cwd {
        return normalized_paths_resolve_to_same_location(cwd, fallback_cwd);
    }
    let row_origin = row_origin.and_then(non_empty_trimmed);
    let normalized_row_origin = row_origin.and_then(normalize_git_origin_url);
    if let (Some(current_origin), Some(_)) = (&identity.normalized_origin, row_origin) {
        return normalized_row_origin.is_some_and(|row_origin| row_origin == *current_origin);
    }
    let is_under_live_root = identity
        .live_roots
        .iter()
        .any(|root| path_is_equal_or_child_for_repo(cwd, root));
    let matches_historical_basename = !identity.repository_basename.is_empty()
        && cwd.file_name().and_then(OsStr::to_str).is_some_and(|leaf| {
            leaf == identity.repository_basename
                || leaf
                    .strip_prefix(&identity.repository_basename)
                    .is_some_and(|suffix| suffix.starts_with('.') || suffix.starts_with('-'))
        });

    match (&identity.normalized_origin, row_origin) {
        (Some(_), Some(_)) => false,
        (Some(_), None) | (None, None) => is_under_live_root || matches_historical_basename,
        (None, Some(_)) => is_under_live_root,
    }
}

fn path_is_equal_or_child_for_repo(candidate: &Path, parent: &Path) -> bool {
    path_sql_values(candidate).into_iter().any(|candidate| {
        path_sql_values(parent).into_iter().any(|parent| {
            candidate == parent
                || candidate
                    .strip_prefix(&parent)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        })
    })
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

pub(crate) fn paths_resolve_to_same_location(left: &Path, right: &Path) -> bool {
    let left = normalize_path(left);
    let right = normalize_path(right);
    normalized_paths_resolve_to_same_location(&left, &right)
}

pub(crate) fn normalized_paths_resolve_to_same_location(left: &Path, right: &Path) -> bool {
    path_sql_values(left)
        .iter()
        .any(|left| path_sql_values(right).contains(left))
}
