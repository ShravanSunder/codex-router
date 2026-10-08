use super::*;
use codex_router_state::quota_snapshot::{
    PersistedQuotaSnapshot, PersistedSelectorQuotaWindow, QuotaSnapshotSource,
    SelectorQuotaWindowStatus,
};
use codex_router_state::repositories::{
    AccountStateRepository, QuotaSnapshotRepository, SelectorQuotaRepository,
};
use tokio::io::AsyncReadExt;

use crate::db_write_actor::{DbWriteRepository, DbWriteRepositoryError};
use codex_router_core::ids::ReservationId;
use codex_router_state::session_account_affinity::SessionAccountAffinity;

struct HeldActorWriteRepository {
    inner: SqliteDbWriteRepository,
    entered: tokio::sync::mpsc::UnboundedSender<PreviousResponseAffinityOwnerRecord>,
    release: Arc<tokio::sync::Semaphore>,
}
impl DbWriteRepository for HeldActorWriteRepository {
    fn record_provider_quota_exhausted<'a>(
        &'a self,
        account: AccountId,
        band: RouteBand,
        classification: ProviderErrorClassification,
        now: u64,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        self.inner
            .record_provider_quota_exhausted(account, band, classification, now)
    }
    fn record_active_client_acquired<'a>(
        &'a self,
        band: RouteBand,
        process: String,
        reservation: ReservationId,
        account: AccountId,
        now: u64,
        pressure: u32,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        self.inner
            .record_active_client_acquired(band, process, reservation, account, now, pressure)
    }
    fn record_active_client_released<'a>(
        &'a self,
        band: RouteBand,
        process: String,
        reservation: ReservationId,
        now: u64,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        self.inner
            .record_active_client_released(band, process, reservation, now)
    }
    fn record_session_account_affinity<'a>(
        &'a self,
        affinity: SessionAccountAffinity,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        self.inner.record_session_account_affinity(affinity)
    }
    fn record_previous_response_affinity_owner<'a>(
        &'a self,
        owner: PreviousResponseAffinityOwnerRecord,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async move {
            self.entered
                .send(owner.clone())
                .expect("actual actor entered accepted response write");
            self.release
                .acquire()
                .await
                .expect("actual write released")
                .forget();
            self.inner
                .record_previous_response_affinity_owner(owner)
                .await
        })
    }
}
#[tokio::test]
async fn true_core_completion_must_wait_for_an_accepted_actor_write_past_original_grace() {
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
    let mut runtime = runtime;
    // Replace only the fixture's unserved setup owner, after joining it. One actual actor remains.
    runtime.db_write_actor.shutdown().await;
    let actor = DbWriteActor::start_on_handle(
        &tokio::runtime::Handle::current(),
        Arc::new(HeldActorWriteRepository {
            inner: SqliteDbWriteRepository::new(writer.clone()),
            entered,
            release: release.clone(),
        }),
        runtime.route_band_queue_health.clone(),
        PROVIDER_EXHAUSTION_QUEUE_CAPACITY,
    );
    runtime.affinity_owner_recorder = Arc::new(DbWriteAffinityOwnerRecorder::new(actor.clone()));
    runtime.db_write_actor = actor;
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
    stopped.drain_responses(&runtime).await;
    runtime
        .credential_refresh_task_supervisor()
        .close_admission();
    runtime
        .credential_refresh_task_supervisor()
        .wait_until_completed()
        .await;
    let began = std::time::Instant::now();
    let completed_while_write_held = tokio::time::timeout(
        Duration::from_millis(350),
        stopped.drain_actor_work(&runtime),
    )
    .await
    .is_ok();
    let elapsed = began.elapsed();
    let at_return = state
        .load_previous_response_owner(owner.affinity_key_hash(), "responses")
        .await
        .expect("literal row at reported completion");
    release.add_permits(1);
    stopped.drain_actor_work(&runtime).await;
    let after = state
        .load_previous_response_owner(owner.affinity_key_hash(), "responses")
        .await
        .expect("actual durable lookup");
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
        "actual_actor_shutdown completed_while_write_held={completed_while_write_held} elapsed_us={} owner_account={} generation={} before={before:?} at_return={at_return:?} after_release={after:?}",
        elapsed.as_micros(),
        owner.account_id().as_str(),
        owner.credential_generation()
    );
    assert_eq!(outcome.expect("original serving result"), 1);
    assert!(reply.starts_with(b"HTTP/1.1 200"));
    assert!(
        !completed_while_write_held,
        "accepted durable response write must not be aborted into successful drain completion"
    );
    assert!(matches!(
        before,
        codex_router_state::affinity_owner::PreviousResponseAffinityOwnerLookup::Missing
    ));
    assert!(
        matches!(after,codex_router_state::affinity_owner::PreviousResponseAffinityOwnerLookup::Found(ref stored) if stored.account_id()==&account && stored.credential_generation()==1)
    );
}
