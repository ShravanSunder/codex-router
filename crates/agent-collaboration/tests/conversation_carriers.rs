use collaboration_client::protocol::{ConversationRecord, SessionRef};
use collaboration_client::{
    ClientError, ConversationCreatePromptResult, ConversationEnd, OperationEffect,
    OperationFailure, OperationFailureKind,
};
use serde_json::json;

#[expect(clippy::expect_used, reason = "fixed valid test fixture")]
fn fixture_target(session_id: &str) -> SessionRef {
    serde_json::from_value(json!({
        "endpoint":{"serviceId":"00000000-0000-4000-0000-000000000001","endpointId":"codex-local"},
        "sessionId":session_id
    }))
    .expect("valid fixture target")
}

#[test]
fn cli_conversation_carrier_preserves_permission_required_and_approver_identity() {
    let target = fixture_target("created-thread");
    let permission_record = ConversationRecord::PermissionRequired {
        target: target.clone(),
    };
    assert_eq!(
        serde_json::to_value(permission_record).unwrap_or_default()["kind"],
        "permissionRequired"
    );

    let result = ConversationCreatePromptResult {
        target,
        end: ConversationEnd::Cancelled,
        updates: vec![],
        permission_required: true,
        result: Some(json!({"stopReason":"cancelled"})),
    };
    let encoded = serde_json::to_value(result).unwrap_or_default();
    assert_eq!(encoded["permissionRequired"], true);
    assert_eq!(encoded["end"], "cancelled");
}

#[test]
fn cli_conversation_carrier_preserves_busy_rejection_and_unknown_effect() {
    let failure = OperationFailure::from_client_error(
        ClientError::Rejected {
            code: -32050,
            data: Some(json!({
                "kind":"nativeRejected",
                "stage":"prompt",
                "reason":"busy",
                "nextAction":"inspectTarget"
            })),
        },
        OperationEffect::Unknown,
    );
    assert_eq!(failure.kind, OperationFailureKind::Rejected);
    assert_eq!(failure.service_kind.as_deref(), Some("nativeRejected"));
    assert_eq!(failure.stage, "prompt");
    assert_eq!(failure.effect, OperationEffect::Unknown);
    let null = serde_json::Value::Null;
    let data = failure.data.as_ref().unwrap_or(&null);
    assert_eq!(data["reason"], "busy");
    assert_eq!(data["nextAction"], "inspectTarget");
}
