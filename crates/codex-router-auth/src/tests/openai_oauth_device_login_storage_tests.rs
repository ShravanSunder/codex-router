use tokio_util::sync::CancellationToken;

use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::account_tokens::openai_account_credential_bundle_key;
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStoreStatus;
use codex_router_secret_store::test_support::FileWriteTraceEvent;
use codex_router_state::sqlite::AsyncSqliteStateStore;

use crate::credential_activation::CredentialActivation;
use crate::credential_activation::CredentialActivationRequest;
use crate::openai_oauth::OpenAiOAuthDeviceLoginClient;

use super::FakeDeviceIssuer;
use super::FakeResponse;
use super::TestDirectory;
use super::real_shaped_id_token;

#[tokio::test]
async fn device_login_activation_write_trace_contains_encrypted_tokens_only() {
    let issuer = FakeDeviceIssuer::spawn(vec![
        FakeResponse::json(
            200,
            r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-EFGH","interval":"0"}"#,
        ),
        FakeResponse::json(
            200,
            r#"{"authorization_code":"authorization-code","code_challenge":"challenge","code_verifier":"verifier"}"#,
        ),
        FakeResponse::json(
            200,
            format!(
                r#"{{"id_token":"{}","access_token":"access-token-canary","refresh_token":"refresh-token-canary"}}"#,
                real_shaped_id_token(Some("chatgpt-account-id"))
            ),
        ),
    ])
    .await;
    let client = OpenAiOAuthDeviceLoginClient::with_test_issuer(
        issuer.base_url(),
        std::time::Duration::from_secs(60),
    );
    let cancellation = CancellationToken::new();
    let device_code = client
        .request_user_code(&cancellation)
        .await
        .expect("user-code request should succeed");
    let tokens = client
        .complete_device_code_login(&device_code, &cancellation)
        .await
        .expect("approved device login should exchange tokens");
    let mut bundle = AccountCredentialBundle::imported_codex_auth(
        tokens.access_token().expose_secret().to_owned(),
        Some(tokens.refresh_token().expose_secret().to_owned()),
    );
    if let Some(account_id) = tokens.chatgpt_account_id() {
        bundle = bundle.with_chatgpt_account_id(account_id.as_str());
    }

    let temp_directory = TestDirectory::new("device-login-encrypted-write-trace");
    let state_path = temp_directory.path().join("state.sqlite");
    let secret_root = temp_directory.path().join("secrets");
    let state = AsyncSqliteStateStore::open(&state_path)
        .await
        .expect("test state store should open");
    let (secrets, write_trace) =
        codex_router_secret_store::test_support::open_encrypted_credential_store_with_write_trace(
            &secret_root,
        )
        .expect("test encrypted store should open");
    assert_eq!(secrets.status(), EncryptedCredentialStoreStatus::Ready);
    let account_id =
        AccountId::new("device-login-write-trace").expect("test account id should be valid");
    let generation = CredentialActivation::activate_login(
        &state,
        &secrets,
        CredentialActivationRequest::new(
            Provider::Openai,
            account_id.clone(),
            "device login",
            bundle,
        ),
    )
    .await
    .expect("login should activate an encrypted generation");

    assert_eq!(generation, 1);
    let token_canaries = [
        b"access-token-canary".as_slice(),
        b"refresh-token-canary".as_slice(),
    ];
    let mut temporary_writes = 0;
    for event in write_trace.events() {
        if let FileWriteTraceEvent::TemporaryFileWritten { contents, .. } = event {
            temporary_writes += 1;
            for token_canary in token_canaries {
                assert!(
                    !contents
                        .windows(token_canary.len())
                        .any(|window| window == token_canary),
                    "temporary credential write must not contain token plaintext"
                );
            }
        }
    }
    assert!(
        temporary_writes > 0,
        "activation should write a credential file"
    );

    let key = openai_account_credential_bundle_key(&account_id, generation)
        .expect("account bundle key should be valid");
    let stored_bundle = AccountCredentialBundle::from_secret_string(
        secrets
            .read_secret(&key)
            .expect("encrypted credential should read back"),
    )
    .expect("stored bundle should decrypt");
    assert_eq!(
        stored_bundle.access_token().expose_secret(),
        "access-token-canary"
    );
    assert_eq!(
        stored_bundle
            .refresh_token()
            .map(codex_router_core::redaction::SecretString::expose_secret),
        Some("refresh-token-canary")
    );
    assert_eq!(issuer.finish().await.len(), 3);
}
