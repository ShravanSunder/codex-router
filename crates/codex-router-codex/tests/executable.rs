use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use codex_router_codex::UpdaterCommandSpec;
use codex_router_codex::executable_identity;
use codex_router_codex::managed_executable_version;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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
