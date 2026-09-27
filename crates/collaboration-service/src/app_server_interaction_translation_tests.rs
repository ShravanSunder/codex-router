use super::*;
use message_board::Identity;
use serde_json::json;
use session_event_model::{
    ApprovalChoice, ApprovalEffect, ApprovalRequest, ApprovalScope, ApprovalSubject,
    ApprovalToolCallSubject, OfferedOption, OfferedOptionId, OfferedOptions, QuestionField,
    QuestionFields, QuestionRequest,
};

fn identity(human_id: &str) -> Identity {
    serde_json::from_value(json!({"kind":"human","humanId":human_id})).expect("actor")
}

fn offered(id: &str, label: &str, effect: ApprovalEffect, scope: ApprovalScope) -> OfferedOption {
    OfferedOption {
        option_id: OfferedOptionId::new(id).expect("option id"),
        label: label.into(),
        choice: ApprovalChoice::new(effect, scope),
    }
}

fn approval(kind: &str, options: Vec<OfferedOption>) -> ApprovalRequest {
    ApprovalRequest {
        request_id: "request-1".into(),
        title: "Allow the action?".into(),
        description: Some("The agent is waiting".into()),
        subject: Some(ApprovalSubject::ToolCall {
            tool_call: ApprovalToolCallSubject {
                tool_call_id: "tool-1".into(),
                kind: kind.into(),
                title: "Tool action".into(),
            },
        }),
        options_origin: session_event_model::OptionsOrigin::AgentOffered,
        options: OfferedOptions::new(options).expect("options"),
    }
}

fn context() -> InteractionDisplayContext<'static> {
    InteractionDisplayContext {
        thread_id: "thread-1",
        turn_id: "turn-1",
        started_at_ms: 1_700_000_000_000,
    }
}

/// Oracle: pinned Codex 0.157.1 tui/bottom_pane/mcp_server_elicitation.rs:617-619.
#[test]
fn multi_choice_question_is_read_only_when_tui_cannot_render_it() {
    let request: QuestionRequest = serde_json::from_value(json!({
        "requestId":"multi-1","prompt":"Pick items","fields":[
            {"kind":"multiChoice","fieldId":"items","label":"Items","description":null,
             "required":true,"min":1,"max":2,"options":[
                {"optionId":"first","label":"First"},
                {"optionId":"second","label":"Second"}
             ]}
        ]
    }))
    .expect("question");
    let approver = identity("owner");
    let QuestionPresentation::ReadOnly { summary } =
        translate_question_request(&request, &approver, &approver, context())
    else {
        panic!("expected unsupported-here notice");
    };
    assert_eq!(summary["id"], "multi-1");
    assert!(
        summary["text"]
            .as_str()
            .is_some_and(|text| text.contains("unsupportedHere"))
    );
    let number_request: QuestionRequest = serde_json::from_value(json!({
        "requestId":"number-1","prompt":"Choose count","fields":[
            {"kind":"number","fieldId":"count","label":"Count","description":null,"required":true}
        ]
    }))
    .expect("number question");
    let QuestionPresentation::ReadOnly { summary } =
        translate_question_request(&number_request, &approver, &approver, context())
    else {
        panic!("number question must be unsupported here");
    };
    assert!(
        summary["text"]
            .as_str()
            .is_some_and(|text| text.contains("unsupportedHere"))
    );
}

/// Oracle: Codex item.rs:66-85,1549-1627. availableDecisions is exactly
/// the mapped offered set plus cancel; the selected agent option ID survives.
#[test]
fn command_approval_uses_native_prompt_only_for_exact_decisions() {
    let request = approval(
        "execute",
        vec![
            offered(
                "allow-once",
                "Allow once",
                ApprovalEffect::Allow,
                ApprovalScope::Once,
            ),
            offered(
                "allow-session",
                "Allow session",
                ApprovalEffect::Allow,
                ApprovalScope::Session,
            ),
            offered(
                "reject-once",
                "Reject once",
                ApprovalEffect::Decline,
                ApprovalScope::Once,
            ),
        ],
    );
    let actor = identity("owner");
    let presentation = translate_approval_request(&request, &actor, &actor, context());
    let ApprovalPresentation::Interactive { method, params, .. } = &presentation else {
        panic!("expected native command approval");
    };
    assert_eq!(method, "item/commandExecution/requestApproval");
    assert_eq!(
        params["availableDecisions"],
        json!(["accept", "acceptForSession", "decline", "cancel"])
    );
    assert_eq!(params["itemId"], "tool-1");
    assert_eq!(
        map_approval_reply(&presentation, &json!({"decision":"acceptForSession"})),
        Ok(ApprovalReply::Selected(
            OfferedOptionId::new("allow-session").expect("id")
        ))
    );
}

/// Oracle: Codex item.rs:115-124,1632-1652. File prompts have no usable
/// decline control in this facade, so any reject option requires the form.
#[test]
fn file_change_uses_native_only_for_two_allow_scopes() {
    let actor = identity("owner");
    let native = approval(
        "edit",
        vec![
            offered(
                "once",
                "Allow once",
                ApprovalEffect::Allow,
                ApprovalScope::Once,
            ),
            offered(
                "session",
                "Allow session",
                ApprovalEffect::Allow,
                ApprovalScope::Session,
            ),
        ],
    );
    let presentation = translate_approval_request(&native, &actor, &actor, context());
    assert!(
        matches!(presentation, ApprovalPresentation::Interactive { ref method, .. }
        if method == "item/fileChange/requestApproval")
    );
    let with_reject = approval(
        "edit",
        vec![
            offered(
                "once",
                "Allow once",
                ApprovalEffect::Allow,
                ApprovalScope::Once,
            ),
            offered(
                "session",
                "Allow session",
                ApprovalEffect::Allow,
                ApprovalScope::Session,
            ),
            offered(
                "reject",
                "Reject",
                ApprovalEffect::Decline,
                ApprovalScope::Once,
            ),
        ],
    );
    let presentation = translate_approval_request(&with_reject, &actor, &actor, context());
    assert!(
        matches!(presentation, ApprovalPresentation::Interactive { ref method, .. }
        if method == "mcpServer/elicitation/request")
    );
}

/// Oracle: Codex mcp.rs:409-425,645-670,776-826,890-900. A form keeps
/// distinct agent option IDs even when two options have the same effect/scope.
#[test]
fn form_preserves_two_reject_options_and_discloses_persistence() {
    let actor = identity("owner");
    let persistent = ApprovalScope::persistent("Cursor allowlist").expect("destination");
    let request = approval(
        "execute",
        vec![
            offered(
                "reject-a",
                "Reject this",
                ApprovalEffect::Decline,
                ApprovalScope::Once,
            ),
            offered(
                "reject-b",
                "Reject and explain",
                ApprovalEffect::Decline,
                ApprovalScope::Once,
            ),
            offered("always", "Always allow", ApprovalEffect::Allow, persistent),
        ],
    );
    let presentation = translate_approval_request(&request, &actor, &actor, context());
    let ApprovalPresentation::Interactive { method, params, .. } = &presentation else {
        panic!("expected exact option form");
    };
    assert_eq!(method, "mcpServer/elicitation/request");
    let choices = params["requestedSchema"]["properties"]["choice"]["oneOf"]
        .as_array()
        .expect("choices");
    assert_eq!(choices.len(), 3);
    assert_eq!(choices[0]["const"], "reject-a");
    assert_eq!(choices[1]["const"], "reject-b");
    assert_eq!(choices[2]["const"], "always");
    assert!(
        choices[2]["title"]
            .as_str()
            .expect("title")
            .contains("Cursor allowlist")
    );
    assert_eq!(
        map_approval_reply(
            &presentation,
            &json!({"action":"accept","content":{"choice":"reject-b"}})
        ),
        Ok(ApprovalReply::Selected(
            OfferedOptionId::new("reject-b").expect("id")
        ))
    );
    assert_eq!(
        map_approval_reply(&presentation, &json!({"action":"decline"})),
        Ok(ApprovalReply::Cancelled)
    );
}

#[test]
fn synthesized_plan_approval_discloses_origin_and_plan_subject() {
    let actor = identity("owner");
    let mut request = approval(
        "execute",
        vec![offered(
            "allow",
            "Allow",
            ApprovalEffect::Allow,
            ApprovalScope::Once,
        )],
    );
    request.options_origin = session_event_model::OptionsOrigin::RouterSynthesized;
    request.subject = Some(ApprovalSubject::Plan {
        tool_call_id: "tool-plan".into(),
        plan_item_id: "plan-item-2".into(),
    });
    let presentation = translate_approval_request(&request, &actor, &actor, context());
    let ApprovalPresentation::Interactive { method, params, .. } = presentation else {
        panic!("expected disclosed form");
    };
    assert_eq!(method, "mcpServer/elicitation/request");
    assert!(
        params["message"]
            .as_str()
            .expect("message")
            .contains("Router synthesized")
    );
    assert!(
        params["message"]
            .as_str()
            .expect("message")
            .contains("plan-item-2")
    );
}

#[test]
fn non_approver_sees_only_read_only_interaction() {
    let request = approval(
        "execute",
        vec![offered(
            "once",
            "Allow once",
            ApprovalEffect::Allow,
            ApprovalScope::Once,
        )],
    );
    let presentation = translate_approval_request(
        &request,
        &identity("observer"),
        &identity("owner"),
        context(),
    );
    assert!(matches!(
        presentation,
        ApprovalPresentation::ReadOnly { .. }
    ));
    assert_eq!(
        map_approval_reply(&presentation, &json!({"decision":"accept"})),
        Err(InteractionReplyError::ReadOnly)
    );
}

/// Oracle: Codex mcp.rs:429-590,776-826,890-900. Field types, labels,
/// descriptions and required flags survive the form translation.
#[test]
fn question_form_keeps_typed_fields_and_distinct_outcomes() {
    let actor = identity("owner");
    let request = QuestionRequest {
        request_id: "question-1".into(),
        prompt: "Configure the run".into(),
        fields: QuestionFields::new(vec![
            QuestionField::Text {
                field_id: "name".into(),
                label: "Name".into(),
                description: Some("Display name".into()),
                required: true,
            },
            QuestionField::Boolean {
                field_id: "enabled".into(),
                label: "Enabled".into(),
                description: None,
                required: false,
            },
            QuestionField::SingleChoice {
                field_id: "mode".into(),
                label: "Mode".into(),
                description: None,
                required: true,
                options: vec![
                    session_event_model::ChoiceOption {
                        option_id: "safe".into(),
                        label: "Safe".into(),
                    },
                    session_event_model::ChoiceOption {
                        option_id: "fast".into(),
                        label: "Fast".into(),
                    },
                ],
            },
        ])
        .expect("fields"),
    };
    let presentation = translate_question_request(&request, &actor, &actor, context());
    let QuestionPresentation::InteractiveForm { params } = &presentation else {
        panic!("expected form");
    };
    let schema = &params["requestedSchema"];
    assert_eq!(schema["properties"]["name"]["type"], "string");
    assert_eq!(schema["properties"]["name"]["title"], "Name");
    assert_eq!(schema["properties"]["enabled"]["type"], "boolean");
    assert_eq!(schema["properties"]["mode"]["oneOf"][1]["const"], "fast");
    assert_eq!(schema["required"], json!(["name", "mode"]));
    assert_eq!(
        map_question_form_reply(&request, &json!({"action":"decline"})),
        Ok(QuestionReply::Declined)
    );
    assert_eq!(
        map_question_form_reply(&request, &json!({"action":"cancel"})),
        Ok(QuestionReply::Cancelled)
    );
    assert_eq!(
        map_question_form_reply(
            &request,
            &json!({"action":"accept","content":{"mode":"safe"}})
        ),
        Ok(QuestionReply::Answered(
            json!({"mode":{"selectedOptionIds":["safe"]}})
        ))
    );
    assert_eq!(
        map_question_form_reply(
            &request,
            &json!({"action":"accept","content":{"mode":{"selectedOptionIds":["fast"]}}})
        ),
        Ok(QuestionReply::Answered(
            json!({"mode":{"selectedOptionIds":["fast"]}})
        ))
    );
    assert_eq!(
        map_question_form_reply(
            &request,
            &json!({"action":"accept","content":{"mode":["safe"]}})
        ),
        Err(InteractionReplyError::InvalidReply)
    );
}

#[test]
fn non_approver_cannot_answer_a_question() {
    let request = QuestionRequest {
        request_id: "question-2".into(),
        prompt: "Choose a mode".into(),
        fields: QuestionFields::new(vec![QuestionField::SingleChoice {
            field_id: "mode".into(),
            label: "Mode".into(),
            description: None,
            required: true,
            options: vec![
                session_event_model::ChoiceOption {
                    option_id: "safe".into(),
                    label: "Safe".into(),
                },
                session_event_model::ChoiceOption {
                    option_id: "fast".into(),
                    label: "Fast".into(),
                },
            ],
        }])
        .expect("fields"),
    };
    let presentation = translate_question_request(
        &request,
        &identity("observer"),
        &identity("owner"),
        context(),
    );
    assert!(matches!(
        presentation,
        QuestionPresentation::ReadOnly { .. }
    ));
}

/// Oracle: Codex item.rs:1734-1803. An empty submitted answer means skip,
/// while a cancelled Turn remains distinct.
#[test]
fn request_user_input_reply_distinguishes_answer_skip_and_cancel() {
    assert_eq!(
        map_request_user_input_reply(&json!({"answers":{"choice":{"answers":["safe"]}}}), false),
        Ok(QuestionReply::Answered(json!({"choice":"safe"})))
    );
    assert_eq!(
        map_request_user_input_reply(&json!({"answers":{}}), false),
        Ok(QuestionReply::Declined)
    );
    assert_eq!(
        map_request_user_input_reply(&json!({"answers":{}}), true),
        Ok(QuestionReply::Cancelled)
    );
}
