use communication_protocol::ControlSchema;
use communication_service::publish_control_schema;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

#[test]
fn control_schema_publication_is_private_complete_and_rejects_corrupt_reuse() {
    let root = std::env::temp_dir().join(format!("control-schema-proof-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap_or_else(|error| panic!("directory: {error}"));
    let schema = ControlSchema::generate(None).unwrap_or_else(|error| panic!("schema: {error}"));
    let published =
        publish_control_schema(&root, &schema).unwrap_or_else(|error| panic!("publish: {error}"));
    let bytes = std::fs::read(&published).unwrap_or_else(|error| panic!("schema bytes: {error}"));
    let permissions = std::fs::metadata(&published)
        .unwrap_or_else(|error| panic!("metadata: {error}"))
        .permissions()
        .mode()
        & 0o777;
    let repeated =
        publish_control_schema(&root, &schema).unwrap_or_else(|error| panic!("reuse: {error}"));
    std::fs::write(&published, b"corrupt")
        .unwrap_or_else(|error| panic!("fixture corruption: {error}"));
    let rejected = publish_control_schema(&root, &schema);
    let retained =
        std::fs::read(&published).unwrap_or_else(|error| panic!("retained bytes: {error}"));
    std::fs::remove_file(&published).unwrap_or_else(|error| panic!("file cleanup: {error}"));
    std::fs::remove_dir(root).unwrap_or_else(|error| panic!("directory cleanup: {error}"));
    assert_eq!(bytes, schema.bytes());
    assert_eq!(permissions, 0o600);
    assert_eq!(published, repeated);
    assert!(rejected.is_err());
    assert_eq!(retained, b"corrupt");
}
