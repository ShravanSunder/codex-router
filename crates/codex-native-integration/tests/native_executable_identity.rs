use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use codex_native_integration::UpdaterCommandSpec;
use codex_native_integration::executable_identity;
use codex_native_integration::managed_executable_version;
use codex_native_integration::{RecordedExecutableIdentity, RecordedExecutableIdentityError};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[tokio::test]
async fn captured_identity_reconstructs_after_file_removal_without_observation()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let executable = directory.path().join("codex");
    std::fs::write(&executable, b"abc")?;
    let observed = executable_identity(&executable).await?;
    let captured = RecordedExecutableIdentity::from(&observed);
    let expected_digest = [
        0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22,
        0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00,
        0x15, 0xad,
    ];

    std::fs::remove_file(&executable)?;
    let reconstructed =
        RecordedExecutableIdentity::new(captured.recorded_path().to_path_buf(), expected_digest)?;

    if captured.recorded_path() != observed.canonical_path() {
        return Err("capture must retain the observed canonical path".into());
    }
    if captured.content_digest() != &expected_digest {
        return Err("capture must retain the known SHA-256 of abc".into());
    }
    if reconstructed != captured || !reconstructed.matches_observed(&observed) {
        return Err(
            "structural reconstruction after deletion must preserve captured identity".into(),
        );
    }
    if executable_identity(&executable).await.is_ok() {
        return Err("fresh observation must fail after the executable is removed".into());
    }
    Ok(())
}

#[tokio::test]
async fn captured_identity_distinguishes_changed_content_and_different_paths()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let executable = directory.path().join("codex");
    let other_executable = directory.path().join("other-codex");
    std::fs::write(&executable, b"first")?;
    std::fs::write(&other_executable, b"first")?;
    let original = executable_identity(&executable).await?;
    let captured = RecordedExecutableIdentity::from(&original);

    std::fs::write(&executable, b"second")?;
    let changed = executable_identity(&executable).await?;
    let other = executable_identity(&other_executable).await?;
    let reconstructed = RecordedExecutableIdentity::new(
        captured.recorded_path().to_path_buf(),
        *captured.content_digest(),
    )?;

    if reconstructed != captured || !captured.matches_observed(&original) {
        return Err("changed file must not invalidate its previously captured record".into());
    }
    if captured.matches_observed(&changed) || captured.matches_observed(&other) {
        return Err(
            "changed content or a different path must not match the original record".into(),
        );
    }
    if changed.canonical_path() != original.canonical_path() {
        return Err("the changed-content scenario must retain the same canonical path".into());
    }
    if RecordedExecutableIdentity::from(&other).content_digest() != captured.content_digest() {
        return Err("the different-path scenario must retain identical file contents".into());
    }
    Ok(())
}

#[test]
fn recorded_identity_rejects_nonabsolute_or_invalid_executable_paths() {
    for path in ["", "codex", "./codex", "../codex"] {
        assert!(matches!(
            RecordedExecutableIdentity::new(PathBuf::from(path), [0; 32]),
            Err(RecordedExecutableIdentityError::RelativePath)
        ));
    }
    for path in ["/", "/codex\0hidden", "/tmp/../codex", "/tmp/./codex"] {
        assert!(matches!(
            RecordedExecutableIdentity::new(PathBuf::from(path), [0; 32]),
            Err(RecordedExecutableIdentityError::InvalidPath)
        ));
    }
}

#[test]
fn recorded_identity_rejects_empty_path_components_without_filesystem_lookup() {
    use std::os::unix::ffi::OsStringExt;

    for path_bytes in [
        b"//codex".as_slice(),
        b"///codex".as_slice(),
        b"/tmp//codex".as_slice(),
        b"/tmp///codex".as_slice(),
        b"/tmp/codex/".as_slice(),
        b"/tmp/codex//".as_slice(),
        b"/absent//codex-\xff".as_slice(),
    ] {
        let recorded_path = PathBuf::from(std::ffi::OsString::from_vec(path_bytes.to_vec()));
        assert!(
            matches!(
                RecordedExecutableIdentity::new(recorded_path, [0; 32]),
                Err(RecordedExecutableIdentityError::InvalidPath)
            ),
            "empty path components must be rejected: {path_bytes:?}",
        );
    }

    for path_bytes in [
        b"/absent/codex".as_slice(),
        b"/absent/codex-\xff".as_slice(),
    ] {
        let recorded_path = PathBuf::from(std::ffi::OsString::from_vec(path_bytes.to_vec()));
        assert!(RecordedExecutableIdentity::new(recorded_path, [0; 32]).is_ok());
    }
}

#[test]
fn recorded_identity_preserves_valid_non_utf8_path_without_filesystem_lookup()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::ffi::OsStringExt;
    let path = PathBuf::from(std::ffi::OsString::from_vec(b"/absent/codex-\xff".to_vec()));

    let record = RecordedExecutableIdentity::new(path.clone(), [0; 32])?;

    if record.recorded_path() != path || record.content_digest() != &[0; 32] {
        return Err(
            "structural construction must preserve valid non-UTF-8 path bytes and digest".into(),
        );
    }
    Ok(())
}

#[tokio::test]
async fn executable_identity_uses_canonical_path_and_changes_with_content() {
    let directory = TestDirectory::new("identity")
        .unwrap_or_else(|error| panic!("identity test directory should create: {error}"));
    let executable = directory.path().join("codex");
    std::fs::write(&executable, b"first")
        .unwrap_or_else(|error| panic!("first executable content should write: {error}"));

    let first = executable_identity(&executable)
        .await
        .unwrap_or_else(|error| panic!("first identity should resolve: {error}"));
    std::fs::write(&executable, b"second")
        .unwrap_or_else(|error| panic!("second executable content should write: {error}"));
    let second = executable_identity(&executable)
        .await
        .unwrap_or_else(|error| panic!("second identity should resolve: {error}"));

    let canonical_executable = std::fs::canonicalize(&executable)
        .unwrap_or_else(|error| panic!("expected executable path should canonicalize: {error}"));
    assert_eq!(first.canonical_path(), canonical_executable);
    assert_eq!(second.canonical_path(), canonical_executable);
    assert_ne!(first, second);
}

#[tokio::test]
async fn managed_version_and_updater_use_the_same_resolved_executable() {
    let directory = TestDirectory::new("version")
        .unwrap_or_else(|error| panic!("version test directory should create: {error}"));
    let executable = directory.path().join("codex");
    std::fs::write(
        &executable,
        b"#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'codex-cli 1.2.3'; exit 0; fi\nexit 9\n",
    )
    .unwrap_or_else(|error| panic!("version executable should write: {error}"));
    let mut permissions = std::fs::metadata(&executable)
        .unwrap_or_else(|error| panic!("version executable metadata should read: {error}"))
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&executable, permissions)
        .unwrap_or_else(|error| panic!("version executable permissions should set: {error}"));

    let identity = executable_identity(&executable)
        .await
        .unwrap_or_else(|error| panic!("managed identity should resolve: {error}"));
    let version = managed_executable_version(identity.canonical_path())
        .await
        .unwrap_or_else(|error| panic!("managed version should resolve: {error}"));
    let updater = UpdaterCommandSpec::new(&identity);

    assert_eq!(version, "1.2.3");
    assert_eq!(updater.executable(), identity.canonical_path());
    assert_eq!(updater.arguments(), ["update"]);
}

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(name: &str) -> std::io::Result<Self> {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "codex-router-executable-{name}-{}-{counter}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _cleanup_result = std::fs::remove_dir_all(&self.path);
    }
}
