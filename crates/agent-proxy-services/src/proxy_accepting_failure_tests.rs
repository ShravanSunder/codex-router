use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepting_task_failure_retains_claim_and_cleanup_across_resumed_shutdown() {
    let mut fixture = held_role(Provider::Openai, HeldProducer::Upkeep).await;
    let claim = fixture
        .state
        .load_credential_maintenance(&fixture.account)
        .await
        .expect("actual claim before accepting-owner failure")
        .expect("durable held claim");
    match &fixture.role.lifecycle {
        crate::proxy_role_lifecycle::ProxyServingLifecycle::Accepting(task) => task.abort(),
        _ => panic!("fixture must cancel the same actual accepting owner"),
    }
    let began = Instant::now();
    let first_shutdown =
        tokio::time::timeout(Duration::from_millis(20), fixture.role.shutdown()).await;
    let elapsed = began.elapsed();
    let cleanup_pending = first_shutdown.is_err();
    let workers_retained = fixture.role.upkeep.is_some() && fixture.role.watcher.is_some();
    let at_observation = fixture
        .state
        .load_credential_maintenance(&fixture.account)
        .await
        .expect("actual claim at shutdown observation")
        .expect("claim remains recorded");
    let generation_before_release = fixture
        .state
        .load_account(&fixture.account)
        .await
        .expect("account at observation")
        .expect("original account")
        .active_credential_generation();
    eprintln!(
        "accepting_owner_failure first_shutdown={first_shutdown:?} elapsed_us={} cleanup_pending={cleanup_pending} workers_retained={workers_retained} claim_at_observation={:?} generation_before_release={generation_before_release:?}",
        elapsed.as_micros(),
        at_observation.state
    );
    fixture
        .release
        .send(())
        .expect("release only after failure observation");
    let resumed_shutdown = tokio::time::timeout(Duration::from_secs(2), fixture.role.shutdown())
        .await
        .expect("same shutdown observation stays bounded after fixture release");
    let original_error_retained = matches!(
        &resumed_shutdown,
        Err(ProxyActivationError::ServingTask(error)) if error.is_cancelled()
    );
    let workers_joined_by_shutdown = fixture.role.upkeep.is_none()
        && fixture.role.quota.is_none()
        && fixture.role.watcher.is_none();
    eprintln!(
        "accepting_owner_failure resumed_shutdown={resumed_shutdown:?} original_error_retained={original_error_retained} workers_joined_by_shutdown={workers_joined_by_shutdown}"
    );
    let generation_at_shutdown_return = fixture
        .state
        .load_account(&fixture.account)
        .await
        .expect("actual generation before diagnostic cleanup")
        .expect("account survives failure")
        .active_credential_generation();
    assert!(matches!(
        fixture.role.lifecycle,
        crate::proxy_role_lifecycle::ProxyServingLifecycle::Failed { .. }
    ));
    // Always join the same acquired fixture owners before a failing assertion reports the red.
    // This is diagnostic cleanup, not evidence that the failed shutdown path joined them.
    fixture.role.cleanup_acquired().await;
    let successor = fixture
        .state
        .load_account(&fixture.account)
        .await
        .expect("actual successor after same-owner fixture cleanup")
        .expect("account survives")
        .active_credential_generation();
    let disposition = fixture
        .state
        .load_credential_maintenance(&fixture.account)
        .await
        .expect("actual durable disposition")
        .expect("disposition remains recorded");
    fixture.state.close().await.expect("actual observer closes");
    eprintln!(
        "accepting_owner_failure generation_at_shutdown_return={generation_at_shutdown_return:?} diagnostic_cleanup_joined=true successor={successor:?} disposition={:?}",
        disposition.state
    );
    assert_eq!(claim.state, CredentialMaintenanceState::InProgress);
    assert_eq!(claim.claimed_successor_generation, Some(2));
    assert_eq!(at_observation.state, CredentialMaintenanceState::InProgress);
    assert_eq!(generation_before_release, Some(1));
    assert_eq!(successor, Some(2));
    assert_eq!(generation_at_shutdown_return, Some(2));
    assert_eq!(disposition.state, CredentialMaintenanceState::Healthy);
    assert!(
        cleanup_pending,
        "serving-owner failure must retain cleanup until the real claimed renewal returns"
    );
    assert!(
        original_error_retained,
        "resumed cleanup must report the actual accepting-owner JoinError"
    );
    assert!(
        workers_joined_by_shutdown,
        "shutdown itself must join the same acquired workers"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_serving_failure_retains_original_cause_through_cli_style_cleanup() {
    let mut fixture = held_role(Provider::Openai, HeldProducer::Upkeep).await;
    match &fixture.role.lifecycle {
        crate::proxy_role_lifecycle::ProxyServingLifecycle::Accepting(task) => task.abort(),
        _ => panic!("same actual accepting owner must be present"),
    }
    let pending = tokio::time::timeout(Duration::from_millis(20), fixture.role.wait_serving())
        .await
        .is_err();
    let actual_failure_retained = matches!(
        &fixture.role.lifecycle,
        crate::proxy_role_lifecycle::ProxyServingLifecycle::Failed {
            accepting_error: Some(error)
        } if error.is_cancelled()
    );
    let claim = fixture
        .state
        .load_credential_maintenance(&fixture.account)
        .await
        .expect("actual held claim")
        .expect("durable claim");
    fixture
        .release
        .send(())
        .expect("release after cancelled observer");
    let serve_result = tokio::time::timeout(Duration::from_secs(2), fixture.role.wait_serving())
        .await
        .expect("same failed serving observation completes cleanup");
    let workers_joined_before_cleanup_call = fixture.role.upkeep.is_none()
        && fixture.role.quota.is_none()
        && fixture.role.watcher.is_none();
    let generation_before_cleanup_call = fixture
        .state
        .load_account(&fixture.account)
        .await
        .expect("actual successor before caller cleanup")
        .expect("account")
        .active_credential_generation();
    let cleanup_result = fixture.role.shutdown().await;
    let original_error_wins = match serve_result {
        Err(ProxyActivationError::ServingTask(error)) => error.is_cancelled(),
        _ => false,
    };
    let repeated_error_not_success = matches!(
        cleanup_result,
        Err(ProxyActivationError::LifecycleUnavailable)
    );
    let no_deactivation_receipt = fixture.role.deactivate().await.is_err();
    let no_drain_completion = fixture.role.wait_drained().await.is_err();
    fixture.role.cleanup_acquired().await;
    fixture.state.close().await.expect("actual observer closes");
    eprintln!(
        "failed_wait_serving pending={pending} actual_failure_retained={actual_failure_retained} workers_joined_before_cleanup_call={workers_joined_before_cleanup_call} generation_before_cleanup_call={generation_before_cleanup_call:?} original_error_wins={original_error_wins} repeated_error_not_success={repeated_error_not_success} no_deactivation_receipt={no_deactivation_receipt} no_drain_completion={no_drain_completion}"
    );
    assert!(pending);
    assert!(actual_failure_retained);
    assert_eq!(claim.state, CredentialMaintenanceState::InProgress);
    assert!(workers_joined_before_cleanup_call);
    assert_eq!(generation_before_cleanup_call, Some(2));
    assert!(original_error_wins);
    assert!(repeated_error_not_success);
    assert!(no_deactivation_receipt);
    assert!(no_drain_completion);
}
