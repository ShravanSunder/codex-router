//! Bind the global Claude runtime before an ACP package runner changes PATH.

use std::{ffi::OsStr, fs, path::Path};

const CLAUDE_EXECUTABLE_ENV: &str = "CLAUDE_CODE_EXECUTABLE";
#[cfg(windows)]
const CLAUDE_EXECUTABLE_FILENAME: &str = "claude.exe";
#[cfg(not(windows))]
const CLAUDE_EXECUTABLE_FILENAME: &str = "claude";

pub(crate) fn bind_global_claude_executable(environment: &mut Vec<(String, String)>) -> bool {
    let inherited_path = std::env::var_os("PATH");
    bind_global_claude_executable_from_path(environment, inherited_path.as_deref())
}

fn bind_global_claude_executable_from_path(
    environment: &mut Vec<(String, String)>,
    inherited_path: Option<&OsStr>,
) -> bool {
    // Match the last declared PATH forwarded by the generic ACP spawn owner.
    let search_path = environment
        .iter()
        .rev()
        .find(|(name, _)| name == "PATH")
        .map(|(_, value)| OsStr::new(value))
        .or(inherited_path);
    let Some(executable) = search_path.and_then(resolve_global_claude_executable) else {
        return false;
    };
    environment.retain(|(name, _)| name != CLAUDE_EXECUTABLE_ENV);
    environment.push((CLAUDE_EXECUTABLE_ENV.to_owned(), executable));
    true
}

fn resolve_global_claude_executable(search_path: &OsStr) -> Option<String> {
    std::env::split_paths(search_path).find_map(|directory| {
        if !directory.is_absolute() || is_package_search_path(&directory) {
            return None;
        }
        let canonical_directory = fs::canonicalize(directory).ok()?;
        if is_package_search_path(&canonical_directory) {
            return None;
        }
        let executable =
            fs::canonicalize(canonical_directory.join(CLAUDE_EXECUTABLE_FILENAME)).ok()?;
        if is_transient_package_path(&executable) || is_claude_sdk_path(&executable) {
            return None;
        }
        let metadata = fs::metadata(&executable).ok()?;
        if !metadata.is_file() || !has_executable_permission(&executable) {
            return None;
        }
        // The existing ACP environment contract is UTF-8; never bind a lossy path.
        executable.to_str().map(str::to_owned)
    })
}

fn is_package_search_path(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "node_modules")
        || is_transient_package_path(path)
}

fn is_transient_package_path(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component.as_os_str().to_str(), Some("_npx" | "dlx")))
}

fn is_claude_sdk_path(path: &Path) -> bool {
    path.components().any(|component| {
        component.as_os_str().to_str().is_some_and(|name| {
            name.starts_with("claude-agent-sdk")
                || name.starts_with("@anthropic-ai+claude-agent-sdk")
        })
    })
}

#[cfg(unix)]
fn has_executable_permission(executable: &Path) -> bool {
    rustix::fs::accessat(
        rustix::fs::CWD,
        executable,
        rustix::fs::Access::EXEC_OK,
        rustix::fs::AtFlags::EACCESS,
    )
    .is_ok()
}

#[cfg(not(unix))]
fn has_executable_permission(_executable: &Path) -> bool {
    true
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::{PermissionsExt as _, symlink},
        path::PathBuf,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn executable_in(directory: &Path) -> Result<PathBuf, std::io::Error> {
        fs::create_dir_all(directory)?;
        let executable = directory.join("claude");
        fs::write(&executable, "#!/bin/sh\nexit 0\n")?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
        Ok(executable)
    }

    #[test]
    fn global_executable_is_canonical_and_nonempty() -> TestResult {
        let root = tempfile::tempdir()?;
        let executable = executable_in(root.path())?;
        let resolved = resolve_global_claude_executable(root.path().as_os_str());
        assert_eq!(
            resolved,
            Some(executable.canonicalize()?.to_string_lossy().into_owned())
        );
        Ok(())
    }

    #[test]
    fn last_declared_path_wins_and_replaces_duplicate_native_overrides() -> TestResult {
        let root = tempfile::tempdir()?;
        let global_directory = root.path().join("global-bin");
        let executable = executable_in(&global_directory)?;
        let mut environment = vec![
            ("PATH".to_owned(), "/missing/first-path".to_owned()),
            (
                CLAUDE_EXECUTABLE_ENV.to_owned(),
                "/wrong/first-native".to_owned(),
            ),
            (
                "PATH".to_owned(),
                global_directory.to_string_lossy().into_owned(),
            ),
            (
                CLAUDE_EXECUTABLE_ENV.to_owned(),
                "/wrong/last-native".to_owned(),
            ),
            ("UNRELATED_SETTING".to_owned(), "preserved".to_owned()),
        ];
        assert!(bind_global_claude_executable_from_path(
            &mut environment,
            Some(OsStr::new("/missing/inherited"))
        ));
        let native_selections: Vec<_> = environment
            .iter()
            .filter(|(name, _)| name == CLAUDE_EXECUTABLE_ENV)
            .collect();
        assert_eq!(native_selections.len(), 1);
        let expected_executable = executable.canonicalize()?;
        assert!(
            native_selections
                .iter()
                .all(|(_, value)| Path::new(value) == expected_executable)
        );
        assert!(
            environment
                .iter()
                .any(|(name, value)| name == "UNRELATED_SETTING" && value == "preserved")
        );
        Ok(())
    }

    #[test]
    fn absent_declared_path_uses_inherited_global_search_path() -> TestResult {
        let root = tempfile::tempdir()?;
        executable_in(root.path())?;
        let mut environment = Vec::new();
        assert!(bind_global_claude_executable_from_path(
            &mut environment,
            Some(root.path().as_os_str())
        ));
        Ok(())
    }

    #[test]
    fn empty_declared_path_never_falls_back_to_inherited_path() -> TestResult {
        let root = tempfile::tempdir()?;
        executable_in(root.path())?;
        let mut environment = vec![("PATH".to_owned(), String::new())];
        assert!(!bind_global_claude_executable_from_path(
            &mut environment,
            Some(root.path().as_os_str())
        ));
        Ok(())
    }

    #[test]
    fn missing_empty_and_relative_search_paths_reject_selection() {
        assert!(resolve_global_claude_executable(OsStr::new("")).is_none());
        assert!(resolve_global_claude_executable(OsStr::new("relative/bin")).is_none());
        assert!(!bind_global_claude_executable_from_path(
            &mut Vec::new(),
            None
        ));
    }

    #[test]
    fn nonexecutable_and_directory_candidates_are_skipped() -> TestResult {
        let root = tempfile::tempdir()?;
        let nonexecutable_directory = root.path().join("nonexecutable");
        let nonexecutable = executable_in(&nonexecutable_directory)?;
        fs::set_permissions(nonexecutable, fs::Permissions::from_mode(0o600))?;
        let directory_candidate = root.path().join("directory");
        fs::create_dir_all(directory_candidate.join("claude"))?;
        assert!(resolve_global_claude_executable(nonexecutable_directory.as_os_str()).is_none());
        assert!(resolve_global_claude_executable(directory_candidate.as_os_str()).is_none());
        Ok(())
    }

    #[test]
    fn other_execute_bit_does_not_make_an_owned_file_executable() -> TestResult {
        let root = tempfile::tempdir()?;
        let executable = executable_in(root.path())?;
        fs::set_permissions(executable, fs::Permissions::from_mode(0o601))?;
        assert!(resolve_global_claude_executable(root.path().as_os_str()).is_none());
        Ok(())
    }

    #[test]
    fn package_search_directory_alias_is_rejected() -> TestResult {
        let root = tempfile::tempdir()?;
        let project_directory = root.path().join("project/node_modules/.bin");
        executable_in(&project_directory)?;
        let directory_alias = root.path().join("bin-alias");
        symlink(project_directory, &directory_alias)?;
        assert!(resolve_global_claude_executable(directory_alias.as_os_str()).is_none());
        Ok(())
    }

    #[test]
    fn project_and_transient_package_search_entries_are_skipped() -> TestResult {
        let root = tempfile::tempdir()?;
        let project_directory = root.path().join("project/node_modules/.bin");
        let npx_directory = root.path().join(".npm/_npx/fixture/node_modules/.bin");
        let dlx_directory = root.path().join("pnpm/dlx/fixture/bin");
        let global_directory = root.path().join("global-bin");
        for directory in [&project_directory, &npx_directory, &dlx_directory] {
            executable_in(directory)?;
            assert!(resolve_global_claude_executable(directory.as_os_str()).is_none());
        }
        let global_executable = executable_in(&global_directory)?;
        let path = std::env::join_paths([
            project_directory,
            npx_directory,
            dlx_directory,
            global_directory,
        ])?;
        assert_eq!(
            resolve_global_claude_executable(&path),
            Some(
                global_executable
                    .canonicalize()?
                    .to_string_lossy()
                    .into_owned()
            )
        );
        Ok(())
    }

    #[test]
    fn sdk_native_symlink_is_rejected() -> TestResult {
        let root = tempfile::tempdir()?;
        let sdk_executable = executable_in(
            &root
                .path()
                .join("node_modules/@anthropic-ai/claude-agent-sdk-darwin-arm64"),
        )?;
        let global_directory = root.path().join("global-bin");
        fs::create_dir(&global_directory)?;
        symlink(sdk_executable, global_directory.join("claude"))?;
        assert!(resolve_global_claude_executable(global_directory.as_os_str()).is_none());
        Ok(())
    }

    #[test]
    fn transient_package_symlink_is_rejected() -> TestResult {
        let root = tempfile::tempdir()?;
        let transient_executable = executable_in(&root.path().join(".npm/_npx/fixture"))?;
        let global_directory = root.path().join("global-bin");
        fs::create_dir(&global_directory)?;
        symlink(transient_executable, global_directory.join("claude"))?;
        assert!(resolve_global_claude_executable(global_directory.as_os_str()).is_none());
        Ok(())
    }

    #[test]
    fn installed_claude_code_package_symlink_remains_global() -> TestResult {
        let root = tempfile::tempdir()?;
        let global_package_executable = executable_in(
            &root
                .path()
                .join("global/lib/node_modules/@anthropic-ai/claude-code/bin"),
        )?;
        let global_directory = root.path().join("global/bin");
        fs::create_dir(&global_directory)?;
        symlink(&global_package_executable, global_directory.join("claude"))?;
        assert_eq!(
            resolve_global_claude_executable(global_directory.as_os_str()),
            Some(
                global_package_executable
                    .canonicalize()?
                    .to_string_lossy()
                    .into_owned()
            )
        );
        Ok(())
    }

    #[test]
    fn broken_symlink_is_not_a_global_installation() -> TestResult {
        let root = tempfile::tempdir()?;
        symlink(root.path().join("missing"), root.path().join("claude"))?;
        assert!(resolve_global_claude_executable(root.path().as_os_str()).is_none());
        Ok(())
    }
}
