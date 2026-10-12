//! Request-local route snapshots pin membership without retaining the policy owner.
use super::*;
use crate::native_recorded_policy::{
    RecordedNativeRouteClassification, classify_recorded_native_route,
    prepare_classified_native_policy,
};

#[tokio::test]
async fn policy_recorded_snapshot_survives_route_mutation_without_second_lookup() {
    let fixture = RecordedPolicyFixture::create().await.unwrap();
    let broker: Arc<dyn ApprovalBroker> = fixture.broker.clone();
    let weak = Arc::downgrade(&broker);
    let captured = classify_recorded_native_route(Some(&weak), &target()).await;
    assert!(matches!(
        captured,
        RecordedNativeRouteClassification::RecordedRoute { .. }
    ));
    fixture
        .broker
        .register_route(ApprovalRoute {
            thread_id: THREAD.into(),
            created_by: target(),
            approver: target(),
            access: collaboration_protocol::RouterAccess::WorkspaceWrite,
            scratch_path: "new-invalid-relative-scratch".into(),
            root_message_id: None,
        })
        .await
        .unwrap();
    let directory = fixture.directory.path().to_string_lossy().into_owned();
    assert!(
        prepare_classified_native_policy(&captured, &target(), Some(&directory))
            .await
            .unwrap()
            .is_some(),
        "captured existing route was looked up again"
    );
    let next = classify_recorded_native_route(Some(&weak), &target()).await;
    assert!(
        prepare_classified_native_policy(&next, &target(), Some(&directory))
            .await
            .is_err(),
        "next attempt must observe new route data"
    );
}

#[tokio::test]
async fn policy_generic_snapshot_stays_generic_until_the_next_attempt() {
    let fixture = RecordedPolicyFixture::create_without_route().await.unwrap();
    let broker: Arc<dyn ApprovalBroker> = fixture.broker.clone();
    let weak = Arc::downgrade(&broker);
    let captured = classify_recorded_native_route(Some(&weak), &target()).await;
    assert!(matches!(
        captured,
        RecordedNativeRouteClassification::GenericRoute
    ));
    fixture
        .broker
        .register_route(ApprovalRoute {
            thread_id: THREAD.into(),
            created_by: target(),
            approver: target(),
            access: collaboration_protocol::RouterAccess::WorkspaceWrite,
            scratch_path: fixture
                .directory
                .path()
                .join("scratch")
                .to_string_lossy()
                .into_owned(),
            root_message_id: None,
        })
        .await
        .unwrap();
    let directory = fixture.directory.path().to_string_lossy().into_owned();
    assert!(
        prepare_classified_native_policy(&captured, &target(), Some(&directory))
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        classify_recorded_native_route(Some(&weak), &target()).await,
        RecordedNativeRouteClassification::RecordedRoute { .. }
    ));
}

#[tokio::test]
async fn policy_captured_recorded_owner_expiry_refuses_before_resume_or_input() {
    let mut fixture = RecordedPolicyFixture::create().await.unwrap();
    let server = policy_server(
        &fixture,
        PolicyServerCase {
            cold: ReplyPolicy::Matching,
            loaded: ReplyPolicy::Matching,
            loaded_delivery: MessageDelivery::Auto,
            turn_intent: None,
            retire_loaded: None,
            stored_run: None,
        },
    )
    .await
    .unwrap();
    let broker: Arc<dyn ApprovalBroker> = fixture.broker.clone();
    let weak = Arc::downgrade(&broker);
    let first = tokio::time::timeout(
        Duration::from_secs(5),
        dispatch_message(fixture.message_request(&weak)),
    )
    .await
    .unwrap();
    assert!(matches!(first, NativeMessageOutcome::Accepted(_)));
    let captured = classify_recorded_native_route(Some(&weak), &target()).await;
    drop(broker);
    fixture.broker = ServiceInteractionBroker::load(
        fixture.backend.endpoint.service_id.clone(),
        fixture.backend.clone(),
        fixture
            .directory
            .path()
            .join("replacement-owner-routes.json"),
    )
    .await
    .unwrap();
    assert!(weak.upgrade().is_none(), "snapshot retained the owner");
    assert!(matches!(
        classify_recorded_native_route(Some(&weak), &target()).await,
        RecordedNativeRouteClassification::UnresolvedAuthority
    ));
    let mut request = fixture.message_request(&weak);
    request.recorded_route = Some(&captured);
    let second = tokio::time::timeout(Duration::from_secs(5), dispatch_message(request))
        .await
        .unwrap();
    let requests = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let NativeMessageOutcome::Failed(failure) = second else {
        panic!("expired known owner accepted");
    };
    assert_eq!(
        failure.pointer("/error/data/effects/resume"),
        Some(&json!("notRequested"))
    );
    assert_eq!(
        failure.pointer("/error/data/effects/submission"),
        Some(&json!("notDispatched"))
    );
    assert!(requests[1].iter().all(|request| !matches!(
        request["method"].as_str(),
        Some("thread/resume" | "turn/start" | "thread/queue/add" | "turn/steer")
    )));
}

#[tokio::test]
async fn policy_captured_attempt_deadline_refuses_before_native_connection() {
    let fixture = RecordedPolicyFixture::create().await.unwrap();
    let listener =
        tokio::net::UnixListener::bind(fixture.directory.path().join("native.sock")).unwrap();
    let broker: Arc<dyn ApprovalBroker> = fixture.broker.clone();
    let weak = Arc::downgrade(&broker);
    let captured = classify_recorded_native_route(Some(&weak), &target()).await;
    let mut request = fixture.message_request(&weak);
    request.recorded_route = Some(&captured);
    request.attempt_deadline = Some(tokio::time::Instant::now() - Duration::from_secs(1));
    let observed = tokio::select! {
        biased;
        _=listener.accept()=>panic!("expired attempt opened native connection"),
        result=dispatch_message(request)=>result,
    };
    let NativeMessageOutcome::Failed(failure) = observed else {
        panic!("expired attempt accepted");
    };
    assert_eq!(
        failure.pointer("/error/data/effects/resume"),
        Some(&json!("notRequested"))
    );
    assert_eq!(
        failure.pointer("/error/data/effects/submission"),
        Some(&json!("notDispatched"))
    );
}
