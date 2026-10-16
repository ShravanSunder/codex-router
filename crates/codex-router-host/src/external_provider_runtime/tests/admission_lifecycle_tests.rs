use super::*;

#[tokio::test]
async fn missing_executable_fails_before_runtime_admission() {
    let error = ExternalProviderRuntime::initialize(ExternalProviderLaunch {
        persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
        executable: PathBuf::from("/definitely/missing/codex-router-acp-provider"),
        arguments: vec![],
        environment: vec![],
    })
    .await
    .expect_err("missing executable must fail");

    assert!(matches!(error, ExternalProviderRuntimeError::Launch(_)));
}

#[cfg(unix)]
#[tokio::test]
async fn stable_v1_initialize_admits_runtime_and_capabilities() {
    let runtime = ExternalProviderRuntime::initialize(python_fixture(1, None))
        .await
        .expect("fixture initializes");

    assert_eq!(
        runtime.admission(),
        &ExternalProviderAdmission {
            runtime_name: Some("fixture-agent".to_owned()),
            runtime_version: Some("1.2.3".to_owned()),
            supports_load: true,
            supports_mcp_http: false,
            supports_steering: false,
        }
    );

    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn initialize_advertises_exact_supported_client_capabilities() {
    // ACP v1 initialization.mdx:28-54 requires protocolVersion and the
    // supported capabilities; omitted capabilities are unsupported (lines
    // 100-115). The Router supports form elicitation through its Question broker.
    let root = tempfile::tempdir().expect("fixture root");
    let fixture = acp_scripted_fixture::AcpFixtureScript::new()
        .expect_exact_request(
            "initialize",
            "initialize",
            serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": {"auth": {"terminal": false}, "elicitation": {"form": {}}},
                "clientInfo": {"name": "codex-router", "version": env!("CARGO_PKG_VERSION")}
            }),
        )
        .respond("initialize", serde_json::json!({"protocolVersion": 1, "agentCapabilities": {}, "agentInfo": {"name": "initialize-fixture", "version": "1"}}))
        .record_diagnostics(root.path().join("fixture-diagnostics.txt"))
        .launch();
    let runtime = ExternalProviderRuntime::initialize_with_timeout(fixture, Duration::from_secs(2))
        .await
        .unwrap_or_else(|error| {
            panic!(
                "initialize failed: {error}; wire diff: {}",
                std::fs::read_to_string(root.path().join("fixture-diagnostics.txt"))
                    .unwrap_or_default()
            )
        });
    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn capability_report_uses_initialize_and_session_advertisements() {
    // ACP v1 initialization.mdx:100-115 and session-setup.mdx:52-85:
    // omitted features are unsupported; Session responses add modes/options.
    let fixture = acp_scripted_fixture::AcpFixtureScript::new()
        .expect_request(
            "initialize",
            "initialize",
            serde_json::json!({"protocolVersion": 1}),
        )
        .respond(
            "initialize",
            serde_json::json!({
                "protocolVersion": 1,
                "agentCapabilities": {
                    "loadSession": true,
                    "sessionCapabilities": {"list": {}, "resume": {}, "close": {}},
                    "promptCapabilities": {"image": true, "audio": false, "embeddedContext": true}
                },
                "agentInfo": {"name": "capability-fixture", "version": "1"}
            }),
        )
        .expect_request("create", "session/new", serde_json::json!({}))
        .respond(
            "create",
            serde_json::json!({
                "sessionId": "fixture-session",
                "modes": {"currentModeId": "ask", "availableModes": [{"id": "ask", "name": "Ask"}]},
                "configOptions": [],
                "_meta": {"steering": {"supported": true}}
            }),
        )
        .launch();
    let runtime = ExternalProviderRuntime::initialize(fixture)
        .await
        .expect("fixture initializes");
    let before = runtime.capability_report("fixture-session").await;
    assert!(before.supports_load);
    assert!(before.supports_resume);
    assert!(before.supports_close);
    assert!(before.supports_list);
    assert!(before.router_queue);
    assert!(before.supports_cancel_queued);
    assert!(before.accepts_image);
    assert!(!before.accepts_audio);
    assert!(before.accepts_embedded_resource);
    assert!(!before.supports_steering);
    assert!(!before.supports_modes);
    assert!(!before.supports_config_options);
    assert!(before.supports_elicitation);
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");
    let after = runtime.capability_report("fixture-session").await;
    assert!(after.supports_steering);
    assert!(after.supports_modes);
    assert!(after.supports_config_options);
    runtime.shutdown().await;
}

#[test]
fn prompt_content_gate_matches_advertised_optional_types() {
    // ACP v1 initialization.mdx:202-217: text and resource links are baseline;
    // image, audio and embedded resources require promptCapabilities.
    use acp_client_runtime::ProviderCapabilityReport;
    use acp_client_runtime::ProviderPromptContent;
    use agent_client_protocol::schema::v1::ContentBlock;

    let examples = [
        ("text", serde_json::json!({"type": "text", "text": "hello"})),
        (
            "resourceLink",
            serde_json::json!({"type": "resource_link", "name": "reference", "uri": "file:///tmp/reference"}),
        ),
        (
            "image",
            serde_json::json!({"type": "image", "mimeType": "image/png", "data": "AA=="}),
        ),
        (
            "audio",
            serde_json::json!({"type": "audio", "mimeType": "audio/wav", "data": "AA=="}),
        ),
        (
            "embeddedResource",
            serde_json::json!({"type": "resource", "resource": {"uri": "file:///tmp/reference", "text": "content"}}),
        ),
    ];
    for advertised_mask in 0_u8..8 {
        let report = ProviderCapabilityReport {
            accepts_image: advertised_mask & 1 != 0,
            accepts_audio: advertised_mask & 2 != 0,
            accepts_embedded_resource: advertised_mask & 4 != 0,
            ..ProviderCapabilityReport::default()
        };
        for (content_type, wire_block) in &examples {
            let block: ContentBlock =
                serde_json::from_value(wire_block.clone()).expect("ACP content example");
            let result = ProviderPromptContent::new_for_test(vec![block], &report);
            let accepted = match *content_type {
                "text" | "resourceLink" => true,
                "image" => report.accepts_image,
                "audio" => report.accepts_audio,
                "embeddedResource" => report.accepts_embedded_resource,
                _ => false,
            };
            if accepted {
                assert!(result.is_ok(), "{content_type} mask {advertised_mask}");
            } else {
                assert_eq!(
                    result
                        .expect_err("unadvertised content is rejected")
                        .to_string(),
                    format!("unsupportedContent{{{content_type}}}")
                );
            }
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn text_prompt_reaches_agent_without_optional_prompt_capabilities() {
    // ACP v1 initialization.mdx:202-217 requires text and resource links as
    // baseline prompt content even when promptCapabilities is omitted.
    let fixture = acp_scripted_fixture::AcpFixtureScript::new()
        .expect_request(
            "initialize",
            "initialize",
            serde_json::json!({"protocolVersion": 1}),
        )
        .respond(
            "initialize",
            serde_json::json!({
                "protocolVersion": 1,
                "agentCapabilities": {},
                "agentInfo": {"name": "text-only-fixture", "version": "1"}
            }),
        )
        .expect_request("create", "session/new", serde_json::json!({}))
        .respond(
            "create",
            serde_json::json!({"sessionId": "text-only-session"}),
        )
        .expect_request(
            "prompt",
            "session/prompt",
            serde_json::json!({
                "sessionId": "text-only-session",
                "prompt": [{"type": "text", "text": "baseline text"}]
            }),
        )
        .respond("prompt", serde_json::json!({"stopReason": "end_turn"}))
        .launch();
    let runtime = ExternalProviderRuntime::initialize(fixture)
        .await
        .expect("fixture initializes");
    let session_id = runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");
    let outcome = runtime
        .prompt(session_id, "baseline text".to_owned())
        .await
        .expect("baseline text prompt completes");
    assert_eq!(outcome.stop_reason, ProviderPromptStopReason::EndTurn);
    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn configured_router_mcp_is_injected_into_create_and_load_requests() {
    let create_runtime = ExternalProviderRuntime::initialize_with_mcp_http(
        mcp_session_setup_fixture("session/new"),
        acp_client_runtime::ProviderModelPicker::Standard,
        "router-collaboration",
        "http://127.0.0.1:19090/mcp",
    )
    .await
    .expect("create fixture initializes");
    assert!(create_runtime.admission().supports_mcp_http);
    assert_eq!(
        create_runtime
            .create_session(PathBuf::from("/tmp"))
            .await
            .expect("create with MCP binding"),
        "fixture-session"
    );
    create_runtime.shutdown().await;

    let load_runtime = ExternalProviderRuntime::initialize_with_mcp_http(
        mcp_session_setup_fixture("session/load"),
        acp_client_runtime::ProviderModelPicker::Standard,
        "router-collaboration",
        "http://127.0.0.1:19090/mcp",
    )
    .await
    .expect("load fixture initializes");
    assert!(load_runtime.admission().supports_mcp_http);
    load_runtime
        .load_session("fixture-session".to_owned(), PathBuf::from("/tmp"))
        .await
        .expect("load with MCP binding");
    load_runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn unsupported_protocol_version_is_rejected() {
    let fixture_root = tempfile::tempdir().expect("fixture root");
    let process_id_path = fixture_root.path().join("provider.pid");
    let error = ExternalProviderRuntime::initialize(python_fixture(0, Some(&process_id_path)))
        .await
        .expect_err("v0 must not be admitted");

    assert_eq!(
        error.to_string(),
        "provider selected unsupported ACP protocol version ProtocolVersion(0)"
    );
    assert!(matches!(
        &error,
        ExternalProviderRuntimeError::UnsupportedProtocol {
            actual
        } if *actual == acp_client_runtime::AcpProtocolVersion::new(0)
    ));
    assert_process_reaped(&process_id_path).await;
}

#[cfg(unix)]
#[tokio::test]
async fn clean_eof_before_initialize_is_a_typed_failure() {
    let error = ExternalProviderRuntime::initialize(ExternalProviderLaunch {
        persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
        executable: PathBuf::from("/bin/sh"),
        arguments: vec!["-c".to_owned(), "exit 0".to_owned()],
        environment: vec![],
    })
    .await
    .expect_err("EOF must fail initialization");

    assert!(matches!(error, ExternalProviderRuntimeError::Initialize(_)));
}

#[cfg(unix)]
#[tokio::test]
async fn stdout_eof_after_initialize_retires_live_provider() {
    let fixture_root = tempfile::tempdir().expect("fixture root");
    let process_id_path = fixture_root.path().join("provider.pid");
    let close_marker = fixture_root.path().join("close-stdout");
    let fixture = format!(
        "import json,os,sys,time; open({:?},'w').write(str(os.getpid())); request=json.loads(sys.stdin.readline()); print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{}},'agentInfo':{{'name':'eof-fixture','version':'1'}}}}}})); sys.stdout.flush(); marker={:?};
while not os.path.exists(marker): time.sleep(0.01)
os.close(1); sys.stdin.read()",
        process_id_path.display().to_string(), close_marker.display().to_string()
    );
    let runtime = ExternalProviderRuntime::initialize(ExternalProviderLaunch {
        persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture],
        environment: vec![],
    })
    .await
    .expect("fixture initializes before stdout closes");
    std::fs::write(&close_marker, b"close").expect("signal fixture to close stdout");

    tokio::time::timeout(Duration::from_secs(2), runtime.retirement().cancelled())
        .await
        .expect("stdout EOF must retire the connection while provider stays alive");
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let process_id = std::fs::read_to_string(&process_id_path)
                .expect("fixture wrote process id")
                .trim()
                .parse::<i32>()
                .expect("positive process id");
            let process_id =
                rustix::process::Pid::from_raw(process_id).expect("positive process id");
            if matches!(
                rustix::process::test_kill_process(process_id),
                Err(rustix::io::Errno::SRCH)
            ) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("retired provider process must be reaped");
}

/// Blocks until the fixture has recorded its process id, bounded in real time.
#[cfg(unix)]
fn wait_for_recorded_process_id(process_id_path: &std::path::Path) -> bool {
    let real_deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < real_deadline {
        if std::fs::read_to_string(process_id_path)
            .is_ok_and(|value| value.trim().parse::<i32>().is_ok())
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    false
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn held_initialize_times_out_and_reaps_owned_process() {
    let fixture_root = tempfile::tempdir().expect("fixture root");
    let process_id_path = fixture_root.path().join("provider.pid");
    let fixture = format!(
        "import os,sys,time; open({:?},'w').write(str(os.getpid())); sys.stdin.readline(); time.sleep(60)",
        process_id_path.display().to_string()
    );
    // The timeout must not start running until the fixture is provably alive and
    // holding initialize. A cold Python start can outlast a short timeout, and a
    // fixture killed before it records its pid leaves nothing to prove reaped.
    // A running blocking task stops the paused clock from auto-advancing, so the
    // simulated timeout elapses only after the pid file exists.
    let fixture_recorded_process_id = tokio::task::spawn_blocking({
        let process_id_path = process_id_path.clone();
        move || wait_for_recorded_process_id(&process_id_path)
    });
    let error = ExternalProviderRuntime::initialize_with_timeout(
        ExternalProviderLaunch {
            persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
            executable: PathBuf::from("/usr/bin/python3"),
            arguments: vec!["-c".to_owned(), fixture],
            environment: vec![],
        },
        Duration::from_millis(100),
    )
    .await
    .expect_err("held initialize must time out");

    assert!(
        fixture_recorded_process_id
            .await
            .expect("process id watcher finished"),
        "fixture recorded its process id before the timeout"
    );
    assert!(matches!(
        error,
        ExternalProviderRuntimeError::InitializeTimeout
    ));
    assert_process_reaped(&process_id_path).await;
}

#[cfg(unix)]
#[tokio::test]
async fn oversized_initialize_frame_is_rejected_and_reaps_owned_process() {
    let fixture_root = tempfile::tempdir().expect("fixture root");
    let process_id_path = fixture_root.path().join("provider.pid");
    let fixture = format!(
        "import os,sys; open({:?},'w').write(str(os.getpid())); sys.stdin.readline(); print('x'*{}); sys.stdout.flush(); sys.stdin.read()",
        process_id_path.display().to_string(),
        MAX_ACP_FRAME_BYTES + 1
    );
    let error = ExternalProviderRuntime::initialize_with_timeout(
        ExternalProviderLaunch {
            persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
            executable: PathBuf::from("/usr/bin/python3"),
            arguments: vec!["-c".to_owned(), fixture],
            environment: vec![],
        },
        Duration::from_secs(2),
    )
    .await
    .expect_err("oversized frame must fail initialization");

    assert!(matches!(error, ExternalProviderRuntimeError::Initialize(_)));
    assert_process_reaped(&process_id_path).await;
}

#[cfg(unix)]
#[tokio::test]
async fn dropping_admitted_runtime_reaps_owned_process() {
    let fixture_root = tempfile::tempdir().expect("fixture root");
    let process_id_path = fixture_root.path().join("provider.pid");
    let runtime = ExternalProviderRuntime::initialize(python_fixture(1, Some(&process_id_path)))
        .await
        .expect("fixture initializes");

    drop(runtime);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(value) = std::fs::read_to_string(&process_id_path)
                && let Ok(raw) = value.trim().parse::<i32>()
            {
                let process_id = rustix::process::Pid::from_raw(raw).expect("positive process id");
                if matches!(
                    rustix::process::test_kill_process(process_id),
                    Err(rustix::io::Errno::SRCH)
                ) {
                    break;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("dropped runtime reaped provider");
}

#[cfg(unix)]
#[tokio::test]
async fn host_owned_connection_creates_and_prompts_through_official_sdk() {
    let runtime = ExternalProviderRuntime::initialize(conversation_fixture())
        .await
        .expect("fixture initializes");

    let provider_session_id = runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");
    assert_eq!(provider_session_id, "fixture-session");
    let output = runtime
        .prompt(provider_session_id, "hello".to_owned())
        .await
        .expect("prompt settled");
    assert!(output.output.is_empty());
    assert_eq!(output.stop_reason, ProviderPromptStopReason::EndTurn);

    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn explicit_cancel_remains_responsive_while_prompt_is_active() {
    let runtime = ExternalProviderRuntime::initialize(cancellation_fixture())
        .await
        .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");

    {
        let prompt = runtime.prompt("fixture-session".to_owned(), "hold".to_owned());
        tokio::pin!(prompt);
        tokio::select! {
            biased;
            result = &mut prompt => panic!("prompt settled before cancellation: {result:?}"),
            () = tokio::time::sleep(Duration::from_millis(25)) => {}
        }
        runtime
            .cancel_active_prompt("fixture-session".to_owned())
            .await
            .expect("cancel notification accepted");
        let outcome = prompt.await.expect("cancelled prompt settles");
        assert!(outcome.output.is_empty());
        assert_eq!(outcome.stop_reason, ProviderPromptStopReason::Cancelled);
    }

    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn unavailable_broker_refuses_permission_once_without_payload_retention() {
    let runtime = ExternalProviderRuntime::initialize(permission_request_fixture())
        .await
        .expect("fixture initializes");
    runtime.set_endpoint_id("cursor-local".to_owned()).await;
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");

    let outcome = runtime
        .prompt(
            "fixture-session".to_owned(),
            "request permission".to_owned(),
        )
        .await
        .expect("prompt settles after permission cancellation");
    assert_eq!(outcome.stop_reason, ProviderPromptStopReason::EndTurn);
    assert_eq!(
        outcome.permission_refusal_reason,
        Some(ExternalProviderApprovalRefusalReason::ApprovalBrokerUnavailable)
    );
    assert_eq!(
        runtime.approval_refusal_warnings(),
        vec![ExternalProviderApprovalRefusalWarning {
            endpoint: "cursor-local".to_owned(),
            provider_session_id: "fixture-session".to_owned(),
            method: "session/request_permission",
            reason_code: ExternalProviderApprovalRefusalReason::ApprovalBrokerUnavailable,
        }]
    );
    assert_eq!(
        runtime.permission_observation(),
        ExternalProviderPermissionObservation {
            method: "session/request_permission",
            request_count: 1,
            last_outcome: Some(ExternalProviderPermissionOutcome::Cancelled),
            execute_tool_call_count: 0,
        }
    );

    runtime.shutdown().await;
}
