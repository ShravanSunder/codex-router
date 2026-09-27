//! Keep the ACP SDK inside the client crate and front doors outside it.

use std::path::{Path, PathBuf};

#[test]
fn public_protocol_error_uses_client_owned_version() {
    let error = acp_client_runtime::ExternalProviderRuntimeError::UnsupportedProtocol {
        actual: acp_client_runtime::AcpProtocolVersion::new(0),
    };
    assert_eq!(
        error.to_string(),
        "provider selected unsupported ACP protocol version ProtocolVersion(0)"
    );
}

fn rust_sources(directory: &Path, sources: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, sources)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            sources.push(path);
        }
    }
    Ok(())
}

fn production_dependencies(manifest: &str) -> &str {
    manifest
        .split_once("[dependencies]")
        .map(|(_, dependencies)| dependencies)
        .unwrap_or("")
        .split_once("[dev-dependencies]")
        .map_or_else(
            || {
                manifest
                    .split_once("[dependencies]")
                    .map_or("", |(_, rest)| rest)
            },
            |(dependencies, _)| dependencies,
        )
}

#[test]
fn host_sources_do_not_import_the_acp_sdk() {
    let host_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut sources = Vec::new();
    rust_sources(&host_root.join("src"), &mut sources).expect("Host source tree");
    for source in sources {
        let file_name = source
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if matches!(file_name, "tests.rs" | "live_tests.rs") || file_name.ends_with("_tests.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&source).expect("Rust source");
        let production = text
            .split_once("\n#[cfg(test)]\nmod tests")
            .map_or(text.as_str(), |(production, _)| production);
        assert!(
            !production.contains("agent_client_protocol"),
            "{} names the ACP SDK in Host production code",
            source.display()
        );
    }
    let manifest = std::fs::read_to_string(host_root.join("Cargo.toml")).expect("Host manifest");
    assert!(
        !production_dependencies(&manifest).contains("agent-client-protocol"),
        "codex-router-host must use acp-client-runtime in production, never the ACP SDK directly"
    );
}

#[test]
fn front_door_manifests_do_not_import_the_acp_client_crate() {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_owned();
    for package in [
        "collaboration-client",
        "collaboration-mcp",
        "agent-collaboration",
    ] {
        let manifest = workspace_root
            .join("crates")
            .join(package)
            .join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest).expect("front-door manifest");
        assert!(
            !text.contains("acp-client-runtime"),
            "{package} must depend on Host ports, never the ACP client crate"
        );
    }
}

#[test]
fn acp_client_has_no_router_service_or_protocol_dependency() {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_owned();
    let manifest = workspace_root.join("crates/acp-client-runtime/Cargo.toml");
    let text = std::fs::read_to_string(manifest).expect("ACP client manifest");
    for forbidden in ["collaboration-", "codex-router-"] {
        assert!(
            !text.contains(forbidden),
            "acp-client-runtime must reach Router services through injected ports: {forbidden}"
        );
    }
}

#[test]
fn only_acp_client_and_agent_server_depend_on_the_sdk_in_production() {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_owned();
    for entry in std::fs::read_dir(workspace_root.join("crates")).expect("workspace crates") {
        let entry = entry.expect("crate directory");
        let manifest = entry.path().join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let package = entry.file_name().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(manifest).expect("crate manifest");
        if production_dependencies(&text).contains("agent-client-protocol") {
            assert!(
                matches!(package.as_str(), "acp-client-runtime" | "codex-acp-adapter"),
                "{package} imports the ACP SDK in production outside the client/server owners"
            );
        }
    }
}
