//! The board tools' request, result and error types, as their published JSON Schemas.
use serde_json::json;

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

/// The JSON Schema a board tool publishes for `TWire`, as a validator.
fn tool_schema_validator<TWire: schemars::JsonSchema>()
-> Result<jsonschema::Validator, Box<dyn std::error::Error>> {
    let schema = serde_json::to_value(schemars::schema_for!(TWire))?;
    Ok(jsonschema::validator_for(&schema)?)
}

#[test]
fn board_requests_and_results_match_their_tool_types() -> Result<(), Box<dyn std::error::Error>> {
    let project_id = "01890f2e-7b4c-7cc0-98c4-000000000001";
    let request = json!({"projectId": project_id});
    let result = json!({"project": {"projectId": project_id, "name": "Router", "description": "Coordination"}});
    ensure(
        tool_schema_validator::<message_board::ProjectShowRequest>()?.is_valid(&request),
        "valid board request",
    )?;
    ensure(
        tool_schema_validator::<message_board::ProjectShowResult>()?.is_valid(&result),
        "valid board result",
    )?;
    Ok(())
}

#[test]
fn board_variants_and_nested_records_fail_closed() -> Result<(), Box<dyn std::error::Error>> {
    let validator = tool_schema_validator::<message_board::MessageListRequest>()?;
    let topic_id = "01890f2e-7b4c-7cc0-98c4-000000000002";
    let valid = json!({
        "scope": {"kind": "topic", "topicId": topic_id},
        "selection": {"kind": "latest"},
        "page": {"limit": 50, "cursor": null}
    });
    ensure(validator.is_valid(&valid), "valid message-list request")?;

    let mut bad_variant = valid.clone();
    bad_variant["scope"]["kind"] = json!("topicAlias");
    ensure(
        !validator.is_valid(&bad_variant),
        "unknown scope kind rejected",
    )?;

    let mut extra_variant_field = valid.clone();
    extra_variant_field["scope"]["boardId"] = json!(topic_id);
    ensure(
        !validator.is_valid(&extra_variant_field),
        "extra scope field rejected",
    )?;

    let mut extra_request_field = valid;
    extra_request_field["reader"] = json!({"kind": "human", "humanId": "owner"});
    ensure(
        !validator.is_valid(&extra_request_field),
        "extra request field rejected",
    )?;
    Ok(())
}

#[test]
fn thread_wait_contract_uses_subscription_filters_and_bodyless_results()
-> Result<(), Box<dyn std::error::Error>> {
    let request_validator =
        tool_schema_validator::<collaboration_protocol::ThreadSubscriptionWaitRequest>()?;
    let response_validator =
        tool_schema_validator::<collaboration_protocol::ThreadSubscriptionWaitResult>()?;
    let root_id = "01900000-0000-7000-8000-000000000021";
    let topic_id = "01900000-0000-7000-8000-000000000022";
    let valid_request = json!({
        "actor":{"kind":"human","humanId":"reader"},
        "filter":{"kind":"roots","rootMessageIds":[root_id]},
        "maxWaitSeconds":540
    });
    ensure(
        request_validator.is_valid(&valid_request),
        "valid subscription wait request",
    )?;
    let mut unknown_filter = valid_request.clone();
    unknown_filter["filter"]["kind"] = json!("topics");
    ensure(
        !request_validator.is_valid(&unknown_filter),
        "unknown subscription wait filter rejected",
    )?;
    let mut mixed_filter = valid_request.clone();
    mixed_filter["filter"]["topicId"] = json!(topic_id);
    ensure(
        !request_validator.is_valid(&mixed_filter),
        "extra filter fields rejected",
    )?;
    let mut excessive_wait = valid_request;
    excessive_wait["maxWaitSeconds"] = json!(1501);
    ensure(
        !request_validator.is_valid(&excessive_wait),
        "wait limit above 1500 seconds rejected",
    )?;

    let root_range = json!({
        "rootId":root_id,
        "topicId":topic_id,
        "fromSequence":1,
        "throughSequence":2,
        "messageCount":2
    });
    let timeout_response = json!({"batch":null});
    ensure(
        response_validator.is_valid(&timeout_response),
        "empty wait result accepted",
    )?;
    let notice_response = json!({"batch":{
        "kind":"notice",
        "pushId":"01900000-0000-7000-8000-000000000023",
        "line":"thread subscription notice",
        "held":false,
        "draining":false,
        "roots":[root_range]
    }});
    ensure(
        response_validator.is_valid(&notice_response),
        "session notice response accepted",
    )?;
    let ranges_response = json!({"batch":{
        "kind":"ranges",
        "held":true,
        "heldSince":"2026-10-01T00:00:00Z",
        "draining":false,
        "roots":[root_range]
    }});
    ensure(
        response_validator.is_valid(&ranges_response),
        "human range response accepted",
    )?;
    let mut body_bearing_notice = notice_response;
    body_bearing_notice["batch"]["roots"][0]["text"] = json!("message body");
    ensure(
        !response_validator.is_valid(&body_bearing_notice),
        "wait result rejects message bodies",
    )?;
    Ok(())
}

#[test]
fn board_errors_are_closed_and_overloaded_is_a_board_error()
-> Result<(), Box<dyn std::error::Error>> {
    let board_error_validator = tool_schema_validator::<message_board::BoardError>()?;
    let overloaded = json!({
        "kind": "overloaded",
        "stage": "admission",
        "message": "Board service is overloaded. Retry later.",
        "nextAction": "retryLater",
        "details": {"kind": "none"}
    });
    ensure(
        board_error_validator.is_valid(&overloaded),
        "overloaded board error schema",
    )?;

    let mut bad_kind = overloaded.clone();
    bad_kind["kind"] = json!("busy");
    ensure(
        !board_error_validator.is_valid(&bad_kind),
        "unknown board error kind rejected by schema",
    )?;

    let mut extra_data = overloaded;
    extra_data["retryAfterSeconds"] = json!(1);
    ensure(
        !board_error_validator.is_valid(&extra_data),
        "extra board error field rejected by schema",
    )?;
    Ok(())
}
