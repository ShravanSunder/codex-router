use collaboration_client::{BoardRepositoryError, BoardRepositoryLocation};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct RepositoryFixture(PathBuf);

impl RepositoryFixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let suffix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!(
            "board-sdk-repository-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir(&root)?;
        Ok(Self(root))
    }
}

impl Drop for RepositoryFixture {
    fn drop(&mut self) {
        let _removed = std::fs::remove_dir_all(&self.0);
    }
}

fn git(root: &Path, arguments: &[&str]) -> TestResult {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other("fixture Git command failed").into());
    }
    Ok(())
}

#[test]
fn nested_paths_resolve_to_one_common_directory_and_origin_takes_precedence() -> TestResult {
    let fixture = RepositoryFixture::new()?;
    git(&fixture.0, &["init", "--quiet"])?;
    let nested = fixture.0.join("nested");
    std::fs::create_dir(&nested)?;

    let root_location = BoardRepositoryLocation::discover(&fixture.0)?;
    let nested_location = BoardRepositoryLocation::discover(&nested)?;
    match (root_location, nested_location) {
        (BoardRepositoryLocation::Local(root), BoardRepositoryLocation::Local(child)) => {
            assert_eq!(root, child);
            assert_eq!(
                Path::new(root.as_str()),
                fixture.0.join(".git").canonicalize()?
            );
        }
        _ => {
            return Err(std::io::Error::other(
                "origin-free repository must use its common directory",
            )
            .into());
        }
    }

    git(
        &fixture.0,
        &[
            "remote",
            "add",
            "origin",
            "git@EXAMPLE.com:Team/repository.git",
        ],
    )?;
    match BoardRepositoryLocation::discover(&nested)? {
        BoardRepositoryLocation::Origin(origin) => {
            let BoardRepositoryLocation::Origin(expected) =
                BoardRepositoryLocation::from_origin("https://example.com/Team/repository.git")?
            else {
                return Err(
                    std::io::Error::other("explicit origin must remain origin-based").into(),
                );
            };
            assert_eq!(origin, expected);
        }
        _ => return Err(std::io::Error::other("configured origin must take precedence").into()),
    }
    Ok(())
}

#[test]
fn missing_repository_and_unreadable_path_are_distinct() -> TestResult {
    let fixture = RepositoryFixture::new()?;
    assert!(matches!(
        BoardRepositoryLocation::discover(&fixture.0),
        Err(BoardRepositoryError::MissingRepository)
    ));
    assert!(matches!(
        BoardRepositoryLocation::discover(&fixture.0.join("absent")),
        Err(BoardRepositoryError::UnreadablePath)
    ));
    Ok(())
}
