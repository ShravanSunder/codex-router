use super::*;
use collaboration_protocol::{CodexGeneration, EndpointId, EndpointRef};

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
        EndpointDirectory::new(endpoint.service_id.clone()),
        NativeControlBackend {
            endpoint,
            gate,
            codex_home: directory.clone(),
        },
        directory.join("approval-routes.json"),
    )
    .await;
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
