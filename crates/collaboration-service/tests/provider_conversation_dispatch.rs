use collaboration_protocol::{
    ConversationCancelRequest, ConversationCreateRequest, ConversationLoadRequest,
    ConversationOperationFailure, ConversationOperationReconcileRequest,
    ConversationOperationShowRequest, ConversationOperationSnapshot,
    ConversationOperationSubmission, ConversationOperationWaitRequest,
    ConversationOperationWaitResult, ConversationPromptRequest, EndpointDescription, EndpointRef,
    ProviderBindingIdentity, ProviderSessionInspectRequest, ProviderSessionInspectResult,
    ProviderSettingsAcceptRequest, ProviderSettingsResult, ProviderSettingsSetRequest,
};
use collaboration_service::collaboration_application::CollaborationRejection;
use collaboration_service::{
    CollaborationApplication, ProviderConversationBackend, ProviderSessionInspectFuture,
    ProviderSettingsFuture, ServiceIdentity,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::sync::Mutex;
#[path = "support/served_api.rs"]
mod served_api;

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
    fn inspect_session(
        &self,
        request: ProviderSessionInspectRequest,
    ) -> ProviderSessionInspectFuture<'_> {
        let calls = Arc::clone(&self.calls);
        Box::pin(async move {
            let target = request.target.clone();
            calls.lock().await.push(("providerInspect", json!(request)));
            Ok(ProviderSessionInspectResult {
                target,
                state: collaboration_protocol::ProviderSessionState::Unloaded,
                capabilities: session_event_model::CapabilityReport::default(),
                history: collaboration_protocol::ProviderHistoryAvailability::HistoryUnavailable,
                settings_catalog: None,
            })
        })
    }

    fn settings_set(&self, request: ProviderSettingsSetRequest) -> ProviderSettingsFuture<'_> {
        let calls = Arc::clone(&self.calls);
        Box::pin(async move {
            let target = request.target.clone();
            calls.lock().await.push(("settingsSet", json!(request)));
            Ok(settings_result(target))
        })
    }

    fn settings_accept(
        &self,
        request: ProviderSettingsAcceptRequest,
    ) -> ProviderSettingsFuture<'_> {
        let calls = Arc::clone(&self.calls);
        Box::pin(async move {
            let target = request.target.clone();
            calls.lock().await.push(("settingsAccept", json!(request)));
            Ok(settings_result(target))
        })
    }

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
    fn resume(
        &self,
        request: collaboration_protocol::ConversationResumeRequest,
    ) -> BackendFuture<'_, ConversationOperationSubmission> {
        self.record("resume", request, self.submission.clone())
    }
    fn close(
        &self,
        request: collaboration_protocol::ConversationCloseRequest,
    ) -> BackendFuture<'_, ConversationOperationSubmission> {
        self.record("close", request, self.submission.clone())
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

fn settings_result(target: collaboration_protocol::SessionRef) -> ProviderSettingsResult {
    ProviderSettingsResult {
        target,
        effective_settings: collaboration_protocol::EffectiveProviderSettings {
            requested_policy: collaboration_protocol::ProviderRequestedPolicy {
                access: collaboration_protocol::RouterAccess::WorkspaceWrite,
            },
            mapping_status: collaboration_protocol::ProviderSettingsMappingStatus::Verified,
            authentication: collaboration_protocol::ProviderAuthenticationState::Authenticated,
            provider_permission_mode: None,
            permission_outcome: None,
            mode: Some("ask".into()),
            model: None,
            effort: None,
        },
    }
}

#[tokio::test]
async fn settings_methods_forward_typed_actor_without_operation_id() -> TestResult {
    let (fixture, backend) = initialized_fixture().await?;
    let set = json!({"target":session("provider-conversation"),"actor":actor("creator"),"setting":"mode","value":"ask"});
    let set_response = call(&fixture, "conversation/settingsSet", set.clone()).await?;
    ensure(
        set_response["result"]["effectiveSettings"]["mode"] == "ask",
        format!("set: {set_response}"),
    )?;
    let accept = json!({"target":session("provider-conversation"),"actor":actor("approver")});
    let accept_response = call(&fixture, "conversation/settingsAccept", accept.clone()).await?;
    ensure(
        accept_response["result"]["target"] == session("provider-conversation"),
        format!("accept: {accept_response}"),
    )?;
    let invalid = call(&fixture, "conversation/settingsSet", json!({
        "target":session("provider-conversation"),"actor":actor("creator"),"setting":"mode","value":"ask",
        "operationId":operation_id()
    })).await?;
    ensure(
        arguments_refused(&invalid),
        format!("operation ID admitted: {invalid}"),
    )?;
    ensure(
        *backend.calls.lock().await == vec![("settingsSet", set), ("settingsAccept", accept)],
        "settings calls changed".into(),
    )?;
    let inspected = call(
        &fixture,
        "provider/sessionInspect",
        json!({"target":session("provider-conversation")}),
    )
    .await?;
    ensure(
        inspected["result"]["capabilities"]["authStatus"]["kind"] == "notReported",
        format!("inspect: {inspected}"),
    )?;
    Ok(())
}

#[tokio::test]
async fn resume_and_close_dispatch_as_inspectable_provider_operations() -> TestResult {
    let (fixture, backend) = initialized_fixture().await?;
    let resume = json!({
        "operationId":operation_id(),
        "target":session("provider-conversation"),
        "generation":generation(),
        "workingDirectory":"/tmp",
        "requestedBy":actor("creator"),
        "approver":actor("creator"),
        "requestedPolicy":{"access":"workspace-write"}
    });
    let response = call(&fixture, "conversation/resume", resume.clone()).await?;
    ensure(
        response["result"]["admission"] == "admitted",
        format!("resume: {response}"),
    )?;
    let close = json!({
        "operationId":operation_id(),
        "target":session("provider-conversation"),
        "generation":generation(),
        "requestedBy":actor("creator"),
        "approver":actor("creator")
    });
    let response = call(&fixture, "conversation/close", close.clone()).await?;
    ensure(
        response["result"]["admission"] == "admitted",
        format!("close: {response}"),
    )?;
    ensure(
        *backend.calls.lock().await == vec![("resume", resume), ("close", close)],
        "lifecycle requests changed at Control dispatch".into(),
    )?;
    Ok(())
}

#[tokio::test]
async fn human_creator_and_approver_survive_create_and_load_control_dispatch() -> TestResult {
    let (fixture, backend) = initialized_fixture().await?;
    let human = json!({"humanId":"owner"});
    let create = json!({
        "operationId":operation_id(),
        "endpoint":endpoint(),
        "generation":generation(),
        "workingDirectory":"/tmp/provider-work",
        "createdBy":human,
        "approver":human,
        "requestedPolicy":{"access":"workspace-write"}
    });
    let created = call(&fixture, "conversation/create", create.clone()).await?;
    ensure(
        created["result"]["admission"] == "admitted",
        format!("create: {created}"),
    )?;
    let load = json!({
        "operationId":operation_id(),
        "target":session("provider-conversation"),
        "generation":generation(),
        "workingDirectory":"/tmp/provider-work",
        "requestedBy":human,
        "approver":human,
        "requestedPolicy":{"access":"workspace-write"}
    });
    let loaded = call(&fixture, "conversation/load", load.clone()).await?;
    ensure(
        loaded["result"]["admission"] == "admitted",
        format!("load: {loaded}"),
    )?;
    ensure(
        *backend.calls.lock().await == vec![("create", create), ("load", load)],
        "human actor JSON changed at Control dispatch".into(),
    )?;
    Ok(())
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
    "operationId":operation_id(),"operation":"conversationPrompt","binding":{"kind":"externalProvider","binding":binding},"target":session("provider-conversation"),
    "stage":"terminal","effect":"applied","reconciliation":"confirmed","admittedAt":"2026-09-20T12:00:00Z","terminalAt":"2026-09-20T12:00:01Z"
    }))?)
}

/// The served API and the application it serves. Provider create, load, prompt and cancel
/// submissions have no tool of their own (the API runs them as composite tools that also wait),
/// so the submission itself is called on the application the tools run on; every other method
/// is one tool call.
struct Fixture {
    served: served_api::ServedApi,
    application: CollaborationApplication,
}

/// Answers `{"result": ..}` or `{"error": {"code", "message", "data"}}` as the API publishes it.
async fn call(fixture: &Fixture, method: &str, params: Value) -> TestResult<Value> {
    let conversations = fixture.application.conversations();
    macro_rules! submitted {
        ($operation:ident) => {
            match serde_json::from_value(params) {
                Ok(request) => published(conversations.$operation(request).await),
                Err(error) => json!({"error":{"code":-32602,"message":error.to_string()}}),
            }
        };
    }
    let tool = match method {
        "conversation/create" => return Ok(submitted!(conversation_create)),
        "conversation/load" => return Ok(submitted!(conversation_load)),
        "conversation/prompt" => return Ok(submitted!(conversation_prompt)),
        "conversation/cancel" => return Ok(submitted!(conversation_cancel)),
        "conversation/settingsSet" => "conversation_settings_set",
        "conversation/settingsAccept" => "conversation_settings_accept",
        "conversation/resume" => "conversation_resume",
        "conversation/close" => "conversation_close",
        "conversation/operationShow" => "conversation_operation_show",
        "conversation/operationWait" => "conversation_operation_wait",
        "conversation/operationReconcile" => "conversation_operation_reconcile",
        "provider/sessionInspect" => "provider_session_inspect",
        other => return Err(format!("no tool for {other}").into()),
    };
    Ok(fixture.served.call(tool, params).await?)
}

fn published<TResult: Serialize>(result: Result<TResult, impl CollaborationRejection>) -> Value {
    match result {
        Ok(result) => json!({ "result": result }),
        Err(failure) => json!({ "error": failure.published_rejection() }),
    }
}

async fn initialized_fixture() -> TestResult<(Fixture, RecordingBackend)> {
    initialized_fixture_with_endpoint(None).await
}

async fn initialized_fixture_with_endpoint(
    endpoint_description: Option<EndpointDescription>,
) -> TestResult<(Fixture, RecordingBackend)> {
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
    let identity = ServiceIdentity::new(SERVICE, EPOCH)?
        .with_provider_conversation_backend(Arc::new(backend.clone()));
    let identity = match endpoint_description {
        Some(description) => identity.with_endpoints(vec![description])?,
        None => identity,
    };
    Ok((fixture(identity).await?, backend))
}

async fn fixture(identity: ServiceIdentity) -> TestResult<Fixture> {
    Ok(Fixture {
        application: CollaborationApplication::new(identity.clone()),
        served: served_api::ServedApi::start(identity).await?,
    })
}

#[tokio::test]
async fn initialized_control_forwards_all_provider_conversation_methods() -> TestResult {
    let (fixture, backend) = initialized_fixture().await?;
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
    for (method, params) in requests {
        let response = call(&fixture, method, params).await?;
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
async fn omitted_generation_uses_current_binding_for_each_mutation() -> TestResult {
    let (fixture, backend) = initialized_fixture().await?;
    for (method, params) in [
        (
            "conversation/create",
            json!({"operationId":operation_id(),"endpoint":endpoint(),"workingDirectory":"/tmp/provider-work","createdBy":actor("caller"),"approver":actor("approver"),"requestedPolicy":{"access":"workspace-write"}}),
        ),
        (
            "conversation/load",
            json!({"operationId":operation_id(),"target":session("provider-conversation"),"workingDirectory":"/tmp/provider-work","requestedBy":actor("caller"),"approver":actor("approver"),"requestedPolicy":{"access":"workspace-write"}}),
        ),
        (
            "conversation/prompt",
            json!({"operationId":operation_id(),"target":session("provider-conversation"),"requestedBy":actor("caller"),"approver":actor("approver"),"prompt":{"kind":"humanUser","text":"hello"}}),
        ),
        (
            "conversation/cancel",
            json!({"operationId":operation_id(),"targetOperationId":operation_id(),"target":session("provider-conversation"),"requestedBy":actor("caller"),"approver":actor("approver")}),
        ),
    ] {
        let response = call(&fixture, method, params).await?;
        ensure(
            response.get("result").is_some(),
            format!("{method}: {response}"),
        )?;
    }
    let calls = backend.calls.lock().await;
    ensure(
        calls.len() == 4,
        format!("forwarded {} requests", calls.len()),
    )?;
    ensure(
        calls
            .iter()
            .all(|(_, request)| request["generation"] == generation()),
        "omitted generation did not resolve to the selected binding".to_owned(),
    )?;
    Ok(())
}

#[tokio::test]
async fn dispatch_resolves_the_requested_dynamic_provider_binding() -> TestResult {
    let (fixture, backend) = initialized_fixture().await?;
    let response = call(&fixture, "conversation/create", json!({
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
    let (fixture, backend) = initialized_fixture().await?;
    *backend.existing_lookup.lock().await = true;
    let response = call(
        &fixture,
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
    let (fixture, backend) = initialized_fixture().await?;
    let failure: ConversationOperationFailure = serde_json::from_value(json!({
        "kind":"outcomeUnknown","stage":"settlement","effect":"unknown",
        "message":"provider response was lost","operationId":operation_id(),"target":session("provider-conversation")
    }))?;
    *backend.failure.lock().await = Some(failure.clone());
    let response = call(&fixture, "conversation/prompt", json!({
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
    let (fixture, backend) = initialized_fixture().await?;
    // Malformed input is refused by the API's own decoder before any operation runs.
    let malformed = fixture
        .served
        .call("conversation_create", json!({}))
        .await?;
    ensure(
        arguments_refused(&malformed),
        format!("malformed request: {malformed}"),
    )?;
    let stale = call(
        &fixture,
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
    let foreign = call(&fixture, "conversation/prompt", json!({
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

#[tokio::test]
async fn unavailable_provider_conversation_names_endpoint_and_catalog_recovery() -> TestResult {
    let availability = json!({
        "state":"unavailable",
        "observedAt":"2026-09-24T00:00:00Z",
        "reason":"provider executable is missing",
        "fix":"install the provider binary and restart the Host"
    });
    let endpoint_description = serde_json::from_value(json!({
        "endpoint":endpoint(),
        "label":"Fixture Claude",
        "availability":availability,
        "channels":[]
    }))?;
    let identity =
        ServiceIdentity::new(SERVICE, EPOCH)?.with_endpoints(vec![endpoint_description])?;
    let fixture = fixture(identity).await?;
    for (method, params) in [
        (
            "conversation/create",
            json!({
                "operationId":operation_id(),"endpoint":endpoint(),"workingDirectory":"/tmp/provider-work",
                "createdBy":actor("caller"),"approver":actor("approver"),
                "requestedPolicy":{"access":"workspace-write"}
            }),
        ),
        (
            "conversation/load",
            json!({
                "operationId":operation_id(),"target":session("provider-conversation"),
                "workingDirectory":"/tmp/provider-work","requestedBy":actor("caller"),
                "approver":actor("approver"),"requestedPolicy":{"access":"workspace-write"}
            }),
        ),
        (
            "conversation/prompt",
            json!({
                "operationId":operation_id(),"target":session("provider-conversation"),
                "requestedBy":actor("caller"),"approver":actor("approver"),
                "prompt":{"kind":"humanUser","text":"hello"}
            }),
        ),
        (
            "conversation/cancel",
            json!({
                "operationId":operation_id(),"targetOperationId":operation_id(),
                "target":session("provider-conversation"),"requestedBy":actor("caller"),
                "approver":actor("approver")
            }),
        ),
    ] {
        let response = call(&fixture, method, params).await?;
        let data = &response["error"]["data"];
        ensure(
            data["kind"] == "unavailable",
            format!("{method}: {response}"),
        )?;
        ensure(
            data["endpoint"] == endpoint(),
            format!("{method}: {response}"),
        )?;
        ensure(
            data["availability"] == availability,
            format!("{method}: {response}"),
        )?;
        ensure(
            data["message"]
                .as_str()
                .is_some_and(|text| text.contains("claude-code")),
            format!("{method}: {response}"),
        )?;
    }
    fixture.served.stop().await?;
    Ok(())
}

#[tokio::test]
async fn unavailable_catalog_vetoes_a_still_present_provider_binding() -> TestResult {
    let availability = json!({
        "state":"unavailable", "observedAt":"2026-09-24T00:00:00Z",
        "reason":"provider process retired", "fix":"restart the Host to relaunch the provider"
    });
    let description: EndpointDescription = serde_json::from_value(json!({
        "endpoint":endpoint(),"label":"Fixture Claude","availability":availability,"channels":[]
    }))?;
    let (fixture, backend) = initialized_fixture_with_endpoint(Some(description)).await?;
    let response = call(
        &fixture,
        "conversation/prompt",
        json!({
            "operationId":operation_id(),"target":session("provider-conversation"),
            "requestedBy":actor("caller"),"approver":actor("approver"),
            "prompt":{"kind":"humanUser","text":"must not dispatch"}
        }),
    )
    .await?;
    ensure(
        response["error"]["data"]["kind"] == "unavailable",
        format!("{response}"),
    )?;
    ensure(
        response["error"]["data"]["endpoint"] == endpoint(),
        format!("{response}"),
    )?;
    ensure(
        response["error"]["data"]["availability"] == availability,
        format!("{response}"),
    )?;
    ensure(
        backend.calls.lock().await.is_empty(),
        "unavailable binding reached provider I/O".to_owned(),
    )?;
    Ok(())
}

/// The API refused the call's arguments before any operation ran. The Control decoder answered
/// `-32602`; the API reports arguments that do not decode as a tool error with no typed payload,
/// where every operation failure carries one.
fn arguments_refused(response: &Value) -> bool {
    response.get("error").is_some_and(Value::is_object)
        && response.pointer("/error/data").is_none_or(Value::is_null)
}

fn ensure(condition: bool, message: String) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
