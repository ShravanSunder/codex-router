//! Existing-only identity reads preserve literal files and the complete fixture tree.
use collaboration_service::{load_service_identity, read_existing_service_identity};
use std::{
    error::Error,
    fs, io,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
};

type TestResult = Result<(), Box<dyn Error>>;
const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
const IDENTITY_BYTES: &[u8] =
    br#"{"version":1,"serviceId":"00000000-0000-4000-8000-000000000001"}"#;

#[derive(Debug, Eq, PartialEq)]
struct FixtureNode {
    relative_path: PathBuf,
    metadata: NodeMetadata,
    content: NodeContent,
}

#[derive(Debug, Eq, PartialEq)]
struct NodeMetadata {
    mode: u32,
    device: u64,
    inode: u64,
    links: u64,
    owner: u32,
    group: u32,
    length: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

#[derive(Debug, Eq, PartialEq)]
enum NodeContent {
    Directory,
    RegularFile(Vec<u8>),
    SymbolicLink(PathBuf),
}

fn fixture_root() -> io::Result<tempfile::TempDir> {
    let root = tempfile::tempdir()?;
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))?;
    fs::create_dir(root.path().join("canary-directory"))?;
    fs::write(
        root.path().join("canary-directory/unchanged"),
        b"preserve me",
    )?;
    Ok(root)
}

fn write_identity(directory: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let path = directory.join("service-identity.json");
    fs::write(&path, bytes)?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

fn snapshot_tree(root: &Path) -> io::Result<Vec<FixtureNode>> {
    let mut nodes = Vec::new();
    snapshot_node(root, Path::new(""), &mut nodes)?;
    Ok(nodes)
}

fn snapshot_node(
    root: &Path,
    relative_path: &Path,
    nodes: &mut Vec<FixtureNode>,
) -> io::Result<()> {
    let path = root.join(relative_path);
    let metadata = fs::symlink_metadata(&path)?;
    let content = if metadata.is_dir() {
        NodeContent::Directory
    } else if metadata.file_type().is_symlink() {
        NodeContent::SymbolicLink(fs::read_link(&path)?)
    } else {
        NodeContent::RegularFile(fs::read(&path)?)
    };
    nodes.push(FixtureNode {
        relative_path: relative_path.to_owned(),
        metadata: NodeMetadata {
            mode: metadata.mode(),
            device: metadata.dev(),
            inode: metadata.ino(),
            links: metadata.nlink(),
            owner: metadata.uid(),
            group: metadata.gid(),
            length: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        },
        content,
    });
    // Reads may update access time; byte contents and write/identity metadata are the oracle.
    if metadata.is_dir() {
        let mut children = fs::read_dir(&path)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<io::Result<Vec<_>>>()?;
        children.sort();
        for child in children {
            snapshot_node(root, &relative_path.join(child), nodes)?;
        }
    }
    Ok(())
}

#[allow(clippy::panic_in_result_fn)]
fn assert_refused_unchanged(root: &Path, directory: &Path, expected_error: &str) -> TestResult {
    let before = snapshot_tree(root)?;
    let error = read_existing_service_identity(directory)
        .err()
        .ok_or("existing-only reader unexpectedly accepted fixture")?;
    assert_eq!(error.kind(), io::ErrorKind::Other);
    assert_eq!(error.to_string(), expected_error);
    assert_eq!(snapshot_tree(root)?, before);
    Ok(())
}

#[test]
#[allow(clippy::panic_in_result_fn)]
fn literal_existing_identity_is_returned_without_changing_the_tree() -> TestResult {
    // Arrange: the expected bytes/id are literal, independent of the writer serializer.
    let root = fixture_root()?;
    write_identity(root.path(), IDENTITY_BYTES, 0o600)?;
    let before = snapshot_tree(root.path())?;

    let identity = read_existing_service_identity(root.path())?;

    assert_eq!(String::from(identity), SERVICE_ID);
    assert_eq!(snapshot_tree(root.path())?, before);
    Ok(())
}

#[test]
#[allow(clippy::panic_in_result_fn)]
fn missing_identity_returns_not_found_without_creating_any_node() -> TestResult {
    let root = fixture_root()?;
    let before = snapshot_tree(root.path())?;

    let error = read_existing_service_identity(root.path())
        .err()
        .ok_or("missing identity unexpectedly read")?;

    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert_eq!(snapshot_tree(root.path())?, before);
    Ok(())
}

#[test]
#[allow(clippy::panic_in_result_fn)]
fn missing_directory_remains_absent() -> TestResult {
    let root = fixture_root()?;
    let missing = root.path().join("missing-directory");
    let before = snapshot_tree(root.path())?;

    let error = read_existing_service_identity(&missing)
        .err()
        .ok_or("missing directory unexpectedly read")?;

    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert!(!missing.exists());
    assert_eq!(snapshot_tree(root.path())?, before);
    Ok(())
}

#[test]
fn invalid_identity_contents_are_refused_without_repair() -> TestResult {
    let cases: &[(&str, &[u8], &str)] = &[
        ("corrupted-json", b"broken", "invalid identity content"),
        (
            "unsupported-version",
            br#"{"version":2,"serviceId":"00000000-0000-4000-8000-000000000001"}"#,
            "unsupported identity version",
        ),
        (
            "invalid-id",
            br#"{"version":1,"serviceId":"not-a-uuid"}"#,
            "invalid identity content",
        ),
        (
            "uppercase-id",
            br#"{"version":1,"serviceId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaA"}"#,
            "invalid identity content",
        ),
        (
            "missing-id",
            br#"{"version":1}"#,
            "invalid identity content",
        ),
        (
            "unknown-field",
            br#"{"version":1,"serviceId":"00000000-0000-4000-8000-000000000001","extra":true}"#,
            "invalid identity content",
        ),
    ];
    for (case_name, bytes, expected_error) in cases {
        let root = fixture_root()?;
        write_identity(root.path(), bytes, 0o600)?;
        assert_refused_unchanged(root.path(), root.path(), expected_error)
            .map_err(|error| format!("{case_name}: {error}"))?;
    }
    Ok(())
}

#[test]
fn oversized_identity_is_refused_without_replacement() -> TestResult {
    let root = fixture_root()?;
    write_identity(root.path(), &vec![b' '; 1025], 0o600)?;
    assert_refused_unchanged(root.path(), root.path(), "invalid identity file")
}

#[test]
fn insecure_identity_permissions_are_refused_unchanged() -> TestResult {
    for mode in [0o604, 0o640] {
        let root = fixture_root()?;
        write_identity(root.path(), IDENTITY_BYTES, mode)?;
        assert_refused_unchanged(root.path(), root.path(), "invalid identity file")?;
    }
    Ok(())
}

#[test]
fn symlink_and_directory_identity_nodes_are_refused_unchanged() -> TestResult {
    let root = fixture_root()?;
    let target = root.path().join("identity-target.json");
    fs::write(&target, IDENTITY_BYTES)?;
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600))?;
    symlink(&target, root.path().join("service-identity.json"))?;
    assert_refused_unchanged(root.path(), root.path(), "invalid identity file")?;

    let root = fixture_root()?;
    fs::create_dir(root.path().join("service-identity.json"))?;
    assert_refused_unchanged(root.path(), root.path(), "invalid identity file")
}

#[test]
fn invalid_directory_shape_and_privacy_are_refused_unchanged() -> TestResult {
    let root = fixture_root()?;
    let private_directory = root.path().join("private");
    fs::create_dir(&private_directory)?;
    fs::set_permissions(&private_directory, fs::Permissions::from_mode(0o700))?;
    write_identity(&private_directory, IDENTITY_BYTES, 0o600)?;
    let symbolic_directory = root.path().join("symbolic-directory");
    symlink(&private_directory, &symbolic_directory)?;
    assert_refused_unchanged(
        root.path(),
        &symbolic_directory,
        "identity directory must be private and absolute",
    )?;

    let regular_file = root.path().join("regular-file");
    fs::write(&regular_file, b"not a directory")?;
    assert_refused_unchanged(
        root.path(),
        &regular_file,
        "identity directory must be private and absolute",
    )?;

    for mode in [0o705, 0o750] {
        fs::set_permissions(&private_directory, fs::Permissions::from_mode(mode))?;
        assert_refused_unchanged(
            root.path(),
            &private_directory,
            "identity directory must be private and absolute",
        )?;
    }
    Ok(())
}

#[test]
#[allow(clippy::panic_in_result_fn)]
fn relative_path_to_an_existing_private_directory_is_refused_unchanged() -> TestResult {
    let root = fixture_root()?;
    write_identity(root.path(), IDENTITY_BYTES, 0o600)?;
    let mut relative_directory = PathBuf::new();
    for _ in std::env::current_dir()?.components().skip(1) {
        relative_directory.push("..");
    }
    relative_directory.push(root.path().strip_prefix("/")?);
    assert!(!relative_directory.is_absolute());
    assert_eq!(
        fs::canonicalize(&relative_directory)?,
        fs::canonicalize(root.path())?
    );
    assert_refused_unchanged(
        root.path(),
        &relative_directory,
        "identity directory must be private and absolute",
    )
}

#[test]
#[allow(clippy::panic_in_result_fn)]
fn fresh_loader_creates_once_and_corruption_never_reinitializes_identity() -> TestResult {
    let root = fixture_root()?;
    assert!(!root.path().join("service-identity.json").exists());
    let created = load_service_identity(root.path())?;
    let before = snapshot_tree(root.path())?;

    assert_eq!(load_service_identity(root.path())?, created);
    assert_eq!(read_existing_service_identity(root.path())?, created);
    assert_eq!(snapshot_tree(root.path())?, before);
    assert_eq!(
        fs::metadata(root.path().join("service-identity.json"))?.mode() & 0o777,
        0o600
    );

    write_identity(root.path(), b"broken", 0o600)?;
    let corrupted = snapshot_tree(root.path())?;
    assert!(load_service_identity(root.path()).is_err());
    assert_eq!(snapshot_tree(root.path())?, corrupted);
    Ok(())
}
