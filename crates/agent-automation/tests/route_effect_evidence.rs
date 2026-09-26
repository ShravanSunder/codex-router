use agent_automation::{
    AttemptOutcome, CessationEvidence, ClaudeCodePeerEffectEvidence, DeliveryAttempt,
    NativeEffectEvidence, PeerSessionReference, PeerWriteEffect, PreparationEffect,
    ProviderAcpEffectEvidence, ProviderBindingReference, ProviderSettlementEffect,
    RouteEffectEvidence, RouteSettlementState, RunExecutionEvidence, SubmissionEffect,
};
use serde_json::{Value, json};

fn legacy_native() -> Value {
    json!({
        "target": "codex-thread",
        "generation": "generation-1",
        "clientUserMessageId": "message-1",
        "nativeTurnId": "turn-1",
        "nativeSubmissionId": null,
        "allocation": "accepted",
        "resume": "notRequested",
        "submission": "accepted",
        "cessation": "confirmed"
    })
}

#[test]
fn legacy_wake_evidence_decodes_as_codex_app_server() {
    let decoded: RouteEffectEvidence<String, String> =
        serde_json::from_value(legacy_native()).expect("legacy native evidence decodes");
    let RouteEffectEvidence::CodexAppServer(native) = decoded else {
        panic!("legacy native evidence must retain its client identity");
    };
    assert_eq!(native.target.as_deref(), Some("codex-thread"));
    assert_eq!(native.generation.as_deref(), Some("generation-1"));
    assert_eq!(native.submission, SubmissionEffect::Accepted);
    assert_eq!(native.cessation, CessationEvidence::Confirmed);
    assert_eq!(native.allocation, PreparationEffect::Accepted);
}

#[test]
fn legacy_run_field_decodes_and_reencodes_with_route_tag() {
    let legacy = json!({"native": legacy_native(), "timing": null, "acceptance": null});
    let decoded: RunExecutionEvidence<String, String, String> =
        serde_json::from_value(legacy).expect("legacy run evidence decodes");
    assert!(matches!(
        decoded.route,
        Some(RouteEffectEvidence::CodexAppServer(_))
    ));
    let encoded = serde_json::to_value(decoded).expect("new route evidence encodes");
    assert_eq!(encoded["route"]["kind"], "codexAppServer");
    assert!(encoded.get("native").is_none());
}

#[test]
fn waiting_run_has_no_selected_route_until_first_effect() {
    let waiting = RunExecutionEvidence::<String, String, String> {
        route: None,
        timing: None,
        acceptance: None,
    };
    let encoded = serde_json::to_value(&waiting).expect("waiting run encodes");
    assert_eq!(encoded["route"], Value::Null);
    let decoded: RunExecutionEvidence<String, String, String> =
        serde_json::from_value(encoded).expect("waiting run decodes");
    assert!(decoded.route.is_none());
}

#[test]
fn tagged_codex_evidence_round_trips_native_fields() {
    let native: NativeEffectEvidence<String, String> =
        serde_json::from_value(legacy_native()).expect("native fields decode");
    let route = RouteEffectEvidence::CodexAppServer(native);
    let encoded = serde_json::to_value(&route).expect("tagged route evidence encodes");
    assert_eq!(encoded["kind"], "codexAppServer");
    let decoded: RouteEffectEvidence<String, String> =
        serde_json::from_value(encoded.clone()).expect("tagged route evidence decodes");
    assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
}

#[test]
fn router_queued_evidence_is_provider_only() {
    let mut native = legacy_native();
    native["submission"] = json!("routerQueued");
    assert!(serde_json::from_value::<RouteEffectEvidence<String, String>>(native.clone()).is_err());
    native["kind"] = json!("codexAppServer");
    assert!(serde_json::from_value::<RouteEffectEvidence<String, String>>(native).is_err());

    let provider = RouteEffectEvidence::<String, String>::ProviderAcp(ProviderAcpEffectEvidence {
        target: "claude-thread".to_owned(),
        generation: "generation-2".to_owned(),
        binding: ProviderBindingReference::try_from("binding-1".to_owned()).expect("binding"),
        attempt_id: agent_automation::AttemptId::generate(),
        submission: SubmissionEffect::RouterQueued,
        settlement: ProviderSettlementEffect::NotObserved,
    });
    let encoded = serde_json::to_value(&provider).expect("provider queue encodes");
    assert_eq!(encoded["submission"], "routerQueued");
    assert!(serde_json::from_value::<RouteEffectEvidence<String, String>>(encoded).is_ok());
}

#[test]
fn provider_and_peer_evidence_round_trip_as_separate_routes() {
    let provider = RouteEffectEvidence::<String, String>::ProviderAcp(ProviderAcpEffectEvidence {
        target: "claude-thread".to_owned(),
        generation: "generation-2".to_owned(),
        binding: ProviderBindingReference::try_from("binding-1".to_owned()).unwrap(),
        attempt_id: agent_automation::AttemptId::generate(),
        submission: SubmissionEffect::Accepted,
        settlement: ProviderSettlementEffect::StopRequested,
    });
    let encoded = serde_json::to_value(&provider).expect("provider evidence encodes");
    assert_eq!(encoded["kind"], "providerAcp");
    assert_eq!(encoded["settlement"], "stopRequested");
    let decoded: RouteEffectEvidence<String, String> =
        serde_json::from_value(encoded.clone()).expect("provider evidence decodes");
    assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);

    let peer =
        RouteEffectEvidence::<String, String>::ClaudeCodePeer(ClaudeCodePeerEffectEvidence {
            session_id: PeerSessionReference::try_from("peer-thread".to_owned()).unwrap(),
            process_id: 42.try_into().unwrap(),
            write: PeerWriteEffect::Written,
        });
    let encoded = serde_json::to_value(&peer).expect("peer evidence encodes");
    assert_eq!(encoded["kind"], "claudeCodePeer");
    assert_eq!(encoded["write"], "written");
    let decoded: RouteEffectEvidence<String, String> =
        serde_json::from_value(encoded.clone()).expect("peer evidence decodes");
    assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
}

#[test]
fn route_settlement_distinguishes_turn_operation_and_write_evidence() {
    let native: NativeEffectEvidence<String, String> =
        serde_json::from_value(legacy_native()).expect("native evidence decodes");
    assert_eq!(
        RouteEffectEvidence::CodexAppServer(native).settlement_state(),
        RouteSettlementState::NativeTurnConfirmed
    );

    let mut provider_effect = ProviderAcpEffectEvidence {
        target: "claude-thread".to_owned(),
        generation: "generation-2".to_owned(),
        binding: "binding-1".to_owned().try_into().expect("valid binding"),
        attempt_id: agent_automation::AttemptId::generate(),
        submission: SubmissionEffect::Accepted,
        settlement: ProviderSettlementEffect::StopRequested,
    };
    let provider = RouteEffectEvidence::<String, String>::ProviderAcp(provider_effect.clone());
    assert_eq!(provider.settlement_state(), RouteSettlementState::Pending);
    provider_effect.settlement = ProviderSettlementEffect::Confirmed;
    assert_eq!(
        RouteEffectEvidence::ProviderAcp(provider_effect).settlement_state(),
        RouteSettlementState::ProviderOperationConfirmed
    );

    let peer =
        RouteEffectEvidence::<String, String>::ClaudeCodePeer(ClaudeCodePeerEffectEvidence {
            session_id: "peer-thread".to_owned().try_into().expect("valid session"),
            process_id: 42.try_into().expect("valid process"),
            write: PeerWriteEffect::Written,
        });
    assert_eq!(
        peer.settlement_state(),
        RouteSettlementState::PeerMessageWritten
    );
}

#[test]
fn provider_binding_reference_rejects_invalid_stored_ids() {
    assert!(ProviderBindingReference::try_from(String::new()).is_err());
    assert!(ProviderBindingReference::try_from("x".repeat(257)).is_err());
    assert!(ProviderBindingReference::try_from("bad\0binding".to_owned()).is_err());
}

#[test]
fn peer_session_reference_rejects_invalid_stored_ids() {
    assert!(PeerSessionReference::try_from(String::new()).is_err());
    assert!(PeerSessionReference::try_from("x".repeat(4097)).is_err());
    assert!(PeerSessionReference::try_from("bad\0session".to_owned()).is_err());
}

#[test]
fn provider_stored_evidence_rejects_settlement_without_admission() {
    let invalid = json!({
        "kind":"providerAcp",
        "target":"claude-thread",
        "generation":"generation-2",
        "binding":"binding-1",
        "attemptId":agent_automation::AttemptId::generate(),
        "submission":"dispatching",
        "settlement":"confirmed"
    });
    assert!(serde_json::from_value::<RouteEffectEvidence<String, String>>(invalid).is_err());
}

#[test]
fn stored_attempt_cannot_be_accepted_without_a_selected_route() {
    let invalid = json!({
        "attemptId":agent_automation::AttemptId::generate(),
        "attemptNumber":1,
        "startedAtMs":1000,
        "completedAtMs":2000,
        "discardOnNonSubmission":false,
        "effects":null,
        "outcome":{"kind":"accepted"}
    });
    assert!(serde_json::from_value::<DeliveryAttempt<String, String>>(invalid).is_err());
}

#[test]
fn stored_run_cannot_have_dispatch_budget_without_a_selected_route() {
    let invalid = json!({
        "route":null,
        "timing":{
            "dispatchStartedAtMs":1000,
            "effectiveTimeoutSeconds":60,
            "deadlineAtMs":61000
        },
        "acceptance":null
    });
    assert!(
        serde_json::from_value::<RunExecutionEvidence<String, String, String>>(invalid).is_err()
    );
}

#[test]
fn claimed_attempt_has_no_route_and_legacy_attempt_keeps_native_evidence() {
    let attempt_id = agent_automation::AttemptId::generate();
    let legacy = json!({
        "attemptId": attempt_id,
        "attemptNumber": 1,
        "startedAtMs": 1000,
        "completedAtMs": null,
        "discardOnNonSubmission": false,
        "effects": legacy_native(),
        "outcome": {"kind": "inProgress"}
    });
    let decoded: DeliveryAttempt<String, String> =
        serde_json::from_value(legacy).expect("old attempt decodes");
    assert!(matches!(
        decoded.effects,
        Some(RouteEffectEvidence::CodexAppServer(_))
    ));

    let unselected = DeliveryAttempt::<String, String> {
        attempt_id: agent_automation::AttemptId::generate(),
        attempt_number: 1,
        started_at_ms: 1000,
        completed_at_ms: None,
        discard_on_non_submission: false,
        effects: None,
        outcome: AttemptOutcome::InProgress,
    };
    let encoded = serde_json::to_value(&unselected).expect("claimed attempt encodes");
    assert_eq!(encoded["effects"], Value::Null);
    let decoded: DeliveryAttempt<String, String> =
        serde_json::from_value(encoded).expect("claimed attempt decodes");
    assert!(decoded.effects.is_none());
}
