use crate::proxy_drain_test_fixtures::*;
use crate::proxy_role_test_fixtures::*;
use crate::*;
use codex_router_auth::resolver::{
    AsyncRouterCredentialResolver, CredentialResolverError, NoopCredentialRefreshClient,
};
use codex_router_core::{ids::AccountId, provider::Provider};
use codex_router_keeper_protocol::PrepareMode;
use codex_router_state::{
    account::{AccountRecord, AccountStatus},
    credential_maintenance::CredentialMaintenanceState,
};
use std::{
    future::Future,
    task::Poll,
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn actual_deactivation_keeps_parent_port_but_never_serves_queued_new_request() {
    let root = tempfile::tempdir().expect("isolated root");
    let parent = held_listener().await;
    let address = parent.tcp_address().expect("parent address");
    let gate = codex_router_descriptor_boundary::DescriptorGate::global();
    let fd = gate
        .duplicate(parent.as_fd())
        .await
        .expect("same bound port duplicate");
    let grant = codex_router_descriptor_boundary::OwnedListener::from_tcp_owned(fd, address, gate)
        .await
        .expect("actual association");
    let mut role = ProxyRoleRuntime::prepare(
        role_config(root.path(), address),
        PrepareMode::Fresh,
        grant,
        gate,
    )
    .await
    .expect("prepare")
    .activate()
    .await
    .expect("activate");
    assert!(
        http(address, HEALTH_REQUEST)
            .await
            .starts_with(b"HTTP/1.1 200")
    );
    let mut deactivation = Box::pin(role.deactivate());
    let was_pending =
        std::future::poll_fn(|cx| Poll::Ready(deactivation.as_mut().poll(cx).is_pending())).await;
    drop(deactivation);
    let receipt = role
        .deactivate()
        .await
        .expect("same stopped owner after observer cancellation");
    assert_eq!(
        receipt.stop_cause(),
        codex_router_proxy::server::LoopbackServingStopCause::DeactivationRequested
    );
    let mut queued = tokio::net::TcpStream::connect(address)
        .await
        .expect("keeper-style duplicate keeps port live");
    queued
        .write_all(HEALTH_REQUEST)
        .await
        .expect("literal new request queues");
    let mut prefix = [0u8; 12];
    let outgoing_served =
        tokio::time::timeout(Duration::from_millis(30), queued.read_exact(&mut prefix))
            .await
            .is_ok();
    let completion = role.wait_drained().await.expect("real joined completion");
    assert!(
        completion
            .into_serving_result()
            .expect("original outcome")
            .is_ok()
    );
    drop(role);
    assert_eq!(
        parent.tcp_address().expect("parent listener survives"),
        address
    );
    eprintln!(
        "actual_stop observer_pending={was_pending} parent_port_live=true outgoing_served_queued_request={outgoing_served}"
    );
    assert!(
        !outgoing_served,
        "stopped owner must never poll acceptance again"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_provider_producers_retain_claim_and_resume_the_same_drained_wait() {
    for provider in [Provider::Openai, Provider::Claude] {
        for producer in [HeldProducer::Upkeep, HeldProducer::Quota] {
            let mut fixture = held_role(provider, producer).await;
            let claim = fixture
                .state
                .load_credential_maintenance(&fixture.account)
                .await
                .expect("actual claim")
                .expect("durable claim");
            let start = Instant::now();
            fixture.role.deactivate().await.expect("fast actual stop");
            let elapsed = start.elapsed();
            let pending =
                tokio::time::timeout(Duration::from_millis(20), fixture.role.wait_drained())
                    .await
                    .is_err();
            let late = AccountId::new("late-renewal-account").expect("literal late account");
            fixture
                .state
                .upsert_account(
                    &AccountRecord::new(provider, late.clone(), "late", AccountStatus::Enabled)
                        .with_active_credential_generation(1),
                )
                .await
                .expect("late account");
            let credentials =
                codex_router_secret_store::test_support::open_encrypted_credential_store(
                    fixture.root.path().join("secrets"),
                )
                .expect("same fixture handle");
            let original_key =
                codex_router_secret_store::account_tokens::provider_credential_bundle_key(
                    provider,
                    &fixture.account,
                    1,
                )
                .expect("original key");
            let late_key =
                codex_router_secret_store::account_tokens::provider_credential_bundle_key(
                    provider, &late, 1,
                )
                .expect("late key");
            use codex_router_secret_store::SecretStore;
            credentials
                .write_secret(
                    &late_key,
                    &credentials
                        .read_secret(&original_key)
                        .expect("expired literal source"),
                )
                .expect("late expired credential");
            let late_resolver = AsyncRouterCredentialResolver::new(
                fixture.state.clone(),
                credentials,
                NoopCredentialRefreshClient,
                Some(1_000),
            )
            .with_refresh_task_supervisor(fixture.role.credential_refresh_task_supervisor());
            let refused = late_resolver
                .resolve_provider_credentials(&late, provider)
                .await;
            fixture
                .release
                .send(())
                .expect("release only after pending/closed observations");
            let completion =
                tokio::time::timeout(Duration::from_secs(2), fixture.role.wait_drained())
                    .await
                    .expect("same owned drain resumes")
                    .expect("true joined receipt");
            let successor = fixture
                .state
                .load_account(&fixture.account)
                .await
                .expect("actual successor")
                .expect("account survives");
            let disposition = fixture
                .state
                .load_credential_maintenance(&fixture.account)
                .await
                .expect("actual disposition")
                .expect("stored disposition");
            assert!(
                completion
                    .into_serving_result()
                    .expect("original serving result")
                    .is_ok()
            );
            fixture.state.close().await.expect("fixture state closes");
            eprintln!(
                "held_role provider={provider:?} stop_us={} drain_pending={pending} claimed_successor={:?} committed_generation={:?} disposition={:?}",
                elapsed.as_micros(),
                claim.claimed_successor_generation,
                successor.active_credential_generation(),
                disposition.state
            );
            assert_eq!(claim.state, CredentialMaintenanceState::InProgress);
            assert_eq!(claim.claimed_successor_generation, Some(2));
            assert!(elapsed < Duration::from_millis(500));
            assert!(pending);
            assert_eq!(
                refused,
                Err(CredentialResolverError::RenewalAdmissionClosed)
            );
            assert_eq!(successor.active_credential_generation(), Some(2));
            assert_eq!(disposition.state, CredentialMaintenanceState::Healthy);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nominal_sixty_second_observer_never_completes_or_aborts_a_held_claim() {
    let mut fixture = held_role(Provider::Openai, HeldProducer::Upkeep).await;
    fixture
        .role
        .deactivate()
        .await
        .expect("actual stopped acceptance");
    let began = Instant::now();
    let expired = tokio::time::timeout(Duration::from_secs(60), fixture.role.wait_drained())
        .await
        .is_err();
    let elapsed = began.elapsed();
    let claim = fixture
        .state
        .load_credential_maintenance(&fixture.account)
        .await
        .expect("actual retained claim")
        .expect("durable claim");
    let generation = fixture
        .state
        .load_account(&fixture.account)
        .await
        .expect("actual original account")
        .expect("account")
        .active_credential_generation();
    fixture
        .release
        .send(())
        .expect("release after exact nominal observer");
    let complete = tokio::time::timeout(Duration::from_secs(2), fixture.role.wait_drained())
        .await
        .expect("same drain after release")
        .expect("real completion");
    let successor = fixture
        .state
        .load_account(&fixture.account)
        .await
        .expect("real successor")
        .expect("account")
        .active_credential_generation();
    fixture.state.close().await.expect("fixture store closes");
    eprintln!(
        "nominal_observer_seconds={} expired={expired} claim={:?} generation_at_expiry={generation:?} successor_after_release={successor:?}",
        elapsed.as_secs_f64(),
        claim.state
    );
    assert!(expired);
    assert!(elapsed >= Duration::from_secs(60));
    assert_eq!(claim.state, CredentialMaintenanceState::InProgress);
    assert_eq!(generation, Some(1));
    assert_eq!(successor, Some(2));
    assert!(
        complete
            .into_serving_result()
            .expect("original serving result")
            .is_ok()
    );
}

#[tokio::test]
async fn finite_admitted_request_waits_for_renewal_before_final_admission_close() {
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("isolated model upstream");
    let address = upstream.local_addr().expect("upstream address");
    let upstream_task = tokio::spawn(async move {
        let (mut peer, _) = upstream.accept().await.expect("real model request");
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 512];
        while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
            let count = peer.read(&mut chunk).await.expect("model request bytes");
            assert_ne!(count, 0);
            bytes.extend_from_slice(&chunk[..count]);
        }
        peer.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .await
            .expect("literal model response");
    });
    let mut fixture = held_role_at(
        Provider::Openai,
        HeldProducer::Upkeep,
        1,
        format!("http://{address}/v1"),
    )
    .await;
    let mut client = tokio::net::TcpStream::connect(fixture.role.local_addr())
        .await
        .expect("real admitted client");
    client
        .write_all(b"GET /v1/models HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .expect("literal model request");
    let serving_pending =
        tokio::time::timeout(Duration::from_millis(20), fixture.role.wait_serving())
            .await
            .is_err();
    let mut prefix = [0u8; 12];
    let reply_before_release =
        tokio::time::timeout(Duration::from_millis(20), client.read_exact(&mut prefix))
            .await
            .is_ok();
    fixture
        .release
        .send(())
        .expect("release authoritative claimed rotation");
    let handled = tokio::time::timeout(Duration::from_secs(2), fixture.role.wait_serving())
        .await
        .expect("finite role completes")
        .expect("original finite result");
    if !reply_before_release {
        client
            .read_exact(&mut prefix)
            .await
            .expect("literal first reply after successor");
    }
    let mut tail = Vec::new();
    client
        .read_to_end(&mut tail)
        .await
        .expect("actual peer EOF");
    upstream_task.await.expect("upstream fixture joined");
    let generation = fixture
        .state
        .load_account(&fixture.account)
        .await
        .expect("actual durable account")
        .expect("account")
        .active_credential_generation();
    fixture
        .role
        .shutdown()
        .await
        .expect("same already-drained lifecycle");
    fixture.state.close().await.expect("fixture store joins");
    eprintln!(
        "finite_request serving_pending={serving_pending} reply_before_release={reply_before_release} handled={handled} generation={generation:?}"
    );
    assert!(serving_pending);
    assert!(!reply_before_release);
    assert_eq!(&prefix, b"HTTP/1.1 200");
    assert_eq!(handled, 1);
    assert_eq!(generation, Some(2));
}

#[tokio::test]
async fn finite_serving_error_is_retained_separately_from_true_joined_completion() {
    let root = tempfile::tempdir().expect("isolated finite error root");
    let listener = held_listener().await;
    let address = listener.tcp_address().expect("real listener");
    let mut config = role_config(root.path(), address);
    config.max_connections = 1;
    let mut role = ProxyRoleRuntime::prepare(
        config,
        PrepareMode::Fresh,
        listener,
        codex_router_descriptor_boundary::DescriptorGate::global(),
    )
    .await
    .expect("prepare")
    .activate()
    .await
    .expect("actual role");
    let mut client = tokio::net::TcpStream::connect(address)
        .await
        .expect("actual malformed peer");
    client
        .write_all(b"not-http\r\n\r\n")
        .await
        .expect("malformed literal bytes");
    client.shutdown().await.expect("peer half close");
    // Observe natural stop without requesting deactivation, preserving the finite trigger.
    role.observe_stopped_for_test()
        .await
        .expect("actual finite stopped owner");
    let complete = role
        .wait_drained()
        .await
        .expect("all work is terminal even on original serving failure");
    assert!(matches!(
        complete.serving_result(),
        Some(Err(ProxyActivationError::Core(
            codex_router_proxy::server::LoopbackRouterRuntimeError::HyperConnection(_)
        )))
    ));
    assert!(
        complete
            .into_serving_result()
            .expect("original result retained")
            .is_err()
    );
    role.shutdown()
        .await
        .expect("resumed completed shutdown does not detach owners");
}

#[tokio::test]
async fn stopped_quota_scheduler_never_opens_a_new_cycle_before_first_poll() {
    let root = tempfile::tempdir().expect("isolated scheduler root");
    let secrets = codex_router_secret_store::test_support::open_encrypted_credential_store(
        root.path().join("secrets"),
    )
    .expect("external key fixture");
    let resolver = credential_runtime::AsyncCliCredentialResolver::open_with_refresh_client(
        &root.path().join("resolver.sqlite"),
        secrets,
        NoopCredentialRefreshClient,
        codex_router_auth::resolver::CredentialRefreshTaskSupervisor::new(),
    )
    .await
    .expect("actual resolver/store dependencies");
    let never_started = root.path().join("new-cycle.sqlite");
    let mut worker = quota::start_background_quota_refresh_worker_with_reporter(
        never_started.clone(),
        root.path().join("secrets"),
        "http://127.0.0.1:1/v1".to_owned(),
        resolver,
        ControlledQuotaEndpoint,
        quota::BackgroundQuotaRefreshRuntime::new(
            || 1_000,
            |_diagnostic| {},
            Duration::from_secs(180),
        ),
    )
    .await;
    worker.request_stop();
    worker
        .join_stopped()
        .await
        .expect("actual owned scheduler joined");
    assert!(
        !never_started.exists(),
        "stop-before-first-poll must not create/open the cycle's state store"
    );
}

#[path = "proxy_accepting_failure_tests.rs"]
mod proxy_accepting_failure_tests;
