use crate::CliError;
use agent_proxy_services::ProxyPreparationError;
use codex_router_proxy::server::LoopbackRouterRuntimeError;
use codex_router_state::schema_preparation::StateSchemaPreparationError;

#[test]
fn proxy_preparation_preserves_state_error_ownership_and_secret_redaction() {
    let error = CliError::from(ProxyPreparationError::Schema(
        StateSchemaPreparationError::ChecksumMismatch,
    ));
    assert!(
        matches!(
            error,
            CliError::Runtime(LoopbackRouterRuntimeError::SchemaPreparation(
                StateSchemaPreparationError::ChecksumMismatch
            ))
        ),
        "migration history must retain the state/runtime owner: {error}"
    );

    let error = CliError::from(ProxyPreparationError::StateInspection(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "fixture state path",
    )));
    assert!(matches!(
        error,
        CliError::Runtime(LoopbackRouterRuntimeError::StateInspection(_))
    ));

    let error = CliError::from(ProxyPreparationError::Secret(
        codex_router_secret_store::model::SecretStoreError::InvalidDataKey,
    ));
    assert!(matches!(error, CliError::CredentialStoreOpen));
    assert_eq!(
        error.to_string(),
        "encrypted credential store could not be opened"
    );
}

#[tokio::test]
async fn held_port_serve_error_retains_actual_bind_address_and_io_cause() {
    let root = tempfile::tempdir().expect("isolated CLI root");
    let held = std::net::TcpListener::bind("127.0.0.1:0").expect("actual held port");
    let address = held.local_addr().expect("kernel assigned address");
    let parsed = crate::cli_argument_parsing::CliCommand::parse([
        std::ffi::OsString::from("serve"),
        std::ffi::OsString::from("--port"),
        std::ffi::OsString::from(address.port().to_string()),
        std::ffi::OsString::from("--state-db"),
        root.path().join("state.sqlite").into_os_string(),
        std::ffi::OsString::from("--secret-root"),
        root.path().join("secrets").into_os_string(),
        std::ffi::OsString::from("--disable-background-quota-refresh"),
        std::ffi::OsString::from("--max-connections"),
        std::ffi::OsString::from("0"),
    ])
    .expect("real Serve parser");
    let crate::cli_argument_parsing::CliCommand::Serve(command) = parsed else {
        panic!("actual Serve command");
    };
    let error = crate::serve_command::run_serve_command(&mut Vec::new(), command)
        .await
        .expect_err("actual held-port refusal");
    eprintln!("held_port address={address} error={error:?} display={error}");
    assert!(error.to_string().contains(&address.to_string()));
    assert!(matches!(
        error,
        CliError::Runtime(LoopbackRouterRuntimeError::Bind(
            codex_router_proxy::server::ServerBindError::Bind { address: failed, source }
        )) if failed == address && source.kind() == std::io::ErrorKind::AddrInUse
    ));
    assert_eq!(held.local_addr().expect("held listener survives"), address);
}

async fn actual_preparation_error(
    root: &std::path::Path,
    credential_root: &std::path::Path,
    malformed_affinity: bool,
) -> ProxyPreparationError {
    use codex_router_secret_store::SecretStore;
    let credentials =
        codex_router_secret_store::test_support::open_encrypted_credential_store(credential_root)
            .expect("explicit external Keychain stand-in");
    if malformed_affinity {
        credentials
            .write_secret(
                &codex_router_secret_store::model::SecretKey::new("router_affinity_hash_secret.v1")
                    .expect("literal existing affinity key"),
                &codex_router_core::redaction::SecretString::new("invalid-affinity"),
            )
            .expect("actual malformed existing payload");
    }
    let gate = codex_router_descriptor_boundary::DescriptorGate::global();
    let listener = codex_router_descriptor_boundary::OwnedListener::bind_tcp(
        "127.0.0.1:0".parse().expect("loopback"),
        gate,
    )
    .await
    .expect("real listening descriptor");
    let config = agent_proxy_services::ProxyRoleConfig {
        core: codex_router_proxy::server::LoopbackRouterRuntimeConfig::new_tokenless(
            codex_router_proxy::server::LoopbackBindAddress::new("127.0.0.1", 0).expect("loopback"),
            codex_router_proxy::upstream::UpstreamEndpoint::new("http://127.0.0.1:1/v1")
                .expect("no egress fixture"),
            credential_root
                .parent()
                .expect("isolated parent")
                .join("state.sqlite"),
            root.to_path_buf(),
        ),
        local_token: agent_proxy_services::ProxyLocalTokenPolicy::Optional,
        quota_refresh: agent_proxy_services::ProxyQuotaRefreshPolicy::Disabled,
        quota_refresh_interval: std::time::Duration::from_secs(180),
        max_connections: 0,
    };
    match agent_proxy_services::test_support::prepare_fresh_with_fixture_credentials(
        config,
        credentials,
        listener,
        gate,
    )
    .await
    {
        Err(error) => error,
        Ok(_) => panic!("real malformed secret owner must refuse"),
    }
}

#[tokio::test]
async fn fresh_token_root_error_keeps_token_store_origin() {
    let root = tempfile::tempdir().expect("isolated token origin root");
    let credential_root = root.path().join("credentials");
    std::fs::create_dir(&credential_root).expect("owned credential fixture directory");
    let token_root = root.path().join("token-link");
    std::os::unix::fs::symlink(&credential_root, &token_root).expect("owned malformed token root");
    let error =
        CliError::from(actual_preparation_error(&token_root, &credential_root, false).await);
    eprintln!("real_token_root error={error:?} display={error}");
    assert!(matches!(
        error,
        CliError::Token(
            codex_router_secret_store::local_router_token::LocalRouterTokenError::SecretStore(
                codex_router_secret_store::model::SecretStoreError::SymlinkPath { .. }
            )
        )
    ));
}

#[tokio::test]
async fn malformed_affinity_error_keeps_runtime_resource_origin() {
    let root = tempfile::tempdir().expect("isolated affinity origin root");
    let credentials = root.path().join("credentials");
    let error = CliError::from(actual_preparation_error(&credentials, &credentials, true).await);
    eprintln!("real_affinity error={error:?} display={error}");
    assert!(matches!(
        error,
        CliError::Runtime(LoopbackRouterRuntimeError::CredentialResources(_))
    ));
}

#[test]
fn proxy_lifecycle_failure_remains_the_original_typed_error_at_cli_boundary() {
    let error =
        crate::CliError::from(agent_proxy_services::ProxyActivationError::LifecycleUnavailable);
    assert_eq!(
        error.to_string(),
        "proxy serving lifecycle is not available in this state"
    );
    assert!(matches!(
        error,
        crate::CliError::ProxyLifecycle(
            agent_proxy_services::ProxyActivationError::LifecycleUnavailable
        )
    ));
    let role_error = agent_proxy_services::ProxyActivationError::LifecycleUnavailable;
    let error = crate::CliError::from(role_error);
    assert!(
        std::error::Error::source(&error).is_none(),
        "transparent unit-role failure has no fabricated IO/Tokio cause"
    );
}
