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

struct PendingApprovalNoticeDelivery;

impl crate::SessionMessageDelivery for PendingApprovalNoticeDelivery {
    fn deliver<'a>(
        &'a self,
        _: crate::DeliveryRequest,
        _: &'a dyn crate::AttemptEvidenceSink,
    ) -> crate::DeliveryFuture<'a, collaboration_protocol::DeliveryReceipt> {
        Box::pin(std::future::pending())
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

pub(super) async fn fixture_broker() -> (Arc<ServiceApprovalBroker>, CodexGeneration, PathBuf) {
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

#[test]
fn external_options_preserve_once_scope_and_reject_ambiguous_or_persistent_grants() {
    let mapped = map_external_options(vec![
        ExternalApprovalOption {
            option_id: "allow-once".to_owned(),
            label: None,
            scope: ExternalApprovalOptionScope::AllowOnce,
        },
        ExternalApprovalOption {
            option_id: "allow-always".to_owned(),
            label: None,
            scope: ExternalApprovalOptionScope::AllowAlways,
        },
        ExternalApprovalOption {
            option_id: "reject-once".to_owned(),
            label: None,
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
            label: None,
            scope: ExternalApprovalOptionScope::AllowAlways,
        }])
        .is_err()
    );
    assert!(
        map_external_options(vec![ExternalApprovalOption {
            option_id: "reject-once".to_owned(),
            label: None,
            scope: ExternalApprovalOptionScope::RejectOnce,
        }])
        .is_err(),
        "a deny-only request has no supported allow choice"
    );
    assert!(
        map_external_options(vec![
            ExternalApprovalOption {
                option_id: "first".to_owned(),
                label: None,
                scope: ExternalApprovalOptionScope::AllowOnce,
            },
            ExternalApprovalOption {
                option_id: "second".to_owned(),
                label: None,
                scope: ExternalApprovalOptionScope::AllowOnce,
            },
        ])
        .is_err()
    );
    assert!(
        map_external_options(vec![
            ExternalApprovalOption {
                option_id: "duplicate".to_owned(),
                label: None,
                scope: ExternalApprovalOptionScope::AllowOnce,
            },
            ExternalApprovalOption {
                option_id: "duplicate".to_owned(),
                label: None,
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
        reason: None,
        offered_options: Vec::new(),
        presentation: None,
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
                turn_cancellation: tokio_util::sync::CancellationToken::new(),
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
            decision: Some(ApprovalDecision::Allow),
            option_id: None,
            acknowledge_persistent: false,
            actor: board_identity(&approver).expect("actor"),
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
                decision: Some(ApprovalDecision::Allow),
                option_id: None,
                acknowledge_persistent: false,
                actor: board_identity(&session(&broker.service_id, "external-approver"))
                    .expect("actor"),
            })
            .await,
        Err(ApprovalDecisionError::Code("approvalNotPending"))
    ));
}

#[tokio::test]
async fn unreachable_external_approver_history_keeps_curated_decision_details() {
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
            cancellation: tokio_util::sync::CancellationToken::new(),
            turn_cancellation: tokio_util::sync::CancellationToken::new(),
            operation_metadata: metadata,
            presentation: Some(ApprovalPresentation {
                tool_name: Some("router-collaboration-endpoints_list".to_owned()),
                title: Some("TRANSIENT_PRESENTATION_SENTINEL".to_owned()),
                kind: Some("other".to_owned()),
                ..ApprovalPresentation::default()
            }),
            options: vec![ExternalApprovalOption {
                option_id: "allow-once".to_owned(),
                label: Some("Allow once".to_owned()),
                scope: ExternalApprovalOptionScope::AllowOnce,
            }],
        })
        .await
        .expect("external request settles");
    assert_eq!(outcome, BrokeredApprovalOutcome::Cancelled);
    let record = broker.list(false).await.approvals.pop().expect("history");
    assert_eq!(record.state, ApprovalState::ApproverUnreachable);
    assert_eq!(record.operation, operation);
    assert_eq!(record.offered_options[0].option_id, "allow-once");
    assert_eq!(
        record.offered_options[0].scope,
        ApprovalOptionScope::AllowOnce
    );
    assert_eq!(
        record
            .presentation
            .as_ref()
            .and_then(|item| item.title.as_deref()),
        Some("TRANSIENT_PRESENTATION_SENTINEL")
    );
    assert_eq!(
        record
            .presentation
            .as_ref()
            .and_then(|item| item.tool_name.as_deref()),
        Some("router-collaboration-endpoints_list")
    );
}

#[tokio::test]
async fn external_approval_timeout_is_recorded_in_history() {
    let (broker, generation, _directory) = fixture_broker().await;
    broker
        .install_session_delivery(Arc::new(FakeApprovalDelivery(DeliveryOutcome::Started)))
        .expect("delivery injection");
    let operation_id = "019f0000-0000-7000-8000-000000001103"
        .to_owned()
        .try_into()
        .expect("operation ID");
    let target = session(&broker.service_id, "provider-session");

    let outcome = broker
        .request_external_with_timeout(
            ExternalApprovalRequest {
                requester: session(&broker.service_id, "requester"),
                approver: session(&broker.service_id, "approver"),
                generation: generation.clone(),
                retirement: tokio_util::sync::CancellationToken::new(),
                cancellation: tokio_util::sync::CancellationToken::new(),
                turn_cancellation: tokio_util::sync::CancellationToken::new(),
                operation_metadata: ExternalApprovalOperationMetadata {
                    operation_id,
                    target,
                    binding_generation: generation,
                    method: "session/request_permission",
                },
                presentation: None,
                options: vec![ExternalApprovalOption {
                    option_id: "allow-once".to_owned(),
                    label: None,
                    scope: ExternalApprovalOptionScope::AllowOnce,
                }],
            },
            Duration::from_millis(1),
        )
        .await
        .expect("external timeout settles");

    assert_eq!(outcome, BrokeredApprovalOutcome::Cancelled);
    let history = broker.list(false).await.approvals;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].state, ApprovalState::TimedOut);
}

#[tokio::test]
async fn external_timeout_covers_notice_delivery_and_cleans_pending_state() {
    let (broker, generation, _directory) = fixture_broker().await;
    broker
        .install_session_delivery(Arc::new(PendingApprovalNoticeDelivery))
        .expect("delivery injection");
    let target = session(&broker.service_id, "provider-target");
    let metadata = ExternalApprovalOperationMetadata {
        operation_id: "019f0000-0000-7000-8000-000000001207"
            .to_owned()
            .try_into()
            .expect("operation ID"),
        target,
        binding_generation: generation.clone(),
        method: "session/request_permission",
    };

    let outcome = broker
        .request_external_with_timeout(
            ExternalApprovalRequest {
                requester: session(&broker.service_id, "requester"),
                approver: session(&broker.service_id, "approver"),
                generation,
                retirement: tokio_util::sync::CancellationToken::new(),
                cancellation: tokio_util::sync::CancellationToken::new(),
                turn_cancellation: tokio_util::sync::CancellationToken::new(),
                operation_metadata: metadata,
                presentation: None,
                options: vec![ExternalApprovalOption {
                    option_id: "allow-once".to_owned(),
                    label: Some("Allow once".to_owned()),
                    scope: ExternalApprovalOptionScope::AllowOnce,
                }],
            },
            Duration::from_millis(1),
        )
        .await
        .expect("timed out notice delivery is refused");

    assert_eq!(outcome, BrokeredApprovalOutcome::Cancelled);
    assert!(broker.pending.lock().await.is_empty());
    let history = broker.list(false).await.approvals;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].state, ApprovalState::TimedOut);
    assert!(history[0].reason.is_some());
}

#[tokio::test]
async fn external_decision_racing_retirement_has_one_terminal_state() {
    let (broker, generation, _directory) = fixture_broker().await;
    broker
        .install_session_delivery(Arc::new(FakeApprovalDelivery(DeliveryOutcome::Started)))
        .expect("delivery injection");
    let requester_and_approver = session(&broker.service_id, "parent-approver");
    let target = session(&broker.service_id, "provider-target");
    let retirement = tokio_util::sync::CancellationToken::new();
    let operation_id = "019f0000-0000-7000-8000-000000001206"
        .to_owned()
        .try_into()
        .expect("operation ID");
    let request = ExternalApprovalRequest {
        requester: requester_and_approver.clone(),
        approver: requester_and_approver.clone(),
        generation: generation.clone(),
        retirement: retirement.clone(),
        cancellation: tokio_util::sync::CancellationToken::new(),
        turn_cancellation: tokio_util::sync::CancellationToken::new(),
        operation_metadata: ExternalApprovalOperationMetadata {
            operation_id,
            target,
            binding_generation: generation,
            method: "session/request_permission",
        },
        presentation: None,
        options: vec![ExternalApprovalOption {
            option_id: "allow-once".to_owned(),
            label: Some("Allow once".to_owned()),
            scope: ExternalApprovalOptionScope::AllowOnce,
        }],
    };
    let approval_task = tokio::spawn({
        let broker = Arc::clone(&broker);
        async move { broker.request_external(request).await }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !broker.list(true).await.approvals.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("approval becomes pending");
    let pending_id = broker.list(true).await.approvals[0].request_id.clone();
    let race_gate = Arc::new(tokio::sync::Barrier::new(3));
    let decision_task = tokio::spawn({
        let broker = Arc::clone(&broker);
        let race_gate = Arc::clone(&race_gate);
        let actor = requester_and_approver.clone();
        async move {
            race_gate.wait().await;
            broker
                .decide(ApprovalDecideParams {
                    request_id: pending_id,
                    decision: Some(ApprovalDecision::Allow),
                    option_id: None,
                    acknowledge_persistent: false,
                    actor: board_identity(&actor).expect("actor"),
                })
                .await
        }
    });
    let retirement_task = tokio::spawn({
        let race_gate = Arc::clone(&race_gate);
        async move {
            race_gate.wait().await;
            retirement.cancel();
        }
    });
    race_gate.wait().await;
    let decision = decision_task.await.expect("decision task");
    retirement_task.await.expect("retirement task");
    let outcome = approval_task
        .await
        .expect("approval task")
        .expect("settled");

    let history = broker.list(false).await.approvals;
    assert_eq!(history.len(), 1);
    assert!(broker.pending.lock().await.is_empty());
    match history[0].state {
        ApprovalState::Decided => {
            assert!(decision.is_ok());
            assert_eq!(
                outcome,
                BrokeredApprovalOutcome::Selected {
                    option_id: "allow-once".to_owned()
                }
            );
        }
        ApprovalState::Cancelled => {
            assert!(decision.is_err());
            assert_eq!(outcome, BrokeredApprovalOutcome::Cancelled);
        }
        ref state => panic!("unexpected terminal approval state: {state:?}"),
    }
}

#[tokio::test]
async fn external_permission_refusal_keeps_its_reason_in_history() {
    let (broker, generation, directory) = fixture_broker().await;
    let requester = session(&broker.service_id, "requester");
    let approver = session(&broker.service_id, "approver");
    let target = session(&broker.service_id, "provider-session");
    let metadata = ExternalApprovalOperationMetadata {
        operation_id: "019f0000-0000-7000-8000-000000001104"
            .to_owned()
            .try_into()
            .expect("operation ID"),
        target: target.clone(),
        binding_generation: generation.clone(),
        method: "session/request_permission",
    };

    broker
        .record_external_refusal(ExternalApprovalRefusal {
            requester,
            approver,
            generation,
            operation_metadata: metadata,
            offered_options: Vec::new(),
            presentation: None,
            reason: "no supported allow option remains".to_owned(),
        })
        .await
        .expect("refusal recorded");

    let record = broker.list(false).await.approvals.pop().expect("history");
    assert_eq!(record.state, ApprovalState::Cancelled);
    assert_eq!(
        record.reason.as_deref(),
        Some("no supported allow option remains")
    );
    assert_eq!(
        record.operation["target"],
        serde_json::to_value(target).expect("target JSON")
    );

    let reloaded = ServiceApprovalBroker::load(
        broker.service_id.clone(),
        broker.backend.clone(),
        directory.join("approval-routes.json"),
    )
    .await
    .expect("history reload");
    assert_eq!(
        reloaded.list(false).await.approvals[0].reason.as_deref(),
        Some("no supported allow option remains")
    );
    let mut prior_history = serde_json::to_value(record).expect("record JSON");
    let prior_history_object = prior_history.as_object_mut().expect("record object");
    prior_history_object.remove("reason");
    prior_history_object.remove("offeredOptions");
    prior_history_object.remove("presentation");
    let decoded_prior_history: ApprovalRequestRecord =
        serde_json::from_value(prior_history).expect("older history remains readable");
    assert_eq!(decoded_prior_history.reason, None);
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
async fn external_target_cannot_approve_its_own_blocked_request() {
    let (broker, generation, _directory) = fixture_broker().await;
    let requester = session(&broker.service_id, "parent-sender");
    let approver = session(&broker.service_id, "provider-target");
    let target = approver.clone();
    let operation_metadata = ExternalApprovalOperationMetadata {
        operation_id: "019f0000-0000-7000-8000-000000001105"
            .to_owned()
            .try_into()
            .expect("operation ID"),
        target: target.clone(),
        binding_generation: generation.clone(),
        method: "session/request_permission",
    };

    let outcome = broker
        .request_external(ExternalApprovalRequest {
            requester: requester.clone(),
            approver: approver.clone(),
            generation: generation.clone(),
            retirement: tokio_util::sync::CancellationToken::new(),
            cancellation: tokio_util::sync::CancellationToken::new(),
            turn_cancellation: tokio_util::sync::CancellationToken::new(),
            operation_metadata,
            presentation: None,
            options: vec![ExternalApprovalOption {
                option_id: "allow-once".to_owned(),
                label: Some("Allow once".to_owned()),
                scope: ExternalApprovalOptionScope::AllowOnce,
            }],
        })
        .await
        .expect("undecidable approval is refused");

    assert_eq!(outcome, BrokeredApprovalOutcome::Cancelled);
    assert!(broker.pending.lock().await.is_empty());
    let record = broker
        .list(false)
        .await
        .approvals
        .pop()
        .expect("refusal history");
    assert_eq!(record.requester, requester);
    assert_eq!(record.approver, approver);
    assert_eq!(record.state, ApprovalState::Cancelled);
    assert_eq!(
        record.reason.as_deref(),
        Some("set a different approver; the approver cannot be the blocked provider session")
    );
}

#[tokio::test]
async fn external_wrong_service_and_invalid_options_are_recorded() {
    let (broker, generation, _directory) = fixture_broker().await;
    let other_service = crate::new_service_uuid().expect("foreign service");
    let foreign_requester = session(&other_service, "foreign-requester");
    let approver = session(&broker.service_id, "approver");
    let target = session(&broker.service_id, "provider-target");
    let metadata = |suffix: &str| ExternalApprovalOperationMetadata {
        operation_id: format!("019f0000-0000-7000-8000-000000001{suffix}")
            .try_into()
            .expect("operation ID"),
        target: target.clone(),
        binding_generation: generation.clone(),
        method: "session/request_permission",
    };

    broker
        .request_external(ExternalApprovalRequest {
            requester: foreign_requester.clone(),
            approver: approver.clone(),
            generation: generation.clone(),
            retirement: tokio_util::sync::CancellationToken::new(),
            cancellation: tokio_util::sync::CancellationToken::new(),
            turn_cancellation: tokio_util::sync::CancellationToken::new(),
            operation_metadata: metadata("201"),
            presentation: None,
            options: vec![ExternalApprovalOption {
                option_id: "allow-once".to_owned(),
                label: None,
                scope: ExternalApprovalOptionScope::AllowOnce,
            }],
        })
        .await
        .expect("wrong-service request is refused");

    for (suffix, options) in [
        (
            "202",
            vec![
                ExternalApprovalOption {
                    option_id: "duplicate".to_owned(),
                    label: None,
                    scope: ExternalApprovalOptionScope::AllowOnce,
                },
                ExternalApprovalOption {
                    option_id: "duplicate".to_owned(),
                    label: None,
                    scope: ExternalApprovalOptionScope::RejectOnce,
                },
            ],
        ),
        (
            "203",
            vec![ExternalApprovalOption {
                option_id: "allow-always".to_owned(),
                label: Some("Always allow".to_owned()),
                scope: ExternalApprovalOptionScope::AllowAlways,
            }],
        ),
        (
            "204",
            vec![ExternalApprovalOption {
                option_id: "future-scope".to_owned(),
                label: Some("Future".to_owned()),
                scope: ExternalApprovalOptionScope::Unsupported {
                    provider_kind: "future_scope".to_owned(),
                },
            }],
        ),
        (
            "205",
            vec![ExternalApprovalOption {
                option_id: "reject-once".to_owned(),
                label: Some("Reject once".to_owned()),
                scope: ExternalApprovalOptionScope::RejectOnce,
            }],
        ),
    ] {
        broker
            .request_external(ExternalApprovalRequest {
                requester: session(&broker.service_id, "requester"),
                approver: approver.clone(),
                generation: generation.clone(),
                retirement: tokio_util::sync::CancellationToken::new(),
                cancellation: tokio_util::sync::CancellationToken::new(),
                turn_cancellation: tokio_util::sync::CancellationToken::new(),
                operation_metadata: metadata(suffix),
                presentation: None,
                options,
            })
            .await
            .expect("invalid option set is refused");
    }

    let history = broker.list(false).await.approvals;
    assert_eq!(history.len(), 5);
    assert!(
        history
            .iter()
            .all(|record| record.state == ApprovalState::Cancelled && record.reason.is_some())
    );
    assert_eq!(history[0].requester, foreign_requester);
    assert_eq!(history[1].offered_options.len(), 2);
    assert_eq!(history[1].offered_options[0].option_id, "duplicate");
    assert_eq!(
        history[2].offered_options[0].scope,
        ApprovalOptionScope::AllowAlways
    );
    assert_eq!(
        history[3].offered_options[0].scope,
        ApprovalOptionScope::Unsupported {
            provider_kind: "future_scope".to_owned()
        }
    );
    assert_eq!(history[4].offered_options[0].option_id, "reject-once");
    assert_eq!(
        history[4].reason.as_deref(),
        Some("no supported one-time allow option remains")
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
            )
            .await,
        Err(crate::interaction_broker::InteractionHistoryError::SelfApprover)
    ));
    let history_path = directory.join("interaction-history.json");
    tokio::fs::write(&history_path, br#"{"bad":{"requestId":"other"}}"#)
        .await
        .expect("write corrupt stored row");
    let reload = ServiceApprovalBroker::load(
        broker.service_id.clone(),
        broker.backend.clone(),
        directory.join("approval-routes.json"),
    )
    .await;
    assert!(matches!(reload, Err(ApprovalBrokerError::Unavailable)));
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
        .request_typed_approval(requester, approver.clone(), request)
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
        .request_typed_approval(requester, actor.clone(), request)
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
        ServiceApprovalBroker::load(service_id, backend, directory.join("approval-routes.json"))
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
