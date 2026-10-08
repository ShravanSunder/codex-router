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
