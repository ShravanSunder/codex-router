use super::*;

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
        assert_eq!(
            create["params"]["settings"],
            json!({
                "mode":"ask", "model":"provider-model", "effort":"high"
            })
        );
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
            "output":{"kind":"available","settlement":{"kind":"created","target":target(),
                "effectiveSettings":{"requestedPolicy":{"access":"workspace-write"},"mappingStatus":"verified",
                    "authentication":"authenticated","mode":"ask","model":"provider-model","effort":"high"}}}})).await;
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
            "--mode",
            "ask",
            "--model",
            "provider-model",
            "--effort",
            "high",
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
async fn provider_settings_set_and_accept_use_immediate_control_methods() {
    let root = fixture_directory("provider-settings-actions");
    let listener = publish_fixture(&root);
    let fixture = tokio::spawn(async move {
        serve_one(&listener, "conversation/settingsSet", |request| {
            assert_eq!(request["params"]["target"], target());
            assert_eq!(
                request["params"]["actor"],
                json!(serde_json::from_str::<Value>(&actor()).expect("actor"))
            );
            assert_eq!(request["params"]["setting"], "mode");
            assert_eq!(request["params"]["value"], "ask");
            assert!(request["params"].get("operationId").is_none());
            settings_result()
        })
        .await;
        serve_one(&listener, "conversation/settingsAccept", |request| {
            assert_eq!(request["params"]["target"], target());
            assert!(request["params"].get("operationId").is_none());
            settings_result()
        })
        .await;
    });
    let target_json = target().to_string();
    let actor_json = actor();
    let set = run_cli(
        &root,
        vec![
            "conversation",
            "settings",
            "set",
            "--target",
            &target_json,
            "--actor",
            &actor_json,
            "--setting",
            "mode",
            "--value",
            "ask",
            "--json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    )
    .await;
    assert_eq!(
        set.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&set.stdout)
    );
    let set_result: Value = serde_json::from_slice(&set.stdout).expect("set result");
    assert_eq!(set_result["result"]["effectiveSettings"]["mode"], "ask");
    let accept = run_cli(
        &root,
        vec![
            "conversation",
            "settings",
            "accept",
            "--target",
            &target_json,
            "--actor",
            &actor_json,
            "--json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    )
    .await;
    assert_eq!(
        accept.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&accept.stdout)
    );
    let accept_result: Value = serde_json::from_slice(&accept.stdout).expect("accept result");
    assert_eq!(accept_result["result"]["target"], target());
    fixture.await.expect("fixture");
    cleanup_fixture(&root);
}

#[tokio::test]
async fn provider_session_inspect_shows_capabilities_and_last_settings() {
    let root = fixture_directory("provider-session-inspect");
    let listener = publish_fixture(&root);
    let fixture = tokio::spawn(async move {
        serve_one(&listener, "provider/sessionInspect", |request| {
            assert_eq!(request["params"]["target"], target());
            json!({"target":target(),"state":"idle","history":"available",
                "capabilities":{"load":true,"resume":true,"close":true,"list":true,"steer":false,
                    "queue":{"kind":"router","canCancel":true},"modes":true,"configOptions":true,
                    "elicitation":true,"usage":false,"promptContent":{"image":false,"audio":false,"embeddedContext":false},
                    "authStatus":{"kind":"account","label":"Signed in"}},
                "settingsCatalog":{"currentMode":"ask","modes":[{"value":"ask","label":"Ask"}],"configOptions":[]}})
        }).await;
    });
    let output = run_cli(
        &root,
        vec![
            "session",
            "inspect",
            "--endpoint",
            ENDPOINT_ID,
            "--session",
            "provider-thread",
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
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("CLI result");
    assert_eq!(
        value["result"]["record"]["capabilities"]["authStatus"]["kind"],
        "account"
    );
    assert_eq!(
        value["result"]["record"]["settingsCatalog"]["currentMode"],
        "ask"
    );
    fixture.await.expect("fixture");
    cleanup_fixture(&root);
}

fn settings_result() -> Value {
    json!({"target":target(),"effectiveSettings":{
        "requestedPolicy":{"access":"workspace-write"},
        "mappingStatus":"verified","authentication":"authenticated","mode":"ask"
    }})
}
