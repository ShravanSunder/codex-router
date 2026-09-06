use codex_native_integration::NativeSchemaBundle;
use std::{collections::BTreeMap, os::unix::fs::DirBuilderExt};

#[test]
fn publication_is_complete_idempotent_and_never_overwrites_corruption() {
    // Arrange.
    let root = std::env::temp_dir().join(format!("schema-storage-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap_or_else(|error| panic!("directory: {error}"));
    let bundle = NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        b"{}".to_vec(),
    )]))
    .unwrap_or_else(|error| panic!("bundle: {error}"));
    // Act.
    let path = bundle
        .publish(&root)
        .unwrap_or_else(|error| panic!("publish: {error}"));
    let contents = std::fs::read(&path);
    let repeated = bundle.publish(&root);
    std::fs::write(&path, b"corrupted").unwrap_or_else(|error| panic!("corrupt: {error}"));
    let rejected = bundle.publish(&root);
    let retained = std::fs::read(&path).unwrap_or_else(|error| panic!("read: {error}"));
    std::fs::remove_file(&path).unwrap_or_else(|error| panic!("file cleanup: {error}"));
    std::fs::remove_dir(&root).unwrap_or_else(|error| panic!("directory cleanup: {error}"));
    // Assert.
    assert_eq!(contents.ok().as_deref(), Some(bundle.canonical_bytes()));
    assert_eq!(repeated.ok(), Some(path));
    assert!(rejected.is_err());
    assert_eq!(retained, b"corrupted");
}
