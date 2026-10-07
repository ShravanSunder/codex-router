use codex_native_integration::{NativeSchemaBundle, NativeSchemaDigest};
use std::collections::BTreeMap;

#[test]
fn schema_digest_captures_known_canonical_bundle_and_detects_mismatch()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let entrypoint = directory
        .path()
        .join("codex_app_server_protocol.schemas.json");
    std::fs::write(&entrypoint, b"{}")?;
    let bundle = NativeSchemaBundle::from_export_directory(directory.path())?;
    let expected_bytes = b"{\"documents\":{\"codex_app_server_protocol.schemas.json\":{}},\"entrypoint\":\"codex_app_server_protocol.schemas.json\"}";
    let captured = NativeSchemaDigest::from(&bundle);

    std::fs::remove_file(entrypoint)?;

    if bundle.canonical_bytes() != expected_bytes || captured.as_bytes() != bundle.digest() {
        return Err("capture must preserve the known canonical bundle bytes and raw digest".into());
    }
    if captured.to_string()
        != "sha256:9f165066759c5d63ca5c6eb7435f687dcbf34ad9594ae8cd6ea48e0617a5616f"
    {
        return Err("bundle digest encoding must match the independent SHA-256 oracle".into());
    }
    if captured.to_string().parse::<NativeSchemaDigest>()? != captured {
        return Err("computed bundle digest must round trip through its canonical encoding".into());
    }
    let mismatched = format!("sha256:{}", "0".repeat(64)).parse::<NativeSchemaDigest>()?;
    if mismatched == captured {
        return Err(
            "a different announced digest must not equal the captured bundle identity".into(),
        );
    }
    Ok(())
}

#[test]
fn schema_digest_uses_one_canonical_lowercase_hex_encoding()
-> Result<(), Box<dyn std::error::Error>> {
    let bytes = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
        0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
        0xcd, 0xef,
    ];
    let encoded = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    let parsed = encoded.parse::<NativeSchemaDigest>()?;

    if parsed.as_bytes() != &bytes
        || parsed.to_string() != encoded
        || NativeSchemaDigest::from_bytes(bytes) != parsed
    {
        return Err(
            "digest parsing and encoding must preserve the known complete byte vector".into(),
        );
    }
    for digit in b"0123456789abcdef" {
        let encoded = format!("sha256:{}", char::from(*digit).to_string().repeat(64));
        if encoded.parse::<NativeSchemaDigest>()?.to_string() != encoded {
            return Err("every accepted lowercase hexadecimal digit must round trip".into());
        }
    }
    Ok(())
}

#[test]
fn schema_digest_rejects_wrong_prefix_length_or_noncanonical_digits() {
    let hex_digits = "0123456789abcdef".repeat(4);
    for malformed in [
        String::new(),
        hex_digits.clone(),
        format!("SHA256:{hex_digits}"),
        format!("sha256:{}", "0".repeat(63)),
        format!("sha256:{}", "0".repeat(65)),
        format!("sha256:{}", "A".repeat(64)),
        format!("sha256:{}", "g".repeat(64)),
        format!(" sha256:{hex_digits}"),
        format!("sha256:{hex_digits}\n"),
        format!("sha256:{}\0", "0".repeat(63)),
        format!("sha256:{}", "é".repeat(32)),
    ] {
        assert!(malformed.parse::<NativeSchemaDigest>().is_err());
    }
}

#[test]
fn schema_digest_checks_every_hexadecimal_position() {
    for position in 0..64 {
        for invalid_digit in [b'A', b'G', b'/', b' ', 0, 0x7f] {
            let mut encoded = b"sha256:".to_vec();
            encoded.extend((0..64).map(|index| {
                if index == position {
                    invalid_digit
                } else {
                    b'0'
                }
            }));
            let parsed = std::str::from_utf8(&encoded)
                .ok()
                .and_then(|value| value.parse::<NativeSchemaDigest>().ok());
            assert!(parsed.is_none());
        }
    }
}

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
#[test]
fn schema_digest_serde_uses_its_literal_canonical_string() -> Result<(), Box<dyn std::error::Error>>
{
    let bytes = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
        0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
        0xcd, 0xef,
    ];
    let digest = NativeSchemaDigest::from_bytes(bytes);
    let literal = r#""sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef""#;
    let encoded = serde_json::to_string(&digest)?;
    let decoded = serde_json::from_str::<NativeSchemaDigest>(literal)?;

    if encoded != literal || decoded != digest {
        return Err("native schema digest Serde must use the canonical scalar string".into());
    }
    for malformed in [
        r#""SHA256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef""#,
        r#""sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde""#,
        r#""sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0""#,
        r#""sha256:0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef""#,
        r#""sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeg""#,
        r#"[1,2,3]"#,
    ] {
        if serde_json::from_str::<NativeSchemaDigest>(malformed).is_ok() {
            return Err("malformed digest scalar or non-string JSON was accepted".into());
        }
    }
    Ok(())
}
