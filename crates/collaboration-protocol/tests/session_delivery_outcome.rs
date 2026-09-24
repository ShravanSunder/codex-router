use collaboration_protocol::{
    AttemptId, DeliveryCorrelationId, DeliveryOutcome, SessionReachability,
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
            DeliveryOutcome::Rejected {
                reason: "unsupported".to_owned(),
            },
            json!({"kind": "rejected", "reason": "unsupported"}),
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
        SessionReachability::None,
    ] {
        let encoded = to_value(reachability).expect("reachability encodes");
        let decoded: SessionReachability =
            serde_json::from_value(encoded).expect("reachability decodes");
        assert_eq!(decoded, reachability);
    }
}

#[test]
fn delivery_ids_are_distinct_canonical_uuidv7_values() {
    let attempt = AttemptId::generate();
    let correlation = DeliveryCorrelationId::generate();
    assert_ne!(attempt.as_str(), correlation.as_str());
    assert!(AttemptId::try_from(correlation.as_str().to_owned()).is_ok());
    assert!(AttemptId::try_from("not-a-uuid".to_owned()).is_err());
    assert!(
        DeliveryCorrelationId::try_from("00000000-0000-4000-8000-000000000000".to_owned()).is_err()
    );
}
