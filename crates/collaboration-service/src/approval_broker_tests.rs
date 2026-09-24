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

fn session(service_id: &UuidIdentity, id: &str) -> SessionRef {
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

async fn fixture_broker() -> (Arc<ServiceApprovalBroker>, CodexGeneration, PathBuf) {
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
    let broker = ServiceApprovalBroker::load(
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
    broker: &ServiceApprovalBroker,
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
            generation_authority: ApprovalGenerationAuthority::Native,
        },
    );
    broker
        .record(record)
        .await
        .unwrap_or_else(|error| panic!("record: {error}"));
    (
        ApprovalDecideParams {
            request_id,
            decision: ApprovalDecision::Allow,
            actor: approver,
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
    requester.actor = session(&broker.service_id, "requester");
    assert!(matches!(
        broker.decide(requester).await,
        Err("selfDecision")
    ));
    let mut wrong = params.clone();
    wrong.actor = session(&broker.service_id, "other");
    assert!(matches!(broker.decide(wrong).await, Err("wrongActor")));
    let mut unavailable = params.clone();
    unavailable.decision = ApprovalDecision::AllowForSession;
    assert!(matches!(
        broker.decide(unavailable).await,
        Err("decisionNotOffered")
    ));
    let receipt = broker
        .decide(params.clone())
        .await
        .unwrap_or_else(|error| panic!("decision: {error}"));
    assert_eq!(receipt.state, ApprovalState::Decided);
    assert!(matches!(
        broker.decide(params).await,
        Err("approvalNotPending")
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

#[test]
fn external_options_preserve_once_scope_and_reject_ambiguous_or_persistent_grants() {
    let mapped = map_external_options(vec![
        ExternalApprovalOption {
            option_id: "allow-once".to_owned(),
            scope: ExternalApprovalOptionScope::AllowOnce,
        },
        ExternalApprovalOption {
            option_id: "allow-always".to_owned(),
            scope: ExternalApprovalOptionScope::AllowAlways,
        },
        ExternalApprovalOption {
            option_id: "reject-once".to_owned(),
            scope: ExternalApprovalOptionScope::RejectOnce,
        },
    ])
    .expect("once-scoped options map");
    assert_eq!(
        mapped.get(&ApprovalDecision::Allow),
        Some(&"allow-once".to_owned())
    );
    assert_eq!(
        mapped.get(&ApprovalDecision::Deny),
        Some(&"reject-once".to_owned())
    );
    assert!(!mapped.contains_key(&ApprovalDecision::AllowForSession));

    assert!(
        map_external_options(vec![ExternalApprovalOption {
            option_id: "allow-always".to_owned(),
            scope: ExternalApprovalOptionScope::AllowAlways,
        }])
        .is_err()
    );
    assert!(
        map_external_options(vec![
            ExternalApprovalOption {
                option_id: "first".to_owned(),
                scope: ExternalApprovalOptionScope::AllowOnce,
            },
            ExternalApprovalOption {
                option_id: "second".to_owned(),
                scope: ExternalApprovalOptionScope::AllowOnce,
            },
        ])
        .is_err()
    );
    assert!(
        map_external_options(vec![
            ExternalApprovalOption {
                option_id: "duplicate".to_owned(),
                scope: ExternalApprovalOptionScope::AllowOnce,
            },
            ExternalApprovalOption {
                option_id: "duplicate".to_owned(),
                scope: ExternalApprovalOptionScope::RejectOnce,
            },
        ])
        .is_err()
    );
}

#[tokio::test]
async fn external_generation_is_independent_and_decision_remains_actor_bound_single_use() {
    let (broker, native_generation, _directory) = fixture_broker().await;
    let external_generation: CodexGeneration = serde_json::from_value(json!({
        "serviceEpoch": String::from(broker.service_id.clone()), "generation": 99
    }))
    .expect("external generation");
    assert_ne!(external_generation, native_generation);
    let requester = session(&broker.service_id, "external-requester");
    let approver = session(&broker.service_id, "external-approver");
    let request_id = "external-approval-test".to_owned();
    let record = ApprovalRequestRecord {
        request_id: request_id.clone(),
        requester,
        approver: approver.clone(),
        generation: external_generation.clone(),
        state: ApprovalState::PendingClientDecision,
        decision: None,
        operation: json!({"kind":"externalProviderPermission","operationId":"019f0000-0000-7000-8000-000000001101"}),
        expires_at: (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
    };
    let (completion, receiver) = oneshot::channel();
    broker.pending.lock().await.insert(
        request_id.clone(),
        PendingApproval {
            record: record.clone(),
            offered: BTreeMap::from([(ApprovalDecision::Allow, "allow-once".to_owned())]),
            completion,
            generation_authority: ApprovalGenerationAuthority::External {
                generation: external_generation,
                retirement: tokio_util::sync::CancellationToken::new(),
            },
        },
    );
    broker
        .record(record)
        .await
        .expect("record external approval");
    let receipt = broker
        .decide(ApprovalDecideParams {
            request_id: request_id.clone(),
            decision: ApprovalDecision::Allow,
            actor: approver,
        })
        .await
        .expect("external decision");
    assert_eq!(receipt.scope, None);
    assert_eq!(
        receiver.await.expect("outcome"),
        BrokeredApprovalOutcome::Selected {
            option_id: "allow-once".to_owned()
        }
    );
    assert!(matches!(
        broker
            .decide(ApprovalDecideParams {
                request_id,
                decision: ApprovalDecision::Allow,
                actor: session(&broker.service_id, "external-approver"),
            })
            .await,
        Err("approvalNotPending")
    ));
}

#[tokio::test]
async fn unreachable_external_approver_cancels_with_metadata_only_history() {
    let (broker, generation, _directory) = fixture_broker().await;
    let metadata = ExternalApprovalOperationMetadata {
        operation_id: "019f0000-0000-7000-8000-000000001102"
            .to_owned()
            .try_into()
            .expect("operation ID"),
        target: session(&broker.service_id, "provider-session"),
        binding_generation: generation.clone(),
        method: "session/request_permission",
    };
    let operation = serde_json::json!({
        "kind":"externalProviderPermission",
        "operationId":"019f0000-0000-7000-8000-000000001102",
        "target":session(&broker.service_id, "provider-session"),
        "bindingGeneration":generation.clone(),
        "method":"session/request_permission"
    });
    let outcome = broker
        .request_external(ExternalApprovalRequest {
            requester: session(&broker.service_id, "external-requester"),
            approver: session(&broker.service_id, "external-approver"),
            generation,
            retirement: tokio_util::sync::CancellationToken::new(),
            operation_metadata: metadata,
            transient_presentation: Some(serde_json::json!({
                "title":"TRANSIENT_PRESENTATION_SENTINEL",
                "name":"router-collaboration-endpoints_list",
                "kind":"other"
            })),
            options: vec![ExternalApprovalOption {
                option_id: "allow-once".to_owned(),
                scope: ExternalApprovalOptionScope::AllowOnce,
            }],
        })
        .await
        .expect("external request settles");
    assert_eq!(outcome, BrokeredApprovalOutcome::Cancelled);
    let record = broker.list(false).await.approvals.pop().expect("history");
    assert_eq!(record.state, ApprovalState::ApproverUnreachable);
    assert_eq!(record.operation, operation);
    let encoded = serde_json::to_string(&record).expect("history JSON");
    assert!(!encoded.contains("toolCall"));
    assert!(!encoded.contains("prompt"));
    assert!(!encoded.contains("TRANSIENT_PRESENTATION_SENTINEL"));
    assert!(!encoded.contains("router-collaboration-endpoints_list"));
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
    assert!(matches!(broker.decide(expired).await, Err("expired")));
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
    assert!(matches!(broker.decide(stale).await, Err("oldGeneration")));
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
    let loaded = ServiceApprovalBroker::load(
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
    let absent = ServiceApprovalBroker::load(
        service_id.clone(),
        backend(directory.clone()),
        routes.clone(),
    )
    .await;

    // Assert.
    assert!(absent.is_ok());

    // Act: a malformed history file refuses to load.
    std::fs::write(directory.join("approval-history.json"), b"{not json").unwrap();
    let corrupt = ServiceApprovalBroker::load(service_id.clone(), backend(directory), routes).await;

    // Assert.
    assert!(matches!(corrupt, Err(ApprovalBrokerError::Unavailable)));
}
