use message_board::{HumanId, Identity};
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

#[test]
fn question_response_actions_and_typed_values_round_trip_without_loss() {
    for value in [
        json!({"action":"answered","content":{"name":"Ada","count":3,"confirmed":true}}),
        json!({"action":"declined"}),
        json!({"action":"cancelled"}),
    ] {
        let response: session_event_model::QuestionResponse =
            serde_json::from_value(value.clone()).expect("canonical question response");
        assert_eq!(serde_json::to_value(response).expect("round trip"), value);
    }
}

// E9, R25: pending events contain enough approval detail to render on reattach.
#[test]
fn approval_event_round_trip_carries_prompt_subject_and_ordered_choices() {
    use session_event_model::session_profile_codec::ApprovalRequestProfileMetadata;
    let request: ApprovalRequest = serde_json::from_value(json!({
        "requestId":"approval-1",
        "title":"Run command",
        "description":"Needs access",
        "optionsOrigin":"routerSynthesized",
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
    assert_eq!(
        request.options_origin,
        session_event_model::OptionsOrigin::RouterSynthesized
    );
    let event = SessionEvent::InteractionRequested {
        interaction: PendingInteraction::Approval {
            approver: Identity::Human {
                human_id: HumanId::try_from("owner".to_owned()).expect("human"),
            },
            request: Box::new(request),
        },
    };
    let wire = serde_json::to_value(&event).expect("encode pending approval");
    assert_eq!(
        wire["interaction"]["request"]["options"][0]["optionId"],
        "allow-once"
    );
    assert_eq!(wire["interaction"]["approver"]["humanId"], "owner");
    assert_eq!(
        serde_json::from_value::<SessionEvent>(wire).expect("decode event"),
        event
    );
    let SessionEvent::InteractionRequested {
        interaction: PendingInteraction::Approval { request, .. },
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

#[test]
fn plan_subject_and_origin_round_trip_through_profile_metadata() {
    use session_event_model::session_profile_codec::ApprovalRequestProfileMetadata;
    let request: ApprovalRequest = serde_json::from_value(json!({
        "requestId":"plan-1", "title":"Accept plan",
        "subject":{"type":"plan","toolCallId":"call-1","planItemId":"item-1"},
        "optionsOrigin":"routerSynthesized",
        "options":[{"optionId":"plan.accept","label":"Accept","choice":{"effect":"allow","scope":"once"}}]
    })).expect("plan request");
    assert!(matches!(
        request.subject,
        Some(ApprovalSubject::Plan { .. })
    ));
    let metadata = ApprovalRequestProfileMetadata::from_request(&request);
    let encoded = serde_json::to_value(&metadata).expect("metadata");
    assert_eq!(encoded["sessionProfile"]["subject"]["planItemId"], "item-1");
    assert_eq!(
        serde_json::from_value::<ApprovalRequestProfileMetadata>(encoded).expect("decode"),
        metadata
    );
}

#[test]
fn question_choice_ids_are_unique_while_labels_may_repeat() {
    let field = json!({"kind":"multiChoice", "fieldId":"selection", "label":"Select", "description":null,
        "required":true, "options":[{"optionId":"a","label":"Same"},{"optionId":"b","label":"Same"}],
        "min":1,"max":2});
    assert!(serde_json::from_value::<QuestionFields>(json!([field.clone()])).is_ok());
    let mut duplicate = field;
    duplicate["options"][1]["optionId"] = json!("a");
    assert!(serde_json::from_value::<QuestionFields>(json!([duplicate])).is_err());
    let answer: session_event_model::QuestionAnswerValue =
        serde_json::from_value(json!({"selectedOptionIds":["a","b"]})).expect("selection");
    assert_eq!(
        serde_json::to_value(answer).expect("round trip"),
        json!({"selectedOptionIds":["a","b"]})
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
        interaction: PendingInteraction::Question {
            approver: Identity::Human {
                human_id: HumanId::try_from("owner".to_owned()).expect("human"),
            },
            request: Box::new(request),
        },
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
    assert_eq!(wire["interaction"]["approver"]["humanId"], "owner");
    assert_eq!(
        serde_json::from_value::<SessionEvent>(wire).expect("decode event"),
        event
    );
    assert!(serde_json::from_value::<QuestionFields>(json!([])).is_err());
}
