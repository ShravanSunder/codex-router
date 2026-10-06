use super::*;

#[cfg(test)]
mod account_status_error_tests {
    use super::*;

    #[test]
    fn schema_upgrade_error_is_actionable_and_redacted() {
        let rendered = redacted_account_status_state_error(
            StateStoreError::WeeklyQuotaFloorSchemaUpgradeRequired,
        )
        .to_string();
        assert_eq!(
            rendered,
            "account status requires a compatible upgraded router database"
        );
        for canary in ["sensitive-label", "acct_internal", "/private/state.sqlite"] {
            assert!(!rendered.contains(canary));
        }
    }
}

#[cfg(test)]
mod account_provider_cli_tests {
    use super::*;
    use codex_router_core::provider::Provider;

    #[test]
    fn login_refuses_a_label_owned_by_another_provider_before_device_request() {
        let temporary_root = tempfile::tempdir().expect("temporary router root should exist");
        let runtime = account_command_runtime().expect("test runtime should initialize");
        let state = runtime
            .block_on(AsyncSqliteStateStore::open(
                &temporary_root.path().join("state.sqlite"),
            ))
            .expect("test state should open");
        let existing_account = AccountRecord::new(
            Provider::Claude,
            account_id_from_label("shared-label").expect("test account id should parse"),
            "shared-label",
            AccountStatus::Enabled,
        );
        runtime
            .block_on(state.upsert_account(&existing_account))
            .expect("Claude account should persist");
        runtime
            .block_on(state.close())
            .expect("test state should close");

        let client = OpenAiOAuthDeviceLoginClient::with_test_issuer(
            "http://127.0.0.1:1",
            std::time::Duration::from_secs(1),
        );
        let error = login_with_openai_device_auth(
            &mut Vec::new(),
            temporary_root.path().to_path_buf(),
            " shared-label ".to_owned(),
            &client,
            None,
        )
        .expect_err("duplicate labels must be refused before device auth");
        assert!(matches!(
            error,
            AccountCommandError::DuplicateAccountLabel { label } if label == "shared-label"
        ));
    }

    #[test]
    fn account_list_displays_each_provider() {
        let temporary_root = tempfile::tempdir().expect("temporary router root should exist");
        let runtime = account_command_runtime().expect("test runtime should initialize");
        let state = runtime
            .block_on(AsyncSqliteStateStore::open(
                &temporary_root.path().join("state.sqlite"),
            ))
            .expect("test state should open");
        for (label, provider) in [
            ("openai-label", Provider::Openai),
            ("claude-label", Provider::Claude),
        ] {
            let account = AccountRecord::new(
                provider,
                account_id_from_label(label).expect("test account id should parse"),
                label,
                AccountStatus::Enabled,
            );
            runtime
                .block_on(state.upsert_account(&account))
                .expect("test account should persist");
        }
        runtime
            .block_on(state.close())
            .expect("test state should close");

        let mut output = Vec::new();
        list_accounts(&mut output, temporary_root.path().to_path_buf())
            .expect("account list should render");
        let output = String::from_utf8(output).expect("account list output should be UTF-8");
        assert!(output.contains("provider"));
        assert!(output.contains("openai"));
        assert!(output.contains("claude"));
    }
}

#[cfg(test)]
mod claude_login_glue_tests {
    use super::*;
    use codex_router_core::redaction::SecretString;
    use codex_router_secret_store::credential_bundle::CredentialBundle;
    use std::io::Cursor;
    use std::sync::Mutex;

    struct RecordingClaudeLoginFlow {
        flow: ClaudeOAuthLoginFlow,
        pasted_callbacks: Mutex<Vec<String>>,
    }

    impl RecordingClaudeLoginFlow {
        fn new() -> Self {
            Self {
                flow: ClaudeOAuthLoginFlow::new(),
                pasted_callbacks: Mutex::new(Vec::new()),
            }
        }
    }

    impl AccountLoginFlow for RecordingClaudeLoginFlow {
        type PendingLogin = PendingClaudeOAuthLogin;

        fn begin_login(&self) -> Result<Self::PendingLogin, LoginFlowError> {
            self.flow.begin_login()
        }

        fn authorization_url(
            &self,
            pending: &Self::PendingLogin,
        ) -> Result<String, LoginFlowError> {
            self.flow.authorization_url(pending)
        }

        fn finish_login(
            &self,
            _pending: Self::PendingLogin,
            pasted_callback: &str,
        ) -> Result<CredentialBundle, LoginFlowError> {
            self.pasted_callbacks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(pasted_callback.to_owned());
            if pasted_callback.trim().is_empty() {
                return Err(LoginFlowError::InvalidCallback);
            }
            CredentialBundle::new_claude(
                SecretString::new("glue-access-canary"),
                SecretString::new("glue-refresh-canary"),
                2_000,
            )
            .map_err(|_| LoginFlowError::InvalidTokenResponse)
        }
    }

    #[test]
    fn claude_login_glue_prints_prompt_and_passes_the_pasted_callback() {
        let flow = RecordingClaudeLoginFlow::new();
        let mut stdout = Vec::new();
        let mut reader = Cursor::new(b"code-value#state-value\n".to_vec());

        let bundle = collect_claude_oauth_bundle(&mut stdout, &mut reader, &flow)
            .unwrap_or_else(|error| panic!("test login should finish: {error}"));

        let prompt = String::from_utf8(stdout)
            .unwrap_or_else(|error| panic!("test prompt should be UTF-8: {error}"));
        assert!(prompt.contains("Open this URL and finish the Claude authorization:"));
        assert!(prompt.contains("Paste the returned code#state: "));
        assert_eq!(
            *flow
                .pasted_callbacks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            ["code-value#state-value\n"]
        );
        assert_eq!(bundle.access_token().expose_secret(), "glue-access-canary");
    }

    #[test]
    fn claude_login_glue_reports_eof_as_an_invalid_callback() {
        let flow = RecordingClaudeLoginFlow::new();
        let mut stdout = Vec::new();
        let mut reader = Cursor::new(Vec::<u8>::new());

        let error = collect_claude_oauth_bundle(&mut stdout, &mut reader, &flow)
            .expect_err("EOF must not activate an empty callback");

        assert!(matches!(
            error,
            AccountCommandError::ClaudeOAuth(LoginFlowError::InvalidCallback)
        ));
    }

    #[test]
    fn claude_login_glue_maps_provider_mismatch_from_activation() {
        assert!(matches!(
            map_credential_activation_error(CredentialActivationError::AccountProviderMismatch),
            AccountCommandError::AccountProviderMismatch
        ));
    }
}
