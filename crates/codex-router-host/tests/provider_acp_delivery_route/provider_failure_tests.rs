use super::*;

#[tokio::test]
async fn provider_process_transport_failure_remains_retryable() {
    let root = tempfile::tempdir().expect("provider root");
    let exit_marker = root.path().join("provider-exit.txt");
    let target = target();
    let binding = provider_binding(&target);
    let runtime = ExternalProviderRuntime::initialize(exited_provider_fixture(&exit_marker))
        .await
        .expect("fixture provider");
    let store = Arc::new(tokio::sync::Mutex::new(
        ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("store"),
    ));
    store
        .lock()
        .await
        .record_session(&ProviderSessionRecord {
            target: target.clone(),
            working_directory: ProviderWorkingDirectory::try_from("/tmp".to_owned()).expect("cwd"),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: (target.clone()).into(),
            approver: (target.clone()).into(),
            updated_at_ms: 1,
        })
        .await
        .expect("session record");
    let supervisor = Arc::new(
        ExternalProviderSupervisor::new(
            vec![ExternalProviderBinding {
                identity: binding.clone(),
                runtime,
            }],
            Arc::clone(&store),
        )
        .expect("supervisor"),
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while !exit_marker.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("provider exit marker deadline");
    let route = ProviderAcpDeliveryRoute::new(
        target.endpoint.service_id.clone(),
        std::iter::once(target.endpoint.clone()).collect(),
        available_directory(&target, &binding),
        Arc::clone(&supervisor),
        store,
        Arc::new(NoLivePeer),
    );

    let receipt = route
        .deliver(
            request(target, "retry a transient provider disconnect"),
            &RecordedEvidence(tokio::sync::Mutex::new(Vec::new())),
        )
        .await
        .expect("transport failure receipt");

    assert!(
        matches!(
            &receipt.outcome,
            DeliveryOutcome::NotSubmitted {
                retryable: true,
                ..
            }
        ),
        "transport refusal was not retryable: {:?}",
        receipt.outcome
    );
    assert_eq!(
        std::fs::read_to_string(exit_marker)
            .expect("provider exit marker")
            .trim(),
        "provider exited"
    );
    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn live_peer_recheck_prevents_provider_load() {
    let root = tempfile::tempdir().expect("provider root");
    let load_marker = root.path().join("load-method.txt");
    let target = target();
    let binding = provider_binding(&target);
    let runtime = ExternalProviderRuntime::initialize(refusing_load_fixture(&load_marker, -32002))
        .await
        .expect("fixture provider");
    let store = Arc::new(tokio::sync::Mutex::new(
        ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("store"),
    ));
    store
        .lock()
        .await
        .record_session(&ProviderSessionRecord {
            target: target.clone(),
            working_directory: ProviderWorkingDirectory::try_from("/tmp".to_owned()).expect("cwd"),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: (target.clone()).into(),
            approver: (target.clone()).into(),
            updated_at_ms: 1,
        })
        .await
        .expect("session record");
    let supervisor = Arc::new(
        ExternalProviderSupervisor::new(
            vec![ExternalProviderBinding {
                identity: binding.clone(),
                runtime,
            }],
            Arc::clone(&store),
        )
        .expect("supervisor"),
    );
    let route = ProviderAcpDeliveryRoute::new(
        target.endpoint.service_id.clone(),
        std::iter::once(target.endpoint.clone()).collect(),
        available_directory(&target, &binding),
        Arc::clone(&supervisor),
        store,
        Arc::new(LivePeer),
    );
    assert!(matches!(
        route.presence(&target).await.expect("provider presence"),
        RoutePresence::LiveElsewhere { .. }
    ));
    let evidence = RecordedEvidence(tokio::sync::Mutex::new(Vec::new()));

    let receipt = route
        .deliver(request(target, "hello"), &evidence)
        .await
        .expect("live peer veto");

    assert!(matches!(
        receipt.outcome,
        DeliveryOutcome::NotSubmitted {
            retryable: true,
            ..
        }
    ));
    assert!(!load_marker.exists());
    assert!(matches!(evidence.0.lock().await.as_slice(),
        [RouteEffectEvidence::ProviderAcp(before), RouteEffectEvidence::ProviderAcp(after)]
        if before.submission == SubmissionEffect::Dispatching && after.submission == SubmissionEffect::NotDispatched));
    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("shutdown");
}
