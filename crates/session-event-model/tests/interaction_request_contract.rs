use serde_json::json;
use session_event_model::{
    ApprovalChoice, ApprovalEffect, ApprovalRequest, ApprovalScope, ApprovalSubject, OfferedOption,
    OfferedOptionId, OfferedOptions, PendingInteraction, QuestionField, QuestionFields,
    QuestionRequest, SessionEvent,
};

// E9-E10: option IDs stay verbatim and request options are nonempty and unique.
#[test]
fn approval_options_are_ordered_unique_and_nonempty() {
    let option_id = OfferedOptionId::new("allow\nalways").expect("opaque option ID");
    assert_eq!(option_id.as_str(), "allow\nalways");
    assert!(OfferedOptionId::new("").is_err());
    assert!(OfferedOptions::new(vec![]).is_err());
    let option = OfferedOption {
        option_id,
        label: "Always allow".into(),
        choice: ApprovalChoice::new(
            ApprovalEffect::Allow,
            ApprovalScope::persistent("Cursor allowlist").expect("destination"),
        ),
    };
    assert!(OfferedOptions::new(vec![option.clone(), option.clone()]).is_err());
    let options = OfferedOptions::new(vec![option]).expect("one option");
    assert_eq!(options.iter().count(), 1);
    assert!(serde_json::from_value::<OfferedOptions>(json!([])).is_err());
}

// E9, R25: pending events contain enough approval detail to render on reattach.
#[test]
fn approval_event_round_trip_carries_prompt_subject_and_ordered_choices() {
    use session_event_model::session_profile_codec::ApprovalRequestProfileMetadata;
    let request: ApprovalRequest = serde_json::from_value(json!({
        "requestId":"approval-1",
        "title":"Run command",
        "description":"Needs access",
        "subject":{"type":"tool_call","toolCall":{"toolCallId":"call-1","kind":"execute","title":"echo"}},
        "options":[
            {"optionId":"allow-once","label":"Allow once","choice":{"effect":"allow","scope":"once"}},
            {"optionId":"reject-once","label":"Reject","choice":{"effect":"decline","scope":"once"}}
        ]
    })).expect("approval request");
    assert!(matches!(
        request.subject,
        Some(ApprovalSubject::ToolCall { .. })
    ));
    let event = SessionEvent::InteractionRequested {
        interaction: PendingInteraction::Approval { request },
    };
    let wire = serde_json::to_value(&event).expect("encode pending approval");
    assert_eq!(
        wire["interaction"]["request"]["options"][0]["optionId"],
        "allow-once"
    );
    assert_eq!(
        serde_json::from_value::<SessionEvent>(wire).expect("decode event"),
        event
    );
    let SessionEvent::InteractionRequested {
        interaction: PendingInteraction::Approval { request },
    } = &event
    else {
        panic!("approval event expected");
    };
    let metadata = ApprovalRequestProfileMetadata::from_request(request);
    let encoded_metadata = serde_json::to_value(&metadata).expect("encode profile metadata");
    assert_eq!(
        encoded_metadata["sessionProfile"]["prompt"]["title"],
        "Run command"
    );
    assert_eq!(
        encoded_metadata["sessionProfile"]["subject"]["toolCall"]["kind"],
        "execute"
    );
    let decoded_metadata: ApprovalRequestProfileMetadata =
        serde_json::from_value(encoded_metadata).expect("decode profile metadata");
    assert_eq!(decoded_metadata, metadata);
    assert!(
        serde_json::from_value::<ApprovalRequest>(json!({
            "requestId":"approval-1", "title":"Run command", "options":[]
        }))
        .is_err()
    );
}

// E11, R25: a question has at least one labeled field and its prompt survives.
#[test]
fn question_event_round_trip_preserves_field_labels_and_descriptions() {
    assert!(QuestionFields::new(vec![]).is_err());
    let field = QuestionField::Number {
        field_id: "count".into(),
        label: "How many?".into(),
        description: Some("Enter a positive count".into()),
        required: true,
    };
    let request = QuestionRequest {
        request_id: "question-1".into(),
        prompt: "Choose the batch size".into(),
        fields: QuestionFields::new(vec![field]).expect("one field"),
    };
    let event = SessionEvent::InteractionRequested {
        interaction: PendingInteraction::Question { request },
    };
    let wire = serde_json::to_value(&event).expect("encode pending question");
    assert_eq!(
        wire["interaction"]["request"]["prompt"],
        "Choose the batch size"
    );
    assert_eq!(
        wire["interaction"]["request"]["fields"][0]["label"],
        "How many?"
    );
    assert_eq!(
        wire["interaction"]["request"]["fields"][0]["fieldId"],
        "count"
    );
    assert_eq!(
        serde_json::from_value::<SessionEvent>(wire).expect("decode event"),
        event
    );
    assert!(serde_json::from_value::<QuestionFields>(json!([])).is_err());
}
