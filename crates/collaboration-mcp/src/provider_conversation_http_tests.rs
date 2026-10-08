//! Provider conversation tools over the stateless API: the composites run in the Host over the
//! Router's own provider operations, here a scripted provider backend that records what it
//! was asked.
use crate::api_test_harness::{ServedApi, api_config, test_identity};
use collaboration_service::CollaborationApplication;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use scripted_provider_backend::ScriptedProviderBackend;
use serde_json::{Value, json};
use std::sync::Arc;

#[path = "provider_conversation_http_tests/scripted_provider_backend.rs"]
mod scripted_provider_backend;

const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
const SERVICE_EPOCH: &str = "00000000-0000-4000-8000-000000000002";
const CREATE_OPERATION: &str = "019f0000-0000-7000-8000-000000000001";
const PROMPT_OPERATION: &str = "019f0000-0000-7000-8000-000000000002";
const LOAD_OPERATION: &str = "019f0000-0000-7000-8000-000000000004";
const PENDING_OPERATION: &str = "019f0000-0000-7000-8000-000000000005";
const CANCEL_OPERATION: &str = "019f0000-0000-7000-8000-000000000006";

#[tokio::test]
async fn stateless_http_exposes_one_conversation_surface_for_provider_operations() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    let created = json!({"kind":"available","settlement":{"kind":"created",
        "target":{"endpoint":{"serviceId":SERVICE_ID,"endpointId":"claude-code"},"sessionId":"provider-thread"},
        "effectiveSettings":{"requestedPolicy":{"access":"workspace-write"},"mappingStatus":"verified","authentication":"authenticated"}}});
    let completed = json!({"kind":"available","settlement":{
        "kind":"promptCompleted",
        "target":{"endpoint":{"serviceId":SERVICE_ID,"endpointId":"claude-code"},"sessionId":"provider-thread"},
        "stopReason":"end_turn","response":"provider reply"
    }});
    let backend = ScriptedProviderBackend::new()
        .answer(
            "create",
            json!({"result":{"admission":"admitted",
            "operation":terminal_snapshot(CREATE_OPERATION, "conversationCreate")}}),
        )
        .answer(
            "wait",
            json!({"result":{
                "operation":terminal_snapshot(CREATE_OPERATION, "conversationCreate"),
                "output":created
            }}),
        )
        .answer(
            "prompt",
            json!({"result":{"admission":"admitted",
            "operation":operation_snapshot(PROMPT_OPERATION, "conversationPrompt")}}),
        )
        .answer(
            "wait",
            json!({"result":{
                "operation":terminal_snapshot(PROMPT_OPERATION, "conversationPrompt"),
                "output":completed
            }}),
        )
        .answer(
            "load",
            json!({"error":{
                "kind":"busy",
                "stage":"admission",
                "effect":"none",
                "message":"provider binding is busy",
                "operationId":LOAD_OPERATION,
                "target":null
            }}),
        );
    let listener = start_listener(temporary.path(), &backend).await;
    let client = reqwest::Client::new();

    let tools = call_mcp(&client, &listener, 2, "tools/list", json!({})).await;
    let listed = tools["result"]["tools"].as_array().expect("tool array");
    for name in [
        "conversation_create",
        "conversation_load",
        "conversation_prompt",
        "conversation_cancel",
        "conversation_operation_show",
        "conversation_operation_wait",
        "conversation_operation_reconcile",
    ] {
        let tool = listed
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("missing conversation tool {name}"));
        if matches!(name, "conversation_create" | "conversation_cancel") {
            assert!(
                tool["inputSchema"]["required"]
                    .as_array()
                    .expect("required inputs")
                    .contains(&json!("operationId")),
                "{name} must require the outer caller's operationId"
            );
        }
    }

    let endpoint = json!({"serviceId":SERVICE_ID,"endpointId":"claude-code"});
    let actor = json!({"endpoint":endpoint,"sessionId":"caller-session"});
    let create = call_tool(
        &client,
        &listener,
        3,
        "conversation_create",
        json!({
            "operationId":CREATE_OPERATION,
            "endpoint":endpoint,
            "generation":generation(),
            "workingDirectory":temporary.path(),
            "createdBy":actor,
            "approver":actor,
            "access":"workspace-write"
        }),
    )
    .await;
    assert_eq!(create["result"]["isError"], false, "{create}");
    assert_eq!(
        create["result"]["structuredContent"]["operationId"],
        CREATE_OPERATION
    );

    let target = json!({"endpoint":endpoint,"sessionId":"provider-thread"});
    let prompt = call_tool(
        &client,
        &listener,
        4,
        "conversation_prompt",
        json!({
            "operationId":PROMPT_OPERATION,
            "target":target,
            "generation":generation(),
            "requestedBy":actor,
            "approver":actor,
            "message":{"kind":"humanUser","text":"continue the assignment"}
        }),
    )
    .await;
    assert_eq!(prompt["result"]["isError"], false);
    assert_eq!(prompt["result"]["structuredContent"]["kind"], "completed");
    assert_eq!(
        prompt["result"]["structuredContent"]["operationId"],
        PROMPT_OPERATION
    );
    assert_eq!(
        prompt["result"]["structuredContent"]["settlement"]["stopReason"],
        "endTurn"
    );
    assert_eq!(
        prompt["result"]["structuredContent"]["settlement"]["detail"]["kind"],
        "providerPrompt"
    );
    assert_eq!(
        prompt["result"]["structuredContent"]["settlement"]["detail"]["output"]["kind"],
        "available"
    );
    assert_eq!(
        prompt["result"]["structuredContent"]["settlement"]["detail"]["output"]["text"],
        "provider reply"
    );

    let load = call_tool(
        &client,
        &listener,
        6,
        "conversation_load",
        json!({
            "operationId":LOAD_OPERATION,
            "target":target,
            "generation":generation(),
            "workingDirectory":temporary.path(),
            "requestedBy":actor,
            "approver":actor,
            "access":"workspace-write"
        }),
    )
    .await;
    assert_eq!(load["result"]["isError"], true);
    assert_eq!(load["result"]["structuredContent"]["kind"], "busy");
    assert_eq!(
        load["result"]["structuredContent"]["operationId"],
        LOAD_OPERATION
    );
    let calls = backend.calls();
    assert_eq!(
        calls
            .iter()
            .map(|(operation, _)| *operation)
            .collect::<Vec<_>>(),
        ["create", "wait", "prompt", "wait", "load"]
    );
    for ((_, request), operation_id) in calls.iter().zip([
        CREATE_OPERATION,
        CREATE_OPERATION,
        PROMPT_OPERATION,
        PROMPT_OPERATION,
        LOAD_OPERATION,
    ]) {
        assert_eq!(request["operationId"], operation_id);
    }
    listener.stop().await;
}

#[tokio::test]
async fn mcp_provider_create_accepts_typed_human_creator_and_approver() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    let backend = ScriptedProviderBackend::new()
        .answer("create", json!({"result":{
            "admission":"admitted",
            "operation":operation_snapshot(CREATE_OPERATION,"conversationCreate")
        }}))
        .answer("wait", json!({"result":{
            "operation":terminal_snapshot(CREATE_OPERATION,"conversationCreate"),
            "output":{"kind":"available","settlement":{"kind":"created",
                "target":{"endpoint":{"serviceId":SERVICE_ID,"endpointId":"claude-code"},"sessionId":"provider-thread"},
                "effectiveSettings":{"requestedPolicy":{"access":"workspace-write"},"mappingStatus":"verified","authentication":"authenticated"}}}
        }}));
    let listener = start_listener(temporary.path(), &backend).await;
    let client = reqwest::Client::new();
    let human = json!({"kind":"human","humanId":"fixture-owner"});
    let created = call_tool(
        &client,
        &listener,
        2,
        "conversation_create",
        json!({
            "operationId":CREATE_OPERATION,
            "endpoint":{"serviceId":SERVICE_ID,"endpointId":"claude-code"},
            "generation":generation(),
            "workingDirectory":temporary.path(),
            "createdBy":human,
            "approver":human,
            "access":"workspace-write"
        }),
    )
    .await;
    assert_eq!(created["result"]["isError"], false, "{created}");
    assert_eq!(created["result"]["structuredContent"]["kind"], "created");
    let calls = backend.calls();
    assert_eq!(
        calls
            .iter()
            .map(|(operation, _)| *operation)
            .collect::<Vec<_>>(),
        ["create", "wait"]
    );
    let human_wire = json!({"humanId":"fixture-owner"});
    assert_eq!(calls[0].1["createdBy"], human_wire);
    assert_eq!(calls[0].1["approver"], human_wire);
    listener.stop().await;
}

#[tokio::test]
async fn provider_prompt_wait_timeout_returns_pending_exact_operation_and_target() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    let snapshot = operation_snapshot(PENDING_OPERATION, "conversationPrompt");
    let backend = ScriptedProviderBackend::new()
        .answer(
            "prompt",
            json!({"result":{"admission":"admitted","operation":snapshot.clone()}}),
        )
        .answer(
            "wait",
            json!({"result":{"operation":snapshot,"output":{"kind":"pending"}}}),
        );
    let listener = start_listener(temporary.path(), &backend).await;
    let client = reqwest::Client::new();
    let endpoint = json!({"serviceId":SERVICE_ID,"endpointId":"claude-code"});
    let target = json!({"endpoint":endpoint,"sessionId":"provider-thread"});
    let actor = json!({"endpoint":endpoint,"sessionId":"caller-session"});
    let response = call_tool(
        &client,
        &listener,
        2,
        "conversation_prompt",
        json!({
            "operationId":PENDING_OPERATION,"target":target,
            "requestedBy":actor,"approver":actor,
            "message":{"kind":"humanUser","text":"pending proof"},
            "timeoutSeconds":1
        }),
    )
    .await;
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(response["result"]["structuredContent"]["kind"], "pending");
    assert_eq!(
        response["result"]["structuredContent"]["operationId"],
        PENDING_OPERATION
    );
    assert_eq!(response["result"]["structuredContent"]["target"], target);
    let calls = backend.calls();
    assert_eq!(
        calls
            .iter()
            .map(|(operation, _)| *operation)
            .collect::<Vec<_>>(),
        ["prompt", "wait"],
        "a pending prompt must not cancel or retry"
    );
    assert!(
        calls
            .iter()
            .all(|(_, request)| request["operationId"] == PENDING_OPERATION)
    );
    listener.stop().await;
}

#[tokio::test]
async fn provider_prompt_without_operation_id_names_uuidv7_generator_before_mutation() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    let backend = ScriptedProviderBackend::new();
    let listener = start_listener(temporary.path(), &backend).await;
    let client = reqwest::Client::new();
    let endpoint = json!({"serviceId":SERVICE_ID,"endpointId":"claude-code"});
    let target = json!({"endpoint":endpoint,"sessionId":"provider-thread"});
    let actor = json!({"endpoint":endpoint,"sessionId":"caller-session"});
    let response = call_tool(
        &client,
        &listener,
        2,
        "conversation_prompt",
        json!({"target":target,"requestedBy":actor,
            "message":{"kind":"humanUser","text":"must not submit"},"timeoutSeconds":1}),
    )
    .await;
    assert_eq!(response["result"]["isError"], true, "{response}");
    assert_eq!(
        response["result"]["structuredContent"]["kind"],
        "invalidRequest"
    );
    assert_eq!(response["result"]["structuredContent"]["effect"], "none");
    assert_eq!(
        response["result"]["structuredContent"]["operation"],
        "prompt"
    );
    let message = response["result"]["structuredContent"]["message"]
        .as_str()
        .expect("provider operation ID message");
    assert!(
        message.contains("canonical lowercase RFC UUIDv7"),
        "{message}"
    );
    let fix = response["result"]["structuredContent"]["fix"]
        .as_str()
        .expect("generator fix");
    assert!(
        fix.contains("python3 -c 'import uuid; print(uuid.uuid7())'"),
        "{fix}"
    );
    let load = call_tool(
        &client,
        &listener,
        3,
        "conversation_load",
        json!({"target":target,"workingDirectory":temporary.path(),
            "requestedBy":actor,"access":"workspace-write","timeoutSeconds":1}),
    )
    .await;
    assert_eq!(load["result"]["isError"], true, "{load}");
    assert_eq!(
        load["result"]["structuredContent"]["kind"],
        "invalidRequest"
    );
    assert_eq!(load["result"]["structuredContent"]["effect"], "none");
    assert_eq!(load["result"]["structuredContent"]["operation"], "load");
    assert!(
        load["result"]["structuredContent"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("UUIDv7"))
    );
    assert!(
        load["result"]["structuredContent"]["fix"]
            .as_str()
            .is_some_and(|fix| fix.contains("python3 -c 'import uuid; print(uuid.uuid7())'"))
    );
    assert!(
        backend.calls().is_empty(),
        "a provider prompt or load without operationId must not mutate the provider"
    );
    listener.stop().await;
}

#[tokio::test]
async fn provider_cancel_keeps_the_exact_target_operation_on_the_common_mcp_surface() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    let backend = ScriptedProviderBackend::new().answer(
        "cancel",
        json!({"result":{
            "admission":"admitted",
            "operation":operation_snapshot(CANCEL_OPERATION,"conversationCancel")
        }}),
    );
    let listener = start_listener(temporary.path(), &backend).await;
    let client = reqwest::Client::new();
    let endpoint = json!({"serviceId":SERVICE_ID,"endpointId":"claude-code"});
    let target = json!({"endpoint":endpoint,"sessionId":"provider-thread"});
    let actor = json!({"endpoint":endpoint,"sessionId":"caller-session"});
    let response = call_tool(
        &client,
        &listener,
        2,
        "conversation_cancel",
        json!({
            "operationId":CANCEL_OPERATION,"targetOperationId":PROMPT_OPERATION,
            "target":target,"requestedBy":actor,"approver":actor
        }),
    )
    .await;
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(
        response["result"]["structuredContent"]["operation"]["operationId"],
        CANCEL_OPERATION
    );
    assert_eq!(
        response["result"]["structuredContent"]["operation"]["operation"],
        "conversationCancel"
    );
    let calls = backend.calls();
    assert_eq!(
        calls
            .iter()
            .map(|(operation, _)| *operation)
            .collect::<Vec<_>>(),
        ["cancel"],
        "cancel is one exact mutation, not a replay or wait"
    );
    let cancel = &calls[0].1;
    assert_eq!(cancel["operationId"], CANCEL_OPERATION);
    assert_eq!(cancel["targetOperationId"], PROMPT_OPERATION);
    assert_eq!(cancel["target"]["sessionId"], "provider-thread");
    listener.stop().await;
}

/// The API over a Router whose provider endpoint is served by `backend`.
async fn start_listener(path: &std::path::Path, backend: &ScriptedProviderBackend) -> ServedApi {
    let identity = test_identity()
        .with_endpoints(vec![
            serde_json::from_value(provider_endpoint()).expect("provider endpoint"),
        ])
        .expect("endpoint directory")
        .with_provider_conversation_backend(Arc::new(backend.clone()));
    ServedApi::tcp(&api_config(CollaborationApplication::new(identity), path)).await
}

fn operation_snapshot(operation_id: &str, operation: &str) -> Value {
    json!({
        "operationId":operation_id,
        "operation":operation,
        "binding":{"kind":"externalProvider","binding":{
            "endpoint":{"serviceId":SERVICE_ID,"endpointId":"claude-code"},
            "bindingId":"binding-1",
            "runtime":{"provider":"claudeCode","runtimeName":"claude-agent-acp","runtimeVersion":"1.0.0"},
            "transport":"stdioAcp",
            "generation":generation(),
            "capabilities":[{"name":"prompt","status":"supported","evidence":"advertised"}]
        }},
        "target":{"endpoint":{"serviceId":SERVICE_ID,"endpointId":"claude-code"},"sessionId":"provider-thread"},
        "stage":"mayHaveDispatched",
        "effect":"unknown",
        "reconciliation":"unresolved",
        "admittedAt":"2026-09-20T12:00:00Z",
        "terminalAt":null
    })
}

fn terminal_snapshot(operation_id: &str, operation: &str) -> Value {
    let mut snapshot = operation_snapshot(operation_id, operation);
    snapshot["stage"] = json!("terminal");
    snapshot["effect"] = json!("applied");
    snapshot["reconciliation"] = json!("confirmed");
    snapshot["terminalAt"] = json!("2026-09-20T12:00:01Z");
    snapshot
}

fn provider_endpoint() -> Value {
    json!({
        "endpoint":{"serviceId":SERVICE_ID,"endpointId":"claude-code"},
        "label":"Fixture provider",
        "availability":{"state":"available","observedAt":"2026-09-20T12:00:00Z"},
        "channels":[{
            "kind":"externalProvider","transport":"stdioAcp",
            "bindingId":"binding-1","bindingGeneration":3,
            "runtime":{"provider":"claudeCode","runtimeName":"claude-agent-acp"},
            "capabilities":[{"name":"create","status":"supported","evidence":"advertised"}]
        }]
    })
}

/// The binding the scripted backend serves the provider endpoint with.
fn provider_binding() -> Value {
    operation_snapshot(CREATE_OPERATION, "conversationCreate")["binding"]["binding"].clone()
}

fn generation() -> Value {
    json!({"serviceEpoch":SERVICE_EPOCH,"generation":3})
}

async fn call_tool(
    client: &reqwest::Client,
    listener: &ServedApi,
    id: u64,
    name: &str,
    arguments: Value,
) -> Value {
    call_mcp(
        client,
        listener,
        id,
        "tools/call",
        json!({"name":name,"arguments":arguments}),
    )
    .await
}

async fn call_mcp(
    client: &reqwest::Client,
    listener: &ServedApi,
    id: u64,
    method: &str,
    params: Value,
) -> Value {
    let response = client
        .post(listener.url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
        .send()
        .await
        .expect("MCP response");
    protocol_response_json(response).await
}

async fn protocol_response_json(response: reqwest::Response) -> Value {
    let body = response.text().await.expect("MCP body");
    protocol_body_json(&body)
}

fn protocol_body_json(body: &str) -> Value {
    let encoded = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|data| !data.is_empty())
        .unwrap_or(body);
    serde_json::from_str(encoded).unwrap_or_else(|error| panic!("MCP JSON: {error}; body={body:?}"))
}
