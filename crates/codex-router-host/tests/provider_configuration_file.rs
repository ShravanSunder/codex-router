#![allow(clippy::expect_used)]
//! Fail-fast assertions for owner-edited provider configuration fixtures.

use codex_router_host::ProviderConfigurationFile;

#[test]
fn missing_provider_file_creates_enabled_defaults() {
    let root = tempfile::tempdir().expect("router root");

    let configuration = ProviderConfigurationFile::load_or_create(root.path())
        .expect("default provider configuration");

    assert!(configuration.claude().enabled);
    assert!(configuration.cursor().enabled);
    assert_eq!(configuration.claude().executable, None);
    assert_eq!(configuration.cursor().arguments, vec!["acp"]);
    let written =
        std::fs::read_to_string(root.path().join("providers.json")).expect("default file written");
    assert!(written.contains("\"version\": 1"));
}

#[test]
fn malformed_provider_file_is_preserved_and_reports_its_path() {
    let root = tempfile::tempdir().expect("router root");
    let path = root.path().join("providers.json");
    let original = br#"{"version":1,"providers":{"claude":{"enabled":true,"executable":null,"arguments":[]},"cursor":{"enabled":true,"executable":null,"arguments":["acp"]}},"surprise":true}"#;
    std::fs::write(&path, original).expect("malformed fixture");

    let error = ProviderConfigurationFile::load_or_create(root.path())
        .expect_err("unknown key must be rejected");

    assert!(error.to_string().contains(&path.display().to_string()));
    assert_eq!(std::fs::read(&path).expect("preserved file"), original);
}
