use super::*;

#[test]
fn catalog_has_complete_unique_tools_with_resolvable_schemas() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let tools = server.resolved_tools();
    assert_eq!(tools.len(), 107);
    let mut names = tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 107);
    assert!(names.contains(&"conversation_resume"));
    assert!(names.contains(&"conversation_close"));
    assert!(names.contains(&"provider_sessions_list"));
    assert!(names.contains(&"provider_session_inspect"));
    assert!(names.contains(&"question_list"));
    assert!(names.contains(&"question_answer"));
    assert!(names.contains(&"board_thread_subscribe"));
    assert!(names.contains(&"board_thread_unsubscribe"));
    assert!(names.contains(&"board_thread_subscriptions"));
    for tool in tools {
        let input = serde_json::to_value(&tool.input_schema).expect("input schema JSON");
        jsonschema::validator_for(&input)
            .unwrap_or_else(|error| panic!("{} input schema: {error}", tool.name));
        let output = serde_json::to_value(
            tool.output_schema
                .as_ref()
                .unwrap_or_else(|| panic!("{} output schema", tool.name)),
        )
        .expect("output schema JSON");
        jsonschema::validator_for(&output)
            .unwrap_or_else(|error| panic!("{} output schema: {error}", tool.name));
    }
}

#[test]
fn typed_tool_names_cover_every_control_domain_operation() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let actual = server
        .tool_router
        .list_all()
        .into_iter()
        .map(|tool| tool.name.into_owned())
        .collect::<BTreeSet<_>>();
    let document =
        collaboration_protocol::control_schema_document(None).expect("Control schema document");
    let methods = document
        .get("x-methods")
        .and_then(Value::as_object)
        .expect("Control method map");
    let expected = methods
        .keys()
        .filter(|method| {
            !matches!(
                method.as_str(),
                "control/initialize" | "provider/sessionObserve" | "provider/sessionListen"
            )
        })
        .map(|method| expected_tool_name(method))
        .chain([
            "conversation_create".to_owned(),
            "conversation_create_and_prompt".to_owned(),
            "conversation_prompt".to_owned(),
            "events_observe".to_owned(),
        ])
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected);
    assert!(
        actual
            .iter()
            .all(|name| !name.starts_with("provider_conversation_")),
        "the MCP catalog must expose one conversation surface"
    );
}

#[test]
fn conversation_catalog_defers_operation_id_requirement_until_route_selection() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let tools = server.tool_router.list_all();
    for name in ["conversation_load", "conversation_prompt"] {
        let tool = tools
            .iter()
            .find(|tool| tool.name == name)
            .expect("common tool");
        let required = tool.input_schema["required"]
            .as_array()
            .expect("required fields");
        assert!(
            !required.contains(&serde_json::json!("operationId")),
            "{name} selects its client before requiring an operation ID"
        );
    }
    let create = tools
        .iter()
        .find(|tool| tool.name == "conversation_create")
        .expect("create tool");
    assert!(
        create.input_schema["required"]
            .as_array()
            .expect("create required fields")
            .contains(&serde_json::json!("operationId"))
    );
    let convenience = tools
        .iter()
        .find(|tool| tool.name == "conversation_create_and_prompt")
        .expect("create and prompt tool");
    assert!(
        !convenience.input_schema["required"]
            .as_array()
            .expect("convenience required fields")
            .contains(&serde_json::json!("promptOperationId"))
    );
}

#[test]
fn tool_schemas_match_known_runtime_defaults_and_conditional_requirements() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let tools = server.resolved_tools();
    let schema_for = |name: &str| {
        tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("missing tool {name}"))
            .input_schema
            .as_ref()
            .clone()
            .into()
    };
    fn required_for(schema: &Value) -> &[Value] {
        schema["required"]
            .as_array()
            .unwrap_or_else(|| panic!("schema has no required list: {schema}"))
    }

    let create_schema = schema_for("conversation_create");
    assert!(!required_for(&create_schema).contains(&serde_json::json!("approver")));
    assert!(
        create_schema["allOf"].is_array(),
        "Codex create requirements must be conditional"
    );
    let create_and_prompt_schema = schema_for("conversation_create_and_prompt");
    assert!(
        create_and_prompt_schema["allOf"].is_array(),
        "Codex create-and-prompt requirements must be conditional"
    );
    let service_id = "00000000-0000-4000-8000-000000000001";
    let caller = serde_json::json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"schema-caller"
    });
    let provider_create = serde_json::json!({
        "operationId":"019f0000-0000-7000-8000-000000000201",
        "endpoint":{"serviceId":service_id,"endpointId":"cursor-local"},
        "workingDirectory":"/tmp/project","access":"workspace-write","createdBy":caller
    });
    let create_validator = jsonschema::validator_for(&create_schema).expect("create schema");
    assert!(create_validator.is_valid(&provider_create));
    let human = serde_json::json!({"kind":"human","humanId":"fixture-owner"});
    let mut provider_with_human = provider_create.clone();
    provider_with_human["createdBy"] = human.clone();
    provider_with_human["approver"] = human;
    assert!(create_validator.is_valid(&provider_with_human));
    let _: ConversationCreateToolRequest =
        serde_json::from_value(provider_with_human).expect("typed Human MCP create input");
    let mut provider_with_model = provider_create.clone();
    provider_with_model["model"] = serde_json::json!("gpt-5.6-sol");
    assert!(!create_validator.is_valid(&provider_with_model));
    let mut provider_with_effort = provider_create.clone();
    provider_with_effort["effort"] = serde_json::json!("medium");
    assert!(!create_validator.is_valid(&provider_with_effort));
    for (field, value) in [
        ("fork", serde_json::json!("source-thread")),
        (
            "rootMessageId",
            serde_json::json!("019f0000-0000-7000-8000-000000000301"),
        ),
    ] {
        let mut provider_with_codex_only_field = provider_create.clone();
        provider_with_codex_only_field[field] = value;
        assert!(
            !create_validator.is_valid(&provider_with_codex_only_field),
            "provider create schema must reject {field}"
        );
    }
    let mut empty_codex_create = provider_create.clone();
    empty_codex_create["endpoint"]["endpointId"] = serde_json::json!("codex-local");
    assert!(!create_validator.is_valid(&empty_codex_create));
    empty_codex_create["model"] = serde_json::json!("gpt-5.6-sol");
    empty_codex_create["effort"] = serde_json::json!("medium");
    assert!(create_validator.is_valid(&empty_codex_create));

    let create_and_prompt_validator =
        jsonschema::validator_for(&create_and_prompt_schema).expect("create-and-prompt schema");
    let provider_create_and_prompt = serde_json::json!({
        "create":provider_create,
        "message":{"kind":"humanUser","text":"hello"}
    });
    assert!(create_and_prompt_validator.is_valid(&provider_create_and_prompt));
    let mut provider_create_and_prompt_with_effort = provider_create_and_prompt;
    provider_create_and_prompt_with_effort["create"]["effort"] = serde_json::json!("medium");
    assert!(!create_and_prompt_validator.is_valid(&provider_create_and_prompt_with_effort));
    for (field, value) in [
        ("fork", serde_json::json!("source-thread")),
        (
            "rootMessageId",
            serde_json::json!("019f0000-0000-7000-8000-000000000302"),
        ),
    ] {
        let mut provider_create_and_prompt = serde_json::json!({
            "create":provider_create,
            "message":{"kind":"humanUser","text":"hello"}
        });
        provider_create_and_prompt["create"][field] = value;
        assert!(
            !create_and_prompt_validator.is_valid(&provider_create_and_prompt),
            "provider create-and-prompt schema must reject {field}"
        );
    }

    let schedule_schema = schema_for("schedule_create");
    let schedule_definition = &schedule_schema["$defs"]["ScheduleDefinition"];
    assert!(
        schedule_definition["required"]
            .as_array()
            .is_some_and(|fields| fields.contains(&serde_json::json!("effort"))),
        "{schedule_schema}"
    );
    assert!(schedule_definition["allOf"].is_array());
    let mut fresh_schedule = serde_json::json!({
        "operationId":"019f0000-0000-7000-8000-000000000202",
        "definition":{
            "instructionId":"019f0000-0000-7000-8000-000000000203",
            "timing":{"kind":"after","seconds":60},"enabled":false,
            "destination":{"kind":"freshEachRun","endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"cwd":"/tmp/project"},
            "effort":"medium"
        }
    });
    let schedule_validator = jsonschema::validator_for(&schedule_schema).expect("schedule schema");
    assert!(!schedule_validator.is_valid(&fresh_schedule));
    fresh_schedule["definition"]["model"] = serde_json::json!("gpt-5.6-sol");
    assert!(schedule_validator.is_valid(&fresh_schedule));

    let run_schema = schema_for("run_list");
    assert!(!required_for(&run_schema).contains(&serde_json::json!("cursor")));

    let wait = schema_for("board_thread_wait");
    let wait_required = required_for(&wait);
    for field in ["actor", "filter", "maxWaitSeconds"] {
        assert!(wait_required.contains(&serde_json::json!(field)));
    }
    assert!(wait["properties"].get("request").is_none());
    assert!(wait["properties"].get("timeoutSeconds").is_none());
    let descriptions = tools
        .iter()
        .map(|tool| {
            (
                tool.name.as_ref(),
                tool.description.as_deref().unwrap_or_default(),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let wait_description = descriptions
        .get("board_thread_wait")
        .expect("wait description");
    for phrase in ["poll-mode", "empty batch", "acknowledges"] {
        assert!(wait_description.contains(phrase), "{wait_description}");
    }
    let join_description = descriptions
        .get("board_thread_join")
        .expect("join description");
    assert!(join_description.contains("watch to true"));
    assert!(join_description.contains("mode and whenIdle"));

    for tool in ["conversation_create", "conversation_create_and_prompt"] {
        let description = descriptions
            .get(tool)
            .expect("conversation create description");
        assert!(
            description.contains("required for Codex endpoints"),
            "{description}"
        );
        assert!(
            description.contains("rejected for provider endpoints"),
            "{description}"
        );
        assert!(description.contains("rootMessageId"), "{description}");
        assert!(description.contains("fork"), "{description}");
    }
}

#[test]
fn thread_subscription_tools_expose_subscription_policy_and_join_options() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let tools = server.resolved_tools();
    let schema_for = |name: &str| {
        tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("missing tool {name}"))
    };
    let required_fields = |schema: &Value| -> Vec<Value> {
        schema["required"]
            .as_array()
            .unwrap_or_else(|| panic!("schema has no required array: {schema}"))
            .clone()
    };

    let subscribe = schema_for("board_thread_subscribe");
    let subscribe_schema = Value::Object((*subscribe.input_schema).clone());
    for field in ["actor", "scope", "policy"] {
        assert!(
            required_fields(&subscribe_schema).contains(&serde_json::json!(field)),
            "subscribe requires {field}"
        );
    }
    let policy_property = &subscribe_schema["properties"]["policy"];
    let policy = match policy_property["$ref"].as_str() {
        Some(policy_reference) => {
            let policy_path = policy_reference
                .strip_prefix('#')
                .expect("local subscription policy reference");
            subscribe_schema
                .pointer(policy_path)
                .expect("subscription policy schema")
        }
        None => policy_property,
    };
    for field in [
        "mode",
        "whenIdle",
        "quietSeconds",
        "capSeconds",
        "lifetimeSeconds",
    ] {
        assert!(
            policy["properties"].get(field).is_some(),
            "policy exposes {field}"
        );
    }

    let unsubscribe = schema_for("board_thread_unsubscribe");
    let unsubscribe_schema = Value::Object((*unsubscribe.input_schema).clone());
    for field in ["actor", "scope"] {
        assert!(required_fields(&unsubscribe_schema).contains(&serde_json::json!(field)));
    }

    let subscriptions = schema_for("board_thread_subscriptions");
    let subscriptions_schema = Value::Object((*subscriptions.input_schema).clone());
    assert!(required_fields(&subscriptions_schema).contains(&serde_json::json!("actor")));

    let join = schema_for("board_thread_join");
    let join_schema = Value::Object((*join.input_schema).clone());
    for field in ["mode", "whenIdle"] {
        assert!(
            join_schema["properties"].get(field).is_some(),
            "join exposes {field}"
        );
        assert!(
            !required_fields(&join_schema).contains(&serde_json::json!(field)),
            "join option {field} remains optional"
        );
    }

    for name in [
        "board_thread_subscribe",
        "board_thread_unsubscribe",
        "board_thread_subscriptions",
    ] {
        let tool = schema_for(name);
        assert!(
            tool.output_schema.is_some(),
            "{name} exposes its output wrapper"
        );
    }
}

#[test]
fn sdk_only_operations_are_advertised_by_both_cli_and_mcp_catalogs() {
    let help = agent_collaboration::command_help();
    for (tool, cli_command) in [
        ("conversation_create", "conversation create"),
        ("conversation_load", "conversation load"),
        ("conversation_create_and_prompt", "conversation prompt"),
        ("conversation_prompt", "conversation prompt"),
        ("conversation_cancel", "conversation cancel"),
        ("events_observe", "events observe"),
    ] {
        assert!(help.contains(cli_command), "CLI adapter missing for {tool}");
    }
}

#[test]
fn conversation_create_tool_preserves_created_and_pending_operation_identity() {
    let operation_id = OperationId::generate();
    let target = serde_json::from_value(serde_json::json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
        "sessionId":"created-thread"
    }))
    .expect("target");
    let created = conversation_create_tool_result(
        Ok(ConversationCreateOutcome::Created {
            operation_id: operation_id.clone(),
            target,
            effective_settings: None,
        }),
        operation_id.clone(),
    );
    assert_eq!(created.is_error, Some(false));
    assert_eq!(
        created.structured_content.as_ref().unwrap()["kind"],
        "created"
    );
    assert_eq!(
        created.structured_content.as_ref().unwrap()["operationId"],
        serde_json::json!(operation_id)
    );
    assert_eq!(
        created.structured_content.as_ref().unwrap()["target"]["sessionId"],
        "created-thread"
    );

    let pending = conversation_create_tool_result(
        Ok(ConversationCreateOutcome::Pending {
            operation_id: operation_id.clone(),
        }),
        operation_id.clone(),
    );
    assert_eq!(pending.is_error, Some(false));
    assert_eq!(
        pending.structured_content.as_ref().unwrap()["kind"],
        "pending"
    );
    assert_eq!(
        pending.structured_content.as_ref().unwrap()["operationId"],
        serde_json::json!(operation_id)
    );
    assert!(
        pending
            .structured_content
            .as_ref()
            .unwrap()
            .get("target")
            .is_none()
    );
}

#[test]
fn conversation_create_tool_reports_endpoint_named_unsupported_input() {
    let operation_id = OperationId::generate();
    let endpoint = serde_json::from_value(serde_json::json!({
        "serviceId":"00000000-0000-4000-8000-000000000001", "endpointId":"cursor-local"
    }))
    .expect("endpoint");
    let response = conversation_create_tool_result(
        Err(
            collaboration_client::ConversationClientError::UnsupportedInput {
                endpoint,
                field: "model",
                fix: "omit model for cursor-local".to_owned(),
            },
        ),
        operation_id.clone(),
    );
    assert_eq!(response.is_error, Some(true));
    let content = response.structured_content.expect("structured error");
    assert_eq!(content["kind"], "unsupportedCapability");
    assert_eq!(content["operationId"], serde_json::json!(operation_id));
    assert_eq!(content["endpoint"]["endpointId"], "cursor-local");
    assert_eq!(content["field"], "model");
    assert_eq!(content["fix"], "omit model for cursor-local");
}

#[test]
fn representative_catalog_descriptions_explain_operation_specific_behavior() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let server = CollaborationMcpServer::new(temporary.path().to_owned());
    let descriptions = server
        .resolved_tools()
        .into_iter()
        .map(|tool| {
            (
                tool.name.into_owned(),
                tool.description.unwrap_or_default().into_owned(),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    for description in descriptions.values() {
        assert!(!description.contains("Calls the existing typed"));
        assert!(!description.contains("Undocumented collaboration operation"));
    }
    for (name, required_phrases) in [
        ("endpoints_list", &["Read-only"][..]),
        (
            "message_send",
            &["accepted", "replayed", "reported", "not authenticated"][..],
        ),
        (
            "router_show",
            &["reported", "not authenticated", "confusion guard", "read"][..],
        ),
        (
            "message_inbox",
            &["reported", "not authenticated", "confusion guard", "unread"][..],
        ),
        (
            "message_history",
            &[
                "reported",
                "not authenticated",
                "confusion guard",
                "between",
            ][..],
        ),
        (
            "message_reply",
            &[
                "reported",
                "not authenticated",
                "confusion guard",
                "reference",
            ][..],
        ),
        (
            "conversation_create_and_prompt",
            &["advertised client", "completed turn", "peer reply"][..],
        ),
        (
            "wake_send",
            &[
                "required operation identity",
                "local scheduling",
                "native input acceptance",
                "agent reply",
            ][..],
        ),
        (
            "schedule_prepare",
            &[
                "Preparation",
                "reuseThread",
                "allocate a native conversation",
                "ReadThread",
                "without loading or resuming",
                "native input acceptance",
            ][..],
        ),
        ("instruction_create", &["required operation identity"][..]),
        ("wake_pause", &["required operation identity"][..]),
        (
            "board_inbox_fetch",
            &[
                "Unread mode initializes reader tracking",
                "Latest mode does not initialize",
            ][..],
        ),
        (
            "board_message_post",
            &["Saving the message", "reply", "assignment success"][..],
        ),
        (
            "board_thread_wait",
            &["poll-mode", "empty batch", "acknowledges"][..],
        ),
        (
            "board_thread_subscribe",
            &[
                "Creates, updates or reactivates",
                "thread or topic",
                "policy",
            ][..],
        ),
        (
            "board_thread_unsubscribe",
            &["cancelled", "watch remains active", "inbox"][..],
        ),
        (
            "board_thread_subscriptions",
            &[
                "active or draining",
                "pending counts",
                "does not acknowledge",
            ][..],
        ),
        ("operation_reconcile", &["never replays"][..]),
    ] {
        let description = descriptions
            .get(name)
            .unwrap_or_else(|| panic!("missing tool {name}"));
        for phrase in required_phrases {
            assert!(
                description.contains(phrase),
                "{name} description missing {phrase:?}: {description}"
            );
        }
    }
}
