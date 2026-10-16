//! Known-route input must validate policy even after a failed resume left it loaded.
use super::*;
use crate::{RunEvidenceDisposition, RunEvidenceSink, RunSubmission, SessionDeliveryRoute};
use agent_automation::{CapturedRunInputs, ExecutionTiming, RouteEffectEvidence, RunId};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

#[path = "native_loaded_permission_fixture.rs"]
mod policy_fixture;
use policy_fixture::*;

#[path = "native_policy_preservation_tests.rs"]
mod preservation_policy_tests;
#[path = "native_policy_queue_tests.rs"]
mod queue_policy_tests;
#[path = "native_recorded_classification_tests.rs"]
mod recorded_classification_tests;
#[path = "native_policy_stored_run_tests.rs"]
mod stored_policy_tests;

#[tokio::test]
async fn failed_cold_resume_cannot_bypass_policy_on_a_later_loaded_message() {
    let fixture = RecordedPolicyFixture::create().await.unwrap();
    let server = policy_server(
        &fixture,
        PolicyServerCase {
            cold: ReplyPolicy::Narrowed,
            loaded: ReplyPolicy::Narrowed,
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
    let second = tokio::time::timeout(
        Duration::from_secs(5),
        dispatch_message(fixture.message_request(&weak)),
    )
    .await
    .unwrap();
    let exchanges = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(first, NativeMessageOutcome::Failed(_)));
    assert!(
        exchanges
            .iter()
            .flatten()
            .all(|request| request["method"] != "turn/start"),
        "a loaded follow-up must not send input with the rejected grants"
    );
    let second_resume = exchanges[1]
        .iter()
        .find(|request| request["method"] == "thread/resume")
        .expect("loaded policy snapshot must be requested");
    assert_eq!(
        second_resume["params"],
        json!({"threadId":THREAD,"excludeTurns":true}),
        "loaded validation must not override config or trigger upstream cache rebuild"
    );
    let NativeMessageOutcome::Failed(failure) = second else {
        panic!("loaded narrowed state was accepted");
    };
    assert_eq!(
        failure.pointer("/error/data/effects/resume"),
        Some(&json!("accepted"))
    );
    assert_eq!(
        failure.pointer("/error/data/effects/submission"),
        Some(&json!("notDispatched"))
    );
}

#[tokio::test]
async fn scheduled_route_retains_policy_and_refuses_a_narrowed_loaded_followup() {
    for policy in [ReplyPolicy::Narrowed, ReplyPolicy::MissingSettings] {
        let fixture = RecordedPolicyFixture::create().await.unwrap();
        let server = policy_server(
            &fixture,
            PolicyServerCase {
                cold: ReplyPolicy::Matching,
                loaded: policy,
                loaded_delivery: MessageDelivery::Auto,
                turn_intent: None,
                retire_loaded: None,
                stored_run: None,
            },
        )
        .await
        .unwrap();
        let directory = crate::EndpointDirectory::new(fixture.backend.endpoint.service_id.clone());
        directory.publish(fixture.endpoint.clone()).unwrap();
        let route = crate::CodexAppServerDeliveryRoute::new(
            fixture.backend.endpoint.service_id.clone(),
            directory,
            fixture.backend.clone(),
            Arc::new(crate::UnmaterializedThreadHolder::new()),
        )
        .with_approval_broker(&fixture.broker);
        let scheduled = route.scheduled_runs().unwrap();
        let sink = CapturingRunSink {
            evidence: Mutex::new(Vec::new()),
            turn_intent: Arc::new(AtomicBool::new(false)),
        };
        let first = tokio::time::timeout(
            Duration::from_secs(5),
            scheduled.submit_run(fixture.scheduled_run(), &sink),
        )
        .await
        .unwrap()
        .unwrap();
        let second = tokio::time::timeout(
            Duration::from_secs(5),
            scheduled.submit_run(fixture.scheduled_run(), &sink),
        )
        .await
        .unwrap()
        .unwrap();
        let exchanges = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(first, RunSubmission::NotStartedBusy));
        let cold = exchanges[0]
            .iter()
            .find(|request| request["method"] == "thread/resume")
            .unwrap();
        assert_eq!(
            cold.pointer("/params/permissions"),
            Some(&json!("router-workspace-write")),
            "scheduled cold resume must keep the recorded policy dependency"
        );
        assert_eq!(
            cold.pointer("/params/config/permissions.router-workspace-write.filesystem"),
            Some(&json!(fixture.filesystem))
        );
        assert!(
            exchanges
                .iter()
                .flatten()
                .all(|request| request["method"] != "turn/start"),
            "scheduled loaded narrowing must be rejected before input"
        );
        assert!(matches!(
            (policy, second),
            (ReplyPolicy::Narrowed, RunSubmission::Rejected(_))
                | (ReplyPolicy::MissingSettings, RunSubmission::Unknown)
        ));
        let evidence = sink.evidence.lock().unwrap();
        let RouteEffectEvidence::CodexAppServer(last) = evidence.last().unwrap() else {
            panic!("native evidence expected");
        };
        assert_eq!(last.resume, agent_automation::PreparationEffect::Accepted);
        assert_eq!(
            last.submission,
            agent_automation::SubmissionEffect::NotDispatched
        );
    }
}

#[tokio::test]
async fn matching_loaded_policy_allows_each_message_mode_without_overrides() {
    for mode in [
        MessageDelivery::Auto,
        MessageDelivery::Queue,
        MessageDelivery::Steer,
    ] {
        let fixture = RecordedPolicyFixture::create().await.unwrap();
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
        let exchanges = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(first, NativeMessageOutcome::Accepted(_)));
        let NativeMessageOutcome::Accepted(receipt) = second else {
            panic!("matching loaded policy refused");
        };
        assert_eq!(
            receipt.resume_effect,
            collaboration_protocol::AcceptedResumeEffect::Accepted
        );
        let loaded = &exchanges[1];
        assert_eq!(
            loaded
                .iter()
                .find(|call| call["method"] == "thread/resume")
                .unwrap()["params"],
            json!({"threadId":THREAD,"excludeTurns":true})
        );
        let input_method = match mode {
            MessageDelivery::Auto => "turn/start",
            MessageDelivery::Queue => "thread/queue/add",
            MessageDelivery::Steer => "turn/steer",
        };
        assert!(loaded.iter().any(|call| call["method"] == input_method));
    }
}

#[tokio::test]
async fn narrowed_loaded_policy_blocks_queue_and_steer_before_input() {
    for mode in [MessageDelivery::Queue, MessageDelivery::Steer] {
        let fixture = RecordedPolicyFixture::create().await.unwrap();
        let server = policy_server(
            &fixture,
            PolicyServerCase {
                cold: ReplyPolicy::Matching,
                loaded: ReplyPolicy::Narrowed,
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
        let exchanges = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(first, NativeMessageOutcome::Accepted(_)));
        let NativeMessageOutcome::Failed(failure) = second else {
            panic!("narrowed loaded input accepted");
        };
        assert_eq!(
            failure.pointer("/error/data/effects/resume"),
            Some(&json!("accepted"))
        );
        assert_eq!(
            failure.pointer("/error/data/effects/submission"),
            Some(&json!("notDispatched"))
        );
        assert!(!exchanges[1].iter().any(|call| matches!(
            call["method"].as_str(),
            Some("turn/start" | "turn/steer" | "thread/queue/add")
        )));
    }
}

#[tokio::test]
async fn scheduled_matching_loaded_policy_preserves_idle_admission_and_intent_order() {
    let fixture = RecordedPolicyFixture::create().await.unwrap();
    let intent = Arc::new(AtomicBool::new(false));
    let server = policy_server(
        &fixture,
        PolicyServerCase {
            cold: ReplyPolicy::Matching,
            loaded: ReplyPolicy::Matching,
            loaded_delivery: MessageDelivery::Auto,
            turn_intent: Some(intent.clone()),
            retire_loaded: None,
            stored_run: None,
        },
    )
    .await
    .unwrap();
    let directory = crate::EndpointDirectory::new(fixture.backend.endpoint.service_id.clone());
    directory.publish(fixture.endpoint.clone()).unwrap();
    let route = crate::CodexAppServerDeliveryRoute::new(
        fixture.backend.endpoint.service_id.clone(),
        directory,
        fixture.backend.clone(),
        Arc::new(crate::UnmaterializedThreadHolder::new()),
    )
    .with_approval_broker(&fixture.broker);
    let scheduled = route.scheduled_runs().unwrap();
    let sink = CapturingRunSink {
        evidence: Mutex::new(Vec::new()),
        turn_intent: intent,
    };
    let first = tokio::time::timeout(
        Duration::from_secs(5),
        scheduled.submit_run(fixture.scheduled_run(), &sink),
    )
    .await
    .unwrap()
    .unwrap();
    let second = tokio::time::timeout(
        Duration::from_secs(5),
        scheduled.submit_run(fixture.scheduled_run(), &sink),
    )
    .await
    .unwrap()
    .unwrap();
    let exchanges = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(first, RunSubmission::NotStartedBusy));
    assert!(matches!(second, RunSubmission::Started(_)));
    assert!(
        !exchanges[0]
            .iter()
            .any(|call| call["method"] == "turn/start")
    );
    assert_eq!(
        exchanges[1]
            .iter()
            .find(|call| call["method"] == "thread/resume")
            .unwrap()["params"],
        json!({"threadId":THREAD,"excludeTurns":true})
    );
    let input = exchanges[1]
        .iter()
        .find(|call| call["method"] == "turn/start")
        .unwrap();
    assert_eq!(input.pointer("/params/effort"), Some(&json!("medium")));
    assert!(input.pointer("/params/clientUserMessageId").is_some());
}

#[tokio::test]
async fn scheduled_generation_retired_during_loaded_validation_never_submits_input() {
    let fixture = RecordedPolicyFixture::create().await.unwrap();
    let server = policy_server(
        &fixture,
        PolicyServerCase {
            cold: ReplyPolicy::Matching,
            loaded: ReplyPolicy::Matching,
            loaded_delivery: MessageDelivery::Auto,
            turn_intent: None,
            retire_loaded: Some(fixture.backend.gate.clone()),
            stored_run: None,
        },
    )
    .await
    .unwrap();
    let directory = crate::EndpointDirectory::new(fixture.backend.endpoint.service_id.clone());
    directory.publish(fixture.endpoint.clone()).unwrap();
    let route = crate::CodexAppServerDeliveryRoute::new(
        fixture.backend.endpoint.service_id.clone(),
        directory,
        fixture.backend.clone(),
        Arc::new(crate::UnmaterializedThreadHolder::new()),
    )
    .with_approval_broker(&fixture.broker);
    let scheduled = route.scheduled_runs().unwrap();
    let sink = CapturingRunSink {
        evidence: Mutex::new(Vec::new()),
        turn_intent: Arc::new(AtomicBool::new(false)),
    };
    let first = tokio::time::timeout(
        Duration::from_secs(5),
        scheduled.submit_run(fixture.scheduled_run(), &sink),
    )
    .await
    .unwrap()
    .unwrap();
    let second = tokio::time::timeout(
        Duration::from_secs(5),
        scheduled.submit_run(fixture.scheduled_run(), &sink),
    )
    .await
    .unwrap()
    .unwrap();
    let exchanges = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(first, RunSubmission::NotStartedBusy));
    assert!(matches!(second, RunSubmission::NotStartedBusy));
    assert!(
        !exchanges[1]
            .iter()
            .any(|call| call["method"] == "turn/start")
    );
    assert!(!sink.turn_intent.load(Ordering::SeqCst));
}
