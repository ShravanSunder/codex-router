#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
//! Compiled-CLI conversations against the real collaboration API, whose in-Host composition
//! drives a scripted provider backend; fixture assertions fail fast at that boundary.

use collaboration_mcp::test_support::ServedCollaborationApi;
use collaboration_service::{CollaborationApplication, ServiceIdentity};
use serde_json::{Value, json};
use std::{os::unix::fs::DirBuilderExt, sync::Arc};

mod fake_api_support;
#[path = "provider_conversation_cli/scripted_provider_backend.rs"]
mod scripted_provider_backend;
use fake_api_support::{FakeCollaborationApi, FakeReply};
use scripted_provider_backend::{ScriptedAnswer, ScriptedProviderBackend, ScriptedStep};

const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
const SERVICE_EPOCH: &str = "00000000-0000-4000-8000-000000000002";
const ENDPOINT_ID: &str = "claude-fixture";
const CREATE_OPERATION: &str = "019c6e27-e55b-73d1-87d8-4e01f1f75101";
const PROMPT_OPERATION: &str = "019c6e27-e55b-73d1-87d8-4e01f1f75102";
const WRONG_GENERATION_OPERATION: &str = "019c6e27-e55b-73d1-87d8-4e01f1f75103";
const LOST_RESPONSE_OPERATION: &str = "019c6e27-e55b-73d1-87d8-4e01f1f75104";
const LOAD_OPERATION: &str = "019c6e27-e55b-73d1-87d8-4e01f1f75105";
const CANCEL_OPERATION: &str = "019c6e27-e55b-73d1-87d8-4e01f1f75106";

#[path = "provider_conversation_cli/common_operation_tests.rs"]
mod common_operation_tests;

#[tokio::test]
async fn codex_create_missing_model_and_effort_fails_before_start_record() {
    let root = fixture_directory("codex-create-missing-inputs");
    let output = run_cli(
        &root,
        vec![
            "conversation",
            "create",
            "--endpoint",
            "codex-local",
            "--cwd",
            "/tmp/project",
            "--access",
            "workspace-write",
            "--json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    )
    .await;
    assert_eq!(output.status.code(), Some(2));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let rendered = format!("{stdout}{stderr}");
    assert!(
        !rendered.contains("conversationCreateStarted"),
        "{rendered}"
    );
    assert!(rendered.contains("--model"), "{rendered}");
    assert!(rendered.contains("--effort"), "{rendered}");
    assert!(
        rendered.contains("'--endpoint' 'codex-local'"),
        "{rendered}"
    );
    assert!(rendered.contains("'--cwd' '/tmp/project'"), "{rendered}");
    assert!(rendered.contains("<model>"), "{rendered}");
    assert!(rendered.contains("<low|medium|high>"), "{rendered}");
    cleanup_unpublished_fixture(&root);
}

#[tokio::test]
async fn provider_create_accepts_mode_model_and_effort_past_local_preflight() {
    let root = fixture_directory("provider-create-codex-inputs");
    let output = run_cli(
        &root,
        vec![
            "conversation",
            "create",
            "--endpoint",
            "claude-local",
            "--cwd",
            "/tmp/project",
            "--access",
            "workspace-write",
            "--model",
            "gpt-5.6",
            "--mode",
            "ask",
            "--effort",
            "medium",
            "--json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    )
    .await;
    assert_eq!(output.status.code(), Some(3));
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !rendered.contains("conversationCreateStarted"),
        "{rendered}"
    );
    assert!(rendered.contains("manifest-read"), "{rendered}");
    assert!(!rendered.contains("unsupportedCapability"), "{rendered}");
    assert!(!rendered.contains("omit these fields"), "{rendered}");
    cleanup_unpublished_fixture(&root);
}

#[tokio::test]
async fn external_provider_create_then_prompt_preserves_target_and_supplied_operations() {
    let fixture = ProviderFixture::start(
        "create-prompt",
        [
            create_steps(|request| {
                assert_eq!(request["operationId"], CREATE_OPERATION);
                assert_eq!(request["generation"]["generation"], 7);
            }),
            prompt_steps(|request| {
                assert_eq!(request["operationId"], PROMPT_OPERATION);
                assert_eq!(request["target"], target());
            }),
        ]
        .into_iter()
        .flatten()
        .collect(),
    )
    .await;
    let root = fixture.root().to_owned();

    let create = run_cli(&root, create_arguments(CREATE_OPERATION)).await;
    assert_eq!(
        create.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&create.stderr)
    );
    let create_json = common_result(&create.stdout);
    assert_eq!(create_json["kind"], "created");
    assert_eq!(create_json["operationId"], CREATE_OPERATION);
    assert_eq!(create_json["target"], target());

    let prompt = run_cli(&root, prompt_arguments(PROMPT_OPERATION)).await;
    assert_eq!(
        prompt.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&prompt.stdout),
        String::from_utf8_lossy(&prompt.stderr)
    );
    let prompt_json = common_result(&prompt.stdout);
    assert_eq!(prompt_json["kind"], "completed");
    assert_eq!(prompt_json["operationId"], PROMPT_OPERATION);
    assert_eq!(prompt_json["target"], target());
    assert_eq!(
        prompt_json["settlement"]["detail"]["kind"],
        "providerPrompt"
    );

    fixture.finish().await;
}

#[tokio::test]
async fn wrong_generation_is_no_effect_response_loss_retains_id_and_show_is_read_only() {
    // The CLI read generation 7, but the binding has moved on: the Router refuses the
    // prompt before the provider sees it.
    let wrong_fixture = ProviderFixture::start_with("wrong-generation", Vec::new(), 8).await;
    let wrong_root = wrong_fixture.root().to_owned();
    let wrong = run_cli(&wrong_root, prompt_arguments(WRONG_GENERATION_OPERATION)).await;
    assert_eq!(
        wrong.status.code(),
        Some(4),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&wrong.stdout),
        String::from_utf8_lossy(&wrong.stderr)
    );
    let wrong_json = common_result(&wrong.stdout);
    assert_eq!(wrong_json["error"]["kind"], "staleGeneration");
    assert_eq!(wrong_json["error"]["effect"], "none");
    assert_eq!(
        wrong_json["error"]["operationId"],
        WRONG_GENERATION_OPERATION
    );
    wrong_fixture.finish().await;

    // The provider never answers the prompt, so the Router's one-second wait ends pending.
    let lost_fixture = ProviderFixture::start(
        "lost-response",
        vec![ScriptedStep::new(
            "prompt",
            |request| assert_eq!(request["operationId"], LOST_RESPONSE_OPERATION),
            ScriptedAnswer::Never,
        )],
    )
    .await;
    let lost_root = lost_fixture.root().to_owned();
    let lost = run_cli(&lost_root, prompt_arguments(LOST_RESPONSE_OPERATION)).await;
    assert_eq!(lost.status.code(), Some(0));
    let lost_json = common_result(&lost.stdout);
    assert_eq!(lost_json["kind"], "pending");
    assert_eq!(lost_json["operationId"], LOST_RESPONSE_OPERATION);
    assert_eq!(lost_json["target"], target());
    lost_fixture.finish().await;

    let show_fixture = ProviderFixture::start(
        "operation-show",
        vec![ScriptedStep::new(
            "show",
            |request| assert_eq!(request["operationId"], CREATE_OPERATION),
            ScriptedAnswer::Answer(operation_snapshot(
                CREATE_OPERATION,
                "conversationCreate",
                Some(target()),
                "terminal",
                "applied",
            )),
        )],
    )
    .await;
    let show_root = show_fixture.root().to_owned();
    let show = run_cli(
        &show_root,
        vec![
            "conversation",
            "operation",
            "show",
            "--operation-id",
            CREATE_OPERATION,
            "--json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    )
    .await;
    assert_eq!(show.status.code(), Some(0));
    let show_json: Value = serde_json::from_slice(&show.stdout).expect("show JSON");
    assert_eq!(
        show_json["result"]["record"]["operationId"],
        CREATE_OPERATION
    );
    assert_eq!(show_json["result"]["record"]["effect"], "applied");
    show_fixture.finish().await;

    // The API connection drops after the read was sent.
    for (tool, command) in [
        ("conversation_operation_show", "show"),
        ("conversation_operation_wait", "wait"),
        ("conversation_operation_reconcile", "reconcile"),
    ] {
        let mut read_fixture =
            FakeCollaborationApi::new(SERVICE_ID, SERVICE_EPOCH).expect("stand-in API");
        let read_calls = read_fixture.serve(vec![FakeReply::Disconnect]);
        let read_root = read_fixture.directory().to_owned();
        let mut arguments = vec![
            "conversation".to_owned(),
            "operation".to_owned(),
            command.to_owned(),
            "--operation-id".to_owned(),
            CREATE_OPERATION.to_owned(),
            "--json".to_owned(),
        ];
        if command == "wait" {
            arguments.extend(["--timeout-seconds".to_owned(), "1".to_owned()]);
        }
        let read = run_cli(&read_root, arguments).await;
        assert_eq!(read.status.code(), Some(3), "{command} exit");
        let read_json: Value = serde_json::from_slice(&read.stdout).expect("read failure JSON");
        assert_eq!(read_json["error"]["kind"], "unavailable", "{command}");
        assert_eq!(read_json["error"]["effect"], "none", "{command}");
        assert_eq!(
            read_json["error"]["operationId"], CREATE_OPERATION,
            "{command}"
        );
        let calls = read_calls
            .await
            .expect("read failure fixture")
            .expect("read failure fixture");
        assert_eq!(calls[0]["tool"], tool, "{command}");
        assert_eq!(calls[0]["arguments"]["operationId"], CREATE_OPERATION);
    }
}

#[tokio::test]
async fn unavailable_provider_prompt_prints_catalog_reason_and_fix() {
    let availability = json!({
        "state":"unavailable","observedAt":"2026-09-24T00:00:00Z",
        "reason":"provider executable is missing","fix":"install the provider binary"
    });
    let expected_availability = availability.clone();
    // The catalog still read the endpoint as available; the provider's binding refuses the
    // prompt as unavailable, with the catalog's reason and fix.
    let fixture = ProviderFixture::start(
        "unavailable-provider",
        vec![ScriptedStep::new(
            "prompt",
            |request| assert_eq!(request["operationId"], PROMPT_OPERATION),
            ScriptedAnswer::Failure(json!({
                "kind":"unavailable","stage":"binding","effect":"none",
                "message":"provider conversation endpoint claude-local unavailable: provider executable is missing",
                "operationId":PROMPT_OPERATION,"target":target(),
                "endpoint":endpoint(),"availability":availability
            })),
        )],
    )
    .await;
    let root = fixture.root().to_owned();
    let output = run_cli(&root, prompt_arguments(PROMPT_OPERATION)).await;
    assert_eq!(
        output.status.code(),
        Some(3),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result = common_result(&output.stdout);
    assert_eq!(result["error"]["kind"], "unavailable");
    assert_eq!(result["error"]["endpoint"], endpoint());
    assert_eq!(result["error"]["availability"], expected_availability);
    assert!(
        result["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("claude-local"))
    );
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires an explicitly selected authenticated Cursor ACP runtime"]
async fn live_cursor_create_wait_prompt_wait_through_compiled_cli() {
    use codex_router_host::{
        CollaborationRuntime, CollaborationRuntimeInputs, ExternalProviderLaunchBinding,
    };
    use collaboration_client::CollaborationClient;
    use collaboration_client::protocol::{
        ChannelDescription, CodexGeneration, EndpointId, EndpointRef, OperationId, SessionId,
        SessionRef,
    };

    let executable = std::env::var_os("CODEX_ROUTER_TEST_EXTERNAL_ACP_EXECUTABLE")
        .map(std::path::PathBuf::from)
        .expect("external ACP executable");
    let arguments = std::env::var("CODEX_ROUTER_TEST_EXTERNAL_ACP_ARGUMENTS")
        .ok()
        .map(|encoded| serde_json::from_str::<Vec<String>>(&encoded).expect("JSON argument array"))
        .unwrap_or_default();
    let cwd = std::env::var_os("CODEX_ROUTER_TEST_EXTERNAL_ACP_CWD")
        .map(std::path::PathBuf::from)
        .expect("external ACP cwd");
    let service_directory = std::env::var_os("CODEX_ROUTER_TEST_EXTERNAL_ACP_SERVICE_DIRECTORY")
        .map(std::path::PathBuf::from)
        .expect("owned service directory");
    std::fs::create_dir_all(&service_directory).expect("service directory");

    let runtime = CollaborationRuntime::start_with_external_providers(
        CollaborationRuntimeInputs {
            directory: service_directory.clone(),
            codex_home: std::env::var_os("CODEX_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| cwd.clone()),
            backend_socket: service_directory.join("unused-backend.sock"),
            mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            native_schema: None,
            peer_registry_directory: None,
            remote_control_server_name: None,
            owner_human_id: None,
        },
        vec![codex_router_host::ExternalProviderStartup::Launch(
            ExternalProviderLaunchBinding::cursor(executable, arguments)
                .expect("Cursor provider binding"),
        )],
    )
    .await
    .expect("collaboration runtime");

    let control = CollaborationClient::connect(&service_directory, "live-cli-setup", "1")
        .await
        .expect("API setup client");
    let inventory = control.list_endpoints().await.expect("endpoint inventory");
    let provider = inventory
        .endpoints
        .iter()
        .find(|record| String::from(record.endpoint.endpoint_id.clone()) == "cursor-local")
        .expect("Cursor endpoint");
    let generation_number = provider
        .channels
        .iter()
        .find_map(|channel| match channel {
            ChannelDescription::ExternalProvider {
                binding_generation, ..
            } => Some(*binding_generation),
            _ => None,
        })
        .expect("Cursor generation");
    let generation = CodexGeneration {
        service_epoch: inventory.service_epoch,
        generation: generation_number,
    };
    let actor = SessionRef {
        endpoint: EndpointRef {
            service_id: provider.endpoint.service_id.clone(),
            endpoint_id: EndpointId::try_from("codex-local".to_owned()).expect("actor endpoint"),
        },
        session_id: SessionId::try_from("live-cli-caller".to_owned()).expect("actor session"),
    };
    drop(control);

    let create_operation = OperationId::generate();
    let create = run_cli(
        &service_directory,
        vec![
            "conversation".to_owned(),
            "provider".to_owned(),
            "create".to_owned(),
            "--operation-id".to_owned(),
            create_operation.as_str().to_owned(),
            "--endpoint".to_owned(),
            serde_json::to_string(&provider.endpoint).expect("endpoint JSON"),
            "--generation".to_owned(),
            serde_json::to_string(&generation).expect("generation JSON"),
            "--created-by".to_owned(),
            serde_json::to_string(&actor).expect("creator JSON"),
            "--approver".to_owned(),
            serde_json::to_string(&actor).expect("approver JSON"),
            "--cwd".to_owned(),
            cwd.display().to_string(),
            "--access".to_owned(),
            "write-restricted".to_owned(),
            "--json".to_owned(),
        ],
    )
    .await;
    assert_eq!(
        create.status.code(),
        Some(0),
        "create stdout={} stderr={}",
        String::from_utf8_lossy(&create.stdout),
        String::from_utf8_lossy(&create.stderr)
    );
    let create_wait = run_wait(&service_directory, create_operation.as_str()).await;
    let create_wait_json: Value =
        serde_json::from_slice(&create_wait.stdout).expect("create wait JSON");
    assert_eq!(create_wait.status.code(), Some(0), "{create_wait_json}");
    let target = create_wait_json
        .pointer("/result/record/output/settlement/target")
        .cloned()
        .expect("created provider target");

    let prompt_operation = OperationId::generate();
    let prompt = run_cli(
        &service_directory,
        vec![
            "conversation".to_owned(),
            "provider".to_owned(),
            "prompt".to_owned(),
            "--operation-id".to_owned(),
            prompt_operation.as_str().to_owned(),
            "--target".to_owned(),
            target.to_string(),
            "--generation".to_owned(),
            serde_json::to_string(&generation).expect("generation JSON"),
            "--requested-by".to_owned(),
            serde_json::to_string(&actor).expect("requester JSON"),
            "--approver".to_owned(),
            serde_json::to_string(&actor).expect("approver JSON"),
            "--text".to_owned(),
            "Reply with exactly PR2_CLI_LIVE_OK and no other text.".to_owned(),
            "--json".to_owned(),
        ],
    )
    .await;
    assert_eq!(
        prompt.status.code(),
        Some(0),
        "prompt stdout={} stderr={}",
        String::from_utf8_lossy(&prompt.stdout),
        String::from_utf8_lossy(&prompt.stderr)
    );
    let prompt_wait = run_wait(&service_directory, prompt_operation.as_str()).await;
    let prompt_wait_json: Value =
        serde_json::from_slice(&prompt_wait.stdout).expect("prompt wait JSON");
    assert_eq!(prompt_wait.status.code(), Some(0), "{prompt_wait_json}");
    assert_eq!(
        prompt_wait_json.pointer("/result/record/operation/target"),
        Some(&target)
    );
    assert_eq!(
        prompt_wait_json.pointer("/result/record/output/settlement/response"),
        Some(&json!("PR2_CLI_LIVE_OK"))
    );
    eprintln!(
        "live Cursor CLI target={} createOperation={} promptOperation={} settlement={}",
        target,
        create_operation.as_str(),
        prompt_operation.as_str(),
        prompt_wait_json
    );

    runtime.shutdown().await.expect("runtime shutdown");
}

async fn run_wait(root: &std::path::Path, operation_id: &str) -> std::process::Output {
    run_cli(
        root,
        vec![
            "conversation",
            "operation",
            "wait",
            "--operation-id",
            operation_id,
            "--timeout-seconds",
            "60",
            "--json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    )
    .await
}

fn fixture_directory(label: &str) -> std::path::PathBuf {
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "pc-{label}-{}",
        collaboration_client::board::ProjectId::generate().as_str()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .expect("fixture directory");
    root
}

/// The real collaboration API on a private directory, with the provider endpoint published
/// and its backend answering from `steps`.
struct ProviderFixture {
    root: std::path::PathBuf,
    served: ServedCollaborationApi,
    backend: ScriptedProviderBackend,
}

impl ProviderFixture {
    async fn start(label: &str, steps: Vec<ScriptedStep>) -> Self {
        Self::start_with(label, steps, 7).await
    }

    /// `binding_generation` is the backend's current binding; the published endpoint always
    /// advertises generation 7, as the CLI reads it.
    async fn start_with(label: &str, steps: Vec<ScriptedStep>, binding_generation: u64) -> Self {
        let root = fixture_directory(label);
        let binding = serde_json::from_value(json!({
            "endpoint":endpoint(),"bindingId":"fixture-binding",
            "runtime":{"provider":"claudeCode","runtimeName":"fixture"},
            "transport":"stdioAcp",
            "generation":{"serviceEpoch":SERVICE_EPOCH,"generation":binding_generation},
            "capabilities":[
                {"name":"create","status":"supported","evidence":"advertised"},
                {"name":"prompt","status":"supported","evidence":"advertised"},
                {"name":"load","status":"supported","evidence":"advertised"},
                {"name":"cancel","status":"supported","evidence":"advertised"}
            ]
        }))
        .expect("provider binding");
        let backend = ScriptedProviderBackend::new(binding, steps);
        let description = serde_json::from_value(json!({
            "endpoint":endpoint(),"label":"Claude fixture",
            "availability":{"state":"available","observedAt":"2026-09-24T00:00:00Z"},
            "channels":[{"kind":"externalProvider","transport":"stdioAcp","bindingId":"fixture-binding","bindingGeneration":7,
                "runtime":{"provider":"claudeCode","runtimeName":"fixture"},
                "capabilities":[{"name":"create","status":"supported","evidence":"advertised"},
                    {"name":"prompt","status":"supported","evidence":"advertised"},
                    {"name":"load","status":"supported","evidence":"advertised"},
                    {"name":"cancel","status":"supported","evidence":"advertised"}]}]
        }))
        .expect("provider endpoint");
        let identity = ServiceIdentity::new(SERVICE_ID, SERVICE_EPOCH)
            .expect("service identity")
            .with_endpoints(vec![description])
            .expect("endpoint inventory")
            .with_provider_conversation_backend(Arc::new(backend.clone()));
        let served = ServedCollaborationApi::start(&root, CollaborationApplication::new(identity))
            .await
            .expect("served collaboration API");
        Self {
            root,
            served,
            backend,
        }
    }

    fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// Stops the API, requires every scripted provider operation to have run, and removes
    /// the directory.
    async fn finish(self) {
        self.served.stop().await.expect("collaboration API stops");
        assert!(
            self.backend.remaining().is_empty(),
            "unperformed provider operations: {:?}",
            self.backend.remaining()
        );
        std::fs::remove_dir(&self.root).expect("directory cleanup");
    }
}

/// A create the provider admits, then settles with the created target.
fn create_steps(check: impl FnOnce(&Value) + Send + 'static) -> Vec<ScriptedStep> {
    vec![
        ScriptedStep::new(
            "create",
            check,
            ScriptedAnswer::Answer(
                json!({"admission":"admitted","operation":operation_snapshot(
                CREATE_OPERATION, "conversationCreate", None, "admitted", "none")}),
            ),
        ),
        ScriptedStep::new(
            "wait",
            |request| assert_eq!(request["operationId"], CREATE_OPERATION),
            ScriptedAnswer::Answer(json!({
                "operation":operation_snapshot(CREATE_OPERATION, "conversationCreate", Some(target()), "terminal", "applied"),
                "output":{"kind":"available","settlement":{"kind":"created","target":target(),
                    "effectiveSettings":{"requestedPolicy":{"access":"workspace-write"},"mappingStatus":"verified","authentication":"authenticated"}}}
            })),
        ),
    ]
}

/// A prompt the provider admits, then completes.
fn prompt_steps(check: impl FnOnce(&Value) + Send + 'static) -> Vec<ScriptedStep> {
    vec![
        ScriptedStep::new(
            "prompt",
            check,
            ScriptedAnswer::Answer(
                json!({"admission":"admitted","operation":operation_snapshot(
                PROMPT_OPERATION, "conversationPrompt", Some(target()), "admitted", "none")}),
            ),
        ),
        ScriptedStep::new(
            "wait",
            |request| assert_eq!(request["operationId"], PROMPT_OPERATION),
            ScriptedAnswer::Answer(json!({
                "operation":operation_snapshot(PROMPT_OPERATION, "conversationPrompt", Some(target()), "terminal", "applied"),
                "output":{"kind":"available","settlement":{"kind":"promptCompleted","target":target(),"stopReason":"end_turn","response":"fixture reply"}}
            })),
        ),
    ]
}

/// A load the provider admits, then settles with the loaded target.
fn load_steps(check: impl FnOnce(&Value) + Send + 'static) -> Vec<ScriptedStep> {
    vec![
        ScriptedStep::new(
            "load",
            check,
            ScriptedAnswer::Answer(
                json!({"admission":"admitted","operation":operation_snapshot(
                LOAD_OPERATION, "conversationLoad", Some(target()), "admitted", "none")}),
            ),
        ),
        ScriptedStep::new(
            "wait",
            |request| assert_eq!(request["operationId"], LOAD_OPERATION),
            ScriptedAnswer::Answer(json!({
                "operation":operation_snapshot(LOAD_OPERATION, "conversationLoad", Some(target()), "terminal", "applied"),
                "output":{"kind":"available","settlement":{"kind":"loaded","target":target(),"effectiveSettings":{
                    "requestedPolicy":{"access":"workspace-write"},"mappingStatus":"verified","authentication":"authenticated"
                }}}
            })),
        ),
    ]
}

fn common_result(stdout: &[u8]) -> Value {
    let lines: Vec<Value> = String::from_utf8_lossy(stdout)
        .lines()
        .map(|line| serde_json::from_str(line).expect("CLI JSON line"))
        .collect();
    assert!(
        (2..=3).contains(&lines.len()),
        "operation IDs must precede the result"
    );
    assert!(matches!(
        lines[0]["kind"].as_str(),
        Some("conversationOperationStarted" | "conversationCreateStarted")
    ));
    if lines.len() == 3 {
        assert_eq!(lines[1]["kind"], "conversationOperationStarted");
    }
    lines.last().expect("result line").clone()
}

fn operation_snapshot(
    operation_id: &str,
    operation: &str,
    target_value: Option<Value>,
    stage: &str,
    effect: &str,
) -> Value {
    let mut value = json!({
        "operationId":operation_id,"operation":operation,
        "binding":{"kind":"externalProvider","binding":{
            "endpoint":endpoint(),"bindingId":"fixture-binding",
            "runtime":{"provider":"claudeCode","runtimeName":"fixture"},
            "transport":"stdioAcp","generation":generation(),
            "capabilities":[
                {"name":"create","status":"supported","evidence":"advertised"},
                {"name":"prompt","status":"supported","evidence":"advertised"},
                {"name":"callerDetach","status":"supported","evidence":"routerQualified"}
            ]
        }},
        "stage":stage,"effect":effect,"reconciliation":"unresolved",
        "admittedAt":"2026-09-20T00:00:00Z"
    });
    if let Some(target_value) = target_value {
        value["target"] = target_value;
    }
    value
}

fn endpoint() -> Value {
    json!({"serviceId":SERVICE_ID,"endpointId":ENDPOINT_ID})
}

fn generation() -> Value {
    json!({"serviceEpoch":SERVICE_EPOCH,"generation":7})
}

fn target() -> Value {
    json!({"endpoint":endpoint(),"sessionId":"provider-thread"})
}

fn actor() -> String {
    json!({"endpoint":{"serviceId":SERVICE_ID,"endpointId":"codex-local"},"sessionId":"caller"})
        .to_string()
}

fn create_arguments(operation_id: &str) -> Vec<String> {
    vec![
        "conversation",
        "create",
        "--operation-id",
        operation_id,
        "--endpoint",
        &endpoint().to_string(),
        "--generation",
        &generation().to_string(),
        "--from",
        &actor(),
        "--approver",
        &actor(),
        "--cwd",
        "/tmp/project",
        "--access",
        "workspace-write",
        "--json",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn prompt_arguments(operation_id: &str) -> Vec<String> {
    vec![
        "conversation",
        "prompt",
        "--operation-id",
        operation_id,
        "--to",
        &target().to_string(),
        "--generation",
        &generation().to_string(),
        "--from",
        &actor(),
        "--approver",
        &actor(),
        "--text",
        "hello",
        "--timeout-seconds",
        "1",
        "--json",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

async fn run_cli(root: &std::path::Path, arguments: Vec<String>) -> std::process::Output {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args(arguments)
        .arg("--service-directory")
        .arg(root)
        .output()
        .await
        .expect("CLI")
}

fn cleanup_unpublished_fixture(root: &std::path::Path) {
    std::fs::remove_dir(root).expect("directory cleanup");
}
