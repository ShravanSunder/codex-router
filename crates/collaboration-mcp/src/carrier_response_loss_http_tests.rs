//! Carrier response loss through the stateless collaboration API: a lost or refused carrier
//! answer keeps the known target and effect, and nothing is replayed.
use crate::api_test_harness::{ServedApi, api_config, test_identity};
use collaboration_service::CollaborationApplication;
use futures_util::StreamExt;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn stateless_http_resumed_prompt_load_response_loss_retains_target_without_replay() {
    let body = run_stateless_mcp_resumed_prompt_after_load(None).await;

    assert_eq!(body.pointer("/result/isError"), Some(&json!(true)));
    assert_eq!(
        body.pointer("/result/structuredContent/target"),
        Some(&json!({
            "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
            "sessionId":"resumed-thread"
        }))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/stage"),
        Some(&json!("load"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/effect"),
        Some(&json!("unknown"))
    );
}

#[tokio::test]
async fn stateless_http_resumed_prompt_load_rejection_retains_target_code_and_data_without_replay()
{
    let body = run_stateless_mcp_resumed_prompt_after_load(Some(json!({
        "code": -32050,
        "message": "fixture load rejected",
        "data": {"kind":"nativeRejected","stage":"load","reason":"fixture-policy"}
    })))
    .await;

    assert_eq!(body.pointer("/result/isError"), Some(&json!(true)));
    assert_eq!(
        body.pointer("/result/structuredContent/target"),
        Some(&json!({
            "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
            "sessionId":"resumed-thread"
        }))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/stage"),
        Some(&json!("load"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/effect"),
        Some(&json!("unknown"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/code"),
        Some(&json!(-32050))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/data"),
        Some(&json!({"kind":"nativeRejected","stage":"load","reason":"fixture-policy"}))
    );
}

async fn run_stateless_mcp_resumed_prompt_after_load(load_error: Option<Value>) -> Value {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private service directory");
    }
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let digest = format!("sha256:{}", "a".repeat(64));
    let description = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"}, "label":"MCP resumed load fixture",
        "availability":{"state":"available","observedAt":"2026-09-19T00:00:00Z"},
        "channels":[{"kind":"acp","transport":"unixJsonLines","path":"acp.sock","schemaDigest":format!("sha256:{}", collaboration_protocol::ACP_SCHEMA_DIGEST)}]
    }))
    .expect("endpoint description");
    let identity = collaboration_service::ServiceIdentity::new(service_id, epoch, &digest)
        .expect("service identity")
        .with_endpoints(vec![description])
        .expect("endpoint directory");
    let control = collaboration_service::LocalControlService::bind(
        &temporary.path().join("control.sock"),
        identity,
    )
    .expect("control listener");
    let manifest = serde_json::from_value(json!({
        "version":2,"serviceId":service_id,"serviceEpoch":epoch,
        "machineLabel":"fixture-host","control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .expect("service manifest");
    let publication =
        collaboration_service::ManifestPublication::publish(temporary.path(), &manifest)
            .expect("manifest publication");
    let control_stop = CancellationToken::new();
    let control_task = tokio::spawn(control.run(control_stop.clone()));
    let acp =
        tokio::net::UnixListener::bind(temporary.path().join("acp.sock")).expect("ACP listener");
    let peer = tokio::spawn(async move {
        let (stream, _) = acp.accept().await.expect("ACP accept");
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let initialize: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("ACP initialize read")
                .expect("ACP initialize frame"),
        )
        .expect("ACP initialize JSON");
        assert_eq!(initialize["method"], "initialize");
        writer
            .write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":initialize["id"],"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"authMethods":[]}})).as_bytes())
            .await
            .expect("ACP initialize response");
        let load: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("ACP load read")
                .expect("ACP load frame"),
        )
        .expect("ACP load JSON");
        assert_eq!(load["method"], "session/load");
        assert_eq!(load["params"]["sessionId"], "resumed-thread");
        if let Some(error) = load_error {
            writer
                .write_all(
                    format!(
                        "{}\n",
                        json!({"jsonrpc":"2.0","id":load["id"],"error":error})
                    )
                    .as_bytes(),
                )
                .await
                .expect("ACP load rejection");
            assert!(
                lines
                    .next_line()
                    .await
                    .expect("post-rejection read")
                    .is_none(),
                "a rejected load must not prompt or replay"
            );
        } else {
            drop(lines);
            drop(writer);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(250), acp.accept())
                    .await
                    .is_err(),
                "a lost load response must not trigger a second ACP connection or replay"
            );
        }
    });
    let listener = ServedApi::tcp(&api_config(
        CollaborationApplication::new(test_identity()),
        temporary.path(),
    ))
    .await;
    let client = reqwest::Client::new();
    let response = client
        .post(listener.url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"conversation_prompt","arguments":{
            "target":{"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"resumed-thread"},
            "workingDirectory":temporary.path(),
            "requestedBy":{"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"resumed-thread"},
            "message":{"kind":"humanUser","text":"resume proof"},"timeoutSeconds":3
        }}}))
        .send()
        .await
        .expect("resumed prompt response");
    let body = protocol_response_json(response).await;
    listener.stop().await;
    peer.await.expect("ACP peer join");
    control_stop.cancel();
    control_task
        .await
        .expect("control join")
        .expect("control shutdown");
    drop(publication);
    body
}

#[tokio::test]
async fn stateless_http_acp_initialize_response_loss_is_not_replayed() {
    let body = run_stateless_mcp_create_response_loss(InitializeOutcome::Lost, false).await;

    assert_eq!(body.pointer("/result/isError"), Some(&json!(true)));
    assert_eq!(
        body.pointer("/result/structuredContent/stage"),
        Some(&json!("initialize"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/effect"),
        Some(&json!("none"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/target"),
        Some(&Value::Null)
    );
}

#[tokio::test]
async fn stateless_http_acp_version_mismatch_has_no_effect_or_creation() {
    let body =
        run_stateless_mcp_create_response_loss(InitializeOutcome::VersionMismatch, false).await;

    assert_eq!(body.pointer("/result/isError"), Some(&json!(true)));
    assert_eq!(
        body.pointer("/result/structuredContent/stage"),
        Some(&json!("initialize"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/effect"),
        Some(&json!("none"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/target"),
        Some(&Value::Null)
    );
}

#[tokio::test]
async fn stateless_http_fork_response_loss_is_not_replayed() {
    let body = run_stateless_mcp_create_response_loss(InitializeOutcome::Success, true).await;

    assert_eq!(body.pointer("/result/isError"), Some(&json!(true)));
    assert_eq!(
        body.pointer("/result/structuredContent/stage"),
        Some(&json!("fork"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/effect"),
        Some(&json!("unknown"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/target"),
        Some(&Value::Null)
    );
}

#[tokio::test]
async fn stateless_http_observation_attach_failure_retains_target_and_pre_dispatch_effect() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private service directory");
    }
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let digest = format!("sha256:{}", "a".repeat(64));
    let target = json!({"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"observe-thread"});
    let identity = collaboration_service::ServiceIdentity::new(service_id, epoch, &digest)
        .expect("service identity")
        .with_endpoints(vec![serde_json::from_value(json!({
            "endpoint":{"serviceId":service_id,"endpointId":"codex-local"}, "label":"MCP observation fixture",
            "availability":{"state":"available","observedAt":"2026-09-19T00:00:00Z"},
            "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"missing-native.sock","schemaDigest":null,"generation":{"serviceEpoch":epoch,"generation":1}}]
        })).expect("endpoint description")])
        .expect("endpoint directory");
    let control = collaboration_service::LocalControlService::bind(
        &temporary.path().join("control.sock"),
        identity,
    )
    .expect("control listener");
    let manifest = serde_json::from_value(json!({
        "version":2,"serviceId":service_id,"serviceEpoch":epoch,
        "machineLabel":"fixture-host","control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest,
        "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .expect("service manifest");
    let publication =
        collaboration_service::ManifestPublication::publish(temporary.path(), &manifest)
            .expect("manifest publication");
    let stop = CancellationToken::new();
    let control_task = tokio::spawn(control.run(stop.clone()));
    let listener = ServedApi::tcp(&api_config(
        CollaborationApplication::new(test_identity()),
        temporary.path(),
    ))
    .await;
    let client = reqwest::Client::new();
    let response = client.post(listener.url()).header(CONTENT_TYPE, "application/json").header(ACCEPT, "application/json, text/event-stream").header("mcp-protocol-version", "2025-11-25").json(&json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"events_observe","arguments":{"target":target,"timeoutSeconds":1,"maxEvents":1,"maxBytes":1}}
    })).send().await.expect("observation response");
    let body = protocol_response_json(response).await;
    assert_eq!(body.pointer("/result/isError"), Some(&json!(true)));
    assert_eq!(
        body.pointer("/result/structuredContent/target"),
        Some(&target)
    );
    assert_eq!(
        body.pointer("/result/structuredContent/stage"),
        Some(&json!("observation-attach"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/effect"),
        Some(&json!("none"))
    );
    listener.stop().await;
    stop.cancel();
    control_task
        .await
        .expect("control join")
        .expect("control shutdown");
    drop(publication);
}

#[tokio::test]
async fn stateless_http_observation_resume_response_loss_retains_target_and_unknown_effect() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private service directory");
    }
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let digest = format!("sha256:{}", "a".repeat(64));
    let target = json!({"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"observe-thread"});
    let identity = collaboration_service::ServiceIdentity::new(service_id, epoch, &digest).expect("service identity").with_endpoints(vec![serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"}, "label":"MCP observation resume fixture",
        "availability":{"state":"available","observedAt":"2026-09-19T00:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":null,"generation":{"serviceEpoch":epoch,"generation":1}}]
    })).expect("endpoint description")]).expect("endpoint directory");
    let control = collaboration_service::LocalControlService::bind(
        &temporary.path().join("control.sock"),
        identity,
    )
    .expect("control listener");
    let manifest = serde_json::from_value(json!({"version":2,"serviceId":service_id,"serviceEpoch":epoch,"machineLabel":"fixture-host","control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}})).expect("service manifest");
    let publication =
        collaboration_service::ManifestPublication::publish(temporary.path(), &manifest)
            .expect("manifest publication");
    let stop = CancellationToken::new();
    let control_task = tokio::spawn(control.run(stop.clone()));
    let native = tokio::net::UnixListener::bind(temporary.path().join("native.sock"))
        .expect("native listener");
    let native_peer = tokio::spawn(async move {
        let (stream, _) = native.accept().await.expect("native accept");
        let mut socket = tokio_tungstenite::accept_async(stream)
            .await
            .expect("native websocket");
        let initialize = socket
            .next()
            .await
            .expect("initialize frame")
            .expect("initialize receive");
        let initialize: Value =
            serde_json::from_str(initialize.to_text().expect("initialize text"))
                .expect("initialize JSON");
        assert_eq!(initialize["method"], "initialize");
        use futures_util::SinkExt;
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"id":initialize["id"],"result":{"userAgent":"observation-fixture"}})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("initialize response");
        let initialized = socket
            .next()
            .await
            .expect("initialized frame")
            .expect("initialized receive");
        assert_eq!(
            serde_json::from_str::<Value>(initialized.to_text().expect("initialized text"))
                .expect("initialized JSON")["method"],
            "initialized"
        );
        let resume = socket
            .next()
            .await
            .expect("resume frame")
            .expect("resume receive");
        let resume: Value =
            serde_json::from_str(resume.to_text().expect("resume text")).expect("resume JSON");
        assert_eq!(resume["method"], "thread/resume");
        assert_eq!(resume["params"]["threadId"], "observe-thread");
    });
    let listener = ServedApi::tcp(&api_config(
        CollaborationApplication::new(test_identity()),
        temporary.path(),
    ))
    .await;
    let client = reqwest::Client::new();
    let response = client.post(listener.url()).header(CONTENT_TYPE, "application/json").header(ACCEPT, "application/json, text/event-stream").header("mcp-protocol-version", "2025-11-25").json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"events_observe","arguments":{"target":target,"timeoutSeconds":1,"maxEvents":1,"maxBytes":1}}})).send().await.expect("observation response");
    let body = protocol_response_json(response).await;
    assert_eq!(body.pointer("/result/isError"), Some(&json!(true)));
    assert_eq!(
        body.pointer("/result/structuredContent/target"),
        Some(&target)
    );
    assert_eq!(
        body.pointer("/result/structuredContent/stage"),
        Some(&json!("observation-attach"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/effect"),
        Some(&json!("unknown"))
    );
    listener.stop().await;
    native_peer.await.expect("native peer join");
    stop.cancel();
    control_task
        .await
        .expect("control join")
        .expect("control shutdown");
    drop(publication);
}

#[derive(Clone, Copy)]
enum InitializeOutcome {
    Success,
    Lost,
    VersionMismatch,
}

async fn run_stateless_mcp_create_response_loss(
    initialize_outcome: InitializeOutcome,
    fork: bool,
) -> Value {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private service directory");
    }
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let digest = format!("sha256:{}", "a".repeat(64));
    let endpoint = json!({"serviceId":service_id,"endpointId":"codex-local"});
    let description = serde_json::from_value(json!({
        "endpoint":endpoint, "label":"MCP create-loss fixture",
        "availability":{"state":"available","observedAt":"2026-09-19T00:00:00Z"},
        "channels":[{"kind":"acp","transport":"unixJsonLines","path":"acp.sock","schemaDigest":format!("sha256:{}", collaboration_protocol::ACP_SCHEMA_DIGEST)}]
    }))
    .expect("endpoint description");
    let identity = collaboration_service::ServiceIdentity::new(service_id, epoch, &digest)
        .expect("service identity")
        .with_endpoints(vec![description])
        .expect("endpoint directory");
    let control = collaboration_service::LocalControlService::bind(
        &temporary.path().join("control.sock"),
        identity,
    )
    .expect("control listener");
    let manifest = serde_json::from_value(json!({
        "version":2,"serviceId":service_id,"serviceEpoch":epoch,
        "machineLabel":"fixture-host","control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .expect("service manifest");
    let publication =
        collaboration_service::ManifestPublication::publish(temporary.path(), &manifest)
            .expect("manifest publication");
    let control_stop = CancellationToken::new();
    let control_task = tokio::spawn(control.run(control_stop.clone()));
    let acp =
        tokio::net::UnixListener::bind(temporary.path().join("acp.sock")).expect("ACP listener");
    let peer = tokio::spawn(async move {
        let (stream, _) = acp.accept().await.expect("ACP accept");
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let initialize: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("ACP initialize read")
                .expect("ACP initialize frame"),
        )
        .expect("ACP initialize JSON");
        assert_eq!(initialize["method"], "initialize");
        if matches!(initialize_outcome, InitializeOutcome::Lost) {
            drop(writer);
            drop(lines);
        } else {
            let protocol_version =
                if matches!(initialize_outcome, InitializeOutcome::VersionMismatch) {
                    2
                } else {
                    1
                };
            writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":initialize["id"],"result":{"protocolVersion":protocol_version,"agentCapabilities":{"loadSession":true},"authMethods":[]}})).as_bytes()).await.expect("ACP initialize response");
            if matches!(initialize_outcome, InitializeOutcome::VersionMismatch) {
                let next =
                    tokio::time::timeout(std::time::Duration::from_millis(250), lines.next_line())
                        .await;
                assert!(
                    !matches!(next, Ok(Ok(Some(_)))),
                    "version mismatch must not dispatch an ACP conversation operation"
                );
                return;
            }
            let create: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("ACP create read")
                    .expect("ACP create frame"),
            )
            .expect("ACP create JSON");
            assert_eq!(create["method"], "session/new");
            assert_eq!(
                create.pointer("/params/_meta/codexRouter/forkThreadId"),
                Some(&json!("fork-source"))
            );
            drop(writer);
            drop(lines);
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(250), acp.accept())
                .await
                .is_err(),
            "a lost ACP response must not create a replacement connection or replay the mutation"
        );
    });
    let listener = ServedApi::tcp(&api_config(
        CollaborationApplication::new(test_identity()),
        temporary.path(),
    ))
    .await;
    let client = reqwest::Client::new();
    let response = client.post(listener.url()).header(CONTENT_TYPE, "application/json").header(ACCEPT, "application/json, text/event-stream").header("mcp-protocol-version", "2025-11-25").json(&json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"conversation_create","arguments":{
            "operationId":collaboration_protocol::OperationId::generate(),
            "endpoint":endpoint,"workingDirectory":temporary.path(),"fork":if fork { json!("fork-source") } else { Value::Null },
            "model":"gpt-5.6-luna","effort":if fork { Value::Null } else { json!("low") },"access":"workspace-write",
            "createdBy":{"endpoint":endpoint,"sessionId":"mcp-creator"},
            "approver":{"endpoint":endpoint,"sessionId":"mcp-approver"},"rootMessageId":null
        }}
    })).send().await.expect("conversation create response");
    let body = protocol_response_json(response).await;
    listener.stop().await;
    peer.await.expect("ACP peer join");
    control_stop.cancel();
    control_task
        .await
        .expect("control join")
        .expect("control shutdown");
    drop(publication);
    body
}

async fn protocol_response_json(response: reqwest::Response) -> Value {
    let status = response.status();
    let body = response.text().await.expect("protocol response body");
    assert!(
        !body.is_empty(),
        "empty protocol response with status {status}"
    );
    let encoded = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|data| !data.is_empty())
        .unwrap_or(body.as_str());
    serde_json::from_str(encoded)
        .unwrap_or_else(|error| panic!("protocol response JSON: {error}; body={body:?}"))
}
