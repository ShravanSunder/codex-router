use std::fs;
use std::path::{Path, PathBuf};

use super::*;

fn append_sqlite_production_source_paths(
    directory: &Path,
    parent_module_file: &Path,
    source_paths: &mut Vec<PathBuf>,
) {
    let parent_source = fs::read_to_string(parent_module_file).unwrap_or_else(|error| {
        panic!(
            "state sqlite parent module should be readable at {}: {error}",
            parent_module_file.display()
        )
    });
    let parent_lines = parent_source.lines().collect::<Vec<_>>();
    let fixture_modules = parent_lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let module_line = line.trim();
            let module_name = module_line
                .strip_prefix("mod ")
                .and_then(|line| line.strip_suffix(';'))?;
            (index > 0 && parent_lines[index - 1].contains("sync-rusqlite-fixtures"))
                .then_some(module_name)
        })
        .collect::<Vec<_>>();
    let entries = fs::read_dir(directory).unwrap_or_else(|error| {
        panic!(
            "state sqlite source directory should be readable at {}: {error}",
            directory.display()
        )
    });
    for entry in entries {
        let entry = entry.unwrap_or_else(|error| {
            panic!("state sqlite source directory entry should be readable: {error}")
        });
        let path = entry.path();
        let file_type = entry.file_type().unwrap_or_else(|error| {
            panic!("state sqlite source entry type should be readable: {error}")
        });
        let module_name = if file_type.is_dir() {
            path.file_name()
        } else {
            path.file_stem()
        };
        if module_name.is_some_and(|name| fixture_modules.iter().any(|fixture| name == *fixture)) {
            continue;
        }
        if file_type.is_dir() {
            let directory_name = entry.file_name();
            let directory_name = directory_name.to_string_lossy();
            if directory_name == "test"
                || directory_name == "tests"
                || directory_name.ends_with("_tests")
            {
                continue;
            }
            let sibling_module_file = path.with_extension("rs");
            let parent_module_file = if sibling_module_file.is_file() {
                sibling_module_file
            } else {
                path.join("mod.rs")
            };
            append_sqlite_production_source_paths(&path, &parent_module_file, source_paths);
        } else if file_type.is_file()
            && path.extension().is_some_and(|extension| extension == "rs")
            && !path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with("_tests.rs"))
        {
            source_paths.push(path);
        }
    }
}

#[test]
fn production_state_storage_does_not_use_rusqlite() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let sqlite_source_path = manifest_dir.join("src/sqlite.rs");
    let sqlite_child_source_directory = manifest_dir.join("src/sqlite");
    let mut source_paths = vec![sqlite_source_path.clone()];
    if sqlite_child_source_directory.is_dir() {
        append_sqlite_production_source_paths(
            &sqlite_child_source_directory,
            &sqlite_source_path,
            &mut source_paths,
        );
    }
    source_paths.sort();
    assert!(
        !source_paths.is_empty(),
        "production state source collection must not be empty"
    );
    assert!(
        source_paths.iter().any(|path| path == &sqlite_source_path),
        "production state source collection must include src/sqlite.rs"
    );

    let mut forbidden_lines = Vec::new();
    for source_path in source_paths {
        let source = fs::read_to_string(&source_path).unwrap_or_else(|error| {
            panic!(
                "state sqlite source should be readable at {}: {error}",
                source_path.display()
            )
        });
        let production_source = source
            .split("#[cfg(test)]")
            .next()
            .unwrap_or(source.as_str());
        let lines = production_source.lines().collect::<Vec<_>>();
        forbidden_lines.extend(lines.iter().enumerate().filter_map(|(index, line)| {
            let references_rusqlite = line.contains("rusqlite::")
                || line.contains("use rusqlite")
                || line.contains("&rusqlite");
            let fenced_by_fixture_feature =
                index > 0 && lines[index - 1].contains("sync-rusqlite-fixtures");
            (references_rusqlite && !fenced_by_fixture_feature).then_some(format!(
                "{}:{}:{}",
                source_path.display(),
                index + 1,
                line.trim()
            ))
        }));
    }

    assert!(
        forbidden_lines.is_empty(),
        "production state storage must be SQLx-only; forbidden rusqlite lines: {forbidden_lines:?}"
    );
}

#[test]
fn reports_package_name() {
    assert_eq!(package_name(), "codex-router-state");
}
