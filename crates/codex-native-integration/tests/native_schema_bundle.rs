use codex_native_integration::NativeSchemaBundle;
use std::collections::BTreeMap;

#[test]
fn complete_bundle_digest_is_independent_of_json_whitespace_and_key_order() {
    let documents = |entry: &[u8]| {
        BTreeMap::from([
            (
                "codex_app_server_protocol.schemas.json".to_owned(),
                entry.to_vec(),
            ),
            (
                "v2/Thread.json".to_owned(),
                br#"{"type":"object"}"#.to_vec(),
            ),
        ])
    };
    let first =
        NativeSchemaBundle::from_documents(documents(br#"{"title":"Native","definitions":{}}"#))
            .unwrap_or_else(|error| panic!("first bundle: {error}"));
    let second = NativeSchemaBundle::from_documents(documents(
        b"{ \"definitions\": {}, \"title\": \"Native\" }\n",
    ))
    .unwrap_or_else(|error| panic!("second bundle: {error}"));
    assert_eq!(first.canonical_bytes(), second.canonical_bytes());
    assert_eq!(first.digest(), second.digest());
    assert_eq!(first.canonical_bytes().last(), Some(&b'}'));

    let mut changed = documents(br#"{"title":"Native","definitions":{}}"#);
    changed.insert(
        "v2/Thread.json".to_owned(),
        br#"{"type":"string"}"#.to_vec(),
    );
    let changed = NativeSchemaBundle::from_documents(changed)
        .unwrap_or_else(|error| panic!("changed bundle: {error}"));
    assert_ne!(first.digest(), changed.digest());
}

#[test]
fn bundle_rejects_unsafe_paths_missing_entrypoint_and_duplicate_keys() {
    let entrypoint = "codex_app_server_protocol.schemas.json";
    for name in [
        "../Thread.json",
        "/Thread.json",
        "v2//Thread.json",
        "v2\\Thread.json",
        "v2/./Thread.json",
    ] {
        let documents = BTreeMap::from([
            (entrypoint.to_owned(), b"{}".to_vec()),
            (name.to_owned(), b"{}".to_vec()),
        ]);
        assert!(NativeSchemaBundle::from_documents(documents).is_err());
    }
    assert!(NativeSchemaBundle::from_documents(BTreeMap::new()).is_err());
    assert!(
        NativeSchemaBundle::from_documents(BTreeMap::from([(
            entrypoint.to_owned(),
            br#"{"type":"object","type":"string"}"#.to_vec()
        ),]))
        .is_err()
    );
}

#[test]
fn unresolved_and_network_references_are_rejected_without_fetching() {
    for reference in [
        "#/definitions/Missing",
        "https://example.invalid/schema.json",
        "Missing.json",
    ] {
        let document = serde_json::to_vec(&serde_json::json!({"$ref":reference}))
            .unwrap_or_else(|error| panic!("json: {error}"));
        assert!(
            NativeSchemaBundle::from_documents(BTreeMap::from([(
                "codex_app_server_protocol.schemas.json".to_owned(),
                document
            ),]))
            .is_err()
        );
    }
}

#[test]
fn exported_directory_rejects_symlinks_instead_of_reading_outside_files()
-> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join(format!("schema-collection-{}", std::process::id()));
    std::fs::create_dir(&root)?;
    let entrypoint = root.join("codex_app_server_protocol.schemas.json");
    std::fs::write(&entrypoint, "{}")?;
    let outside = root.with_extension("json");
    std::fs::write(&outside, "{}")?;
    let linked = root.join("linked-schema.json");
    std::os::unix::fs::symlink(&outside, &linked)?;

    let rejected = NativeSchemaBundle::from_export_directory(&root);
    std::fs::remove_file(linked)?;
    let accepted = NativeSchemaBundle::from_export_directory(&root);
    std::fs::remove_file(entrypoint)?;
    std::fs::remove_file(outside)?;
    std::fs::remove_dir(root)?;

    if rejected.is_ok() || accepted.is_err() {
        return Err("collection must reject symlink but accept ordinary complete export".into());
    }
    Ok(())
}
