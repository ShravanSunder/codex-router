use message_board::{
    EndpointId, HumanId, Identity, ServiceId, SessionEndpointRef, SessionId, SessionRef,
};
use session_event_model::{
    ApprovalChoice, ApprovalEffect, ApprovalScope, CapabilityReport, InteractionKind,
    PendingInteraction, PendingInteractions, SessionApprover, SessionState, StopReason,
    TurnOutcome,
};

// Specification E4: a confirmed stop and a lost connection are distinct outcomes.
#[test]
fn confirmed_and_lost_turns_keep_distinct_evidence() {
    let confirmed = TurnOutcome::Ended {
        stop_reason: StopReason::Cancelled,
        local_cause: None,
    };
    let lost = TurnOutcome::Lost {
        reason: "provider retired".into(),
    };
    assert_ne!(confirmed, lost);
    assert_eq!(
        serde_json::to_value(lost).expect("serialize lost turn")["kind"],
        "lost"
    );
}

// Specification E7: requiresAction implies at least one pending interaction.
#[test]
fn requires_action_cannot_start_with_an_empty_pending_set() {
    assert!(PendingInteractions::new(vec![]).is_none());
    let pending = PendingInteractions::new(vec![PendingInteraction {
        request_id: "request-1".into(),
        kind: InteractionKind::Approval,
    }])
    .expect("one pending interaction");
    assert_eq!(
        SessionState::RequiresAction { pending }.requires_action_kind(),
        Some(InteractionKind::Approval)
    );
    assert!(
        serde_json::from_str::<SessionState>(r#"{"kind":"requiresAction","pending":[]}"#).is_err()
    );
}

// Specification E10: persistent approval scope always names its destination.
#[test]
fn persistent_choice_requires_a_destination() {
    let choice = ApprovalScope::persistent("Cursor allowlist")
        .map(|scope| ApprovalChoice::new(ApprovalEffect::Allow, scope));
    assert!(choice.is_ok());
    assert!(ApprovalScope::persistent("").is_err());
    assert!(serde_json::from_str::<ApprovalScope>(r#"{"persistent":{"whereStored":""}}"#).is_err());
}

// Specification E13: queue provenance is explicit and baseline text/link content is implicit.
#[test]
fn capability_report_names_only_optional_features() {
    let report = CapabilityReport::default();
    assert!(!report.steer);
    assert!(report.queue.is_none());
    assert!(!report.prompt_content.image);
    assert!(report.accepts_text());
    assert!(report.accepts_resource_link());
}

// Specification E12: actor and approver use the board's existing tagged Identity.
#[test]
fn identity_is_reused_from_message_board() {
    let requester = SessionRef {
        endpoint: SessionEndpointRef {
            service_id: ServiceId::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())
                .expect("valid service ID"),
            endpoint_id: EndpointId::try_from("claude-local".to_owned())
                .expect("valid endpoint ID"),
        },
        session_id: SessionId::try_from("agent-session".to_owned()).expect("valid session ID"),
    };
    assert!(
        SessionApprover::new(
            &requester,
            Identity::Session {
                session: requester.clone()
            }
        )
        .is_err()
    );
    let human = Identity::Human {
        human_id: HumanId::try_from("owner".to_owned()).expect("valid human ID"),
    };
    assert_eq!(
        SessionApprover::new(&requester, human.clone())
            .expect("human approver")
            .identity(),
        &human
    );
}
