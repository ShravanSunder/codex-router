//! External test fixture launch only; production image observation stays unchanged.
use std::{
    ffi::OsStr,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

const ORIGINAL_SHEBANG: &str = "#!/usr/bin/env python3\n";
const MAX_PORTABLE_SHEBANG_BYTES: usize = 128;

#[derive(Debug, thiserror::Error)]
enum FixtureLaunchError {
    #[error("fixture interpreter requires the current PATH")]
    MissingPath,
    #[error("no executable python3 selected by the fixture PATH")]
    MissingInterpreter,
    #[error("cannot inspect selected fixture interpreter {path}")]
    InspectInterpreter {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot check execution access to fixture interpreter {path}: {source}")]
    InterpreterAccess {
        path: PathBuf,
        source: rustix::io::Errno,
    },
    #[error("selected interpreter cannot be represented by a portable direct shebang: {0}")]
    UnusableShebang(PathBuf),
    #[error("fixture template no longer has the expected first shebang")]
    TemplateShebang,
}

struct FixtureInterpreter {
    selected_path: PathBuf,
    canonical_path: PathBuf,
    device: u64,
    inode: u64,
}

fn validate_shebang_path(path: &Path) -> Result<&str, FixtureLaunchError> {
    let invalid = || FixtureLaunchError::UnusableShebang(path.to_owned());
    let text = path.to_str().ok_or_else(invalid)?;
    if !path.is_absolute()
        || text
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte == 0)
        || text.len() + 3 > MAX_PORTABLE_SHEBANG_BYTES
    {
        return Err(invalid());
    }
    Ok(text)
}

fn resolve_interpreter(search_path: &OsStr) -> Result<FixtureInterpreter, FixtureLaunchError> {
    for directory in std::env::split_paths(search_path) {
        let candidate = directory.join("python3");
        let metadata = match fs::metadata(&candidate) {
            Ok(metadata) => metadata,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                continue;
            }
            Err(source) => {
                return Err(FixtureLaunchError::InspectInterpreter {
                    path: candidate,
                    source,
                });
            }
        };
        if !metadata.is_file() {
            continue;
        }
        match rustix::fs::accessat(
            rustix::fs::CWD,
            &candidate,
            rustix::fs::Access::EXEC_OK,
            rustix::fs::AtFlags::EACCESS,
        ) {
            Ok(()) => {}
            Err(rustix::io::Errno::ACCESS | rustix::io::Errno::NOENT) => continue,
            Err(source) => {
                return Err(FixtureLaunchError::InterpreterAccess {
                    path: candidate,
                    source,
                });
            }
        }
        // The first executable wins, just as PATH selection does. A shebang
        // representation error never authorizes choosing another interpreter.
        let canonical_path = fs::canonicalize(&candidate).map_err(|source| {
            FixtureLaunchError::InspectInterpreter {
                path: candidate.clone(),
                source,
            }
        })?;
        validate_shebang_path(&canonical_path)?;
        let actual = fs::metadata(&canonical_path).map_err(|source| {
            FixtureLaunchError::InspectInterpreter {
                path: canonical_path.clone(),
                source,
            }
        })?;
        if !actual.is_file() || actual.dev() != metadata.dev() || actual.ino() != metadata.ino() {
            return Err(FixtureLaunchError::UnusableShebang(canonical_path));
        }
        return Ok(FixtureInterpreter {
            selected_path: candidate,
            canonical_path,
            device: actual.dev(),
            inode: actual.ino(),
        });
    }
    Err(FixtureLaunchError::MissingInterpreter)
}

fn replace_shebang(
    source: &str,
    interpreter: &FixtureInterpreter,
) -> Result<String, FixtureLaunchError> {
    let body = source
        .strip_prefix(ORIGINAL_SHEBANG)
        .ok_or(FixtureLaunchError::TemplateShebang)?;
    let path = validate_shebang_path(&interpreter.canonical_path)?;
    Ok(format!("#!{path}\n{body}"))
}

pub(super) fn direct_python_fixture(
    source: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let search_path = std::env::var_os("PATH").ok_or(FixtureLaunchError::MissingPath)?;
    let interpreter = resolve_interpreter(&search_path)?;
    let rewritten = replace_shebang(source, &interpreter)?;
    eprintln!(
        "FIXTURE_INTERPRETER selected={:?} canonical={:?} dev={} ino={} shebang_only=true body_bytes={}",
        interpreter.selected_path,
        interpreter.canonical_path,
        interpreter.device,
        interpreter.inode,
        source
            .strip_prefix(ORIGINAL_SHEBANG)
            .ok_or(FixtureLaunchError::TemplateShebang)?
            .len()
    );
    Ok(rewritten)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
    fn executable(directory: &Path, mode: u32) -> Result<PathBuf, std::io::Error> {
        fs::create_dir(directory)?;
        let path = directory.join("python3");
        fs::write(&path, b"#!/bin/sh\nexit 0\n")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
        fs::canonicalize(path)
    }
    #[test]
    fn selects_first_executable_and_preserves_body_byte_for_byte() -> TestResult {
        let root = tempfile::tempdir()?;
        let first = executable(&root.path().join("first"), 0o755)?;
        let _second = executable(&root.path().join("second"), 0o755)?;
        let search = std::env::join_paths([
            first.parent().ok_or("parent")?,
            root.path().join("second").as_path(),
        ])?;
        let selected = resolve_interpreter(&search)?;
        if selected.canonical_path != first {
            return Err("PATH did not select the first executable".into());
        }
        let source = "#!/usr/bin/env python3\nprint('literal body')\n# retained bytes\n";
        if replace_shebang(source, &selected)?
            != format!(
                "#!{}\nprint('literal body')\n# retained bytes\n",
                first.display()
            )
        {
            return Err("direct shebang rewrite changed the literal body".into());
        }
        Ok(())
    }
    #[test]
    fn skips_nonexecutables_and_resolves_the_selected_alias() -> TestResult {
        let root = tempfile::tempdir()?;
        let _blocked = executable(&root.path().join("blocked"), 0o644)?;
        let actual = executable(&root.path().join("actual"), 0o755)?;
        let alias = root.path().join("alias");
        fs::create_dir(&alias)?;
        std::os::unix::fs::symlink(&actual, alias.join("python3"))?;
        let selected = resolve_interpreter(&std::env::join_paths([
            root.path().join("blocked"),
            alias.clone(),
        ])?)?;
        if selected.selected_path != alias.join("python3") || selected.canonical_path != actual {
            return Err(
                "PATH access/selected alias resolution differs from the actual file".into(),
            );
        }
        Ok(())
    }
    #[test]
    fn invalid_first_selected_path_never_falls_back_to_a_different_interpreter() -> TestResult {
        let root = tempfile::tempdir()?;
        let invalid = executable(&root.path().join("has space"), 0o755)?;
        let _valid = executable(&root.path().join("valid"), 0o755)?;
        let result = resolve_interpreter(&std::env::join_paths([
            invalid.parent().ok_or("parent")?,
            root.path().join("valid").as_path(),
        ])?);
        if !matches!(result, Err(FixtureLaunchError::UnusableShebang(_))) {
            return Err("unusable selected shebang silently fell back".into());
        }
        Ok(())
    }
    #[test]
    fn missing_interpreter_and_invalid_template_are_explicit_errors() -> TestResult {
        let root = tempfile::tempdir()?;
        if !matches!(
            resolve_interpreter(root.path().as_os_str()),
            Err(FixtureLaunchError::MissingInterpreter)
        ) {
            return Err("missing interpreter was not rejected explicitly".into());
        }
        let path = executable(&root.path().join("actual"), 0o755)?;
        let selected = resolve_interpreter(path.parent().ok_or("parent")?.as_os_str())?;
        if !matches!(
            replace_shebang("print('no header')\n", &selected),
            Err(FixtureLaunchError::TemplateShebang)
        ) {
            return Err("invalid fixture template was not rejected explicitly".into());
        }
        Ok(())
    }
    #[test]
    fn relative_control_and_long_shebang_paths_are_rejected() {
        for path in [
            PathBuf::from("relative/python3"),
            PathBuf::from("/path\n/python3"),
            PathBuf::from(format!("/{}", "a".repeat(128))),
        ] {
            assert!(matches!(
                validate_shebang_path(&path),
                Err(FixtureLaunchError::UnusableShebang(_))
            ));
        }
    }
}
