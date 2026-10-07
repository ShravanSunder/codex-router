//! The driver's own build links the Turso engine and nothing from SQLite or libSQL
//!
//! Scope: a build of the Turso packages by themselves. A combined workspace build unifies
//! features with crates that use SQLx's SQLite driver, so it is not covered by this check.
#![allow(clippy::panic_in_result_fn)]

use std::{collections::BTreeSet, path::PathBuf, process::Command};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

const FORBIDDEN_PACKAGES: [&str; 2] = ["libsqlite3-sys", "sqlx-sqlite"];

#[test]
fn the_driver_build_contains_no_sqlite_or_libsql_engine() -> TestResult {
    // Arrange
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");

    // Act
    let output = Command::new("cargo")
        .args([
            "tree",
            "--locked",
            "--offline",
            "--package",
            "sqlx-turso",
            "--all-features",
            "--edges",
            "normal,build,dev",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .current_dir(&workspace_root)
        .output()?;
    let tree = String::from_utf8(output.stdout)?;
    let packages: BTreeSet<&str> = tree
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();

    // Assert
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(packages.contains("turso"), "the Turso engine is missing");
    let forbidden: Vec<&str> = packages
        .iter()
        .copied()
        .filter(|name| FORBIDDEN_PACKAGES.contains(name) || name.starts_with("libsql"))
        .collect();
    assert!(
        forbidden.is_empty(),
        "non-Turso engines in the graph: {forbidden:?}"
    );
    Ok(())
}
