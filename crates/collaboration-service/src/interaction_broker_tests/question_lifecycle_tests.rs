use super::*;

#[tokio::test]
async fn typed_approval_preserves_cursor_choices_and_returns_exact_option_id() {
    use message_board::{HumanId, Identity};
    use session_event_model::{
        ApprovalChoice, ApprovalEffect, ApprovalRequest, ApprovalScope, OfferedOption,
        OfferedOptionId, OfferedOptions,
    };

    let (broker, _generation, _directory) = fixture_broker().await;
    let requester: message_board::SessionRef = serde_json::from_value(
        serde_json::to_value(session(&broker.service_id, "provider-session"))
            .expect("requester JSON"),
    )
    .expect("board requester");
    let approver = Identity::Human {
        human_id: HumanId::try_from("owner".to_owned()).expect("human ID"),
    };
    let request = ApprovalRequest {
        request_id: "cursor-approval".into(),
        title: "Run command".into(),
        description: Some("Changes the allowlist if always allowed".into()),
        subject: None,
        options_origin: session_event_model::OptionsOrigin::AgentOffered,
        options: OfferedOptions::new(vec![
            OfferedOption {
                option_id: OfferedOptionId::new("allow-once").expect("ID"),
                label: "Allow once".into(),
                choice: ApprovalChoice::new(ApprovalEffect::Allow, ApprovalScope::Once),
            },
            OfferedOption {
                option_id: OfferedOptionId::new("allow-always").expect("ID"),
                label: "Always allow".into(),
                choice: ApprovalChoice::new(
                    ApprovalEffect::Allow,
                    ApprovalScope::persistent("Cursor allowlist").expect("destination"),
                ),
            },
            OfferedOption {
                option_id: OfferedOptionId::new("reject-once").expect("ID"),
                label: "Reject".into(),
                choice: ApprovalChoice::new(ApprovalEffect::Decline, ApprovalScope::Once),
            },
        ])
        .expect("options"),
    };
    let receiver = broker
        .request_typed_approval(
            requester,
            approver.clone(),
            request,
            tokio_util::sync::CancellationToken::new(),
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .expect("pending approval");
    let listed = broker.list_typed_approvals(true).await;
    let offered = listed[0].options.iter().collect::<Vec<_>>();
    assert_eq!(offered[1].option_id.as_str(), "allow-always");
    assert!(matches!(
        &offered[1].choice.scope,
        ApprovalScope::Persistent { where_stored } if where_stored.as_str() == "Cursor allowlist"
    ));
    let detailed = broker
        .list_detailed(true)
        .await
        .expect("detailed approvals");
    assert_eq!(detailed.approvals[0].options.len(), 3);
    assert_eq!(
        detailed.approvals[0].options[1]
            .persistent_target
            .as_deref(),
        Some("Cursor allowlist")
    );
    let decision = |option_id: &str, acknowledge_persistent| ApprovalDecideParams {
        request_id: "cursor-approval".into(),
        decision: None,
        option_id: Some(option_id.into()),
        acknowledge_persistent,
        note: Some("Approver note".into()),
        actor: approver.clone(),
    };
    assert!(matches!(
        broker.decide(decision("unoffered", false)).await,
        Err(ApprovalDecisionError::OptionNotOffered { offered })
            if offered == ["allow-once", "allow-always", "reject-once"]
    ));
    assert!(matches!(
        broker.decide(decision("allow-always", false)).await,
        Err(ApprovalDecisionError::PersistentChoiceNotAcknowledged { persistent_target })
            if persistent_target == "Cursor allowlist"
    ));
    let receipt = broker
        .decide(decision("allow-always", true))
        .await
        .expect("persistent choice");
    assert_eq!(receipt.option_id.as_deref(), Some("allow-always"));
    let crate::interaction_broker::TypedApprovalResolution::Selected(selected) =
        receiver.await.expect("agent option ID")
    else {
        panic!("selection expected");
    };
    assert_eq!(selected.option_id.as_str(), "allow-always");
    assert_eq!(selected.note.as_deref(), Some("Approver note"));
}

#[tokio::test]
async fn synthesized_plan_origin_is_visible_in_list_and_history() {
    use message_board::{HumanId, Identity};
    use session_event_model::{ApprovalSubject, OptionsOrigin};

    let (broker, _generation, _directory) = fixture_broker().await;
    let requester =
        board_session_ref(&session(&broker.service_id, "plan-session")).expect("requester");
    let approver = Identity::Human {
        human_id: HumanId::try_from("owner".to_owned()).expect("human ID"),
    };
    let mut request = typed_approval_request("plan-synthesized");
    request.options_origin = OptionsOrigin::RouterSynthesized;
    request.subject = Some(ApprovalSubject::Plan {
        tool_call_id: "tool-1".into(),
        plan_item_id: "item-1".into(),
    });
    let _receiver = broker
        .request_typed_approval(
            requester,
            approver,
            request,
            tokio_util::sync::CancellationToken::new(),
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .expect("pending plan approval");
    let listed = broker.list_detailed(true).await.expect("detailed list");
    assert_eq!(
        listed.approvals[0].options_origin,
        Some(OptionsOrigin::RouterSynthesized)
    );
    let history = broker.list_typed_approvals(true).await;
    assert_eq!(history[0].options_origin, OptionsOrigin::RouterSynthesized);
    assert!(matches!(
        history[0].subject,
        Some(ApprovalSubject::Plan { .. })
    ));
}

// R17: old decision names are resolved only against choices the agent offered.
#[tokio::test]
async fn claude_choices_resolve_legacy_decisions_without_inventing_an_option() {
    use message_board::{HumanId, Identity};

    let (broker, _generation, _directory) = fixture_broker().await;
    let requester =
        board_session_ref(&session(&broker.service_id, "claude-session")).expect("requester");
    let actor = Identity::Human {
        human_id: HumanId::try_from("owner".to_owned()).expect("human ID"),
    };
    let request: session_event_model::ApprovalRequest = serde_json::from_value(json!({
        "requestId":"claude-approval", "title":"Edit file", "options":[
            {"optionId":"yes-once","label":"Allow this time","choice":{"effect":"allow","scope":"once"}},
            {"optionId":"no-once","label":"Decline","choice":{"effect":"decline","scope":"once"}}
        ]
    }))
    .expect("Claude choices");
    let receiver = broker
        .request_typed_approval(
            requester,
            actor.clone(),
            request,
            tokio_util::sync::CancellationToken::new(),
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .expect("pending request");
    let params = |decision| ApprovalDecideParams {
        request_id: "claude-approval".into(),
        decision: Some(decision),
        option_id: None,
        acknowledge_persistent: false,
        note: None,
        actor: actor.clone(),
    };
    assert!(matches!(
        broker.decide(params(ApprovalDecision::AllowForSession)).await,
        Err(ApprovalDecisionError::OptionNotOffered { offered })
            if offered == ["yes-once", "no-once"]
    ));
    let receipt = broker
        .decide(params(ApprovalDecision::Deny))
        .await
        .expect("legacy decline");
    assert_eq!(receipt.decision, Some(ApprovalDecision::Deny));
    assert!(matches!(receiver.await.expect("agent option ID"),
        crate::interaction_broker::TypedApprovalResolution::Selected(selected)
            if selected.option_id.as_str() == "no-once"));
}

// R18: a form stays pending until its Approver answers it; values are checked
// against the advertised fields before the agent receives an ACP action.
#[tokio::test]
async fn typed_question_checks_fields_and_keeps_answer_decline_cancel_distinct() {
    use crate::interaction_broker::QuestionResponse;
    use message_board::{HumanId, Identity};

    let (broker, _generation, _directory) = fixture_broker().await;
    let requester =
        board_session_ref(&session(&broker.service_id, "provider-session")).expect("requester");
    let approver = Identity::Human {
        human_id: HumanId::try_from("owner".to_owned()).expect("human ID"),
    };
    let other = Identity::Human {
        human_id: HumanId::try_from("other".to_owned()).expect("human ID"),
    };
    let question = |request_id: &str| {
        serde_json::from_value(json!({
        "requestId":request_id,"prompt":"Choose launch settings","fields":[
            {"kind":"number","fieldId":"count","label":"Count","description":null,"required":true},
            {"kind":"boolean","fieldId":"dryRun","label":"Dry run","description":null,"required":true},
            {"kind":"singleChoice","fieldId":"color","label":"Color","description":null,"required":true,"options":[{"optionId":"red-id","label":"Same"},{"optionId":"blue-id","label":"Same"}]},
            {"kind":"multiChoice","fieldId":"features","label":"Features","description":null,"required":true,"options":[{"optionId":"a","label":"First"},{"optionId":"b","label":"Second"}],"min":2,"max":2}
        ]
    })).expect("question")
    };
    assert!(matches!(
        broker
            .request_question(
                requester.clone(),
                Identity::Session {
                    session: requester.clone()
                },
                question("self-question"),
                None,
            )
            .await,
        Err(crate::interaction_broker::InteractionHistoryError::SelfApprover)
    ));
    let duplicate_fields = serde_json::from_value(json!({
        "requestId":"duplicate-fields","prompt":"Choose settings","fields":[
            {"kind":"number","fieldId":"count","label":"First","description":null,"required":true},
            {"kind":"number","fieldId":"count","label":"Second","description":null,"required":true}
        ]
    }))
    .expect("parsed question");
    assert!(matches!(
        broker
            .request_question(requester.clone(), approver.clone(), duplicate_fields, None)
            .await,
        Err(crate::interaction_broker::InteractionHistoryError::InvalidQuestion)
    ));
    let receiver = broker
        .request_question(
            requester.clone(),
            approver.clone(),
            question("q-answer"),
            None,
        )
        .await
        .expect("pending question");
    assert_eq!(broker.list_questions(true).await.len(), 1);
    let answer = QuestionResponse::Answered {
        content: serde_json::from_value(json!({"count":3,"dryRun":true,"color":{"selectedOptionIds":["blue-id"]},"features":{"selectedOptionIds":["a","b"]}}))
            .expect("typed content"),
    };
    assert!(
        broker
            .respond_question("q-answer", &other, answer.clone())
            .await
            .is_err()
    );
    assert!(
        broker
            .respond_question(
                "q-answer",
                &approver,
                QuestionResponse::Answered {
                    content: serde_json::from_value(
                        json!({"count":"three","dryRun":true,"color":{"selectedOptionIds":["blue-id"]},"features":{"selectedOptionIds":["a","b"]}})
                    )
                    .expect("content"),
                }
            )
            .await
            .is_err()
    );
    assert_eq!(broker.list_questions(true).await.len(), 1);
    assert!(matches!(
        broker.respond_question("q-answer", &approver, QuestionResponse::Answered {
            content: serde_json::from_value(json!({"count":3,"dryRun":true,"color":{"selectedOptionIds":["green"]},"features":{"selectedOptionIds":["a","b"]}})).expect("content"),
        }).await,
        Err(crate::interaction_broker::InteractionHistoryError::InvalidAnswer { field_id }) if field_id == "color"
    ));
    assert_eq!(broker.list_questions(true).await.len(), 1);
    assert!(matches!(
        broker.respond_question("q-answer", &approver, QuestionResponse::Answered {
            content: serde_json::from_value(json!({"count":3,"dryRun":true,"color":{"selectedOptionIds":["blue-id"]},"features":{"selectedOptionIds":["a","unknown"]}})).expect("content"),
        }).await,
        Err(crate::interaction_broker::InteractionHistoryError::InvalidAnswer { field_id }) if field_id == "features"
    ));
    broker
        .respond_question("q-answer", &approver, answer.clone())
        .await
        .expect("answer");
    assert_eq!(receiver.await.expect("agent response"), answer);
    assert!(
        broker
            .respond_question("q-answer", &approver, answer)
            .await
            .is_err()
    );

    for (request_id, response) in [
        ("q-decline", QuestionResponse::Declined),
        ("q-cancel", QuestionResponse::Cancelled),
    ] {
        let receiver = broker
            .request_question(
                requester.clone(),
                approver.clone(),
                question(request_id),
                None,
            )
            .await
            .expect("pending question");
        broker
            .respond_question(request_id, &approver, response.clone())
            .await
            .expect("response");
        assert_eq!(receiver.await.expect("agent response"), response);
    }
    assert!(broker.list_questions(true).await.is_empty());
}

// R1: a Turn cancellation answers every pending question before the Turn ends.
#[tokio::test]
async fn cancelling_a_session_answers_its_pending_questions_only() {
    use crate::interaction_broker::QuestionResponse;
    use message_board::{HumanId, Identity};

    let (broker, _generation, _directory) = fixture_broker().await;
    let requester =
        board_session_ref(&session(&broker.service_id, "provider-session")).expect("requester");
    let other_requester =
        board_session_ref(&session(&broker.service_id, "other-session")).expect("other requester");
    let approver = Identity::Human {
        human_id: HumanId::try_from("owner".to_owned()).expect("human ID"),
    };
    let question = |request_id: &str| {
        serde_json::from_value(json!({
            "requestId":request_id,"prompt":"Proceed?","fields":[
                {"kind":"boolean","fieldId":"yes","label":"Yes","description":null,"required":true}
            ]
        }))
        .expect("question")
    };
    let first = broker
        .request_question(
            requester.clone(),
            approver.clone(),
            question("q-first"),
            None,
        )
        .await
        .expect("first");
    let second = broker
        .request_question(
            requester.clone(),
            approver.clone(),
            question("q-second"),
            None,
        )
        .await
        .expect("second");
    let mut other = broker
        .request_question(other_requester, approver, question("q-other"), None)
        .await
        .expect("other");
    assert_eq!(
        broker
            .cancel_questions(&requester, "turn cancelled")
            .await
            .expect("cancel")
            .len(),
        2
    );
    assert_eq!(
        first.await.expect("first response"),
        QuestionResponse::Cancelled
    );
    assert_eq!(
        second.await.expect("second response"),
        QuestionResponse::Cancelled
    );
    assert_eq!(broker.list_questions(true).await.len(), 1);
    assert!(matches!(
        other.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    let all = broker.list_questions(false).await;
    assert_eq!(all.len(), 3);
    assert!(
        all.iter()
            .filter(|record| matches!(record,
                crate::interaction_broker::InteractionHistoryRecord::Question {
                    state: crate::interaction_broker::QuestionHistoryState::Cancelled { reason }, ..
                } if reason.as_str() == "turn cancelled"
            ))
            .count()
            == 2
    );
}

// R2: an agent withdrawal settles only its own request.
#[tokio::test]
async fn agent_withdrawal_cancels_one_question_without_touching_the_next() {
    use crate::interaction_broker::QuestionResponse;
    use message_board::{HumanId, Identity};

    let (broker, _generation, _directory) = fixture_broker().await;
    let requester =
        board_session_ref(&session(&broker.service_id, "provider-session")).expect("requester");
    let approver = Identity::Human {
        human_id: HumanId::try_from("owner".to_owned()).expect("human ID"),
    };
    let question = |request_id: &str| {
        serde_json::from_value(json!({
            "requestId":request_id,"prompt":"Proceed?","fields":[
                {"kind":"boolean","fieldId":"yes","label":"Yes","description":null,"required":true}
            ]
        }))
        .expect("question")
    };
    let withdrawn = broker
        .request_question(
            requester.clone(),
            approver.clone(),
            question("withdrawn"),
            None,
        )
        .await
        .expect("first");
    let mut still_pending = broker
        .request_question(requester, approver, question("still-pending"), None)
        .await
        .expect("second");
    broker
        .cancel_question("withdrawn", "agent withdrew")
        .await
        .expect("withdraw");
    assert_eq!(
        withdrawn.await.expect("response"),
        QuestionResponse::Cancelled
    );
    assert!(matches!(
        still_pending.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(broker.list_questions(true).await.len(), 1);
}

// R5: after a host restart, no pending interaction is answerable by the lost
// agent connection. Keep both rows in history with one explicit cause.
#[tokio::test]
async fn restart_cancels_populated_pending_interaction_history() {
    use message_board::{HumanId, Identity};

    let (broker, generation, directory) = fixture_broker().await;
    let expiry = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
    let (_legacy_params, _legacy_receiver) = insert_pending(&broker, generation, expiry).await;
    let frozen_before = tokio::fs::read(directory.join("approval-history.json"))
        .await
        .expect("frozen history");
    let requester =
        board_session_ref(&session(&broker.service_id, "provider-session")).expect("requester");
    let approver = Identity::Human {
        human_id: HumanId::try_from("owner".to_owned()).expect("human ID"),
    };
    let approval_receiver = broker
        .request_typed_approval(
            requester.clone(),
            approver.clone(),
            typed_approval_request("restart-approval"),
            tokio_util::sync::CancellationToken::new(),
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .expect("approval");
    let question = serde_json::from_value(json!({
        "requestId":"restart-question","prompt":"Proceed?","fields":[
            {"kind":"boolean","fieldId":"yes","label":"Yes","description":null,"required":true}
        ]
    }))
    .expect("question");
    let question_receiver = broker
        .request_question(requester, approver.clone(), question, None)
        .await
        .expect("question");
    let service_id = broker.service_id.clone();
    let backend = broker.backend.clone();
    drop(broker);
    let broker_after_restart =
        ServiceInteractionBroker::load(service_id, backend, directory.join("approval-routes.json"))
            .await
            .expect("reload");
    assert_eq!(
        tokio::fs::read(directory.join("approval-history.json"))
            .await
            .expect("frozen history"),
        frozen_before
    );
    assert!(
        broker_after_restart
            .list_typed_approvals(true)
            .await
            .is_empty()
    );
    assert!(broker_after_restart.list_questions(true).await.is_empty());
    let records = stored_interaction_records(&directory).await;
    assert!(matches!(records["restart-approval"].approval_state(),
        Some(crate::interaction_broker::InteractionHistoryState::Cancelled { reason }) if reason.as_str() == "hostRestarted"));
    assert!(matches!(&records["restart-question"],
        crate::interaction_broker::InteractionHistoryRecord::Question {
            state: crate::interaction_broker::QuestionHistoryState::Cancelled { reason }, ..
        } if reason.as_str() == "hostRestarted"));
    assert!(matches!(
        broker_after_restart
            .decide_typed_interaction("restart-approval", &approver, select_typed("allow-once"))
            .await,
        Err(crate::interaction_broker::InteractionHistoryError::AlreadySettled)
    ));
    assert!(matches!(
        broker_after_restart
            .decide(ApprovalDecideParams {
                request_id: "restart-approval".into(),
                decision: None,
                option_id: Some("allow-once".into()),
                acknowledge_persistent: false,
                note: None,
                actor: approver.clone(),
            })
            .await,
        Err(ApprovalDecisionError::Code("alreadySettled"))
    ));
    assert!(matches!(
        broker_after_restart
            .respond_question(
                "restart-question",
                &approver,
                crate::interaction_broker::QuestionResponse::Declined
            )
            .await,
        Err(crate::interaction_broker::InteractionHistoryError::AlreadySettled)
    ));
    assert!(approval_receiver.await.is_err());
    assert!(question_receiver.await.is_err());
}
