use super::*;

/// An explicit JSON-RPC rejection is definite: the old setting remains
/// effective and a later prompt may proceed.
#[tokio::test]
async fn explicit_setting_rejection_does_not_gate_session() {
    let root = tempfile::tempdir().expect("fixture root");
    let client = AgentSessionClient::initialize(
        setting_response_launch("rejected"),
        Arc::new(NoopInteractionPort),
        Arc::new(NoopEventSink),
    )
    .await
    .expect("fixture initializes");
    let session_id = client
        .create_session(root.path().to_path_buf())
        .await
        .expect("session opens");
    let result = client
        .set_setting(
            session_id.clone(),
            acp_client_runtime::ProviderSettingKind::Mode,
            "ask".to_owned(),
        )
        .await;
    assert!(
        matches!(
            result,
            Err(acp_client_runtime::ExternalProviderRuntimeError::SettingFailed { .. })
        ),
        "setting result: {result:?}"
    );
    assert!(!client.settings_unresolved(&session_id).await);
    client
        .prompt_contents_with_approval_dispatch_for_input(
            session_id,
            session_event_model::InputId::generate(),
            vec![
                session_event_model::PromptContent::text("Proceed".to_owned())
                    .expect("text prompt"),
            ],
            (),
            None,
        )
        .await
        .expect("prompt after rejection");
    client.shutdown().await;
}

#[tokio::test]
async fn malformed_legacy_set_mode_response_is_outcome_unknown() {
    let root = tempfile::tempdir().expect("fixture root");
    let client = AgentSessionClient::initialize(
        ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec![
                "-u".to_owned(),
                "-c".to_owned(),
                LEGACY_SETTING_RESPONSE_FIXTURE.to_owned(),
            ],
            environment: Vec::new(),
            persistence_target: ProviderPersistenceTarget::Unspecified,
        },
        Arc::new(NoopInteractionPort),
        Arc::new(NoopEventSink),
    )
    .await
    .expect("fixture initializes");
    let session_id = client
        .create_session(root.path().to_path_buf())
        .await
        .expect("session opens");
    let result = client
        .set_setting(
            session_id.clone(),
            acp_client_runtime::ProviderSettingKind::Mode,
            "ask".to_owned(),
        )
        .await;
    assert!(
        matches!(
            result,
            Err(acp_client_runtime::ExternalProviderRuntimeError::SettingOutcomeUnknown { .. })
        ),
        "legacy setting result: {result:?}"
    );
    assert!(client.settings_unresolved(&session_id).await);
    client.shutdown().await;
}

#[tokio::test]
async fn disconnected_after_setting_submission_is_outcome_unknown() {
    let root = tempfile::tempdir().expect("fixture root");
    let client = AgentSessionClient::initialize(
        setting_response_launch("disconnect"),
        Arc::new(NoopInteractionPort),
        Arc::new(NoopEventSink),
    )
    .await
    .expect("fixture initializes");
    let session_id = client
        .create_session(root.path().to_path_buf())
        .await
        .expect("session opens");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.set_setting(
            session_id.clone(),
            acp_client_runtime::ProviderSettingKind::Mode,
            "ask".to_owned(),
        ),
    )
    .await
    .expect("setting result bounded");
    assert!(
        matches!(
            result,
            Err(acp_client_runtime::ExternalProviderRuntimeError::SettingOutcomeUnknown { .. })
        ),
        "disconnected setting result: {result:?}"
    );
    assert!(client.settings_unresolved(&session_id).await);
    client.shutdown().await;
}
