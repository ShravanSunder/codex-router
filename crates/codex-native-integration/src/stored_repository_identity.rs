//! Repository membership for one stored Codex thread row.
//!
//! This is the row-side twin of the repository clause in `stored_thread_query.rs`. The
//! SQL bounds the scan and this predicate decides, so both readings of `--repo` live in
//! one crate and any drift between them shows up in a single diff.

use crate::path_sql_values;
use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

pub use message_board::normalize_git_origin_url;

/// Repository evidence used to include live and historical worktree sessions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryIdentity {
    pub normalized_origin: Option<String>,
    pub live_roots: Vec<PathBuf>,
    pub repository_basename: String,
    pub fallback_cwd: Option<PathBuf>,
}

#[must_use]
pub fn path_identity_candidates(path: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![path.to_path_buf(), normalize_path(path)];
    candidates.sort();
    candidates.dedup();
    candidates
}

#[must_use]
pub fn normalize_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_error| path.to_path_buf())
}

#[must_use]
pub fn paths_resolve_to_same_location(left: &Path, right: &Path) -> bool {
    let left = normalize_path(left);
    let right = normalize_path(right);
    normalized_paths_resolve_to_same_location(&left, &right)
}

#[must_use]
pub fn normalized_paths_resolve_to_same_location(left: &Path, right: &Path) -> bool {
    path_sql_values(left)
        .iter()
        .any(|left| path_sql_values(right).contains(left))
}

#[must_use]
pub fn non_empty_trimmed(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Tests whether a stored session belongs to the supplied repository identity.
///
/// An origin on the row decides the match on its own; the live-root and historical
/// basename fallbacks only rescue a row that carries no origin at all.
#[must_use]
pub fn repository_contains_session(
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

#[cfg(test)]
#[path = "stored_repository_identity_tests.rs"]
mod tests;
