use super::*;

#[tokio::test]
async fn common_provider_cancel_allocates_and_prints_omitted_operation_id() {
    let fixture = ProviderFixture::start(
        "common-cancel-generated-id",
        vec![ScriptedStep::responding("cancel", |request| {
            let operation_id = request["operationId"].as_str().expect("generated operation ID");
            let _: collaboration_client::protocol::OperationId = operation_id.to_owned().try_into().expect("UUIDv7 operation ID");
            assert_eq!(request["targetOperationId"], PROMPT_OPERATION);
            ScriptedAnswer::Answer(json!({"admission":"admitted","operation":operation_snapshot(operation_id, "conversationCancel", Some(target()), "admitted", "none")}))
        })],
    )
    .await;
    let root = fixture.root().to_owned();
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
    fixture.finish().await;
}

#[tokio::test]
async fn common_new_prompt_composes_provider_create_then_prompt() {
    let fixture = ProviderFixture::start(
        "common-new-prompt",
        [
            create_steps(|request| assert_eq!(request["operationId"], CREATE_OPERATION)),
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
    fixture.finish().await;
}

#[tokio::test]
async fn common_load_settles_and_cancel_names_exact_operation() {
    let mut steps = load_steps(|request| {
        assert_eq!(request["operationId"], LOAD_OPERATION);
        assert_eq!(request["target"], target());
    });
    steps.push(ScriptedStep::new(
        "cancel",
        |request| {
            assert_eq!(request["operationId"], CANCEL_OPERATION);
            assert_eq!(request["targetOperationId"], PROMPT_OPERATION);
            assert_eq!(request["target"], target());
        },
        ScriptedAnswer::Answer(json!({"admission":"admitted","operation":operation_snapshot(CANCEL_OPERATION, "conversationCancel", Some(target()), "admitted", "none")})),
    ));
    let fixture = ProviderFixture::start("common-load-cancel", steps).await;
    let root = fixture.root().to_owned();
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
    fixture.finish().await;
}

#[tokio::test]
async fn common_create_waits_for_provider_target_and_prints_operation_first() {
    let fixture = ProviderFixture::start(
        "common-create",
        vec![
            ScriptedStep::new(
                "create",
                |create| {
                    assert_eq!(create["operationId"], CREATE_OPERATION);
                    assert_eq!(create["generation"], generation());
                    assert_eq!(
                        create["settings"],
                        json!({
                            "mode":"ask", "model":"provider-model", "effort":"high"
                        })
                    );
                },
                ScriptedAnswer::Answer(json!({"admission":"admitted","operation":operation_snapshot(CREATE_OPERATION, "conversationCreate", None, "admitted", "none")})),
            ),
            ScriptedStep::new(
                "wait",
                |wait| assert_eq!(wait["operationId"], CREATE_OPERATION),
                ScriptedAnswer::Answer(json!({"operation":operation_snapshot(CREATE_OPERATION, "conversationCreate", Some(target()), "terminal", "applied"),
                    "output":{"kind":"available","settlement":{"kind":"created","target":target(),
                        "effectiveSettings":{"requestedPolicy":{"access":"workspace-write"},"mappingStatus":"verified",
                            "authentication":"authenticated","mode":"ask","model":"provider-model","effort":"high"}}}})),
            ),
        ],
    )
    .await;
    let root = fixture.root().to_owned();
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
    fixture.finish().await;
}

#[tokio::test]
async fn provider_settings_set_and_accept_use_immediate_settings_tools() {
    let fixture = ProviderFixture::start(
        "provider-settings-actions",
        vec![
            ScriptedStep::new(
                "settingsSet",
                |request| {
                    assert_eq!(request["target"], target());
                    assert_eq!(
                        request["actor"],
                        json!(serde_json::from_str::<Value>(&actor()).expect("actor"))
                    );
                    assert_eq!(request["setting"], "mode");
                    assert_eq!(request["value"], "ask");
                    assert!(request.get("operationId").is_none());
                },
                ScriptedAnswer::Answer(settings_result()),
            ),
            ScriptedStep::new(
                "settingsAccept",
                |request| {
                    assert_eq!(request["target"], target());
                    assert!(request.get("operationId").is_none());
                },
                ScriptedAnswer::Answer(settings_result()),
            ),
        ],
    )
    .await;
    let root = fixture.root().to_owned();
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
    fixture.finish().await;
}

#[tokio::test]
async fn provider_session_inspect_shows_capabilities_and_last_settings() {
    let fixture = ProviderFixture::start(
        "provider-session-inspect",
        vec![ScriptedStep::new(
            "inspectSession",
            |request| assert_eq!(request["target"], target()),
            ScriptedAnswer::Answer(json!({"target":target(),"state":"idle","history":"available",
                "capabilities":{"load":true,"resume":true,"close":true,"list":true,"steer":false,
                    "queue":{"kind":"router","canCancel":true},"modes":true,"configOptions":true,
                    "elicitation":true,"usage":false,"promptContent":{"image":false,"audio":false,"embeddedContext":false},
                    "authStatus":{"kind":"account","label":"Signed in"}},
                "settingsCatalog":{"currentMode":"ask","modes":[{"value":"ask","label":"Ask"}],"configOptions":[]}})),
        )],
    )
    .await;
    let root = fixture.root().to_owned();
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
    fixture.finish().await;
}

#[tokio::test]
async fn provider_resume_close_and_operation_reads_reach_the_scripted_provider() {
    const RESUME_OPERATION: &str = "019c6e27-e55b-73d1-87d8-4e01f1f75107";
    const CLOSE_OPERATION: &str = "019c6e27-e55b-73d1-87d8-4e01f1f75108";
    let settled = |operation_id: &'static str, operation: &'static str, settlement: Value| {
        ScriptedStep::new(
            "wait",
            move |request| assert_eq!(request["operationId"], operation_id),
            ScriptedAnswer::Answer(json!({
                "operation":operation_snapshot(operation_id, operation, Some(target()), "terminal", "applied"),
                "output":{"kind":"available","settlement":settlement}
            })),
        )
    };
    let admitted = |method: &'static str, operation_id: &'static str, operation: &'static str| {
        ScriptedStep::new(
            method,
            move |request| {
                assert_eq!(request["operationId"], operation_id);
                assert_eq!(request["target"], target());
                // The CLI's Human `--approver` reaches the provider operation unchanged.
                assert_eq!(request["approver"], json!({"humanId":"lifecycle-owner"}));
            },
            ScriptedAnswer::Answer(
                json!({"admission":"admitted","operation":operation_snapshot(
                operation_id, operation, Some(target()), "admitted", "none")}),
            ),
        )
    };
    let effective = json!({"requestedPolicy":{"access":"workspace-write"},"mappingStatus":"verified","authentication":"authenticated"});
    let fixture = ProviderFixture::start(
        "lifecycle-and-reads",
        vec![
            admitted("resume", RESUME_OPERATION, "conversationResume"),
            settled(
                RESUME_OPERATION,
                "conversationResume",
                json!({"kind":"resumed","target":target(),"effectiveSettings":effective,"history":"available"}),
            ),
            admitted("close", CLOSE_OPERATION, "conversationClose"),
            settled(
                CLOSE_OPERATION,
                "conversationClose",
                json!({"kind":"closed","target":target()}),
            ),
            settled(
                CLOSE_OPERATION,
                "conversationClose",
                json!({"kind":"closed","target":target()}),
            ),
            ScriptedStep::new(
                "reconcile",
                |request| assert_eq!(request["operationId"], CLOSE_OPERATION),
                ScriptedAnswer::Answer(operation_snapshot(
                    CLOSE_OPERATION,
                    "conversationClose",
                    Some(target()),
                    "terminal",
                    "applied",
                )),
            ),
        ],
    )
    .await;
    let root = fixture.root().to_owned();
    let lifecycle = |command: &str, operation_id: &str| {
        let mut arguments = vec![
            "conversation".to_owned(),
            command.to_owned(),
            "--operation-id".to_owned(),
            operation_id.to_owned(),
            "--target".to_owned(),
            target().to_string(),
            "--generation".to_owned(),
            generation().to_string(),
            "--from".to_owned(),
            actor(),
            "--approver".to_owned(),
            json!({"humanId":"lifecycle-owner"}).to_string(),
            "--json".to_owned(),
        ];
        if command == "resume" {
            arguments.extend(
                ["--cwd", "/tmp/project", "--access", "workspace-write"].map(str::to_owned),
            );
        }
        arguments
    };
    for (command, operation_id, settlement) in [
        ("resume", RESUME_OPERATION, "resumed"),
        ("close", CLOSE_OPERATION, "closed"),
    ] {
        let output = run_cli(&root, lifecycle(command, operation_id)).await;
        assert_eq!(
            output.status.code(),
            Some(0),
            "{command}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let lines: Vec<Value> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| serde_json::from_str(line).expect("CLI JSON line"))
            .collect();
        assert_eq!(lines.len(), 2, "{command}");
        assert_eq!(lines[0]["kind"], "conversationOperationStarted");
        assert_eq!(lines[0]["operationId"], operation_id);
        assert_eq!(lines[1]["operation"]["operationId"], operation_id);
        assert_eq!(lines[1]["output"]["settlement"]["kind"], settlement);
    }
    for (command, expected) in [("wait", "closed"), ("reconcile", "terminal")] {
        let mut arguments: Vec<String> = [
            "conversation",
            "operation",
            command,
            "--operation-id",
            CLOSE_OPERATION,
            "--json",
        ]
        .map(str::to_owned)
        .to_vec();
        if command == "wait" {
            arguments.extend(["--timeout-seconds", "5"].map(str::to_owned));
        }
        let output = run_cli(&root, arguments).await;
        assert_eq!(
            output.status.code(),
            Some(0),
            "{command}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let read: Value = serde_json::from_slice(&output.stdout).expect("operation read JSON");
        let observed = if command == "wait" {
            &read["result"]["record"]["output"]["settlement"]["kind"]
        } else {
            &read["result"]["record"]["stage"]
        };
        assert_eq!(observed, expected, "{command}: {read}");
    }
    fixture.finish().await;
}

fn settings_result() -> Value {
    json!({"target":target(),"effectiveSettings":{
        "requestedPolicy":{"access":"workspace-write"},
        "mappingStatus":"verified","authentication":"authenticated","mode":"ask"
    }})
}
