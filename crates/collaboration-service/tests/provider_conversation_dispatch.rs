use collaboration_protocol::{
    ConversationCancelRequest, ConversationCreateRequest, ConversationLoadRequest,
    ConversationOperationFailure, ConversationOperationReconcileRequest,
    ConversationOperationShowRequest, ConversationOperationSnapshot,
    ConversationOperationSubmission, ConversationOperationWaitRequest,
    ConversationOperationWaitResult, ConversationPromptRequest, EndpointRef,
    ProviderBindingIdentity,
};
use collaboration_service::{
    ProviderConversationBackend, ServiceIdentity, serve_control_connection,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::Mutex,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
type BackendFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ConversationOperationFailure>> + Send + 'a>>;

const SERVICE: &str = "019f0000-0000-7000-8000-000000000001";
const EPOCH: &str = "019f0000-0000-7000-8000-000000000002";

#[derive(Clone)]
struct RecordingBackend {
    bindings: Vec<ProviderBindingIdentity>,
    calls: Arc<Mutex<Vec<(&'static str, Value)>>>,
    failure: Arc<Mutex<Option<ConversationOperationFailure>>>,
    existing_lookup: Arc<Mutex<bool>>,
    submission: ConversationOperationSubmission,
    snapshot: ConversationOperationSnapshot,
    wait_result: ConversationOperationWaitResult,
}

impl ProviderConversationBackend for RecordingBackend {
    fn binding(&self, endpoint: &EndpointRef) -> Option<ProviderBindingIdentity> {
        self.bindings
            .iter()
            .find(|binding| &binding.endpoint == endpoint)
            .cloned()
    }
    fn lookup_existing(
        &self,
        _operation_id: collaboration_protocol::OperationId,
    ) -> BackendFuture<'_, Option<ConversationOperationSnapshot>> {
        let existing = Arc::clone(&self.existing_lookup);
        let snapshot = self.snapshot.clone();
        Box::pin(async move { Ok(existing.lock().await.then_some(snapshot)) })
    }
    fn create(
        &self,
        request: ConversationCreateRequest,
    ) -> BackendFuture<'_, ConversationOperationSubmission> {
        self.record("create", request, self.submission.clone())
    }
    fn load(
        &self,
        request: ConversationLoadRequest,
    ) -> BackendFuture<'_, ConversationOperationSubmission> {
        self.record("load", request, self.submission.clone())
    }
    fn prompt(
        &self,
        request: ConversationPromptRequest,
    ) -> BackendFuture<'_, ConversationOperationSubmission> {
        self.record("prompt", request, self.submission.clone())
    }
    fn cancel(
        &self,
        request: ConversationCancelRequest,
    ) -> BackendFuture<'_, ConversationOperationSubmission> {
        self.record("cancel", request, self.submission.clone())
    }
    fn show(
        &self,
        request: ConversationOperationShowRequest,
    ) -> BackendFuture<'_, ConversationOperationSnapshot> {
        self.record("show", request, self.snapshot.clone())
    }
    fn wait(
        &self,
        request: ConversationOperationWaitRequest,
    ) -> BackendFuture<'_, ConversationOperationWaitResult> {
        self.record("wait", request, self.wait_result.clone())
    }
    fn reconcile(
        &self,
        request: ConversationOperationReconcileRequest,
    ) -> BackendFuture<'_, ConversationOperationSnapshot> {
        self.record("reconcile", request, self.snapshot.clone())
    }
}

impl RecordingBackend {
    fn record<Request: Serialize + Send + 'static, T: Send + 'static>(
        &self,
        name: &'static str,
        request: Request,
        result: T,
    ) -> BackendFuture<'_, T> {
        let calls = Arc::clone(&self.calls);
        let failure = Arc::clone(&self.failure);
        Box::pin(async move {
            calls.lock().await.push((name, json!(request)));
            match failure.lock().await.take() {
                Some(failure) => Err(failure),
                None => Ok(result),
            }
        })
    }
}

fn endpoint() -> Value {
    json!({"serviceId":SERVICE,"endpointId":"claude-code"})
}
fn cursor_endpoint() -> Value {
    json!({"serviceId":SERVICE,"endpointId":"cursor"})
}
fn session(id: &str) -> Value {
    json!({"endpoint":endpoint(),"sessionId":id})
}
fn actor(id: &str) -> Value {
    json!({"endpoint":{"serviceId":SERVICE,"endpointId":"codex-local"},"sessionId":id})
}
fn generation() -> Value {
    json!({"serviceEpoch":EPOCH,"generation":3})
}
fn operation_id() -> &'static str {
    "019f0000-0000-7000-8000-000000000011"
}
fn binding(
    endpoint: Value,
    provider: &str,
    runtime_name: &str,
) -> TestResult<ProviderBindingIdentity> {
    Ok(serde_json::from_value(json!({
    "endpoint":endpoint,"bindingId":format!("{runtime_name}-binding-3"),
    "runtime":{"provider":provider,"runtimeName":runtime_name,"runtimeVersion":"0.14.2"},
    "transport":"stdioAcp","generation":generation(),
    "capabilities":[{"name":"prompt","status":"supported","evidence":"observed"},{"name":"callerDetach","status":"supported","evidence":"routerQualified"}]
    }))?)
}
fn snapshot(binding: ProviderBindingIdentity) -> TestResult<ConversationOperationSnapshot> {
    Ok(serde_json::from_value(json!({
    "operationId":operation_id(),"operation":"conversationPrompt","binding":binding,"target":session("provider-conversation"),
    "stage":"terminal","effect":"applied","reconciliation":"confirmed","admittedAt":"2026-09-20T12:00:00Z","terminalAt":"2026-09-20T12:00:01Z"
    }))?)
}

async fn call(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    id: &str,
    method: &str,
    params: Value,
) -> TestResult<Value> {
    writer
        .write_all(
            format!(
                "{}\n",
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
            )
            .as_bytes(),
        )
        .await?;
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    Ok(serde_json::from_str(&line)?)
}

async fn initialized_fixture() -> TestResult<(
    tokio::net::unix::OwnedWriteHalf,
    BufReader<tokio::net::unix::OwnedReadHalf>,
    RecordingBackend,
)> {
    let claude_binding = binding(endpoint(), "claudeCode", "claude-agent-acp")?;
    let cursor_binding = binding(cursor_endpoint(), "cursor", "cursor-agent-acp")?;
    let snapshot = snapshot(claude_binding.clone())?;
    let submission =
        serde_json::from_value(json!({"admission":"admitted","operation":snapshot.clone()}))?;
    let wait_result =
        serde_json::from_value(json!({"operation":snapshot.clone(),"output":{"kind":"pending"}}))?;
    let backend = RecordingBackend {
        bindings: vec![claude_binding, cursor_binding],
        calls: Arc::new(Mutex::new(Vec::new())),
        failure: Arc::new(Mutex::new(None)),
        existing_lookup: Arc::new(Mutex::new(false)),
        submission,
        snapshot,
        wait_result,
    };
    let identity = ServiceIdentity::new(SERVICE, EPOCH, &format!("sha256:{}", "a".repeat(64)))?
        .with_provider_conversation_backend(Arc::new(backend.clone()));
    let (client, server) = tokio::net::UnixStream::pair()?;
    tokio::spawn(serve_control_connection(server, identity));
    let (read, mut write) = client.into_split();
    let mut reader = BufReader::new(read);
    let response = call(&mut write, &mut reader, "init", "control/initialize", json!({"version":{"major":1,"minor":0},"client":{"name":"provider-dispatch-test","version":"1"}})).await?;
    ensure(
        response.get("result").is_some(),
        format!("initialize: {response}"),
    )?;
    Ok((write, reader, backend))
}

#[tokio::test]
async fn initialized_control_forwards_all_provider_conversation_methods() -> TestResult {
    let (mut writer, mut reader, backend) = initialized_fixture().await?;
    let mutation = json!({"operationId":operation_id(),"generation":generation(),"requestedBy":actor("caller"),"approver":actor("approver")});
    let requests = [
        (
            "conversation/create",
            json!({"operationId":operation_id(),"endpoint":endpoint(),"generation":generation(),"workingDirectory":"/tmp/provider-work","createdBy":actor("caller"),"approver":actor("approver"),"requestedPolicy":{"access":"workspace-write"}}),
        ),
        (
            "conversation/load",
            json!({"operationId":operation_id(),"target":session("provider-conversation"),"generation":generation(),"workingDirectory":"/tmp/provider-work","requestedBy":actor("caller"),"approver":actor("approver"),"requestedPolicy":{"access":"workspace-write"}}),
        ),
        ("conversation/prompt", {
            let mut value = mutation.clone();
            value["target"] = session("provider-conversation");
            value["prompt"] = json!({"kind":"humanUser","text":"hello"});
            value
        }),
        ("conversation/cancel", {
            let mut value = mutation;
            value["targetOperationId"] = json!(operation_id());
            value["target"] = session("provider-conversation");
            value
        }),
        (
            "conversation/operationShow",
            json!({"operationId":operation_id()}),
        ),
        (
            "conversation/operationWait",
            json!({"operationId":operation_id(),"timeoutSeconds":1}),
        ),
        (
            "conversation/operationReconcile",
            json!({"operationId":operation_id()}),
        ),
    ];
    for (index, (method, params)) in requests.into_iter().enumerate() {
        let response = call(
            &mut writer,
            &mut reader,
            &format!("call-{index}"),
            method,
            params,
        )
        .await?;
        ensure(
            response.get("result").is_some(),
            format!("{method}: {response}"),
        )?;
    }
    let calls = backend.calls.lock().await;
    ensure(
        calls.iter().map(|(name, _)| *name).collect::<Vec<_>>()
            == [
                "create",
                "load",
                "prompt",
                "cancel",
                "show",
                "wait",
                "reconcile",
            ],
        "unexpected forwarded method sequence".to_owned(),
    )?;
    ensure(
        calls
            .iter()
            .all(|(_, request)| request["operationId"] == operation_id()),
        "operation identity changed during forwarding".to_owned(),
    )?;
    Ok(())
}

#[tokio::test]
async fn dispatch_resolves_the_requested_dynamic_provider_binding() -> TestResult {
    let (mut writer, mut reader, backend) = initialized_fixture().await?;
    let response = call(&mut writer, &mut reader, "cursor-create", "conversation/create", json!({
        "operationId":operation_id(),"endpoint":cursor_endpoint(),"generation":generation(),
        "workingDirectory":"/tmp/provider-work","createdBy":actor("caller"),"approver":actor("approver"),
        "requestedPolicy":{"access":"workspace-write"}
    })).await?;
    ensure(
        response.get("result").is_some(),
        format!("cursor create: {response}"),
    )?;
    let calls = backend.calls.lock().await;
    ensure(
        calls.len() == 1 && calls[0].1["endpoint"] == cursor_endpoint(),
        "cursor binding was not selected".to_owned(),
    )?;
    Ok(())
}

#[tokio::test]
async fn existing_operation_id_returns_existing_before_stale_generation_validation() -> TestResult {
    let (mut writer, mut reader, backend) = initialized_fixture().await?;
    *backend.existing_lookup.lock().await = true;
    let response = call(
        &mut writer,
        &mut reader,
        "stale-duplicate",
        "conversation/prompt",
        json!({
            "operationId":operation_id(),"target":session("provider-conversation"),
            "generation":{"serviceEpoch":EPOCH,"generation":999},
            "requestedBy":actor("caller"),"approver":actor("approver"),
            "prompt":{"kind":"humanUser","text":"must not dispatch"}
        }),
    )
    .await?;
    ensure(
        response["result"]["admission"] == "existing",
        format!("existing duplicate response: {response}"),
    )?;
    ensure(
        backend.calls.lock().await.is_empty(),
        "existing stale-generation operation reached mutation backend".to_owned(),
    )?;
    Ok(())
}

#[tokio::test]
async fn backend_failure_is_returned_as_the_structured_control_error() -> TestResult {
    let (mut writer, mut reader, backend) = initialized_fixture().await?;
    let failure: ConversationOperationFailure = serde_json::from_value(json!({
        "kind":"outcomeUnknown","stage":"settlement","effect":"unknown",
        "message":"provider response was lost","operationId":operation_id(),"target":session("provider-conversation")
    }))?;
    *backend.failure.lock().await = Some(failure.clone());
    let response = call(&mut writer, &mut reader, "failure", "conversation/prompt", json!({
        "operationId":operation_id(),"target":session("provider-conversation"),"generation":generation(),
        "requestedBy":actor("caller"),"approver":actor("approver"),"prompt":{"kind":"humanUser","text":"hello"}
    })).await?;
    ensure(
        response["error"]["code"] == -32050,
        format!("wrong error code: {response}"),
    )?;
    ensure(
        response["error"]["data"] == serde_json::to_value(failure)?,
        format!("failure data changed: {response}"),
    )?;
    Ok(())
}

#[tokio::test]
async fn malformed_and_foreign_identity_requests_never_reach_backend() -> TestResult {
    let (mut writer, mut reader, backend) = initialized_fixture().await?;
    let malformed = call(
        &mut writer,
        &mut reader,
        "bad",
        "conversation/create",
        json!({}),
    )
    .await?;
    ensure(
        malformed["error"]["code"] == -32602,
        format!("malformed request: {malformed}"),
    )?;
    let stale = call(
        &mut writer,
        &mut reader,
        "stale",
        "conversation/prompt",
        json!({
            "operationId":operation_id(),"target":session("provider-conversation"),
            "generation":{"serviceEpoch":EPOCH,"generation":4},"requestedBy":actor("caller"),
            "approver":actor("approver"),"prompt":{"kind":"humanUser","text":"hello"}
        }),
    )
    .await?;
    ensure(
        stale["error"]["data"]["kind"] == "staleGeneration",
        format!("stale generation: {stale}"),
    )?;
    let foreign = call(&mut writer, &mut reader, "foreign", "conversation/prompt", json!({
        "operationId":operation_id(),"target":{"endpoint":{"serviceId":"019f0000-0000-7000-8000-000000000099","endpointId":"cursor"},"sessionId":"provider-conversation"},
        "generation":generation(),"requestedBy":actor("caller"),"approver":actor("approver"),"prompt":{"kind":"humanUser","text":"hello"}
    })).await?;
    ensure(
        foreign["error"]["data"]["kind"] == "invalidRequest",
        format!("foreign kind: {foreign}"),
    )?;
    ensure(
        foreign["error"]["data"]["effect"] == "none",
        format!("foreign effect: {foreign}"),
    )?;
    ensure(
        backend.calls.lock().await.is_empty(),
        "foreign request reached backend".to_owned(),
    )?;
    Ok(())
}

fn ensure(condition: bool, message: String) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
