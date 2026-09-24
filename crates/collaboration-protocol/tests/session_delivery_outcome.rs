use collaboration_protocol::{
    AttemptId, DeliveryCorrelationId, DeliveryNextAction, DeliveryOutcome, DeliveryRejection,
    DeliveryRejectionReason, SessionReachability,
};
use serde_json::{json, to_value};

#[test]
fn delivery_outcomes_and_reachability_round_trip_without_losing_strength() {
    for (outcome, expected) in [
        (DeliveryOutcome::Started, json!({"kind": "started"})),
        (DeliveryOutcome::Steered, json!({"kind": "steered"})),
        (
            DeliveryOutcome::StartedOrSteered,
            json!({"kind": "startedOrSteered"}),
        ),
        (DeliveryOutcome::Queued, json!({"kind": "queued"})),
        (
            DeliveryOutcome::PeerMessageWritten,
            json!({"kind": "peerMessageWritten"}),
        ),
        (
            DeliveryOutcome::NotSubmitted {
                retryable: true,
                reason: "busy".to_owned(),
            },
            json!({"kind": "notSubmitted", "retryable": true, "reason": "busy"}),
        ),
        (
            DeliveryOutcome::Rejected(DeliveryRejection {
                reason: DeliveryRejectionReason::UnsupportedCapability,
                next_action: DeliveryNextAction::CorrectRequest,
                client_code: Some(-32601),
                detail: Some("unsupported".to_owned()),
            }),
            json!({"kind": "rejected", "reason": "unsupportedCapability", "nextAction":"correctRequest", "clientCode":-32601, "detail":"unsupported"}),
        ),
        (DeliveryOutcome::Unknown, json!({"kind": "unknown"})),
    ] {
        let encoded = to_value(&outcome).expect("outcome encodes");
        assert_eq!(encoded, expected);
        let decoded: DeliveryOutcome = serde_json::from_value(encoded).expect("outcome decodes");
        assert_eq!(decoded, outcome);
    }

    for reachability in [
        SessionReachability::CodexAppServer,
        SessionReachability::ProviderAcp,
        SessionReachability::ClaudeCodePeer,
    ] {
        let encoded = to_value(reachability).expect("reachability encodes");
        let decoded: SessionReachability =
            serde_json::from_value(encoded).expect("reachability decodes");
        assert_eq!(decoded, reachability);
    }
}

#[test]
fn delivery_correlation_accepts_caller_values_and_allocates_uuidv7_values() {
    let attempt = AttemptId::generate();
    let correlation = DeliveryCorrelationId::generate();
    assert_ne!(attempt.as_str(), correlation.as_str());
    assert!(AttemptId::try_from(correlation.as_str().to_owned()).is_ok());
    assert!(AttemptId::try_from("not-a-uuid".to_owned()).is_err());
    let caller = DeliveryCorrelationId::try_from("caller-correlation".to_owned())
        .expect("caller correlation remains valid");
    assert_eq!(caller.as_str(), "caller-correlation");
    assert_eq!(
        serde_json::to_value(&caller).expect("encode"),
        "caller-correlation"
    );
    let decoded: DeliveryCorrelationId =
        serde_json::from_value(serde_json::json!("caller-correlation")).expect("decode");
    assert_eq!(decoded, caller);
    for invalid in [
        String::new(),
        "bad\0correlation".to_owned(),
        "x".repeat(4097),
    ] {
        assert!(DeliveryCorrelationId::try_from(invalid).is_err());
    }
}
