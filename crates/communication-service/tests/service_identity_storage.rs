use communication_service::load_service_identity;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

#[test]
fn persistent_identity_survives_restart_and_corruption_never_reinitializes_it() {
    let root = std::path::PathBuf::from(format!("/tmp/service-identity-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap_or_else(|e| panic!("directory: {e}"));
    let first = load_service_identity(&root).unwrap_or_else(|e| panic!("create: {e}"));
    let again = load_service_identity(&root).unwrap_or_else(|e| panic!("read: {e}"));
    assert_eq!(first, again);
    let path = root.join("service-identity.json");
    assert_eq!(
        std::fs::metadata(&path)
            .unwrap_or_else(|e| panic!("metadata: {e}"))
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    std::fs::write(&path, b"broken").unwrap_or_else(|e| panic!("corrupt fixture: {e}"));
    assert!(load_service_identity(&root).is_err());
    assert_eq!(
        std::fs::read(&path).unwrap_or_else(|e| panic!("read: {e}")),
        b"broken"
    );
    std::fs::remove_file(path).unwrap_or_else(|e| panic!("cleanup: {e}"));
    std::fs::remove_dir(root).unwrap_or_else(|e| panic!("cleanup: {e}"));
}
