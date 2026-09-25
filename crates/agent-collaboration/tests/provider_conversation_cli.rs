#![allow(clippy::expect_used, clippy::indexing_slicing)]
//! Compiled-CLI fixture assertions deliberately fail fast at the exact wire boundary.

use serde_json::{Value, json};
use std::os::unix::fs::DirBuilderExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
const SERVICE_EPOCH: &str = "00000000-0000-4000-8000-000000000002";
const ENDPOINT_ID: &str = "claude-fixture";
const CREATE_OPERATION: &str = "019c6e27-e55b-73d1-87d8-4e01f1f75101";
const PROMPT_OPERATION: &str = "019c6e27-e55b-73d1-87d8-4e01f1f75102";
const WRONG_GENERATION_OPERATION: &str = "019c6e27-e55b-73d1-87d8-4e01f1f75103";
const LOST_RESPONSE_OPERATION: &str = "019c6e27-e55b-73d1-87d8-4e01f1f75104";
const LOAD_OPERATION: &str = "019c6e27-e55b-73d1-87d8-4e01f1f75105";
const CANCEL_OPERATION: &str = "019c6e27-e55b-73d1-87d8-4e01f1f75106";

#[tokio::test]
async fn common_provider_cancel_allocates_and_prints_omitted_operation_id() {
    let root = fixture_directory("common-cancel-generated-id");
    let listener = publish_fixture(&root);
    let fixture = tokio::spawn(async move {
        serve_one(&listener, "conversation/cancel", |request| {
            let operation_id = request["params"]["operationId"].as_str().expect("generated operation ID");
            let _: collaboration_client::protocol::OperationId = operation_id.to_owned().try_into().expect("UUIDv7 operation ID");
            assert_eq!(request["params"]["targetOperationId"], PROMPT_OPERATION);
            json!({"admission":"admitted","operation":operation_snapshot(operation_id, "conversationCancel", Some(target()), "admitted", "none")})
        }).await;
    });
    let output = run_cli(
        &root,
        vec![
            "conversation",
            "cancel",
            "--target-operation-id",
            PROMPT_OPERATION,
            "--target",
            &target().to_string(),
            "--from",
            &actor(),
            "--json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    )
    .await;
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).expect("CLI JSON line"))
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["kind"], "conversationOperationStarted");
    assert_eq!(
        lines[0]["operationId"],
        lines[1]["operation"]["operationId"]
    );
    fixture.await.expect("fixture");
    cleanup_fixture(&root);
}

#[tokio::test]
async fn common_new_prompt_composes_provider_create_then_prompt() {
    let root = fixture_directory("common-new-prompt");
    let listener = publish_fixture(&root);
    let fixture = tokio::spawn(async move {
        serve_inventory_only(&listener).await;
        serve_one(&listener, "conversation/create", |request| {
            assert_eq!(request["params"]["operationId"], CREATE_OPERATION);
            json!({"admission":"admitted","operation":operation_snapshot(CREATE_OPERATION, "conversationCreate", Some(target()), "terminal", "applied")})
        }).await;
        serve_one(&listener, "conversation/prompt", |request| {
            assert_eq!(request["params"]["operationId"], PROMPT_OPERATION);
            assert_eq!(request["params"]["target"], target());
            json!({"admission":"admitted","operation":operation_snapshot(PROMPT_OPERATION, "conversationPrompt", Some(target()), "admitted", "none")})
        }).await;
    });
    let output = run_cli(
        &root,
        vec![
            "conversation",
            "prompt",
            "--new",
            "--endpoint",
            &endpoint().to_string(),
            "--operation-id",
            CREATE_OPERATION,
            "--prompt-operation-id",
            PROMPT_OPERATION,
            "--from",
            &actor(),
            "--approver",
            &actor(),
            "--cwd",
            "/tmp/project",
            "--access",
            "workspace-write",
            "--text",
            "hello",
            "--json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    )
    .await;
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).expect("CLI JSON line"))
        .collect();
    assert_eq!(lines[0]["operationId"], CREATE_OPERATION);
    assert_eq!(lines[1]["operationId"], PROMPT_OPERATION);
    let outcome = common_result(&output.stdout);
    assert_eq!(outcome["kind"], "prompt");
    assert_eq!(outcome["createOperationId"], CREATE_OPERATION);
    assert_eq!(outcome["prompt"]["kind"], "completed");
    assert_eq!(outcome["prompt"]["operationId"], PROMPT_OPERATION);
    assert_eq!(
        outcome["prompt"]["settlement"]["detail"]["kind"],
        "providerPrompt"
    );
    fixture.await.expect("fixture");
    cleanup_fixture(&root);
}

#[tokio::test]
async fn common_load_settles_and_cancel_names_exact_operation() {
    let root = fixture_directory("common-load-cancel");
    let listener = publish_fixture(&root);
    let fixture = tokio::spawn(async move {
        serve_one(&listener, "conversation/load", |request| {
            assert_eq!(request["params"]["operationId"], LOAD_OPERATION);
            assert_eq!(request["params"]["target"], target());
            json!({"admission":"admitted","operation":operation_snapshot(LOAD_OPERATION, "conversationLoad", Some(target()), "admitted", "none")})
        }).await;
        serve_one(&listener, "conversation/cancel", |request| {
            assert_eq!(request["params"]["operationId"], CANCEL_OPERATION);
            assert_eq!(request["params"]["targetOperationId"], PROMPT_OPERATION);
            assert_eq!(request["params"]["target"], target());
            json!({"admission":"admitted","operation":operation_snapshot(CANCEL_OPERATION, "conversationCancel", Some(target()), "admitted", "none")})
        }).await;
    });
    let load = run_cli(
        &root,
        vec![
            "conversation",
            "load",
            "--operation-id",
            LOAD_OPERATION,
            "--target",
            &target().to_string(),
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
        .collect(),
    )
    .await;
    assert_eq!(
        load.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&load.stdout),
        String::from_utf8_lossy(&load.stderr)
    );
    let loaded = common_result(&load.stdout);
    assert_eq!(loaded["kind"], "completed");
    assert_eq!(loaded["operationId"], LOAD_OPERATION);
    assert_eq!(loaded["settlement"]["detail"]["kind"], "providerLoad");

    let cancel = run_cli(
        &root,
        vec![
            "conversation",
            "cancel",
            "--operation-id",
            CANCEL_OPERATION,
            "--target-operation-id",
            PROMPT_OPERATION,
            "--target",
            &target().to_string(),
            "--generation",
            &generation().to_string(),
            "--from",
            &actor(),
            "--approver",
            &actor(),
            "--json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    )
    .await;
    assert_eq!(
        cancel.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&cancel.stdout),
        String::from_utf8_lossy(&cancel.stderr)
    );
    let cancelled = common_result(&cancel.stdout);
    assert_eq!(cancelled["operation"]["operationId"], CANCEL_OPERATION);
    fixture.await.expect("fixture");
    cleanup_fixture(&root);
}

#[tokio::test]
async fn common_create_waits_for_provider_target_and_prints_operation_first() {
    let root = fixture_directory("common-create");
    let listener = publish_fixture(&root);
    let fixture = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept common create");
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        initialize_fixture(&mut lines, &mut write).await;
        let list: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("list read")
                .expect("list frame"),
        )
        .expect("list JSON");
        assert_eq!(list["method"], "endpoint/list");
        write_response(&mut write, &list, json!({"serviceEpoch":SERVICE_EPOCH,"sequence":1,"endpoints":[{
            "endpoint":endpoint(),"label":"Claude fixture",
            "availability":{"state":"available","observedAt":"2026-09-24T00:00:00Z"},
            "channels":[{"kind":"externalProvider","transport":"stdioAcp","bindingId":"fixture-binding","bindingGeneration":7,
                "runtime":{"provider":"claudeCode","runtimeName":"fixture"},
                "capabilities":[{"name":"create","status":"supported","evidence":"advertised"}]}]
        }]})).await;
        let create: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("create read")
                .expect("create frame"),
        )
        .expect("create JSON");
        assert_eq!(create["method"], "conversation/create");
        assert_eq!(create["params"]["operationId"], CREATE_OPERATION);
        assert_eq!(create["params"]["generation"], generation());
        write_response(&mut write, &create, json!({"admission":"admitted","operation":operation_snapshot(CREATE_OPERATION, "conversationCreate", None, "admitted", "none")})).await;
        let wait: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("wait read")
                .expect("wait frame"),
        )
        .expect("wait JSON");
        assert_eq!(wait["method"], "conversation/operationWait");
        assert_eq!(wait["params"]["operationId"], CREATE_OPERATION);
        write_response(&mut write, &wait, json!({"operation":operation_snapshot(CREATE_OPERATION, "conversationCreate", Some(target()), "terminal", "applied"),
            "output":{"kind":"outputUnavailable","reason":"notRetained"}})).await;
    });
    let create = run_cli(
        &root,
        vec![
            "conversation",
            "create",
            "--operation-id",
            CREATE_OPERATION,
            "--endpoint",
            &endpoint().to_string(),
            "--generation",
            &generation().to_string(),
            "--from",
            &actor(),
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
    assert_eq!(
        create.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&create.stdout),
        String::from_utf8_lossy(&create.stderr)
    );
    let lines: Vec<Value> = String::from_utf8_lossy(&create.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).expect("line JSON"))
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["kind"], "conversationCreateStarted");
    assert_eq!(lines[0]["operationId"], CREATE_OPERATION);
    assert_eq!(lines[1]["kind"], "created");
    assert_eq!(lines[1]["operationId"], CREATE_OPERATION);
    assert_eq!(lines[1]["target"], target());
    fixture.await.expect("fixture");
    cleanup_fixture(&root);
}

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
async fn provider_create_rejects_model_and_effort_before_start_record() {
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
            "--effort",
            "medium",
            "--json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    )
    .await;
    assert_eq!(output.status.code(), Some(4));
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !rendered.contains("conversationCreateStarted"),
        "{rendered}"
    );
    assert!(rendered.contains("claude-local"), "{rendered}");
    assert!(rendered.contains("--model"), "{rendered}");
    assert!(rendered.contains("--effort"), "{rendered}");
    cleanup_unpublished_fixture(&root);
}

#[tokio::test]
async fn external_provider_create_then_prompt_preserves_target_and_supplied_operations() {
    let root = fixture_directory("create-prompt");
    let listener = publish_fixture(&root);
    let fixture = tokio::spawn(async move {
        let create = serve_one(&listener, "conversation/create", |request| {
            assert_eq!(request["params"]["operationId"], CREATE_OPERATION);
            assert_eq!(request["params"]["generation"]["generation"], 7);
            json!({"admission":"admitted","operation":operation_snapshot(CREATE_OPERATION, "conversationCreate", Some(target()), "terminal", "applied")})
        })
        .await;
        let expected_target = target();
        serve_one(&listener, "conversation/prompt", move |request| {
            assert_eq!(request["params"]["operationId"], PROMPT_OPERATION);
            assert_eq!(request["params"]["target"], expected_target);
            json!({"admission":"admitted","operation":operation_snapshot(PROMPT_OPERATION, "conversationPrompt", Some(target()), "admitted", "none")})
        })
        .await;
        create
    });

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

    fixture.await.expect("fixture");
    cleanup_fixture(&root);
}

#[tokio::test]
async fn wrong_generation_is_no_effect_response_loss_retains_id_and_show_is_read_only() {
    let wrong_root = fixture_directory("wrong-generation");
    let wrong_listener = publish_fixture(&wrong_root);
    let wrong_fixture = tokio::spawn(async move {
        serve_one_error(
            &wrong_listener,
            "conversation/prompt",
            json!({
                "kind":"staleGeneration","stage":"binding","effect":"none",
                "message":"provider binding generation changed",
                "operationId":WRONG_GENERATION_OPERATION,"target":target()
            }),
        )
        .await;
    });
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
    wrong_fixture.await.expect("wrong-generation fixture");
    cleanup_fixture(&wrong_root);

    let lost_root = fixture_directory("lost-response");
    let lost_listener = publish_fixture(&lost_root);
    let lost_fixture = tokio::spawn(async move {
        serve_one_without_response(&lost_listener, "conversation/prompt").await;
    });
    let lost = run_cli(&lost_root, prompt_arguments(LOST_RESPONSE_OPERATION)).await;
    assert_eq!(lost.status.code(), Some(0));
    let lost_json = common_result(&lost.stdout);
    assert_eq!(lost_json["kind"], "pending");
    assert_eq!(lost_json["operationId"], LOST_RESPONSE_OPERATION);
    assert_eq!(lost_json["target"], target());
    lost_fixture.await.expect("lost-response fixture");
    cleanup_fixture(&lost_root);

    let show_root = fixture_directory("operation-show");
    let show_listener = publish_fixture(&show_root);
    let show_fixture = tokio::spawn(async move {
        serve_one(&show_listener, "conversation/operationShow", |request| {
            assert_eq!(request["params"]["operationId"], CREATE_OPERATION);
            operation_snapshot(
                CREATE_OPERATION,
                "conversationCreate",
                Some(target()),
                "terminal",
                "applied",
            )
        })
        .await;
    });
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
    show_fixture.await.expect("show fixture");
    cleanup_fixture(&show_root);

    for (fixture_name, method, command) in [
        ("show-response-loss", "conversation/operationShow", "show"),
        ("wait-response-loss", "conversation/operationWait", "wait"),
        (
            "reconcile-response-loss",
            "conversation/operationReconcile",
            "reconcile",
        ),
    ] {
        let read_root = fixture_directory(fixture_name);
        let read_listener = publish_fixture(&read_root);
        let read_fixture = tokio::spawn(async move {
            serve_one_without_response(&read_listener, method).await;
        });
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
        read_fixture.await.expect("read failure fixture");
        cleanup_fixture(&read_root);
    }
}

#[tokio::test]
async fn unavailable_provider_prompt_prints_catalog_reason_and_fix() {
    let root = fixture_directory("unavailable-provider");
    let listener = publish_fixture(&root);
    let availability = json!({
        "state":"unavailable","observedAt":"2026-09-24T00:00:00Z",
        "reason":"provider executable is missing","fix":"install the provider binary"
    });
    let expected_availability = availability.clone();
    let fixture = tokio::spawn(async move {
        serve_one_error(&listener, "conversation/prompt", json!({
            "kind":"unavailable","stage":"binding","effect":"none",
            "message":"provider conversation endpoint claude-local unavailable: provider executable is missing",
            "operationId":PROMPT_OPERATION,"target":target(),
            "endpoint":endpoint(),"availability":availability
        })).await;
    });
    let output = run_cli(&root, prompt_arguments(PROMPT_OPERATION)).await;
    assert_eq!(output.status.code(), Some(3));
    let result = common_result(&output.stdout);
    assert_eq!(result["error"]["kind"], "unavailable");
    assert_eq!(result["error"]["endpoint"], endpoint());
    assert_eq!(result["error"]["availability"], expected_availability);
    assert!(
        result["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("claude-local"))
    );
    fixture.await.expect("fixture");
    cleanup_fixture(&root);
}

#[tokio::test]
#[ignore = "requires an explicitly selected authenticated Cursor ACP runtime"]
async fn live_cursor_create_wait_prompt_wait_through_compiled_cli() {
    use codex_router_host::{
        CollaborationRuntime, CollaborationRuntimeInputs, ExternalProviderLaunchBinding,
    };
    use collaboration_client::ControlClient;
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
        },
        vec![codex_router_host::ExternalProviderStartup::Launch(
            ExternalProviderLaunchBinding::cursor(executable, arguments)
                .expect("Cursor provider binding"),
        )],
    )
    .await
    .expect("collaboration runtime");

    let mut control = ControlClient::connect(&service_directory, "live-cli-setup", "1")
        .await
        .expect("Control setup client");
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
    let _closed = control.close().await;

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

fn publish_fixture(root: &std::path::Path) -> tokio::net::UnixListener {
    let listener = tokio::net::UnixListener::bind(root.join("control.sock")).expect("listener");
    let digest = format!("sha256:{}", "a".repeat(64));
    let manifest = serde_json::from_value(json!({
        "version":2,"serviceId":SERVICE_ID,"serviceEpoch":SERVICE_EPOCH,
        "control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest,
        "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .expect("manifest");
    let publication =
        collaboration_service::ManifestPublication::publish(root, &manifest).expect("publish");
    Box::leak(Box::new(publication));
    listener
}

async fn serve_one(
    listener: &tokio::net::UnixListener,
    method: &str,
    result: impl FnOnce(&Value) -> Value,
) {
    let (stream, _) = listener.accept().await.expect("accept");
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let initialize: Value =
        serde_json::from_str(&lines.next_line().await.expect("read").expect("initialize"))
            .expect("initialize JSON");
    write_response(
        &mut write,
        &initialize,
        json!({"version":{"major":1,"minor":0},"serviceId":SERVICE_ID,"serviceEpoch":SERVICE_EPOCH,"controlSchemaDigest":format!("sha256:{}", "a".repeat(64))}),
    )
    .await;
    let request = read_operation_request(&mut lines, &mut write).await;
    assert_eq!(request["method"], method);
    write_response(&mut write, &request, result(&request)).await;
    if method == "conversation/prompt" {
        let wait: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("wait read")
                .expect("wait frame"),
        )
        .expect("wait JSON");
        assert_eq!(wait["method"], "conversation/operationWait");
        write_response(&mut write, &wait, json!({
            "operation":operation_snapshot(PROMPT_OPERATION, "conversationPrompt", Some(target()), "terminal", "applied"),
            "output":{"kind":"available","settlement":{"kind":"promptCompleted","target":target(),"stopReason":"end_turn","response":"fixture reply"}}
        })).await;
    } else if method == "conversation/load" {
        let wait: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("wait read")
                .expect("wait frame"),
        )
        .expect("wait JSON");
        assert_eq!(wait["method"], "conversation/operationWait");
        write_response(&mut write, &wait, json!({
            "operation":operation_snapshot(LOAD_OPERATION, "conversationLoad", Some(target()), "terminal", "applied"),
            "output":{"kind":"available","settlement":{"kind":"loaded","target":target(),"effectiveSettings":{
                "requestedPolicy":{"access":"workspace-write"},"mappingStatus":"verified","authentication":"authenticated"
            }}}
        })).await;
    }
}

async fn serve_one_error(listener: &tokio::net::UnixListener, method: &str, data: Value) {
    let (stream, _) = listener.accept().await.expect("accept");
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    initialize_fixture(&mut lines, &mut write).await;
    let request = read_operation_request(&mut lines, &mut write).await;
    assert_eq!(request["method"], method);
    write
        .write_all(
            format!(
                "{}\n",
                json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32050,"message":"fixture rejection","data":data}})
            )
            .as_bytes(),
        )
        .await
        .expect("error response");
}

async fn serve_one_without_response(listener: &tokio::net::UnixListener, method: &str) {
    let (stream, _) = listener.accept().await.expect("accept");
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    initialize_fixture(&mut lines, &mut write).await;
    let request = read_operation_request(&mut lines, &mut write).await;
    assert_eq!(request["method"], method);
    if method == "conversation/prompt" {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

async fn read_operation_request(
    lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    write: &mut tokio::net::unix::OwnedWriteHalf,
) -> Value {
    let request: Value =
        serde_json::from_str(&lines.next_line().await.expect("read").expect("request"))
            .expect("request JSON");
    if request["method"] != "endpoint/list" {
        return request;
    }
    write_response(write, &request, json!({"serviceEpoch":SERVICE_EPOCH,"sequence":1,"endpoints":[{
        "endpoint":endpoint(),"label":"Claude fixture",
        "availability":{"state":"available","observedAt":"2026-09-24T00:00:00Z"},
        "channels":[{"kind":"externalProvider","transport":"stdioAcp","bindingId":"fixture-binding","bindingGeneration":7,
            "runtime":{"provider":"claudeCode","runtimeName":"fixture"},
            "capabilities":[{"name":"create","status":"supported","evidence":"advertised"},
                {"name":"prompt","status":"supported","evidence":"advertised"},
                {"name":"load","status":"supported","evidence":"advertised"},
                {"name":"cancel","status":"supported","evidence":"advertised"}]}]
    }]})).await;
    serde_json::from_str(
        &lines
            .next_line()
            .await
            .expect("operation read")
            .expect("operation frame"),
    )
    .expect("operation JSON")
}

async fn serve_inventory_only(listener: &tokio::net::UnixListener) {
    let (stream, _) = listener.accept().await.expect("inventory accept");
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    initialize_fixture(&mut lines, &mut write).await;
    let request: Value = serde_json::from_str(
        &lines
            .next_line()
            .await
            .expect("inventory read")
            .expect("inventory frame"),
    )
    .expect("inventory JSON");
    assert_eq!(request["method"], "endpoint/list");
    write_response(&mut write, &request, json!({"serviceEpoch":SERVICE_EPOCH,"sequence":1,"endpoints":[{
        "endpoint":endpoint(),"label":"Claude fixture",
        "availability":{"state":"available","observedAt":"2026-09-24T00:00:00Z"},
        "channels":[{"kind":"externalProvider","transport":"stdioAcp","bindingId":"fixture-binding","bindingGeneration":7,
            "runtime":{"provider":"claudeCode","runtimeName":"fixture"},
            "capabilities":[{"name":"create","status":"supported","evidence":"advertised"},
                {"name":"prompt","status":"supported","evidence":"advertised"}]}]
    }]})).await;
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

async fn initialize_fixture(
    lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    write: &mut tokio::net::unix::OwnedWriteHalf,
) {
    let initialize: Value =
        serde_json::from_str(&lines.next_line().await.expect("read").expect("initialize"))
            .expect("initialize JSON");
    write_response(
        write,
        &initialize,
        json!({"version":{"major":1,"minor":0},"serviceId":SERVICE_ID,"serviceEpoch":SERVICE_EPOCH,"controlSchemaDigest":format!("sha256:{}", "a".repeat(64))}),
    )
    .await;
}

async fn write_response(
    write: &mut tokio::net::unix::OwnedWriteHalf,
    request: &Value,
    result: Value,
) {
    write
        .write_all(
            format!(
                "{}\n",
                json!({"jsonrpc":"2.0","id":request["id"],"result":result})
            )
            .as_bytes(),
        )
        .await
        .expect("response");
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

fn cleanup_fixture(root: &std::path::Path) {
    std::fs::remove_file(root.join("control.sock")).expect("socket cleanup");
    std::fs::remove_file(root.join("service.json")).expect("manifest cleanup");
    std::fs::remove_dir(root).expect("directory cleanup");
}

fn cleanup_unpublished_fixture(root: &std::path::Path) {
    std::fs::remove_dir(root).expect("directory cleanup");
}
