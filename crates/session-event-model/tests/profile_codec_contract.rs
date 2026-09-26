use serde_json::json;
use session_event_model::session_profile_codec::{
    PROFILE_VERSION, ProfileAdvertisement, ProfileElement, ProfileError, ProfileState,
    QueueAddRequest, QueueCancelRequest, QueueListRequest, RouterIdentityMetadata,
    StateNotification, SteeringOutcome, SteeringRequest, decode_choice_metadata,
    encode_choice_metadata,
};
use session_event_model::{ApprovalChoice, ApprovalEffect, ApprovalScope, Identity};

// R20-R22, Program Design profile table: advertise exact names and version.
#[test]
fn advertisement_round_trip_and_absent_element_are_explicit() {
    use session_event_model::session_profile_codec::InitializeProfileMetadata;
    let advertisement = ProfileAdvertisement::new([ProfileElement::Steer, ProfileElement::Queue]);
    let encoded = serde_json::to_value(&advertisement).expect("serialize advertisement");
    assert_eq!(encoded["version"], PROFILE_VERSION);
    assert_eq!(encoded["elements"], json!(["steer", "queue"]));
    let decoded: ProfileAdvertisement =
        serde_json::from_value(encoded).expect("decode advertisement");
    assert!(decoded.supports(ProfileElement::Queue).is_ok());
    assert_eq!(
        decoded.supports(ProfileElement::State),
        Err(ProfileError::Unsupported(ProfileElement::State))
    );
    let unsupported_version: ProfileAdvertisement =
        serde_json::from_value(json!({"version":2,"elements":["queue"]}))
            .expect("decode future profile");
    assert_eq!(
        unsupported_version.supports(ProfileElement::Queue),
        Err(ProfileError::UnsupportedVersion(2))
    );
    let initialize = InitializeProfileMetadata::new(advertisement);
    let initialize_wire = serde_json::to_value(initialize).expect("encode initialize metadata");
    assert_eq!(
        initialize_wire["sessionProfile"]["version"],
        PROFILE_VERSION
    );
    assert_eq!(initialize_wire["steering"]["supported"], true);
}

// R20-R21, Program Design: steering preserves the shared method and outcome union.
#[test]
fn steering_request_and_all_four_outcomes_round_trip() {
    let request: SteeringRequest = serde_json::from_value(json!({
        "sessionId":"s1", "prompt":[{"type":"text","text":"hello"}],
        "_meta":{"steering":{"idleBehavior":"promptRequired"}}
    }))
    .expect("decode steering request");
    assert_eq!(request.method(), "_session/steering");
    assert_eq!(
        serde_json::to_value(request).expect("encode steering request")["_meta"]["steering"]["idleBehavior"],
        "promptRequired"
    );
    for outcome in ["injected", "startedNewTurn", "promptRequired", "failed"] {
        let decoded: SteeringOutcome =
            serde_json::from_value(json!({"outcome":outcome})).expect("decode outcome");
        assert_eq!(
            serde_json::to_value(decoded).expect("encode outcome")["outcome"],
            outcome
        );
    }
}

// R20, Program Design: queue operations carry the Session ID on every request.
#[test]
fn queue_requests_have_distinct_methods_and_typed_ids() {
    let add: QueueAddRequest =
        serde_json::from_value(json!({"sessionId":"s1","prompt":[{"type":"text","text":"later"}]}))
            .expect("queue add");
    let list: QueueListRequest =
        serde_json::from_value(json!({"sessionId":"s1"})).expect("queue list");
    let cancel: QueueCancelRequest =
        serde_json::from_value(json!({"sessionId":"s1","inputId":"i1"})).expect("queue cancel");
    assert_eq!(add.method(), "_session/queue/add");
    assert_eq!(list.method(), "_session/queue/list");
    assert_eq!(cancel.method(), "_session/queue/cancel");
    assert_eq!(cancel.input_id, "i1");
}

// R20-R21: state uses a separate notification and requiresAction has a kind.
#[test]
fn state_notification_round_trips_without_a_session_update_kind() {
    let state: StateNotification = serde_json::from_value(json!({
        "sessionId":"s1", "state":"requires_action", "requiresAction":"approval"
    }))
    .expect("decode state");
    assert_eq!(state.method(), "_session/state");
    assert_eq!(
        state.state,
        ProfileState::RequiresAction {
            kind: session_event_model::InteractionKind::Approval
        }
    );
    assert_eq!(
        serde_json::to_value(state).expect("encode state")["requiresAction"],
        "approval"
    );
    assert!(
        serde_json::from_value::<StateNotification>(
            json!({"sessionId":"s1", "state":"requires_action"})
        )
        .is_err()
    );
}

// R20, E10: choice metadata uses effect/scope with a required persistent destination.
#[test]
fn choice_metadata_round_trips_and_rejects_missing_destination() {
    let choice = ApprovalChoice::new(
        ApprovalEffect::Allow,
        ApprovalScope::persistent("Cursor allowlist").expect("destination"),
    );
    let encoded = encode_choice_metadata(&choice);
    assert_eq!(
        encoded,
        json!({"effect":"allow","scope":"persistent","where":"Cursor allowlist"})
    );
    assert_eq!(
        decode_choice_metadata(encoded).expect("decode choice"),
        choice
    );
    assert!(decode_choice_metadata(json!({"effect":"allow","scope":"persistent"})).is_err());
    assert!(
        decode_choice_metadata(json!({"effect":"allow","scope":"once","where":"unexpected"}))
            .is_err()
    );
}

// R20: prompt title is outside subject; a tool-call subject nests toolCall.
#[test]
fn approval_prompt_and_subject_keep_v2_alignment() {
    use session_event_model::session_profile_codec::{ApprovalPromptMetadata, ApprovalSubject};
    let prompt: ApprovalPromptMetadata =
        serde_json::from_value(json!({"title":"Run command","description":"Needs access"}))
            .expect("prompt");
    let subject: ApprovalSubject = serde_json::from_value(
        json!({"type":"tool_call","toolCall":{"toolCallId":"call-1","title":"echo","kind":"execute","status":"pending"}}),
    )
    .expect("subject");
    assert_eq!(
        serde_json::to_value(prompt).expect("encode prompt")["title"],
        "Run command"
    );
    let encoded_subject = serde_json::to_value(subject).expect("encode subject");
    assert_eq!(encoded_subject["toolCall"]["toolCallId"], "call-1");
    assert_eq!(encoded_subject["toolCall"]["kind"], "execute");
}

// R20-R22: Router identity lives under _meta.router and absence has no actor.
#[test]
fn router_identity_metadata_round_trips_a_human_actor() {
    use session_event_model::session_profile_codec::SessionResponseProfileMetadata;
    let metadata: RouterIdentityMetadata = serde_json::from_value(json!({
        "actor":{"kind":"human","humanId":"owner"}, "endpoint":"claude-local"
    }))
    .expect("decode identity");
    assert!(matches!(metadata.actor, Some(Identity::Human { .. })));
    let encoded = serde_json::to_value(metadata).expect("encode identity");
    assert_eq!(encoded["endpoint"], "claude-local");
    let absent: RouterIdentityMetadata =
        serde_json::from_value(json!({})).expect("absent identity");
    assert!(absent.actor.is_none());
    let response: SessionResponseProfileMetadata = serde_json::from_value(json!({
        "sessionProfile":{"capabilities":serde_json::to_value(session_event_model::CapabilityReport::default()).expect("capabilities")},
        "router":{"approver":{"kind":"human","humanId":"owner"}}
    })).expect("response metadata");
    let wire = serde_json::to_value(response).expect("encode response metadata");
    assert_eq!(wire["router"]["approver"]["humanId"], "owner");
    assert_eq!(
        wire["sessionProfile"]["capabilities"]["authStatus"]["kind"],
        "notReported"
    );
}

// R20: capability reports use the typed E13 vocabulary, not provider methods.
#[test]
fn capability_metadata_round_trips() {
    let capabilities = session_event_model::CapabilityReport::default();
    let encoded = serde_json::to_value(&capabilities).expect("encode capabilities");
    let decoded: session_event_model::CapabilityReport =
        serde_json::from_value(encoded).expect("decode capabilities");
    assert_eq!(decoded, capabilities);
    assert!(!decoded.steer);
}

// R23: edge translation never returns a provider-specific method name.
#[test]
fn provider_plan_edge_becomes_a_plan_item() {
    use session_event_model::session_profile_codec::{
        CursorTodo, CursorTodoStatus, PlanChangeMode, ProviderEdge, translate_provider_edge,
    };
    let plan = translate_provider_edge(ProviderEdge::CursorPlan {
        item_id: "plan-1".into(),
        text: "ship".into(),
    });
    assert!(matches!(
        plan.item.kind,
        session_event_model::SessionItemKind::Plan
    ));
    assert_eq!(plan.mode, PlanChangeMode::Replace);
    let todos = translate_provider_edge(ProviderEdge::CursorTodos {
        item_id: "plan-1".into(),
        merge: true,
        items: vec![CursorTodo {
            id: "todo-1".into(),
            content: "prove codec".into(),
            status: CursorTodoStatus::InProgress,
        }],
    });
    assert_eq!(todos.mode, PlanChangeMode::Merge);
    assert_eq!(todos.entries[0].status, CursorTodoStatus::InProgress);
}

// R23, E13: a connection auth update carries only kind/label; silence differs from logout.
#[test]
fn connection_auth_status_drops_private_fields_and_preserves_absence() {
    use session_event_model::ProviderAuthStatus;
    use session_event_model::session_profile_codec::decode_connection_auth_status;
    let report = session_event_model::CapabilityReport::default();
    assert_eq!(report.auth_status, ProviderAuthStatus::NotReported);
    let account = decode_connection_auth_status(json!({
        "authStatus": {"kind":"account","label":"Signed in", "account":{"email":"private@example.invalid"}, "vendor":"private-vendor"}
    }))
    .expect("decode account status");
    assert_eq!(
        account,
        ProviderAuthStatus::Account {
            label: "Signed in".into()
        }
    );
    let encoded = serde_json::to_value(&account).expect("encode sanitized status");
    assert_eq!(encoded, json!({"kind":"account","label":"Signed in"}));
    let decoded: ProviderAuthStatus = serde_json::from_value(encoded).expect("round trip status");
    assert_eq!(decoded, account);
    let logged_out = decode_connection_auth_status(json!({
        "authStatus":{"kind":"none","label":"No login"}
    }))
    .expect("decode logout");
    assert_eq!(logged_out, ProviderAuthStatus::LoggedOut);
    assert_ne!(logged_out, report.auth_status);
}
