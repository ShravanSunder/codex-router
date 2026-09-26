use collaboration_protocol::{
    ApprovalDecideParams, ApprovalDecision, ApprovalListParams, ApprovalOptionView,
};
use message_board::Identity;
use serde_json::json;

#[test]
fn legacy_decision_actor_and_list_request_remain_readable() {
    let service = "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89";
    let old_actor = json!({
        "endpoint":{"serviceId":service,"endpointId":"codex-local"},
        "sessionId":"approver"
    });
    let request: ApprovalDecideParams = serde_json::from_value(json!({
        "requestId":"request-1","decision":"allow","actor":old_actor
    }))
    .expect("old decide request");
    assert_eq!(request.decision, Some(ApprovalDecision::Allow));
    assert!(request.option_id.is_none());
    assert!(!request.acknowledge_persistent);
    assert!(matches!(request.actor, Identity::Session { .. }));
    let list: ApprovalListParams =
        serde_json::from_value(json!({"pending":true})).expect("old list request");
    assert!(!list.include_options);
}

#[test]
fn offered_option_decision_supports_human_and_explicit_acknowledgement() {
    let request: ApprovalDecideParams = serde_json::from_value(json!({
        "requestId":"request-2", "optionId":"allow-always",
        "actor":{"kind":"human","humanId":"owner"},
        "acknowledgePersistent":true
    }))
    .expect("new decide request");
    assert_eq!(request.option_id.as_deref(), Some("allow-always"));
    assert!(request.decision.is_none());
    assert!(request.acknowledge_persistent);
    assert!(matches!(request.actor, Identity::Human { .. }));
    let list: ApprovalListParams =
        serde_json::from_value(json!({"pending":true,"includeOptions":true}))
            .expect("detailed list request");
    assert!(list.include_options);
}

#[test]
fn detailed_option_names_the_persistent_target() {
    let option: ApprovalOptionView = serde_json::from_value(json!({
        "optionId":"allow-always", "label":"Always allow",
        "effect":"allow", "scope":"persistent", "persistentTarget":"Cursor allowlist"
    }))
    .expect("detailed option");
    let encoded = serde_json::to_value(option).expect("encode option");
    assert_eq!(encoded["persistentTarget"], "Cursor allowlist");
}
