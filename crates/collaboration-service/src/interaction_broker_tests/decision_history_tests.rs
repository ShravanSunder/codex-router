use super::*;

#[tokio::test]
async fn decision_is_single_use_actor_bound_and_maps_only_offered_options() {
    let (broker, generation, _directory) = fixture_broker().await;
    let expiry = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
    let (params, receiver) = insert_pending(&broker, generation, expiry).await;
    let mut requester = params.clone();
    requester.actor = board_identity(&session(&broker.service_id, "requester")).expect("actor");
    assert!(matches!(
        broker.decide(requester).await,
        Err(ApprovalDecisionError::Code("selfDecision"))
    ));
    let mut wrong = params.clone();
    wrong.actor = board_identity(&session(&broker.service_id, "other")).expect("actor");
    assert!(matches!(
        broker.decide(wrong).await,
        Err(ApprovalDecisionError::Code("wrongActor"))
    ));
    let mut unavailable = params.clone();
    unavailable.decision = Some(ApprovalDecision::AllowForSession);
    assert!(matches!(
        broker.decide(unavailable).await,
        Err(ApprovalDecisionError::OptionNotOffered { .. })
    ));
    let receipt = broker
        .decide(params.clone())
        .await
        .unwrap_or_else(|error| panic!("decision: {error}"));
    assert_eq!(receipt.state, ApprovalState::Decided);
    assert!(matches!(
        broker.decide(params).await,
        Err(ApprovalDecisionError::Code("approvalNotPending"))
    ));
    assert_eq!(
        receiver.await.unwrap_or_else(|_| panic!("outcome")),
        BrokeredApprovalOutcome::Selected {
            option_id: "native-accept".to_owned()
        }
    );
    assert_eq!(
        broker.list(false).await.approvals[0].decision,
        Some(ApprovalDecision::Allow)
    );
}

#[tokio::test]
async fn native_request_by_its_own_approver_is_recorded_and_refused() {
    use codex_acp_adapter::{ApprovalBroker as _, ApprovalRoute, BrokeredApprovalRequest};
    use collaboration_protocol::RouterAccess;

    let (broker, generation, _directory) = fixture_broker().await;
    let self_approver = session(&broker.service_id, "self-approver");
    broker
        .register_route(ApprovalRoute {
            thread_id: "self-approver".to_owned(),
            created_by: self_approver.clone(),
            approver: self_approver.clone(),
            access: RouterAccess::WorkspaceWrite,
            scratch_path: "/tmp/approval-self-approver".to_owned(),
            root_message_id: None,
        })
        .await
        .expect("route registered");

    let outcome = broker
        .request(BrokeredApprovalRequest {
            thread_id: "self-approver".to_owned(),
            generation,
            request: json!({
                "method":"item/commandExecution/requestApproval",
                "params":{"options":[{"optionId":"native-accept"}]}
            }),
        })
        .await
        .expect("self-approval refusal settles");

    assert_eq!(outcome, BrokeredApprovalOutcome::Cancelled);
    let history = broker.list(false).await.approvals;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].requester, self_approver);
    assert_eq!(history[0].approver, self_approver);
    assert_eq!(history[0].state, ApprovalState::ApproverIsRequester);
    assert_eq!(
        history[0].reason.as_deref(),
        Some("set a different approver")
    );
    assert_eq!(history[0].offered_options[0].option_id, "native-accept");
    assert_eq!(
        history[0].offered_options[0].scope,
        ApprovalOptionScope::AllowOnce
    );
}

#[tokio::test]
async fn missing_native_approval_route_emits_only_typed_safe_diagnostic() {
    use codex_acp_adapter::{ApprovalBroker as _, BrokeredApprovalRequest};

    let (broker, generation, _directory) = fixture_broker().await;
    let provider_session_id = "requester";
    let request = json!({
        "method":"item/commandExecution/requestApproval",
        "params":{"options":[],"command":"PRIVATE_COMMAND_SENTINEL"}
    });
    let diagnostic = native_approval_refusal_diagnostic(
        &broker.backend.endpoint,
        provider_session_id,
        &request,
        NativeApprovalRefusalReason::MissingRoute,
    );

    assert_eq!(
        diagnostic,
        NativeApprovalRefusalDiagnostic {
            endpoint: "codex-local".to_owned(),
            provider_session_id: "requester".to_owned(),
            method: "item/commandExecution/requestApproval",
            reason_code: "nativeApprovalRouteUnavailable",
        }
    );
    assert!(
        broker
            .request(BrokeredApprovalRequest {
                thread_id: provider_session_id.to_owned(),
                generation,
                request,
            })
            .await
            .is_err()
    );
    assert!(broker.list(false).await.approvals.is_empty());
}

#[tokio::test]
async fn native_request_with_empty_option_mapping_is_recorded_as_refused() {
    use codex_acp_adapter::{ApprovalBroker as _, ApprovalRoute, BrokeredApprovalRequest};
    use collaboration_protocol::RouterAccess;

    let (broker, generation, _directory) = fixture_broker().await;
    let requester = session(&broker.service_id, "requester");
    let approver = session(&broker.service_id, "approver");
    broker
        .register_route(ApprovalRoute {
            thread_id: "requester".to_owned(),
            created_by: requester.clone(),
            approver: approver.clone(),
            access: RouterAccess::WorkspaceWrite,
            scratch_path: "/tmp/approval-empty-options".to_owned(),
            root_message_id: None,
        })
        .await
        .expect("route registered");

    let outcome = broker
        .request(BrokeredApprovalRequest {
            thread_id: "requester".to_owned(),
            generation,
            request: json!({
                "method":"item/commandExecution/requestApproval",
                "params":{"options":[]}
            }),
        })
        .await
        .expect("invalid native approval is refused");

    assert_eq!(outcome, BrokeredApprovalOutcome::Cancelled);
    assert!(broker.pending.lock().await.is_empty());
    let record = broker
        .list(false)
        .await
        .approvals
        .pop()
        .expect("refusal history");
    assert_eq!(record.state, ApprovalState::Cancelled);
    assert_eq!(
        record.reason.as_deref(),
        Some("native permission request has no supported decision option")
    );
}

#[tokio::test]
async fn terminal_history_failure_is_visible_and_does_not_strand_pending_entry() {
    let (broker, generation, _directory) = fixture_broker().await;
    let expiry = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
    let (params, _receiver) = insert_pending(&broker, generation, expiry).await;
    tokio::fs::remove_file(&broker.history_path)
        .await
        .expect("remove file before simulating storage failure");
    tokio::fs::create_dir(&broker.history_path)
        .await
        .expect("replace history file with directory");

    let result = broker
        .finish_pending(
            &params.request_id,
            ApprovalState::TimedOut,
            Some("approval timed out"),
        )
        .await;

    assert!(matches!(result, Err(ApprovalBrokerError::Unavailable)));
    assert!(broker.pending.lock().await.is_empty());
    let record = broker
        .list(false)
        .await
        .approvals
        .pop()
        .expect("visible failure row");
    assert_eq!(record.state, ApprovalState::TimedOut);
    assert_eq!(
        record.reason.as_deref(),
        Some("approval ended, but its history could not be persisted; inspect Router diagnostics")
    );
}

#[tokio::test]
async fn expired_and_old_generation_decisions_are_refused_with_terminal_history() {
    let (broker, generation, _directory) = fixture_broker().await;
    let (expired, _receiver) = insert_pending(
        &broker,
        generation.clone(),
        (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339(),
    )
    .await;
    assert!(matches!(
        broker.decide(expired).await,
        Err(ApprovalDecisionError::Code("expired"))
    ));
    assert_eq!(
        broker.list(false).await.approvals[0].state,
        ApprovalState::TimedOut
    );

    let (broker, generation, _directory) = fixture_broker().await;
    let (stale, _receiver) = insert_pending(
        &broker,
        generation,
        (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
    )
    .await;
    broker
        .backend
        .gate
        .retire()
        .unwrap_or_else(|error| panic!("retire: {error}"));
    let next_generation: CodexGeneration = serde_json::from_value(json!({
        "serviceEpoch": String::from(broker.service_id.clone()), "generation": 2
    }))
    .unwrap_or_else(|error| panic!("next generation: {error}"));
    broker
        .backend
        .gate
        .activate(
            next_generation,
            PathBuf::from("/tmp/approval-backend-2.sock"),
            None,
        )
        .unwrap_or_else(|error| panic!("reactivate: {error}"));
    assert!(matches!(
        broker.decide(stale).await,
        Err(ApprovalDecisionError::Code("oldGeneration"))
    ));
    assert_eq!(
        broker.list(false).await.approvals[0].state,
        ApprovalState::Cancelled
    );
}

#[tokio::test]
async fn malformed_persisted_routes_fail_closed() {
    let service_id = crate::new_service_uuid().unwrap();
    let generation: CodexGeneration = serde_json::from_value(json!({
        "serviceEpoch": String::from(service_id.clone()), "generation": 1
    }))
    .unwrap();
    let gate = crate::NativeGenerationGate::default();
    gate.activate(
        generation,
        PathBuf::from("/tmp/approval-corrupt.sock"),
        None,
    )
    .unwrap();
    let endpoint = EndpointRef {
        service_id: service_id.clone(),
        endpoint_id: "codex-local".to_owned().try_into().unwrap(),
    };
    let directory = std::env::temp_dir().join(format!(
        "approval-corrupt-{}",
        String::from(crate::new_service_uuid().unwrap())
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let routes = directory.join("approval-routes.json");
    std::fs::write(&routes, br#"[{"threadId":"thread","access":"invalid"}]"#).unwrap();
    let loaded = ServiceInteractionBroker::load(
        service_id.clone(),
        NativeControlBackend {
            endpoint,
            gate,
            codex_home: directory,
        },
        routes,
    )
    .await;
    assert!(matches!(loaded, Err(ApprovalBrokerError::Unavailable)));
}

#[tokio::test]
async fn malformed_persisted_history_fails_closed_while_absence_stays_empty() {
    // Arrange: valid routes beside an unreadable decision history.
    let service_id = crate::new_service_uuid().unwrap();
    let generation: CodexGeneration = serde_json::from_value(json!({
        "serviceEpoch": String::from(service_id.clone()), "generation": 1
    }))
    .unwrap();
    let gate = crate::NativeGenerationGate::default();
    gate.activate(
        generation,
        PathBuf::from("/tmp/approval-history-corrupt.sock"),
        None,
    )
    .unwrap();
    let endpoint = EndpointRef {
        service_id: service_id.clone(),
        endpoint_id: "codex-local".to_owned().try_into().unwrap(),
    };
    let directory = std::env::temp_dir().join(format!(
        "approval-history-corrupt-{}",
        String::from(crate::new_service_uuid().unwrap())
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let routes = directory.join("approval-routes.json");
    std::fs::write(&routes, b"[]").unwrap();
    let backend = |directory: PathBuf| NativeControlBackend {
        endpoint: endpoint.clone(),
        gate: gate.clone(),
        codex_home: directory,
    };

    // Act: a missing history file is an empty history.
    let absent = ServiceInteractionBroker::load(
        service_id.clone(),
        backend(directory.clone()),
        routes.clone(),
    )
    .await;

    // Assert.
    assert!(absent.is_ok());

    // Act: a malformed history file refuses to load.
    std::fs::write(directory.join("approval-history.json"), b"{not json").unwrap();
    let corrupt =
        ServiceInteractionBroker::load(service_id.clone(), backend(directory), routes).await;

    // Assert.
    assert!(matches!(corrupt, Err(ApprovalBrokerError::Unavailable)));
}

// F9 and E12: v0.1.38's unchanged ApprovalRequestRecord reader must load a
// populated approval-history.json after the new interaction store is written.
// `git diff v0.1.38 -- crates/collaboration-protocol/src/approval_contract.rs`
// is empty at this slice's base, so the imported type is that exact reader.
#[tokio::test]
async fn populated_old_approval_reader_survives_human_interaction_history() {
    use message_board::{HumanId, Identity};

    let (broker, generation, directory) = fixture_broker().await;
    let expiry = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
    let (_legacy_params, _legacy_receiver) = insert_pending(&broker, generation, expiry).await;
    let requester: message_board::SessionRef = serde_json::from_value(
        serde_json::to_value(session(&broker.service_id, "provider-session"))
            .expect("requester JSON"),
    )
    .expect("board requester");
    let human = Identity::Human {
        human_id: HumanId::try_from("owner".to_owned()).expect("human ID"),
    };
    let receiver = broker
        .request_typed_approval(
            requester,
            human.clone(),
            typed_approval_request("human-approval-1"),
            tokio_util::sync::CancellationToken::new(),
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .expect("record new interaction");

    let wrong_actor = Identity::Human {
        human_id: HumanId::try_from("other".to_owned()).expect("other human ID"),
    };
    assert!(matches!(
        broker
            .decide_typed_interaction("human-approval-1", &wrong_actor, select_typed("allow-once"))
            .await,
        Err(crate::interaction_broker::InteractionHistoryError::WrongActor)
    ));
    broker
        .decide_typed_interaction("human-approval-1", &human, select_typed("allow-once"))
        .await
        .expect("human decision");
    assert!(matches!(receiver.await.expect("agent option"),
        crate::interaction_broker::TypedApprovalResolution::Selected(selected)
            if selected.option_id.as_str() == "allow-once"));
    assert!(matches!(
        broker
            .decide_typed_interaction("human-approval-1", &human, select_typed("allow-once"))
            .await,
        Err(crate::interaction_broker::InteractionHistoryError::AlreadySettled)
    ));

    let old_bytes = tokio::fs::read(directory.join("approval-history.json"))
        .await
        .expect("populated legacy history");
    let old_records: Vec<ApprovalRequestRecord> =
        serde_json::from_slice(&old_bytes).expect("v0.1.38 reader loads new-build history");
    assert_eq!(old_records.len(), 1);
    let default_list = serde_json::to_value(broker.list(false).await).expect("default list");
    let frozen_list: collaboration_protocol::ApprovalListResult =
        serde_json::from_value(default_list.clone()).expect("0.1.38 list reader");
    assert_eq!(frozen_list.approvals.len(), 1);
    assert_eq!(
        default_list,
        json!({"approvals":[serde_json::to_value(&old_records[0]).expect("old record")]})
    );
    let new_bytes = tokio::fs::read(directory.join("interaction-history.json"))
        .await
        .expect("new interaction history");
    let new_records = interaction_records_without_timestamps(&new_bytes);
    assert!(matches!(
        new_records["human-approval-1"].approval_state(),
        Some(crate::interaction_broker::InteractionHistoryState::Decided { option_id })
            if option_id.as_str() == "allow-once"
    ));
}

#[tokio::test]
async fn typed_interaction_rejects_self_approver_and_corrupt_stored_rows() {
    use message_board::Identity;

    let (broker, _generation, directory) = fixture_broker().await;
    let requester: message_board::SessionRef = serde_json::from_value(
        serde_json::to_value(session(&broker.service_id, "provider-session"))
            .expect("requester JSON"),
    )
    .expect("board requester");
    assert!(matches!(
        broker
            .request_typed_approval(
                requester.clone(),
                Identity::Session { session: requester },
                typed_approval_request("self-request"),
                tokio_util::sync::CancellationToken::new(),
                tokio_util::sync::CancellationToken::new(),
                None
            )
            .await,
        Err(crate::interaction_broker::InteractionHistoryError::SelfApprover)
    ));
    let history_path = directory.join("interaction-history.json");
    tokio::fs::write(&history_path, br#"{"bad":{"requestId":"other"}}"#)
        .await
        .expect("write corrupt stored row");
    let reload = ServiceInteractionBroker::load(
        broker.service_id.clone(),
        broker.backend.clone(),
        directory.join("approval-routes.json"),
    )
    .await;
    assert!(matches!(reload, Err(ApprovalBrokerError::Unavailable)));
}

#[tokio::test]
async fn typed_approval_rejects_foreign_service_participants_without_pending_history() {
    use message_board::Identity;

    let (broker, _, _) = fixture_broker().await;
    let foreign_service = crate::new_service_uuid().expect("foreign service");
    let foreign_requester =
        board_session_ref(&session(&foreign_service, "provider-session")).expect("requester");
    let local_approver =
        board_session_ref(&session(&broker.service_id, "approver")).expect("approver");
    assert!(matches!(
        broker
            .request_typed_approval(
                foreign_requester,
                Identity::Session {
                    session: local_approver,
                },
                typed_approval_request("foreign-service"),
                tokio_util::sync::CancellationToken::new(),
                tokio_util::sync::CancellationToken::new(),
                None
            )
            .await,
        Err(crate::interaction_broker::InteractionHistoryError::Unavailable)
    ));
    let local_requester =
        board_session_ref(&session(&broker.service_id, "provider-session")).expect("requester");
    let foreign_approver =
        board_session_ref(&session(&foreign_service, "approver")).expect("foreign approver");
    assert!(matches!(
        broker
            .request_typed_approval(
                local_requester,
                Identity::Session {
                    session: foreign_approver,
                },
                typed_approval_request("foreign-approver"),
                tokio_util::sync::CancellationToken::new(),
                tokio_util::sync::CancellationToken::new(),
                None
            )
            .await,
        Err(crate::interaction_broker::InteractionHistoryError::Unavailable)
    ));
    assert!(broker.list_interactions().await.is_empty());
}

#[tokio::test]
async fn refused_typed_offer_preserves_reviewed_fields_and_order_without_legacy_write() {
    use message_board::{HumanId, Identity};

    let (broker, _, directory) = fixture_broker().await;
    let requester =
        board_session_ref(&session(&broker.service_id, "provider-session")).expect("requester");
    let approver = Identity::Human {
        human_id: HumanId::try_from("owner".to_owned()).expect("human ID"),
    };
    let refusal = crate::interaction_broker::RefusedTypedApproval {
        request_id: "malformed-offer".into(),
        title: "Run command".into(),
        description: Some("Reviewed description".into()),
        subject: None,
        options: vec![
            crate::interaction_broker::RefusedApprovalOption {
                option_id: "duplicate".into(),
                label: "First".into(),
                provider_kind: "AllowOnce".into(),
            },
            crate::interaction_broker::RefusedApprovalOption {
                option_id: "duplicate".into(),
                label: "Second".into(),
                provider_kind: "RejectOnce".into(),
            },
        ],
        reason: "permission options contain a duplicate identifier".into(),
    };
    broker
        .record_typed_refusal(requester, approver, refusal.clone())
        .await
        .expect("persist refusal");
    let recorded = broker.list_interactions().await;
    assert!(matches!(
        &recorded[0],
        crate::interaction_broker::InteractionHistoryRecord::RefusedApproval { refusal: stored, .. }
            if stored == &refusal && stored.options[0].label == "First" && stored.options[1].label == "Second"
    ));
    assert!(!directory.join("approval-history.json").exists());
    let detailed = broker.list_detailed(false).await.expect("detailed list");
    let row = detailed
        .approvals
        .iter()
        .find(|row| row.request_id == "malformed-offer")
        .expect("refusal listed");
    assert_eq!(row.state, collaboration_protocol::ApprovalState::Refused);
    assert_eq!(row.reason.as_deref(), Some(refusal.reason.as_str()));
    let reloaded = ServiceInteractionBroker::load(
        broker.service_id.clone(),
        broker.backend.clone(),
        directory.join("approval-routes.json"),
    )
    .await
    .expect("reload typed refusal");
    assert_eq!(reloaded.list_interactions().await, recorded);
}

#[tokio::test]
async fn self_approver_refusal_is_recorded_and_listed_with_a_fix() {
    let (broker, _, _) = fixture_broker().await;
    let requester =
        board_session_ref(&session(&broker.service_id, "provider-session")).expect("requester");
    let refusal = crate::interaction_broker::RefusedTypedApproval {
        request_id: "self-offer".into(),
        title: "Run command".into(),
        description: None,
        subject: None,
        options: Vec::new(),
        reason: "set a different approver".into(),
    };
    broker
        .record_typed_refusal(
            requester.clone(),
            message_board::Identity::Session { session: requester },
            refusal,
        )
        .await
        .expect("self refusal recorded");
    let row = broker
        .list_detailed(false)
        .await
        .expect("detailed list")
        .approvals
        .into_iter()
        .find(|row| row.request_id == "self-offer")
        .expect("self refusal listed");
    assert_eq!(
        row.state,
        collaboration_protocol::ApprovalState::ApproverIsRequester
    );
    assert_eq!(row.reason.as_deref(), Some("set a different approver"));
}

// R17: the exact option ID is returned to the waiting agent. Persistent scope
// and destination remain visible in the pending request before the decision.
