use super::*;
use codex_router_state::quota_snapshot::{
    PersistedQuotaSnapshot, PersistedSelectorQuotaWindow, QuotaSnapshotSource,
    SelectorQuotaWindowStatus,
};
use codex_router_state::repositories::{
    AccountStateRepository, QuotaSnapshotRepository, SelectorQuotaRepository,
};
use tokio::io::AsyncReadExt;

struct HeldDurableAffinityRecorder {
    state: AsyncSqliteStateStore,
    entered: tokio::sync::mpsc::UnboundedSender<PreviousResponseAffinityOwnerRecord>,
    release: Arc<tokio::sync::Semaphore>,
}
impl AsyncHttpAffinityOwnerRecorder for HeldDurableAffinityRecorder {
    fn record_affinity_owner<'a>(
        &'a self,
        owner: PreviousResponseAffinityOwnerRecord,
    ) -> BoxFuture<'a, Result<(), HttpProxyError>> {
        Box::pin(async move {
            self.entered
                .send(owner.clone())
                .expect("real response-side task entered");
            self.release
                .acquire()
                .await
                .expect("fixture write release")
                .forget();
            self.state
                .write_previous_response_owner(&owner)
                .await
                .expect("actual durable affinity write");
            Ok(())
        })
    }
}
#[tokio::test]
async fn stopped_owner_retains_real_response_write_across_cancelled_drain_observer() {
    let root = tempfile::tempdir().expect("isolated response root");
    let path = root.path().join("state.sqlite");
    let secret_root = root.path().join("secrets");
    let account = AccountId::new("drain-response-account").expect("literal account");
    let legacy = SqliteStateStore::open(&path).expect("existing fixture initializer");
    AccountStateRepository::upsert_account(
        &legacy,
        &AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            account.clone(),
            "response owner",
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1),
    )
    .expect("literal account");
    QuotaSnapshotRepository::upsert_snapshot(
        &legacy,
        &PersistedQuotaSnapshot::new(account.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(1_000)
            .with_route_band("responses", 70)
            .with_stale_penalty(false),
    )
    .expect("representative quota");
    for window in [18_000, 604_800] {
        SelectorQuotaRepository::upsert_selector_window(
            &legacy,
            &PersistedSelectorQuotaWindow::new(
                account.clone(),
                "responses",
                window,
                SelectorQuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(70)
            .with_effective(window == 18_000)
            .with_observed_unix_seconds(1_000)
            .with_reset_unix_seconds(1_000 + window),
        )
        .expect("literal eligible windows");
    }
    drop(legacy);
    let secrets =
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root)
            .expect("external key fixture");
    secrets
        .write_secret(
            &openai_account_credential_bundle_key(&account, 1).expect("credential key"),
            &AccountCredentialBundle::imported_codex_auth(
                "response-access",
                Some("response-refresh".to_owned()),
            )
            .with_expires_unix_seconds(10_000)
            .to_secret_string()
            .expect("typed literal bundle"),
        )
        .expect("real credential");
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("fixture upstream");
    let upstream_address = upstream.local_addr().expect("upstream address");
    let upstream_task = tokio::spawn(async move {
        let (mut client, _) = upstream.accept().await.expect("actual upstream request");
        let mut request = Vec::new();
        let mut chunk = [0u8; 1024];
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let read = client
                .read(&mut chunk)
                .await
                .expect("upstream reads request");
            assert_ne!(read, 0);
            request.extend_from_slice(&chunk[..read]);
        }
        client.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 38\r\nConnection: close\r\n\r\ndata: {\"id\":\"resp_drain_completion\"}\n\n").await.expect("literal upstream SSE");
    });
    let config = LoopbackRouterRuntimeConfig::new_tokenless(
        LoopbackBindAddress::new("127.0.0.1", 0).expect("loopback"),
        UpstreamEndpoint::new(format!("http://{upstream_address}/v1")).expect("fixture endpoint"),
        path.clone(),
        secret_root,
    )
    .with_quota_clock(1_000, 60);
    let runtime = LoopbackRouterRuntime::start_with_credentials_for_test(config, secrets)
        .await
        .expect("real core");
    let state = AsyncSqliteStateStore::open_read_only(&path)
        .await
        .expect("actual observer");
    let writer = AsyncSqliteStateStore::open(&path)
        .await
        .expect("real write owner");
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let (entered, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let runtime = runtime.with_affinity_owner_recorder(Arc::new(HeldDurableAffinityRecorder {
        state: writer.clone(),
        entered,
        release: release.clone(),
    }));
    let address = runtime.local_addr();
    let client_task = tokio::spawn(async move {
        let mut client = tokio::net::TcpStream::connect(address)
            .await
            .expect("real proxy client");
        client.write_all(b"POST /v1/responses HTTP/1.1\r\nHost: localhost\r\nContent-Length: 17\r\nConnection: close\r\n\r\n{\"model\":\"gpt-5\"}").await.expect("literal model request");
        let mut reply = Vec::new();
        client
            .read_to_end(&mut reply)
            .await
            .expect("real proxy reply");
        reply
    });
    let mut stopped = runtime
        .stop_owned_protocol_connections_until_cancelled(1, CancellationToken::new())
        .await;
    let owner = tokio::time::timeout(Duration::from_secs(2), observed.recv())
        .await
        .expect("bounded response task entry")
        .expect("real response completion");
    let before = state
        .load_previous_response_owner(owner.affinity_key_hash(), "responses")
        .await
        .expect("actual pre-write lookup");
    assert!(
        stopped.take_serving_result().is_none(),
        "held real response owners forbid an early success result"
    );
    let pending =
        tokio::time::timeout(Duration::from_millis(20), stopped.drain_responses(&runtime))
            .await
            .is_err();
    release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(2), stopped.drain_responses(&runtime))
        .await
        .expect("same context resumes and joins write");
    let after = state
        .load_previous_response_owner(owner.affinity_key_hash(), "responses")
        .await
        .expect("actual durable lookup");
    runtime
        .credential_refresh_task_supervisor()
        .close_admission();
    runtime
        .credential_refresh_task_supervisor()
        .wait_until_completed()
        .await;
    stopped.drain_actor_work(&runtime).await;
    let outcome = stopped
        .take_serving_result()
        .expect("completed response owners expose outcome once");
    assert!(
        stopped.take_serving_result().is_none(),
        "original outcome is never silently replayed as success"
    );
    let reply = client_task.await.expect("client joined");
    upstream_task.await.expect("upstream joined");
    writer.close().await.expect("writer joins");
    state.close().await.expect("observer joins");
    eprintln!(
        "actual_response_write drain_pending={pending} owner_account={} generation={} before={before:?} after={after:?}",
        owner.account_id().as_str(),
        owner.credential_generation()
    );
    assert!(pending);
    assert_eq!(outcome.expect("original serving result"), 1);
    assert!(reply.starts_with(b"HTTP/1.1 200"));
    assert!(matches!(
        before,
        codex_router_state::affinity_owner::PreviousResponseAffinityOwnerLookup::Missing
    ));
    assert_eq!(owner.account_id(), &account);
    assert_eq!(owner.credential_generation(), 1);
    assert!(
        matches!(after,codex_router_state::affinity_owner::PreviousResponseAffinityOwnerLookup::Found(ref stored) if stored.account_id()==&account && stored.credential_generation()==1)
    );
}
