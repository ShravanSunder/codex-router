use super::*;
use serde_json::{Value, json};

fn registry_document() -> Value {
    json!({"version":1,"routers":[{
        "name":"Sunbook", "connection":{"kind":"remote",
            "serviceId":"00000000-0000-4000-8000-000000000001",
            "mcpUrl":"https://machine.example.invalid:443/mcp",
            "credential":{"kind":"environment","variable":"ROUTER_TEST_AUTH"},
            "nativeCodex":{"address":"wss://machine.example.invalid:443/",
                "credential":{"kind":"environment","variable":"ROUTER_TEST_NATIVE_AUTH"}}},
        "defaultRemoteCwd":"/remote/only/project"
    }]})
}

#[test]
fn registry_accepts_comments_trailing_commas_and_preserves_explicit_native_port() {
    let text = r#"{
        // Human-owned ordering and notes survive reads.
        "version":1, "routers":[{
            "name":"Sunbook", "connection":{"kind":"remote",
                "serviceId":"00000000-0000-4000-8000-000000000001",
                "mcpUrl":"https://machine.example.invalid:443/mcp",
                "nativeCodex":{"address":"wss://machine.example.invalid:443/",},
            }, "defaultRemoteCwd":"/remote/only/project",
        },], /* retained note */
    }"#;
    let registry = RouterConnectionRegistry::parse(text).unwrap();
    assert_eq!(registry.routers[0].name.as_str(), "Sunbook");
    assert_eq!(
        registry.routers[0]
            .native_codex
            .as_ref()
            .unwrap()
            .address
            .as_str(),
        "wss://machine.example.invalid:443/"
    );
    assert_eq!(
        registry.routers[0]
            .default_remote_cwd
            .as_ref()
            .unwrap()
            .as_str(),
        "/remote/only/project"
    );
}

#[test]
fn registry_rejects_duplicate_members_before_map_conversion() {
    for text in [
        r#"{"version":1,"version":1,"routers":[]}"#,
        r#"{"version":1,"\u0076ersion":1,"routers":[]}"#,
        r#"{"version":1,"routers":[{"name":"A","name":"B"}]}"#,
        r#"{"version":1,"routers":[{"connection":{"kind":"remote","kind":"remote"}}]}"#,
    ] {
        assert_eq!(
            RouterConnectionRegistry::parse(text),
            Err(RouterRegistryError::DuplicateMembers)
        );
    }
}

#[test]
fn registry_rejects_extra_json_extensions_and_inline_credentials_without_echoing_values() {
    for text in [
        "{version:1,routers:[]}",
        "{'version':1,'routers':[]}",
        r#"{"version":1 "routers":[]}"#,
        r#"{"version":0x1,"routers":[]}"#,
        r#"{"version":+1,"routers":[]}"#,
        r#"{"version":1,"routers":[],ignored}"#,
    ] {
        assert!(RouterConnectionRegistry::parse(text).is_err());
    }
    let mut value = registry_document();
    value["routers"][0]["connection"]["token"] = json!("SECRET_CANARY_NEVER_PRINT");
    let error = RouterConnectionRegistry::parse(&value.to_string()).unwrap_err();
    assert_eq!(error, RouterRegistryError::InvalidShape);
    assert!(!error.to_string().contains("SECRET_CANARY_NEVER_PRINT"));
}

#[test]
fn registry_rejects_duplicate_names_and_unknown_version() {
    let mut document = registry_document();
    let repeated = document["routers"][0].clone();
    document["routers"].as_array_mut().unwrap().push(repeated);
    assert_eq!(
        RouterConnectionRegistry::parse(&document.to_string()),
        Err(RouterRegistryError::DuplicateName)
    );
    document = registry_document();
    document["version"] = json!(2);
    assert_eq!(
        RouterConnectionRegistry::parse(&document.to_string()),
        Err(RouterRegistryError::UnsupportedVersion)
    );
}

#[test]
fn registry_validates_address_reference_and_destination_shapes() {
    for address in [
        "https://user:SECRET@example.invalid/mcp",
        "https://example.invalid/mcp?token=SECRET",
        "https://example.invalid/mcp#SECRET",
        "unix:///tmp/control.sock",
        "https://",
    ] {
        let mut value = registry_document();
        value["routers"][0]["connection"]["mcpUrl"] = json!(address);
        let error = RouterConnectionRegistry::parse(&value.to_string()).unwrap_err();
        assert_eq!(error, RouterRegistryError::InvalidMcpAddress);
        assert!(!error.to_string().contains("SECRET"));
    }
    for address in [
        "wss://example.invalid/",
        "wss://example.invalid:0/",
        "wss://example.invalid:443/native",
        "wss://example.invalid:443/?token=SECRET",
    ] {
        let mut value = registry_document();
        value["routers"][0]["connection"]["nativeCodex"]["address"] = json!(address);
        assert_eq!(
            RouterConnectionRegistry::parse(&value.to_string()),
            Err(RouterRegistryError::InvalidNativeAddress)
        );
    }
    for (field, value, expected) in [
        ("name", "", RouterRegistryError::InvalidName),
        ("name", " bad ", RouterRegistryError::InvalidName),
        (
            "defaultRemoteCwd",
            "relative/project",
            RouterRegistryError::InvalidRemoteCwd,
        ),
    ] {
        let mut document = registry_document();
        document["routers"][0][field] = json!(value);
        assert_eq!(
            RouterConnectionRegistry::parse(&document.to_string()),
            Err(expected)
        );
    }
    let mut document = registry_document();
    document["routers"][0]["connection"]["credential"]["variable"] = json!("VALUE=SECRET");
    assert_eq!(
        RouterConnectionRegistry::parse(&document.to_string()),
        Err(RouterRegistryError::InvalidCredentialReference)
    );
}

#[test]
fn registry_reader_keeps_missing_empty_and_invalid_distinct_without_writing() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(
        read_router_registry(root.path()),
        RouterRegistryRead::Missing
    );
    let path = root.path().join("routers.jsonc");
    let content = b"{ /* owner note */ \"version\":1,\"routers\":[], }\n";
    std::fs::write(&path, content).unwrap();
    assert_eq!(
        read_router_registry(root.path()),
        RouterRegistryRead::Ready(RouterConnectionRegistry::default())
    );
    assert_eq!(std::fs::read(&path).unwrap(), content);
    std::fs::write(&path, "SECRET_CANARY_INVALID_JSON").unwrap();
    let rejected = read_router_registry(root.path());
    assert_eq!(
        rejected,
        RouterRegistryRead::Rejected(RouterRegistryError::InvalidJsonc)
    );
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "SECRET_CANARY_INVALID_JSON"
    );
}
