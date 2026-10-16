//! Loaded generic and expired-owner paths preserve dispatch semantics.
use super::*;

#[tokio::test]
async fn policy_loaded_generic_preserves_each_mode_without_resume() {
    for mode in [
        MessageDelivery::Auto,
        MessageDelivery::Queue,
        MessageDelivery::Steer,
    ] {
        let fixture = RecordedPolicyFixture::create_without_route().await.unwrap();
        let server = policy_server(
            &fixture,
            PolicyServerCase {
                cold: ReplyPolicy::Matching,
                loaded: ReplyPolicy::Matching,
                loaded_delivery: mode,
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
        let mut request = fixture.message_request(&weak);
        request.params.delivery = mode;
        let second = tokio::time::timeout(Duration::from_secs(5), dispatch_message(request))
            .await
            .unwrap();
        let requests = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(first, NativeMessageOutcome::Accepted(_)));
        let NativeMessageOutcome::Accepted(receipt) = second else {
            panic!("generic loaded route refused");
        };
        assert_eq!(
            receipt.resume_effect,
            collaboration_protocol::AcceptedResumeEffect::NotRequested
        );
        assert!(
            requests[1]
                .iter()
                .all(|request| request["method"] != "thread/resume")
        );
        let method = match mode {
            MessageDelivery::Auto => "turn/start",
            MessageDelivery::Queue => "thread/queue/add",
            MessageDelivery::Steer => "turn/steer",
        };
        assert!(
            requests[1]
                .iter()
                .any(|request| request["method"] == method)
        );
    }
}

#[tokio::test]
async fn policy_loaded_expired_broker_refuses_each_mode_before_resume_or_input() {
    for mode in [
        MessageDelivery::Auto,
        MessageDelivery::Queue,
        MessageDelivery::Steer,
    ] {
        let mut fixture = RecordedPolicyFixture::create().await.unwrap();
        let server = policy_server(
            &fixture,
            PolicyServerCase {
                cold: ReplyPolicy::Matching,
                loaded: ReplyPolicy::Matching,
                loaded_delivery: mode,
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
        drop(broker);
        fixture.broker = ServiceInteractionBroker::load(
            fixture.backend.endpoint.service_id.clone(),
            fixture.backend.clone(),
            fixture.directory.path().join("new-owner-routes.json"),
        )
        .await
        .unwrap();
        assert!(weak.upgrade().is_none());
        let mut request = fixture.message_request(&weak);
        request.params.delivery = mode;
        let second = tokio::time::timeout(Duration::from_secs(5), dispatch_message(request))
            .await
            .unwrap();
        let requests = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(first, NativeMessageOutcome::Accepted(_)));
        let NativeMessageOutcome::Failed(failure) = second else {
            panic!("expired authority was accepted");
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
}
