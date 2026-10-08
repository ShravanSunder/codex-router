use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unserved_runtime_shutdown_closes_actor_admission_and_preserves_caller_runtime() {
    let database_path = test_database_path("unserved_runtime_shutdown");
    let secret_root = database_path.with_extension("secrets");
    let config = LoopbackRouterRuntimeConfig::new_tokenless(
        LoopbackBindAddress::new("127.0.0.1", 0)
            .unwrap_or_else(|error| panic!("fixture bind address should validate: {error}")),
        UpstreamEndpoint::new("http://127.0.0.1:1/v1")
            .unwrap_or_else(|error| panic!("unused fixture upstream should validate: {error}")),
        database_path,
        secret_root,
    );
    let runtime = LoopbackRouterRuntime::start_for_test(config)
        .await
        .unwrap_or_else(|error| panic!("unserved runtime should start: {error}"));

    runtime.shutdown().await;

    assert_eq!(
        runtime
            .db_write_actor
            .try_enqueue(DbWriteCommand::provider_quota_exhausted(
                AccountId::new("unserved-runtime-shutdown")
                    .unwrap_or_else(|error| panic!("fixture account id should validate: {error}")),
                RouteBand::Responses,
                ProviderErrorClassification::AccountQuotaExhausted,
                1_000,
            )),
        DbWriteEnqueueResult::ClosedDegraded,
        "runtime shutdown must close DB-write admission before returning"
    );
    assert_eq!(
        runtime.maintenance_actor.try_enqueue(
            MaintenanceHint::CleanupStaleSessionAccountAffinities {
                stale_before_unix_seconds: 1_000,
            },
        ),
        crate::maintenance_actor::MaintenanceEnqueueResult::ClosedDegraded,
        "runtime shutdown must close maintenance admission before returning"
    );

    let caller_runtime_result = tokio::spawn(async {
        tokio::task::yield_now().await;
        7
    })
    .await
    .unwrap_or_else(|error| panic!("caller runtime task should join after shutdown: {error}"));
    assert_eq!(caller_runtime_result, 7);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hyper_connection_error_drains_actors_and_preserves_caller_runtime() {
    use tokio::io::AsyncWriteExt;

    let database_path = test_database_path("hyper_error_runtime_shutdown");
    let secret_root = database_path.with_extension("secrets");
    let config = LoopbackRouterRuntimeConfig::new_tokenless(
        LoopbackBindAddress::new("127.0.0.1", 0)
            .unwrap_or_else(|error| panic!("fixture bind address should validate: {error}")),
        UpstreamEndpoint::new("http://127.0.0.1:1/v1")
            .unwrap_or_else(|error| panic!("unused fixture upstream should validate: {error}")),
        database_path,
        secret_root,
    );
    let runtime = LoopbackRouterRuntime::start_for_test(config)
        .await
        .unwrap_or_else(|error| panic!("runtime should start for Hyper error proof: {error}"));
    let local_address = runtime.local_addr();
    let client_task = tokio::spawn(async move {
        let mut client = tokio::net::TcpStream::connect(local_address)
            .await
            .unwrap_or_else(|error| panic!("malformed request client should connect: {error}"));
        client
            .write_all(b"\x16\x03\x01not-http\r\n\r\n")
            .await
            .unwrap_or_else(|error| panic!("malformed literal request should write: {error}"));
        client
            .shutdown()
            .await
            .unwrap_or_else(|error| panic!("malformed request client should half-close: {error}"));
    });

    let serve_result = runtime.serve_protocol_connections(1).await;
    client_task
        .await
        .unwrap_or_else(|error| panic!("malformed request client task should join: {error}"));

    assert!(
        matches!(
            &serve_result,
            Err(LoopbackRouterRuntimeError::HyperConnection(_))
        ),
        "Hyper failure should be returned after the serving cleanup tail: {serve_result:?}"
    );
    assert_eq!(
        runtime
            .db_write_actor
            .try_enqueue(DbWriteCommand::provider_quota_exhausted(
                AccountId::new("hyper-error-runtime-shutdown")
                    .unwrap_or_else(|error| panic!("fixture account id should validate: {error}")),
                RouteBand::Responses,
                ProviderErrorClassification::AccountQuotaExhausted,
                1_000,
            )),
        DbWriteEnqueueResult::ClosedDegraded,
        "Hyper failure cleanup must close DB-write admission before returning"
    );
    assert_eq!(
        runtime.maintenance_actor.try_enqueue(
            MaintenanceHint::CleanupStaleSessionAccountAffinities {
                stale_before_unix_seconds: 1_000,
            },
        ),
        crate::maintenance_actor::MaintenanceEnqueueResult::ClosedDegraded,
        "Hyper failure cleanup must close maintenance admission before returning"
    );

    let caller_runtime_result = tokio::spawn(async {
        tokio::task::yield_now().await;
        11
    })
    .await
    .unwrap_or_else(|error| panic!("caller runtime task should join after Hyper error: {error}"));
    assert_eq!(caller_runtime_result, 11);
}
