use communication_protocol::{control_error_is_valid, control_schema_document};
use serde_json::{Value, json};

const BOARD_METHODS: [&str; 27] = [
    "board/projectCreate",
    "board/projectUpdate",
    "board/projectShow",
    "board/projectList",
    "board/repositoryAttach",
    "board/repositoryDetach",
    "board/repositoryList",
    "board/create",
    "board/update",
    "board/show",
    "board/list",
    "board/archive",
    "board/topicCreate",
    "board/topicUpdate",
    "board/topicList",
    "board/messagePost",
    "board/messageShow",
    "board/messageList",
    "board/threadShow",
    "board/threadResolve",
    "board/threadUnresolve",
    "board/threadWatch",
    "board/threadUnwatch",
    "board/threadList",
    "board/inboxFetch",
    "board/inboxAcknowledge",
    "board/inboxProjects",
];

#[derive(Debug)]
struct TestContractError(&'static str);

impl std::fmt::Display for TestContractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}
impl std::error::Error for TestContractError {}

fn ensure(condition: bool, message: &'static str) -> Result<(), TestContractError> {
    if condition {
        Ok(())
    } else {
        Err(TestContractError(message))
    }
}

fn board_schema() -> Result<Value, serde_json::Error> {
    control_schema_document(None)
}

fn method_schema(
    document: &Value,
    method: &str,
    pairing: &str,
) -> Result<Value, TestContractError> {
    let mut schema = document.clone();
    let reference = document
        .get("x-methods")
        .and_then(|methods| methods.get(method))
        .and_then(|contract| contract.get(pairing))
        .and_then(|pairing| pairing.get("$ref"))
        .cloned()
        .ok_or(TestContractError("method pairing reference"))?;
    schema
        .as_object_mut()
        .ok_or(TestContractError("schema object"))?
        .insert("$ref".to_owned(), reference);
    Ok(schema)
}

#[test]
fn all_board_methods_publish_resolved_frame_contracts() -> Result<(), Box<dyn std::error::Error>> {
    let schema = board_schema()?;
    let methods = schema["x-methods"]
        .as_object()
        .ok_or(TestContractError("x-methods object"))?;
    let registered_board_method_count = methods
        .keys()
        .filter(|method| method.starts_with("board/"))
        .count();
    ensure(
        registered_board_method_count == BOARD_METHODS.len(),
        "board method count",
    )?;
    ensure(
        schema["x-maxFrameUtf8Bytes"] == 1_048_576,
        "Control frame byte limit",
    )?;

    let frame_variants = schema["anyOf"]
        .as_array()
        .ok_or(TestContractError("frame variants"))?;
    for method in BOARD_METHODS {
        ensure(methods.contains_key(method), "missing board method")?;
        for pairing in ["params", "result", "request", "response", "error"] {
            let reference = methods
                .get(method)
                .and_then(|contract| contract.get(pairing))
                .and_then(|pairing| pairing.get("$ref"))
                .and_then(Value::as_str)
                .ok_or(TestContractError("method pairing reference"))?;
            ensure(
                schema.pointer(reference.trim_start_matches('#')).is_some(),
                "unresolved board method pairing",
            )?;
            if matches!(pairing, "request" | "response" | "error") {
                ensure(
                    frame_variants
                        .iter()
                        .any(|variant| variant["$ref"] == reference),
                    "board method pairing absent from frame union",
                )?;
            }
        }
    }
    Ok(())
}

#[test]
fn board_requests_and_results_are_real_control_frame_types()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = board_schema()?;
    let validator = jsonschema::validator_for(&schema)?;
    let project_id = "01890f2e-7b4c-7cc0-98c4-000000000001";
    let request = json!({
        "jsonrpc": "2.0",
        "id": "board-show-1",
        "method": "board/projectShow",
        "params": {"projectId": project_id}
    });
    let response = json!({
        "jsonrpc": "2.0",
        "id": "board-show-1",
        "result": {"project": {"projectId": project_id, "name": "Router", "description": "Coordination"}}
    });
    ensure(validator.is_valid(&request), "valid board request frame")?;
    ensure(validator.is_valid(&response), "valid board response frame")?;

    let request_validator =
        jsonschema::validator_for(&method_schema(&schema, "board/projectShow", "request")?)?;
    let mut wrong_method = request;
    wrong_method["method"] = json!("board/projectList");
    ensure(
        !request_validator.is_valid(&wrong_method),
        "method-specific request rejects another method",
    )?;
    Ok(())
}

#[test]
fn board_variants_and_nested_records_fail_closed() -> Result<(), Box<dyn std::error::Error>> {
    let schema = board_schema()?;
    let validator =
        jsonschema::validator_for(&method_schema(&schema, "board/messageList", "request")?)?;
    let topic_id = "01890f2e-7b4c-7cc0-98c4-000000000002";
    let valid = json!({
        "jsonrpc": "2.0",
        "id": "message-list-1",
        "method": "board/messageList",
        "params": {
            "scope": {"kind": "topic", "topicId": topic_id},
            "selection": {"kind": "latest"},
            "page": {"limit": 50, "cursor": null}
        }
    });
    ensure(validator.is_valid(&valid), "valid message-list request")?;

    let mut bad_variant = valid.clone();
    bad_variant["params"]["scope"]["kind"] = json!("topicAlias");
    ensure(
        !validator.is_valid(&bad_variant),
        "unknown scope kind rejected",
    )?;

    let mut extra_variant_field = valid.clone();
    extra_variant_field["params"]["scope"]["boardId"] = json!(topic_id);
    ensure(
        !validator.is_valid(&extra_variant_field),
        "extra scope field rejected",
    )?;

    let mut extra_request_field = valid;
    extra_request_field["params"]["reader"] = json!({"kind": "human", "humanId": "owner"});
    ensure(
        !validator.is_valid(&extra_request_field),
        "extra request field rejected",
    )?;
    Ok(())
}

#[test]
fn board_errors_are_closed_and_overloaded_is_method_valid() -> Result<(), Box<dyn std::error::Error>>
{
    let schema = board_schema()?;
    let board_error_validator =
        jsonschema::validator_for(&method_schema(&schema, "board/messagePost", "error")?)?;
    let overloaded = json!({
        "jsonrpc": "2.0",
        "id": "message-post-1",
        "error": {
            "code": -32050,
            "message": "Board service is overloaded",
            "data": {
                "kind": "overloaded",
                "stage": "admission",
                "message": "Board service is overloaded. Retry later.",
                "nextAction": "retryLater",
                "details": {"kind": "none"}
            }
        }
    });
    ensure(
        board_error_validator.is_valid(&overloaded),
        "overloaded board error schema",
    )?;
    ensure(
        control_error_is_valid("board/messagePost", &overloaded),
        "overloaded board error runtime validation",
    )?;
    ensure(
        !control_error_is_valid("unknown/boardMethod", &overloaded),
        "unknown method rejected",
    )?;

    let mut bad_kind = overloaded.clone();
    bad_kind["error"]["data"]["kind"] = json!("busy");
    ensure(
        !board_error_validator.is_valid(&bad_kind),
        "unknown board error kind rejected by schema",
    )?;
    ensure(
        !control_error_is_valid("board/messagePost", &bad_kind),
        "unknown board error kind rejected by runtime validator",
    )?;

    let mut extra_data = overloaded;
    extra_data["error"]["data"]["retryAfterSeconds"] = json!(1);
    ensure(
        !board_error_validator.is_valid(&extra_data),
        "extra board error field rejected by schema",
    )?;
    ensure(
        !control_error_is_valid("board/messagePost", &extra_data),
        "extra board error field rejected by runtime validator",
    )?;
    Ok(())
}
