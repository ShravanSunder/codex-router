    #[tokio::test]
    async fn hyper_loopback_classifier_maps_closed_canceled_incomplete_to_client_disconnect() {
        let incomplete_message = hyper_error_from_client_bytes(
            b"POST /v1/responses HTTP/1.1\r\nHost: localhost\r\nContent-Length: 12\r\n\r\nshort",
        )
        .await;

        let diagnostic = loopback_connection_diagnostic(
            &LoopbackRouterRuntimeError::HyperConnection(incomplete_message),
        );

        assert_eq!(diagnostic.class(), "client_disconnect");
        assert_eq!(diagnostic.safe_reason(), "hyper_incomplete_message");
        assert_eq!(diagnostic.severity(), "debug");

        let malformed_request =
            hyper_error_from_client_bytes(b"\x16\x03\x01not-http\r\n\r\n").await;
        let diagnostic = loopback_connection_diagnostic(
            &LoopbackRouterRuntimeError::HyperConnection(malformed_request),
        );

        assert_eq!(diagnostic.class(), "malformed_request");
        assert_eq!(diagnostic.safe_reason(), "hyper_parse");
        assert_eq!(diagnostic.severity(), "warn");
    }

    #[tokio::test]
    async fn detached_hyper_connection_reports_scrubbed_root_cause_class() {
        let incomplete_message = hyper_error_from_client_bytes(
            b"POST /v1/responses HTTP/1.1\r\nHost: localhost\r\nContent-Length: 12\r\n\r\nshort",
        )
        .await;
        let reporter = Arc::new(RecordingLoopbackConnectionErrorReporter::default());
        let detached = tokio::spawn(async move {
            Err(LoopbackRouterRuntimeError::HyperConnection(
                incomplete_message,
            ))
        });

        supervise_detached_connection_handler(detached, reporter.clone());
        let diagnostic = wait_for_connection_diagnostic(&reporter).await;

        assert!(diagnostic.contains("class=client_disconnect"));
        assert!(diagnostic.contains("reason=hyper_incomplete_message"));
        assert!(diagnostic.contains("severity=debug"));
        assert!(!diagnostic.contains("/v1/responses"));
        assert!(!diagnostic.contains("Content-Length"));
    }

    #[tokio::test]
    async fn failed_hyper_response_body_drop_releases_active_reservation_and_mirror() {
        let active_reservations = RouteBandReservationBooks::default();
        let account_id = codex_router_core::ids::AccountId::new("acct_hyper_cleanup")
            .unwrap_or_else(|error| panic!("test account id should validate: {error}"));
        let reservation_handle = {
            let mut reservations = active_reservations
                .lock()
                .unwrap_or_else(|error| panic!("reservations lock should be available: {error}"));
            reservations
                .entry(RouteBand::Responses.as_str().to_owned())
                .or_default()
                .reserve_next_at(account_id.clone(), 1, 1_000)
        };
        let lease_reporter = RecordingActiveClientLeaseReporter::default();
        let active_reservation_guard =
            crate::account_selection::ActiveReservationGuard::new_with_active_client_leases(
                active_reservations.clone(),
                RouteBand::Responses.as_str().to_owned(),
                reservation_handle.clone(),
                Some(Arc::new(lease_reporter.clone())),
            );
        let body =
            hold_active_reservation_until_body_drop(empty_body(), Some(active_reservation_guard));

        drop(body);

        let active_count_after_drop = {
            let reservations = active_reservations
                .lock()
                .unwrap_or_else(|error| panic!("reservations lock should be available: {error}"));
            reservations
                .get(RouteBand::Responses.as_str())
                .map_or(0, |book| book.active_session_count(&account_id))
        };
        assert_eq!(
            active_count_after_drop, 0,
            "failed Hyper serving must release process-local active reservation when the response body is dropped"
        );
        assert_eq!(
            lease_reporter.released(),
            vec![(
                RouteBand::Responses.as_str().to_owned(),
                reservation_handle.reservation_id().as_str().to_owned(),
            )],
            "failed Hyper serving must also mirror active-client release"
        );
    }

    #[tokio::test]
    async fn hyper_connection_error_still_drains_upgrade_tasks() {
        let upgrade_tasks = SharedUpgradeTasks::default();
        let upgrade_task_was_drained = Arc::new(AtomicBool::new(false));
        let drained_marker = Arc::clone(&upgrade_task_was_drained);
        upgrade_tasks.lock().await.push(tokio::spawn(async move {
            drained_marker.store(true, Ordering::SeqCst);
            Ok(())
        }));

        let result = finish_hyper_connection_after_serve_result(
            Err(LoopbackRouterRuntimeError::Connection(
                ServerConnectionError::PartialRequest,
            )),
            Arc::clone(&upgrade_tasks),
        )
        .await;

        assert!(result.is_err());
        assert!(
            upgrade_task_was_drained.load(Ordering::SeqCst),
            "Hyper connection cleanup must drain upgrade tasks even when serve_connection fails"
        );
        assert!(
            upgrade_tasks.lock().await.is_empty(),
            "drained upgrade tasks must be removed from the shared task list"
        );
    }

    #[tokio::test]
    async fn provider_error_observer_marks_route_band_queue_health_degraded_when_queue_closed() {
        let database_path = test_database_path("provider_error_queue_health_closed");
        let store = AsyncSqliteStateStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("async state store should open: {error}"));
        let route_band_queue_health = RouteBandQueueHealth::default();
        let db_write_actor = DbWriteActor::start_on_handle(
            &tokio::runtime::Handle::current(),
            Arc::new(SqliteDbWriteRepository::new(store.clone())),
            route_band_queue_health.clone(),
            1,
        );
        db_write_actor.shutdown().await;
        let observer = AsyncSqliteProviderErrorObserver::new(
            store.clone(),
            store,
            RouteBandReservationBooks::default(),
            RouteBandRuntimeExhaustions::default(),
            route_band_queue_health.clone(),
            db_write_actor,
        );
        let account_id = codex_router_core::ids::AccountId::new("acct_queue_closed")
            .unwrap_or_else(|error| panic!("test account id should validate: {error}"));

        let enqueue_result = observer.enqueue_provider_quota_exhaustion(
            account_id,
            RouteBand::Responses,
            ProviderErrorClassification::AccountQuotaExhausted,
            1_000,
        );

        assert_eq!(enqueue_result, DbWriteEnqueueResult::ClosedDegraded);
        assert!(
            crate::account_selection::route_band_queue_health_allows_selection(
                &route_band_queue_health,
                RouteBand::Responses,
            )
            .is_err(),
            "closed DB write queues must degrade the whole route band for new selections"
        );
    }

    #[tokio::test]
    async fn runtime_writable_state_stores_use_distinct_pools_for_credential_db_write_and_maintenance()
     {
        let database_path = test_database_path("runtime_writable_store_pool_isolation");
        let stores = super::open_runtime_writable_state_stores(&database_path)
            .await
            .unwrap_or_else(|error| panic!("runtime writable stores should open: {error}"));

        let held_maintenance_connection = stores
            .maintenance_state_store
            .acquire_connection_for_test()
            .await
            .unwrap_or_else(|error| panic!("maintenance connection should be held: {error}"));
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            stores.credential_state_store.schema_version(),
        )
        .await
        .unwrap_or_else(|_elapsed| {
            panic!("credential store must not wait behind a held maintenance pool connection")
        })
        .unwrap_or_else(|error| panic!("credential store should remain readable: {error}"));
        drop(held_maintenance_connection);

        let held_db_write_connection = stores
            .db_write_state_store
            .acquire_connection_for_test()
            .await
            .unwrap_or_else(|error| panic!("DB-write connection should be held: {error}"));
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            stores.credential_state_store.schema_version(),
        )
        .await
        .unwrap_or_else(|_elapsed| {
            panic!("credential store must not wait behind a held DB-write pool connection")
        })
        .unwrap_or_else(|error| panic!("credential store should remain readable: {error}"));
        drop(held_db_write_connection);

        stores
            .credential_state_store
            .close()
            .await
            .unwrap_or_else(|error| panic!("credential store should close: {error}"));

        stores
            .db_write_state_store
            .schema_version()
            .await
            .unwrap_or_else(|error| {
                panic!("DB-write store must not share the credential pool: {error}")
            });
        stores
            .maintenance_state_store
            .schema_version()
            .await
            .unwrap_or_else(|error| {
                panic!("maintenance store must not share the credential pool: {error}")
            });

        stores
            .db_write_state_store
            .close()
            .await
            .unwrap_or_else(|error| panic!("DB-write store should close: {error}"));
        stores
            .maintenance_state_store
            .schema_version()
            .await
            .unwrap_or_else(|error| {
                panic!("maintenance store must not share the DB-write pool: {error}")
            });
    }

    #[tokio::test]
    async fn state_unavailable_selection_response_is_not_all_accounts_exhausted() {
        let response = http_error_response(HttpProxyError::Selection {
            reason: crate::account_selection::QuotaAwareAccountSelectorError::StateUnavailable,
        });

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| panic!("router error body should collect: {error}"))
            .to_bytes();
        let rendered = String::from_utf8(body.to_vec())
            .unwrap_or_else(|error| panic!("router error body should be utf-8: {error}"));

        assert!(rendered.contains("codex_router_quota_state_unavailable"));
        assert!(!rendered.contains("codex_router_all_accounts_exhausted"));
    }

    #[tokio::test]
    async fn account_attempt_limit_read_failure_maps_to_state_unavailable_not_all_exhausted() {
        let read_failure = StateStoreError::Sqlite {
            message: "list accounts unavailable".to_owned(),
        };

        let result = enabled_account_attempt_limit_from_accounts(Err(read_failure));

        assert!(
            result.is_err(),
            "failed account-list reads must not collapse to a one-attempt retry limit"
        );
        let response = quota_state_unavailable_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| panic!("router error body should collect: {error}"))
            .to_bytes();
        let rendered = String::from_utf8(body.to_vec())
            .unwrap_or_else(|error| panic!("router error body should be utf-8: {error}"));

        assert!(rendered.contains("codex_router_quota_state_unavailable"));
        assert!(!rendered.contains("codex_router_all_accounts_exhausted"));
    }

    async fn hyper_error_from_client_bytes(bytes: &'static [u8]) -> hyper::Error {
        let listener = TokioTcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|error| panic!("test hyper listener should bind: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("test hyper listener address should read: {error}"));
        let server = tokio::spawn(async move {
            let (stream, _peer_addr) = listener
                .accept()
                .await
                .unwrap_or_else(|error| panic!("test hyper listener should accept: {error}"));
            let io = TokioIo::new(stream);
            let service = service_fn(|request: HttpRequest<Incoming>| async move {
                request.into_body().collect().await?;
                Ok::<_, hyper::Error>(HttpResponse::new(Full::new(Bytes::new())))
            });
            http1::Builder::new().serve_connection(io, service).await
        });
        let mut client = tokio::net::TcpStream::connect(address)
            .await
            .unwrap_or_else(|error| panic!("test hyper client should connect: {error}"));
        client
            .write_all(bytes)
            .await
            .unwrap_or_else(|error| panic!("test hyper client should write: {error}"));
        client
            .shutdown()
            .await
            .unwrap_or_else(|error| panic!("test hyper client should shutdown: {error}"));

        match server
            .await
            .unwrap_or_else(|error| panic!("test hyper server task should join: {error}"))
        {
            Ok(()) => panic!("test hyper server should fail for malformed client bytes"),
            Err(error) => error,
        }
    }

    async fn wait_for_connection_diagnostic(
        reporter: &RecordingLoopbackConnectionErrorReporter,
    ) -> String {
        let started_at = tokio::time::Instant::now();
        loop {
            if let Some(diagnostic) = reporter.diagnostics().into_iter().next() {
                return diagnostic;
            }
            assert!(
                started_at.elapsed() < Duration::from_secs(2),
                "connection diagnostic should be reported"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn test_database_path(name: &str) -> PathBuf {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!(
            "codex-router-proxy-server-{name}-{}-{counter}.sqlite",
            std::process::id()
        ))
    }

    fn lock_test_mutex<'a, T>(mutex: &'a Mutex<T>, label: &str) -> MutexGuard<'a, T> {
        mutex
            .lock()
            .unwrap_or_else(|error| panic!("{label} lock should be available: {error}"))
    }

    impl RecordingAsyncAffinityOwnerRecorder {
        fn records(&self) -> Vec<PreviousResponseAffinityOwnerRecord> {
            lock_test_mutex(&self.records, "affinity recorder").clone()
        }
    }

    impl AsyncHttpAffinityOwnerRecorder for RecordingAsyncAffinityOwnerRecorder {
        fn record_affinity_owner<'a>(
            &'a self,
            owner: PreviousResponseAffinityOwnerRecord,
        ) -> BoxFuture<'a, Result<(), HttpProxyError>> {
            Box::pin(async move {
                match self.records.lock() {
                    Ok(mut records) => records.push(owner),
                    Err(error) => panic!("test recorder lock should be available: {error}"),
                }
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn async_http_affinity_scan_stops_at_explicit_bounds_without_gating_body() {
        let recorder = RecordingAsyncAffinityOwnerRecorder::default();
        let account_id = match codex_router_core::ids::AccountId::new("acct_selected") {
            Ok(account_id) => account_id,
            Err(error) => panic!("test account id should validate: {error}"),
        };
        let affinity_secret = match codex_router_core::affinity::RouterAffinityHashSecret::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ) {
            Ok(secret) => secret,
            Err(error) => panic!("test affinity secret should validate: {error}"),
        };
        let completion = StreamingHttpProxyCompletion::new_for_test(
            Some(affinity_secret),
            account_id,
            7,
            crate::http_sse::allowed_audit_event(
                TransportKind::Http,
                AuditRouteKind::Responses,
                "acct_hash".to_owned(),
            ),
        );
        let late_response_id =
            Bytes::from_static(br#"data: {"id":"resp_after_bound_should_not_record"}\n\n"#);
        let chunks = vec![
            Bytes::from(vec![b'a'; HTTP_RESPONSE_AFFINITY_SCAN_MAX_BYTES]),
            late_response_id.clone(),
        ];
        let body_stream = futures_util::stream::iter(
            chunks
                .into_iter()
                .map(|chunk| Ok::<_, AsyncHttpBodyError>(Frame::data(chunk))),
        );
        let body = BodyExt::boxed(StreamBody::new(body_stream));
        let affinity_record_tasks = TaskTracker::new();
        let forwarded_body = record_affinity_owner_from_async_body(
            body,
            completion,
            Arc::new(recorder.clone()),
            None,
            affinity_record_tasks.clone(),
        );
        let forwarded_bytes = match forwarded_body.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(error) => panic!("forwarded body should collect: {error}"),
        };
        affinity_record_tasks.close();
        affinity_record_tasks.wait().await;

        assert!(forwarded_bytes.ends_with(&late_response_id));
        assert_eq!(recorder.records(), Vec::new());
    }

    #[tokio::test]
    async fn async_http_usage_limit_body_is_forwarded_unchanged_and_observed() {
        let affinity_recorder = RecordingAsyncAffinityOwnerRecorder::default();
        let provider_error_observer = RecordingAsyncProviderErrorObserver::default();
        let account_id = match codex_router_core::ids::AccountId::new("acct_selected") {
            Ok(account_id) => account_id,
            Err(error) => panic!("test account id should validate: {error}"),
        };
        let completion = StreamingHttpProxyCompletion::new_for_test(
            None,
            account_id.clone(),
            7,
            crate::http_sse::allowed_audit_event(
                TransportKind::Http,
                AuditRouteKind::Responses,
                "acct_hash".to_owned(),
            ),
        )
        .with_route_band_for_test(RouteBand::Responses);
        let usage_limit_body = Bytes::from_static(
            br#"{"type":"error","error":{"type":"usage_limit_reached","code":"usage_limit_reached"}}"#,
        );
        let body_stream = futures_util::stream::iter(std::iter::once(Ok::<_, AsyncHttpBodyError>(
            Frame::data(usage_limit_body.clone()),
        )));
        let body = BodyExt::boxed(StreamBody::new(body_stream));
        let metadata_tasks = TaskTracker::new();
        let forwarded_body = record_affinity_owner_from_async_body(
            body,
            completion,
            Arc::new(affinity_recorder.clone()),
            Some(Arc::new(provider_error_observer.clone())),
            metadata_tasks.clone(),
        );

        let forwarded_bytes = match forwarded_body.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(error) => panic!("forwarded body should collect: {error}"),
        };
        metadata_tasks.close();
        metadata_tasks.wait().await;

        assert_eq!(forwarded_bytes, usage_limit_body);
        assert_eq!(affinity_recorder.records(), Vec::new());
        assert_eq!(
            provider_error_observer.records(),
            vec![RecordedHttpProviderError {
                account_id,
                route_band: RouteBand::Responses,
                classification: ProviderErrorClassification::AccountQuotaExhausted,
            }]
        );
    }

    #[tokio::test]
    async fn precommit_http_usage_limit_body_requests_account_retry_before_commit() {
        let usage_limit_body = Bytes::from_static(
            br#"{"type":"error","error":{"type":"usage_limit_reached","code":"usage_limit_reached"}}"#,
        );
        let body_stream = futures_util::stream::iter(std::iter::once(Ok::<_, AsyncHttpBodyError>(
            Frame::data(usage_limit_body.clone()),
        )));
        let body = BodyExt::boxed(StreamBody::new(body_stream));
        let response =
            AsyncStreamingHttpProxyResponse::new(429, HeaderCollection::new(Vec::new()), body);

        match split_precommit_http_quota_response(response).await {
            Ok(PrecommitHttpResponseProbe::AccountQuotaExhausted { body }) => {
                assert_eq!(body, usage_limit_body.to_vec());
            }
            Ok(PrecommitHttpResponseProbe::Forward(_response)) => {
                panic!("quota response should request account retry before commit");
            }
            Err(error) => panic!("quota response should classify before commit: {error}"),
        }
    }

    #[tokio::test]
    async fn precommit_http_quota_retry_enqueues_without_awaiting_durable_observation() {
        let observer = Arc::new(NonblockingHttpQuotaObserver::default());
        let account_id = codex_router_core::ids::AccountId::new("acct_http_quota")
            .unwrap_or_else(|error| panic!("test account id should validate: {error}"));

        tokio::time::timeout(Duration::from_millis(50), async {
            observe_precommit_http_quota_exhaustion_for_retry(
                Some(observer.clone()),
                account_id.clone(),
                RouteBand::Responses,
                1_000,
            )
        })
        .await
        .unwrap_or_else(|_elapsed| {
            panic!("precommit quota retry must not await durable provider-error observation")
        })
        .unwrap_or_else(|error| panic!("precommit quota enqueue should succeed: {error:?}"));

        assert!(!observer.durable_observation_called.load(Ordering::SeqCst));
        assert_eq!(
            observer.enqueued_records(),
            vec![RecordedHttpProviderError {
                account_id,
                route_band: RouteBand::Responses,
                classification: ProviderErrorClassification::AccountQuotaExhausted,
            }]
        );
    }

    #[tokio::test]
    async fn postcommit_http_quota_observation_uses_db_write_actor_without_direct_sqlite_wait() {
        let affinity_recorder = RecordingAsyncAffinityOwnerRecorder::default();
        let observer = Arc::new(NonblockingHttpQuotaObserver::default());
        let account_id = codex_router_core::ids::AccountId::new("acct_http_postcommit_quota")
            .unwrap_or_else(|error| panic!("test account id should validate: {error}"));
        let completion = StreamingHttpProxyCompletion::new_for_test(
            None,
            account_id.clone(),
            7,
            crate::http_sse::allowed_audit_event(
                TransportKind::Http,
                AuditRouteKind::Responses,
                "acct_hash".to_owned(),
            ),
        )
        .with_route_band_for_test(RouteBand::Responses);
        let usage_limit_body = Bytes::from_static(
            br#"{"type":"error","error":{"type":"usage_limit_reached","code":"usage_limit_reached"}}"#,
        );
        let body_stream = futures_util::stream::iter(std::iter::once(Ok::<_, AsyncHttpBodyError>(
            Frame::data(usage_limit_body.clone()),
        )));
        let body = BodyExt::boxed(StreamBody::new(body_stream));
        let metadata_tasks = TaskTracker::new();
        let forwarded_body = record_affinity_owner_from_async_body(
            body,
            completion,
            Arc::new(affinity_recorder),
            Some(observer.clone()),
            metadata_tasks.clone(),
        );

        let forwarded_bytes = forwarded_body
            .collect()
            .await
            .unwrap_or_else(|error| panic!("forwarded body should collect: {error}"))
            .to_bytes();
        metadata_tasks.close();
        tokio::time::timeout(Duration::from_millis(50), metadata_tasks.wait())
            .await
            .unwrap_or_else(|_elapsed| {
                panic!("postcommit quota observation must not wait on direct durable observer")
            });

        assert_eq!(forwarded_bytes, usage_limit_body);
        assert!(!observer.durable_observation_called.load(Ordering::SeqCst));
        assert_eq!(
            observer.enqueued_records(),
            vec![RecordedHttpProviderError {
                account_id,
                route_band: RouteBand::Responses,
                classification: ProviderErrorClassification::AccountQuotaExhausted,
            }]
        );
    }

    #[tokio::test]
    async fn precommit_http_non_error_body_replays_exact_bytes() {
        let response_body = Bytes::from_static(br#"data: {"id":"resp-ok"}\n\n"#);
        let body_stream = futures_util::stream::iter(std::iter::once(Ok::<_, AsyncHttpBodyError>(
            Frame::data(response_body.clone()),
        )));
        let body = BodyExt::boxed(StreamBody::new(body_stream));
        let response =
            AsyncStreamingHttpProxyResponse::new(200, HeaderCollection::new(Vec::new()), body);

        match split_precommit_http_quota_response(response).await {
            Ok(PrecommitHttpResponseProbe::Forward(response)) => {
                let (_status, _headers, body) = response.into_parts();
                let forwarded_bytes = match body.collect().await {
                    Ok(collected) => collected.to_bytes(),
                    Err(error) => panic!("forwarded body should collect: {error}"),
                };
                assert_eq!(forwarded_bytes, response_body);
            }
            Ok(PrecommitHttpResponseProbe::AccountQuotaExhausted { .. }) => {
                panic!("non-error response should pass through");
            }
            Err(error) => panic!("non-error response should pass through: {error}"),
        }
    }

    #[tokio::test]
    async fn async_sse_usage_limit_data_line_is_forwarded_unchanged_and_observed() {
        let affinity_recorder = RecordingAsyncAffinityOwnerRecorder::default();
        let provider_error_observer = RecordingAsyncProviderErrorObserver::default();
        let account_id = match codex_router_core::ids::AccountId::new("acct_selected") {
            Ok(account_id) => account_id,
            Err(error) => panic!("test account id should validate: {error}"),
        };
        let completion = StreamingHttpProxyCompletion::new_for_test(
            None,
            account_id.clone(),
            7,
            crate::http_sse::allowed_audit_event(
                TransportKind::Http,
                AuditRouteKind::Responses,
                "acct_hash".to_owned(),
            ),
        )
        .with_route_band_for_test(RouteBand::Responses);
        let provider_error_json = br#"{"type":"error","error":{"code":"usage_limit_reached"}}"#;
        let sse_body = Bytes::from(
            [
                b"event: error\n".as_slice(),
                b"data: ".as_slice(),
                provider_error_json.as_slice(),
                b"\n\n".as_slice(),
            ]
            .concat(),
        );
        let body_stream = futures_util::stream::iter(std::iter::once(Ok::<_, AsyncHttpBodyError>(
            Frame::data(sse_body.clone()),
        )));
        let body = BodyExt::boxed(StreamBody::new(body_stream));
        let metadata_tasks = TaskTracker::new();
        let forwarded_body = record_affinity_owner_from_async_body(
            body,
            completion,
            Arc::new(affinity_recorder.clone()),
            Some(Arc::new(provider_error_observer.clone())),
            metadata_tasks.clone(),
        );

        let forwarded_bytes = match forwarded_body.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(error) => panic!("forwarded body should collect: {error}"),
        };
        metadata_tasks.close();
        metadata_tasks.wait().await;

        assert_eq!(forwarded_bytes, sse_body);
        assert_eq!(affinity_recorder.records(), Vec::new());
        assert_eq!(
            provider_error_observer.records(),
            vec![RecordedHttpProviderError {
                account_id,
                route_band: RouteBand::Responses,
                classification: ProviderErrorClassification::AccountQuotaExhausted,
            }]
        );
    }

    #[tokio::test]
    async fn async_http_ambiguous_quota_text_is_forwarded_unchanged_without_observation() {
        let affinity_recorder = RecordingAsyncAffinityOwnerRecorder::default();
        let provider_error_observer = RecordingAsyncProviderErrorObserver::default();
        let account_id = match codex_router_core::ids::AccountId::new("acct_selected") {
            Ok(account_id) => account_id,
            Err(error) => panic!("test account id should validate: {error}"),
        };
        let completion = StreamingHttpProxyCompletion::new_for_test(
            None,
            account_id,
            7,
            crate::http_sse::allowed_audit_event(
                TransportKind::Http,
                AuditRouteKind::Responses,
                "acct_hash".to_owned(),
            ),
        )
        .with_route_band_for_test(RouteBand::Responses);
        let model_text_body = Bytes::from_static(
            br#"{"type":"response.output_text.delta","delta":"usage_limit_reached is only text"}"#,
        );
        let body_stream = futures_util::stream::iter(std::iter::once(Ok::<_, AsyncHttpBodyError>(
            Frame::data(model_text_body.clone()),
        )));
        let body = BodyExt::boxed(StreamBody::new(body_stream));
        let metadata_tasks = TaskTracker::new();
        let forwarded_body = record_affinity_owner_from_async_body(
            body,
            completion,
            Arc::new(affinity_recorder.clone()),
            Some(Arc::new(provider_error_observer.clone())),
            metadata_tasks.clone(),
        );

        let forwarded_bytes = match forwarded_body.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(error) => panic!("forwarded body should collect: {error}"),
        };
        metadata_tasks.close();
        metadata_tasks.wait().await;

        assert_eq!(forwarded_bytes, model_text_body);
        assert_eq!(affinity_recorder.records(), Vec::new());
        assert_eq!(provider_error_observer.records(), Vec::new());
    }
