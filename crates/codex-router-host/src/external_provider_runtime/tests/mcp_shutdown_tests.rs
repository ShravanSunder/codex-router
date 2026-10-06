use super::*;

#[cfg(unix)]
async fn observe_mcp_fixture(
    raw_output: Option<serde_json::Value>,
    response_text: &str,
    split_result_before_status: bool,
) -> (Vec<ExternalProviderToolCall>, ExternalProviderPromptOutcome) {
    let runtime = ExternalProviderRuntime::initialize(named_mcp_tool_call_fixture(
        raw_output,
        response_text,
        split_result_before_status,
    ))
    .await
    .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");
    let prompt_outcome = runtime
        .prompt("fixture-session".to_owned(), "invoke".to_owned())
        .await
        .expect("prompt settles");
    assert_eq!(runtime.permission_observation().execute_tool_call_count, 0);
    let tool_calls = runtime.take_test_tool_calls();
    runtime.shutdown().await;
    (tool_calls, prompt_outcome)
}

#[cfg(unix)]
#[tokio::test]
async fn decoded_mcp_success_split_before_completed_status_is_accepted() {
    let expected_service_id = "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89";
    let expected_identity =
        collaboration_protocol::UuidIdentity::try_from(expected_service_id.to_owned())
            .expect("expected identity");
    let (tool_calls, prompt_outcome) = observe_mcp_fixture(
        Some(serde_json::json!({"success":true})),
        expected_service_id,
        true,
    )
    .await;
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(
        tool_calls[0].name.as_deref(),
        Some("router-collaboration-endpoints_list")
    );
    assert_eq!(
        tool_calls[0].status,
        agent_client_protocol::schema::v1::ToolCallStatus::Completed
    );
    assert_eq!(tool_calls[0].kind, ToolKind::Other);
    assert_eq!(tool_calls[0].outcome, ExternalProviderToolOutcome::Success);
    assert!(has_completed_router_endpoints_call(&tool_calls));
    assert!(accepts_native_router_result(
        &tool_calls,
        &prompt_outcome.output,
        &expected_identity
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn completed_mcp_non_success_outcomes_are_not_accepted() {
    let expected_service_id = "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89";
    let expected_identity =
        collaboration_protocol::UuidIdentity::try_from(expected_service_id.to_owned())
            .expect("expected identity");
    for raw_output in [
        Some(serde_json::json!({"error":"failed"})),
        Some(serde_json::json!({"success":true,"error":"failed"})),
        Some(serde_json::json!({"rejected":true})),
        Some(serde_json::json!({"permissionDenied":true})),
        Some(serde_json::json!({"success":false})),
        None,
    ] {
        let (tool_calls, prompt_outcome) =
            observe_mcp_fixture(raw_output, expected_service_id, false).await;
        assert!(!accepts_native_router_result(
            &tool_calls,
            &prompt_outcome.output,
            &expected_identity
        ));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn completed_mcp_success_with_wrong_returned_service_is_not_accepted() {
    let expected_identity = collaboration_protocol::UuidIdentity::try_from(
        "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned(),
    )
    .expect("expected identity");
    let (tool_calls, prompt_outcome) = observe_mcp_fixture(
        Some(serde_json::json!({"success":true})),
        "2ff962c5-7fa3-4c18-a5ca-1bbe8db09e81",
        false,
    )
    .await;
    assert!(!accepts_native_router_result(
        &tool_calls,
        &prompt_outcome.output,
        &expected_identity
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn completed_mcp_success_accepts_one_bounded_owned_identity_in_presentation() {
    let expected_service_id = "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89";
    let expected_identity =
        collaboration_protocol::UuidIdentity::try_from(expected_service_id.to_owned())
            .expect("expected identity");
    for response in [
        expected_service_id.to_owned(),
        format!("`{expected_service_id}`"),
        format!("The service ID is {expected_service_id}."),
    ] {
        let (tool_calls, prompt_outcome) =
            observe_mcp_fixture(Some(serde_json::json!({"success":true})), &response, false).await;
        assert!(accepts_native_router_result(
            &tool_calls,
            &prompt_outcome.output,
            &expected_identity
        ));
    }
}

#[test]
fn echoed_router_marker_without_typed_tool_event_is_not_native_execution() {
    let expected_identity = collaboration_protocol::UuidIdentity::try_from(
        "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned(),
    )
    .expect("expected identity");
    assert!(!accepts_native_router_result(
        &[],
        "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89",
        &expected_identity
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn overlapping_prompt_cannot_replace_context_and_dropped_waiter_cleans_it() {
    let runtime = ExternalProviderRuntime::initialize(cancellation_fixture())
        .await
        .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");
    let first_operation = "019f0000-0000-7000-8000-000000002001";
    let mut first = Box::pin(runtime.prompt_with_approval_context(
        "fixture-session".to_owned(),
        "hold".to_owned(),
        approval_context(first_operation),
    ));
    tokio::select! {
        result = &mut first => panic!("first prompt settled unexpectedly: {result:?}"),
        () = tokio::time::sleep(Duration::from_millis(25)) => {}
    }
    let second = runtime
        .prompt_with_approval_context(
            "fixture-session".to_owned(),
            "overlap".to_owned(),
            approval_context("019f0000-0000-7000-8000-000000002002"),
        )
        .await
        .expect_err("overlapping prompt must not replace active authorization context");
    assert!(matches!(second, ExternalProviderRuntimeError::LocalBusy));
    assert_eq!(
        runtime
            .active_approval_operation("fixture-session")
            .map(String::from),
        Some(first_operation.to_owned())
    );
    drop(first);
    assert!(runtime.active_approval_count() == 0);
    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_joins_runtime_and_settles_held_prompt() {
    let runtime = ExternalProviderRuntime::initialize(cancellation_fixture())
        .await
        .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");
    let mut prompt = Box::pin(runtime.prompt_with_approval_context(
        "fixture-session".to_owned(),
        "hold".to_owned(),
        approval_context("019f0000-0000-7000-8000-000000002099"),
    ));
    tokio::select! {
        result = &mut prompt => panic!("held prompt settled unexpectedly: {result:?}"),
        () = tokio::time::sleep(Duration::from_millis(25)) => {}
    }
    runtime.shutdown().await;
    assert!(prompt.await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_records_runtime_owner_join_failure() {
    let runtime = ExternalProviderRuntime::initialize(conversation_fixture())
        .await
        .expect("fixture initializes");
    runtime.abort_owner_for_test().await;
    runtime.shutdown().await;
    assert!(runtime.shutdown_failed());
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_drains_more_than_completion_channel_capacity_of_held_loads() {
    let root = tempfile::tempdir().expect("fixture root");
    let admission_log = root.path().join("load-admissions.log");
    let process_id_path = root.path().join("provider.pid");
    let runtime = Arc::new(
        ExternalProviderRuntime::initialize(saturated_load_admission_fixture(
            &admission_log,
            &process_id_path,
        ))
        .await
        .expect("fixture initializes"),
    );
    let mut loads = Vec::new();
    for index in 0..40 {
        let runtime = Arc::clone(&runtime);
        loads.push(tokio::spawn(async move {
            runtime
                .load_session(format!("held-load-{index}"), PathBuf::from("/tmp"))
                .await
        }));
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let admissions = std::fs::read_to_string(&admission_log).unwrap_or_default();
            if admissions.lines().count() == 40 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("all held loads reached the provider");

    tokio::time::timeout(Duration::from_secs(2), runtime.shutdown())
        .await
        .expect("shutdown drains the saturated completion path");
    for load in loads {
        assert!(matches!(
            load.await.expect("load task"),
            Err(ExternalProviderRuntimeError::TransportFailure)
        ));
    }
    assert_process_reaped(&process_id_path).await;
}

#[cfg(unix)]
#[tokio::test]
async fn prompt_cancel_remains_responsive_during_create_admission() {
    let root = tempfile::tempdir().expect("temporary root");
    let observed = root.path().join("create-observed");
    let runtime = ExternalProviderRuntime::initialize(concurrent_admission_cancel_fixture(
        "session/new",
        &observed,
    ))
    .await
    .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("active session");
    let prompt_id: OperationId = "019f0000-0000-7000-8000-000000002101"
        .to_owned()
        .try_into()
        .expect("operation ID");
    let mut prompt = Box::pin(runtime.prompt_for_operation(
        "active-session".to_owned(),
        Some(prompt_id.clone()),
        "hold".to_owned(),
        None,
    ));
    assert!(futures_util::poll!(&mut prompt).is_pending());
    let mut create = Box::pin(runtime.create_session(PathBuf::from("/tmp")));
    assert!(futures_util::poll!(&mut create).is_pending());
    tokio::time::timeout(Duration::from_secs(2), async {
        while !observed.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("create reached provider");
    runtime
        .cancel_prompt_operation("active-session".to_owned(), prompt_id)
        .await
        .expect("cancel routed");
    assert_eq!(
        prompt.await.expect("prompt settles").stop_reason,
        ProviderPromptStopReason::Cancelled
    );
    assert_eq!(create.await.expect("create settles"), "new-session");
    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn prompt_cancel_remains_responsive_during_load_admission() {
    let root = tempfile::tempdir().expect("temporary root");
    let observed = root.path().join("load-observed");
    let runtime = ExternalProviderRuntime::initialize(concurrent_admission_cancel_fixture(
        "session/load",
        &observed,
    ))
    .await
    .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("active session");
    let prompt_id: OperationId = "019f0000-0000-7000-8000-000000002102"
        .to_owned()
        .try_into()
        .expect("operation ID");
    let mut prompt = Box::pin(runtime.prompt_for_operation(
        "active-session".to_owned(),
        Some(prompt_id.clone()),
        "hold".to_owned(),
        None,
    ));
    assert!(futures_util::poll!(&mut prompt).is_pending());
    let mut load =
        Box::pin(runtime.load_session("loaded-session".to_owned(), PathBuf::from("/tmp")));
    assert!(futures_util::poll!(&mut load).is_pending());
    tokio::time::timeout(Duration::from_secs(2), async {
        while !observed.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("load reached provider");
    runtime
        .cancel_prompt_operation("active-session".to_owned(), prompt_id)
        .await
        .expect("cancel routed");
    assert_eq!(
        prompt.await.expect("prompt settles").stop_reason,
        ProviderPromptStopReason::Cancelled
    );
    load.await.expect("load settles");
    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn late_cancel_for_prior_operation_cannot_cancel_successor_prompt() {
    let runtime = ExternalProviderRuntime::initialize(successor_prompt_fixture())
        .await
        .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");
    let first_id = "019f0000-0000-7000-8000-000000002101";
    runtime
        .prompt_with_approval_context(
            "fixture-session".to_owned(),
            "first".to_owned(),
            approval_context(first_id),
        )
        .await
        .expect("first prompt");
    let second_id = "019f0000-0000-7000-8000-000000002102";
    let mut second = Box::pin(runtime.prompt_with_approval_context(
        "fixture-session".to_owned(),
        "second".to_owned(),
        approval_context(second_id),
    ));
    tokio::select! {
        result = &mut second => panic!("successor settled too early: {result:?}"),
        () = tokio::time::sleep(Duration::from_millis(25)) => {}
    }
    let stale_cancel = runtime
        .cancel_prompt_operation(
            "fixture-session".to_owned(),
            first_id.to_owned().try_into().expect("operation ID"),
        )
        .await
        .expect_err("stale cancel must not reach successor prompt");
    assert!(stale_cancel.to_string().contains("no longer active"));
    assert_eq!(
        second.await.expect("successor settles").stop_reason,
        ProviderPromptStopReason::EndTurn
    );
    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn aggregate_prompt_output_is_bounded_across_valid_frames() {
    let runtime = ExternalProviderRuntime::initialize(aggregate_output_fixture())
        .await
        .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");

    let error = runtime
        .prompt("fixture-session".to_owned(), "overflow".to_owned())
        .await
        .expect_err("aggregate output must be bounded");
    assert!(matches!(
        error,
        ExternalProviderRuntimeError::PromptOutputLimitExceeded
    ));

    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn load_history_replay_is_not_returned_as_the_next_prompt_output() {
    let runtime = ExternalProviderRuntime::initialize(load_replay_fixture())
        .await
        .expect("fixture initializes");
    runtime
        .load_session("fixture-session".to_owned(), PathBuf::from("/tmp"))
        .await
        .expect("session loads");

    let outcome = runtime
        .prompt("fixture-session".to_owned(), "next".to_owned())
        .await
        .expect("prompt settles");
    assert_eq!(outcome.output, "NEW_OUTPUT");

    runtime.shutdown().await;
}

#[tokio::test]
#[ignore = "requires an explicitly selected authenticated external provider runtime"]
async fn live_external_provider_create_and_prompt() {
    let executable = std::env::var_os("CODEX_ROUTER_TEST_EXTERNAL_ACP_EXECUTABLE")
        .map(PathBuf::from)
        .expect("external ACP executable");
    let arguments = std::env::var("CODEX_ROUTER_TEST_EXTERNAL_ACP_ARGUMENTS")
        .ok()
        .map(|encoded| serde_json::from_str::<Vec<String>>(&encoded).expect("JSON argument array"))
        .unwrap_or_default();
    let cwd = std::env::var_os("CODEX_ROUTER_TEST_EXTERNAL_ACP_CWD")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("current directory"));
    let launch = ExternalProviderLaunch {
        persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
        executable,
        arguments,
        environment: Vec::new(),
    };
    let runtime = ExternalProviderRuntime::initialize(launch.clone())
        .await
        .expect("provider initializes");
    eprintln!("provider admission: {:?}", runtime.admission());
    let provider_session_id = runtime
        .create_session(cwd.clone())
        .await
        .expect("provider conversation created");
    let outcome = runtime
        .prompt(
            provider_session_id.clone(),
            "Reply with exactly PR2_LIVE_PROVIDER_OK and no other text.".to_owned(),
        )
        .await
        .expect("provider prompt settled");
    eprintln!(
        "provider target={provider_session_id} stop={:?} output={:?}",
        outcome.stop_reason, outcome.output
    );
    assert_eq!(outcome.stop_reason, ProviderPromptStopReason::EndTurn);
    assert_eq!(outcome.output.trim(), "PR2_LIVE_PROVIDER_OK");
    runtime.shutdown().await;

    let resumed = ExternalProviderRuntime::initialize(launch)
        .await
        .expect("provider reinitializes for load");
    resumed
        .load_session(provider_session_id.clone(), cwd)
        .await
        .expect("provider conversation loads");
    let resumed_outcome = resumed
        .prompt(
            provider_session_id.clone(),
            "Reply with exactly PR2_LIVE_PROVIDER_RESUMED and no other text.".to_owned(),
        )
        .await
        .expect("loaded provider prompt settled");
    eprintln!(
        "loaded provider target={provider_session_id} stop={:?} output={:?}",
        resumed_outcome.stop_reason, resumed_outcome.output
    );
    assert_eq!(
        resumed_outcome.stop_reason,
        ProviderPromptStopReason::EndTurn
    );
    assert_eq!(resumed_outcome.output.trim(), "PR2_LIVE_PROVIDER_RESUMED");
    resumed.shutdown().await;
}

#[tokio::test]
#[ignore = "requires an explicitly selected authenticated external provider runtime"]
async fn live_external_provider_explicit_cancel() {
    let executable = std::env::var_os("CODEX_ROUTER_TEST_EXTERNAL_ACP_EXECUTABLE")
        .map(PathBuf::from)
        .expect("external ACP executable");
    let arguments = std::env::var("CODEX_ROUTER_TEST_EXTERNAL_ACP_ARGUMENTS")
        .ok()
        .map(|encoded| serde_json::from_str::<Vec<String>>(&encoded).expect("JSON argument array"))
        .unwrap_or_default();
    let cwd = std::env::var_os("CODEX_ROUTER_TEST_EXTERNAL_ACP_CWD")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("current directory"));
    let runtime = ExternalProviderRuntime::initialize(ExternalProviderLaunch {
        persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
        executable,
        arguments,
        environment: Vec::new(),
    })
    .await
    .expect("provider initializes");
    let provider_session_id = runtime
        .create_session(cwd)
        .await
        .expect("provider conversation created");

    {
        let prompt = runtime.prompt(
            provider_session_id.clone(),
            "Without using tools, write the integers from 1 through 100000, one per line."
                .to_owned(),
        );
        tokio::pin!(prompt);
        tokio::select! {
            result = &mut prompt => panic!("provider prompt settled before explicit cancel: {result:?}"),
            () = tokio::time::sleep(Duration::from_millis(500)) => {}
        }
        runtime
            .cancel_active_prompt(provider_session_id.clone())
            .await
            .expect("provider accepts cancel notification");
        let outcome = tokio::time::timeout(Duration::from_secs(15), prompt)
            .await
            .expect("cancelled prompt settles")
            .expect("cancelled prompt outcome");
        eprintln!(
            "cancelled provider target={provider_session_id} stop={:?}",
            outcome.stop_reason
        );
        assert_eq!(outcome.stop_reason, ProviderPromptStopReason::Cancelled);
    }

    runtime.shutdown().await;
}
