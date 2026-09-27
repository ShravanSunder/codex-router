use super::*;
use collaboration_protocol::{CodexGeneration, EndpointId, EndpointRef};

struct FakeApprovalDelivery(DeliveryOutcome);

impl crate::SessionMessageDelivery for FakeApprovalDelivery {
    fn deliver<'a>(
        &'a self,
        _: crate::DeliveryRequest,
        _: &'a dyn crate::AttemptEvidenceSink,
    ) -> crate::DeliveryFuture<'a, collaboration_protocol::DeliveryReceipt> {
        Box::pin(async move {
            Ok(collaboration_protocol::DeliveryReceipt {
                outcome: self.0.clone(),
                reachability: Some(collaboration_protocol::SessionReachability::CodexAppServer),
                client: None,
            })
        })
    }

    fn reconcile_attempt(
        &self,
        _: crate::AttemptReconciliationContext,
    ) -> crate::DeliveryFuture<'_, crate::AttemptReconciliation> {
        Box::pin(async { Ok(crate::AttemptReconciliation::StillUnknown) })
    }
}

#[tokio::test]
async fn approval_notice_uses_selected_delivery_outcome() {
    for (outcome, delivered) in [
        (DeliveryOutcome::Started, true),
        (DeliveryOutcome::Unknown, true),
        (
            DeliveryOutcome::NotSubmitted {
                retryable: true,
                reason: "starting".into(),
            },
            false,
        ),
        (
            DeliveryOutcome::Rejected(collaboration_protocol::DeliveryRejection {
                reason: collaboration_protocol::DeliveryRejectionReason::Busy,
                next_action: collaboration_protocol::DeliveryNextAction::InspectTarget,
                client_code: None,
                detail: None,
            }),
            false,
        ),
    ] {
        let (broker, generation, _) = fixture_broker().await;
        let expiry = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
        let (params, _receiver) = insert_pending(&broker, generation, expiry).await;
        let record = broker
            .pending
            .lock()
            .await
            .get(&params.request_id)
            .expect("pending fixture")
            .record
            .clone();
        broker
            .install_session_delivery(Arc::new(FakeApprovalDelivery(outcome)))
            .expect("delivery injection");
        assert_eq!(broker.deliver(&record).await.is_ok(), delivered);
    }
}

pub(super) fn session(service_id: &UuidIdentity, id: &str) -> SessionRef {
    SessionRef {
        endpoint: EndpointRef {
            service_id: service_id.clone(),
            endpoint_id: EndpointId::try_from("codex-local".to_owned())
                .unwrap_or_else(|error| panic!("endpoint: {error}")),
        },
        session_id: id
            .to_owned()
            .try_into()
            .unwrap_or_else(|error| panic!("session: {error}")),
    }
}

fn typed_approval_request(request_id: &str) -> session_event_model::ApprovalRequest {
    serde_json::from_value(json!({
        "requestId":request_id,"title":"Run command","options":[
            {"optionId":"allow-once","label":"Allow once","choice":{"effect":"allow","scope":"once"}}
        ]
    }))
    .expect("typed approval request")
}

pub(super) async fn fixture_broker() -> (Arc<ServiceInteractionBroker>, CodexGeneration, PathBuf) {
    let service_id = crate::new_service_uuid().unwrap_or_else(|error| panic!("service: {error}"));
    let generation: CodexGeneration = serde_json::from_value(json!({
        "serviceEpoch": String::from(service_id.clone()), "generation": 1
    }))
    .unwrap_or_else(|error| panic!("generation: {error}"));
    let gate = crate::NativeGenerationGate::default();
    gate.activate(
        generation.clone(),
        PathBuf::from("/tmp/approval-backend.sock"),
        None,
    )
    .unwrap_or_else(|error| panic!("activate: {error}"));
    let endpoint = EndpointRef {
        service_id: service_id.clone(),
        endpoint_id: "codex-local"
            .to_owned()
            .try_into()
            .unwrap_or_else(|error| panic!("endpoint: {error}")),
    };
    let directory = std::env::temp_dir().join(format!(
        "approval-broker-{}",
        String::from(crate::new_service_uuid().unwrap_or_else(|error| panic!("path: {error}")))
    ));
    tokio::fs::create_dir_all(&directory)
        .await
        .unwrap_or_else(|error| panic!("directory: {error}"));
    let broker = ServiceInteractionBroker::load(
        service_id,
        NativeControlBackend {
            endpoint,
            gate,
            codex_home: directory.clone(),
        },
        directory.join("approval-routes.json"),
    )
    .await
    .unwrap_or_else(|error| panic!("broker: {error}"));
    (broker, generation, directory)
}

async fn insert_pending(
    broker: &ServiceInteractionBroker,
    generation: CodexGeneration,
    expires_at: String,
) -> (
    ApprovalDecideParams,
    oneshot::Receiver<BrokeredApprovalOutcome>,
) {
    let requester = session(&broker.service_id, "requester");
    let approver = session(&broker.service_id, "approver");
    let request_id = "approval-test".to_owned();
    let record = ApprovalRequestRecord {
        request_id: request_id.clone(),
        requester,
        approver: approver.clone(),
        generation,
        state: ApprovalState::PendingClientDecision,
        reason: None,
        offered_options: Vec::new(),
        presentation: None,
        decision: None,
        operation: json!({"params":{"options":[]}}),
        expires_at,
    };
    let (completion, receiver) = oneshot::channel();
    broker.pending.lock().await.insert(
        request_id.clone(),
        PendingApproval {
            record: record.clone(),
            offered: BTreeMap::from([(ApprovalDecision::Allow, "native-accept".to_owned())]),
            completion,
        },
    );
    broker
        .record(record)
        .await
        .unwrap_or_else(|error| panic!("record: {error}"));
    (
        ApprovalDecideParams {
            request_id,
            decision: Some(ApprovalDecision::Allow),
            option_id: None,
            acknowledge_persistent: false,
            actor: board_identity(&approver).expect("approver identity"),
        },
        receiver,
    )
}

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
        )
        .await
        .expect("record new interaction");

    let wrong_actor = Identity::Human {
        human_id: HumanId::try_from("other".to_owned()).expect("other human ID"),
    };
    assert!(matches!(
        broker
            .decide_typed_interaction("human-approval-1", &wrong_actor, "allow-once", false)
            .await,
        Err(crate::interaction_broker::InteractionHistoryError::WrongActor)
    ));
    broker
        .decide_typed_interaction("human-approval-1", &human, "allow-once", false)
        .await
        .expect("human decision");
    assert_eq!(receiver.await.expect("agent option").as_str(), "allow-once");
    assert!(matches!(
        broker
            .decide_typed_interaction("human-approval-1", &human, "allow-once", false)
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
    let new_records: std::collections::BTreeMap<
        String,
        crate::interaction_broker::InteractionHistoryRecord,
    > = serde_json::from_slice(&new_bytes).expect("typed interaction history");
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
    let reloaded = ServiceInteractionBroker::load(
        broker.service_id.clone(),
        broker.backend.clone(),
        directory.join("approval-routes.json"),
    )
    .await
    .expect("reload typed refusal");
    assert_eq!(reloaded.list_interactions().await, recorded);
}

// R17: the exact option ID is returned to the waiting agent. Persistent scope
// and destination remain visible in the pending request before the decision.
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
    assert_eq!(
        receiver.await.expect("agent option ID").as_str(),
        "allow-always"
    );
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
        )
        .await
        .expect("pending request");
    let params = |decision| ApprovalDecideParams {
        request_id: "claude-approval".into(),
        decision: Some(decision),
        option_id: None,
        acknowledge_persistent: false,
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
    assert_eq!(receiver.await.expect("agent option ID").as_str(), "no-once");
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
            {"kind":"singleChoice","fieldId":"color","label":"Color","description":null,"required":true,"options":["red","blue"]}
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
            .request_question(requester.clone(), approver.clone(), duplicate_fields)
            .await,
        Err(crate::interaction_broker::InteractionHistoryError::InvalidQuestion)
    ));
    let receiver = broker
        .request_question(requester.clone(), approver.clone(), question("q-answer"))
        .await
        .expect("pending question");
    assert_eq!(broker.list_questions(true).await.len(), 1);
    let answer = QuestionResponse::Answered {
        content: serde_json::from_value(json!({"count":3,"dryRun":true,"color":"blue"}))
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
                        json!({"count":"three","dryRun":true,"color":"blue"})
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
            content: serde_json::from_value(json!({"count":3,"dryRun":true,"color":"green"})).expect("content"),
        }).await,
        Err(crate::interaction_broker::InteractionHistoryError::InvalidAnswer { field_id }) if field_id == "color"
    ));
    assert_eq!(broker.list_questions(true).await.len(), 1);
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
            .request_question(requester.clone(), approver.clone(), question(request_id))
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
        .request_question(requester.clone(), approver.clone(), question("q-first"))
        .await
        .expect("first");
    let second = broker
        .request_question(requester.clone(), approver.clone(), question("q-second"))
        .await
        .expect("second");
    let mut other = broker
        .request_question(other_requester, approver, question("q-other"))
        .await
        .expect("other");
    assert_eq!(
        broker
            .cancel_questions(&requester, "turn cancelled")
            .await
            .expect("cancel"),
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
                } if reason == "turn cancelled"
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
        .request_question(requester.clone(), approver.clone(), question("withdrawn"))
        .await
        .expect("first");
    let mut still_pending = broker
        .request_question(requester, approver, question("still-pending"))
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
        .request_question(requester, approver.clone(), question)
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
    let records: std::collections::BTreeMap<
        String,
        crate::interaction_broker::InteractionHistoryRecord,
    > = serde_json::from_slice(
        &tokio::fs::read(directory.join("interaction-history.json"))
            .await
            .expect("history"),
    )
    .expect("persisted rows");
    assert!(matches!(records["restart-approval"].approval_state(),
        Some(crate::interaction_broker::InteractionHistoryState::Cancelled { reason }) if reason == "hostRestarted"));
    assert!(matches!(&records["restart-question"],
        crate::interaction_broker::InteractionHistoryRecord::Question {
            state: crate::interaction_broker::QuestionHistoryState::Cancelled { reason }, ..
        } if reason == "hostRestarted"));
    assert!(matches!(
        broker_after_restart
            .decide_typed_interaction("restart-approval", &approver, "allow-once", false)
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
