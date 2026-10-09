use super::*;

#[tokio::test]
async fn session_record_reload_worker_runs_single_flight_and_keeps_only_latest_pending_query() {
    let initial_request = SessionRecordsReloadRequest {
        generation: 0,
        query: reload_query("initial"),
        sources: vec![crate::presentation::session_picker::PickerSourceContext::DefaultHosted],
    };
    let (sender, receiver) = tokio::sync::watch::channel(initial_request);
    let (started_sender, mut started_receiver) = tokio::sync::mpsc::unbounded_channel();
    let active_loads = Arc::new(AtomicUsize::new(0));
    let maximum_active_loads = Arc::new(AtomicUsize::new(0));
    struct ActiveSourceRead(Arc<AtomicUsize>);
    impl Drop for ActiveSourceRead {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let loader: SessionsPickerRecordLoader = Arc::new({
        let active_loads = Arc::clone(&active_loads);
        let maximum_active_loads = Arc::clone(&maximum_active_loads);
        move |request| {
            let active_loads = Arc::clone(&active_loads);
            let maximum_active_loads = Arc::clone(&maximum_active_loads);
            let started_sender = started_sender.clone();
            Box::pin(async move {
                let active = active_loads.fetch_add(1, Ordering::SeqCst) + 1;
                let _active_read = ActiveSourceRead(active_loads);
                maximum_active_loads.fetch_max(active, Ordering::SeqCst);
                let search = request.query.search.clone();
                let _ = started_sender.send(search.clone());
                if search == "a" {
                    // Supersession must drop this owned read; a blocking external procedure
                    // would remain alive after its join future was dropped.
                    futures_util::future::pending::<()>().await;
                }
                crate::presentation::session_picker::SourceInventoryResult::Ready {
                    bound_endpoint: None,
                    request,
                    snapshot: observed_records(vec![picker_record(
                        &format!("thread-{search}"),
                        &format!("result {search}"),
                        "/repo/project-a",
                        "codex-router",
                        "cli",
                    )]),
                }
            })
        }
    });
    let current_generation = Arc::new(AtomicU64::new(0));
    let (accepted_sender, mut accepted_receiver) = tokio::sync::mpsc::unbounded_channel::<String>();
    let worker = tokio::spawn({
        let current_generation = Arc::clone(&current_generation);
        async move {
            run_session_record_reload_worker(receiver, loader, move |request, update| {
                if update.into_snapshot().is_some()
                    && current_generation.load(Ordering::SeqCst) == request.generation
                {
                    let _ = accepted_sender.send(request.query.search);
                }
            })
            .await;
        }
    });

    current_generation.store(1, Ordering::SeqCst);
    sender.send_replace(SessionRecordsReloadRequest {
        generation: 1,
        query: reload_query("a"),
        sources: vec![crate::presentation::session_picker::PickerSourceContext::DefaultHosted],
    });
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), started_receiver.recv())
            .await
            .unwrap()
            .as_deref(),
        Some("a")
    );

    current_generation.store(2, Ordering::SeqCst);
    sender.send_replace(SessionRecordsReloadRequest {
        generation: 2,
        query: reload_query("b"),
        sources: vec![crate::presentation::session_picker::PickerSourceContext::DefaultHosted],
    });
    current_generation.store(3, Ordering::SeqCst);
    sender.send_replace(SessionRecordsReloadRequest {
        generation: 3,
        query: reload_query("c"),
        sources: vec![crate::presentation::session_picker::PickerSourceContext::DefaultHosted],
    });
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), started_receiver.recv())
            .await
            .unwrap()
            .as_deref(),
        Some("c")
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), accepted_receiver.recv())
            .await
            .unwrap()
            .as_deref(),
        Some("c")
    );
    assert_eq!(maximum_active_loads.load(Ordering::SeqCst), 1);
    assert_eq!(
        active_loads.load(Ordering::SeqCst),
        0,
        "the superseded pending read and completed current read must both drop"
    );
    assert!(
        started_receiver.try_recv().is_err(),
        "obsolete B must not run"
    );

    drop(sender);
    tokio::time::timeout(Duration::from_secs(2), worker)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn periodic_same_generation_refresh_does_not_starve_a_slow_result() {
    let (sender, receiver) = tokio::sync::watch::channel(SessionRecordsReloadRequest {
        generation: 0,
        query: reload_query("initial"),
        sources: vec![crate::presentation::session_picker::PickerSourceContext::DefaultHosted],
    });
    let (release_slow_sender, release_slow_receiver) = mpsc::channel::<()>();
    let release_slow_receiver = Arc::new(Mutex::new(release_slow_receiver));
    let (started_sender, mut started_receiver) = tokio::sync::mpsc::unbounded_channel();
    let load_count = Arc::new(AtomicUsize::new(0));
    let active_loads = Arc::new(AtomicUsize::new(0));
    let maximum_active_loads = Arc::new(AtomicUsize::new(0));
    let loader: SessionsPickerRecordLoader = fixture_record_loader({
        let release_slow_receiver = Arc::clone(&release_slow_receiver);
        let load_count = Arc::clone(&load_count);
        let active_loads = Arc::clone(&active_loads);
        let maximum_active_loads = Arc::clone(&maximum_active_loads);
        move |query| {
            let active = active_loads.fetch_add(1, Ordering::SeqCst) + 1;
            maximum_active_loads.fetch_max(active, Ordering::SeqCst);
            let invocation = load_count.fetch_add(1, Ordering::SeqCst);
            let _ = started_sender.send(query.search);
            if invocation == 0 {
                release_slow_receiver
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap_or_else(|error| panic!("slow refresh should release: {error}"));
            }
            active_loads.fetch_sub(1, Ordering::SeqCst);
            Ok(observed_records(Vec::new()))
        }
    });
    let (accepted_sender, mut accepted_receiver) = tokio::sync::mpsc::unbounded_channel();
    let worker = tokio::spawn(async move {
        run_session_record_reload_worker(receiver, loader, move |request, update| {
            if request.generation == 1
                && update
                    .into_snapshot()
                    .is_some_and(|records| records.is_ok())
            {
                let _ = accepted_sender.send(request.query.search);
            }
        })
        .await;
    });
    let refresh_request = SessionRecordsReloadRequest {
        generation: 1,
        query: reload_query("same-query"),
        sources: vec![crate::presentation::session_picker::PickerSourceContext::DefaultHosted],
    };

    sender.send_replace(refresh_request.clone());
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), started_receiver.recv())
            .await
            .unwrap_or_else(|error| panic!("slow refresh should start: {error}"))
            .as_deref(),
        Some("same-query")
    );
    sender.send_replace(refresh_request);
    release_slow_sender
        .send(())
        .unwrap_or_else(|error| panic!("slow refresh release should send: {error}"));

    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), accepted_receiver.recv())
            .await
            .unwrap_or_else(|error| panic!("slow result should be accepted: {error}"))
            .as_deref(),
        Some("same-query")
    );
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), started_receiver.recv())
            .await
            .unwrap_or_else(|error| panic!("pending refresh should start: {error}"))
            .as_deref(),
        Some("same-query")
    );
    assert_eq!(maximum_active_loads.load(Ordering::SeqCst), 1);

    worker.abort();
}

#[tokio::test]
async fn sessions_picker_view_shortcut_filters_the_latest_loaded_records() {
    let observed_queries = Arc::new(Mutex::new(Vec::<SessionsPickerDataQuery>::new()));
    let loader_queries = Arc::clone(&observed_queries);
    let record_loader: SessionsPickerRecordLoader = fixture_record_loader(move |query| {
        loader_queries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(query);
        let mut record = picker_record(
            "thread-reloaded",
            "Reloaded SQL result",
            "/repo/project-a",
            "codex-router",
            "subagent",
        );
        record.runtime_status = crate::picker_runtime_status::PickerRuntimeStatus::Blocked;
        Ok(observed_records(vec![record]))
    });
    let mut request = picker_request();
    request.source = SessionsSource::All;
    request.records = vec![picker_record(
        "thread-initial",
        "Initial SQL result",
        "/repo/project-a",
        "codex-router",
        "cli",
    )];

    let events =
        futures_util::stream::once(async { ctrl_key('t') }).chain(futures_util::stream::pending());
    let mut picker = element! {
        SessionsPickerComponent(
            request,
            record_loader: Some(record_loader),
            width: 100usize,
            // This test observes reload behavior, so keep both Start New and the loaded row visible.
            height: 40usize,
        )
    };
    let frames = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(events));
    tokio::pin!(frames);
    let actual = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(canvas) = frames.next().await {
            let snapshot = canvas.to_string();
            if snapshot.contains("View: [Blocked]") && snapshot.contains("Reloaded SQL result") {
                return snapshot;
            }
        }
        panic!("picker ended before rendering its loaded result");
    })
    .await
    .expect("loaded blocked result must render within the deadline");

    let queries = observed_queries
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert!(
        queries
            .iter()
            .any(|query| query.source == SessionsSource::All),
        "runtime view changes must preserve the request's source query: {queries:?}"
    );
    assert!(
        actual.contains('◆')
            && !actual.contains("◆ Blocked")
            && actual.contains("Reloaded SQL result"),
        "the Blocked view should show the blocked loaded result: {actual:?}"
    );
}

#[tokio::test]
async fn picker_explains_unavailable_live_status_and_clears_notice_after_recovery() {
    use crate::picker_runtime_status::{PickerRecordsSnapshot, PickerRuntimeCoverage};
    let attempts = Arc::new(AtomicUsize::new(0));
    let record_loader: SessionsPickerRecordLoader = fixture_record_loader(move |_| {
        let available = attempts.fetch_add(1, Ordering::SeqCst) > 0;
        let mut record = picker_record(
            "saved",
            "Saved session",
            "/repo/project-a",
            "codex-router",
            "cli",
        );
        if available {
            record.runtime_status = PickerRuntimeStatus::Idle;
        }
        Ok(PickerRecordsSnapshot {
            records: vec![record],
            runtime_coverage: if available {
                PickerRuntimeCoverage::Available
            } else {
                PickerRuntimeCoverage::Unavailable
            },
        })
    });
    let (send_event, receive_event) = tokio::sync::mpsc::unbounded_channel();
    let events = futures_util::stream::unfold(receive_event, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let mut picker = element! {
        SessionsPickerComponent(request: picker_request(), record_loader: Some(record_loader), width: 100usize)
    };
    let frames = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(events));
    tokio::pin!(frames);
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut observed_unavailable = false;
        while let Some(canvas) = frames.next().await {
            let text = canvas.to_string();
            if !observed_unavailable
                && text.contains("Live status unavailable")
                && text.contains("Saved session")
            {
                assert!(!text.contains("? Unknown"), "{text}");
                observed_unavailable = true;
                send_event
                    .send(ctrl_key('r'))
                    .expect("picker event stream available");
            } else if observed_unavailable
                && text.contains("Saved session")
                && text.contains('○')
                && !text.contains("Live status unavailable")
            {
                assert!(!text.contains("○ Idle"), "{text}");
                return;
            }
        }
        panic!("picker closed before live-status recovery");
    })
    .await
    .expect("unavailability and recovery must both reach the rendered picker");
}

#[test]
fn selected_conversation_preview_requests_background_load_without_reading_jsonl() {
    let mut record = picker_request().records.remove(0);
    record.conversation = SessionConversationPreview::unavailable("history loading");
    let source = SessionConversationSource::new(
        "/tmp/codex-router-history.jsonl",
        "/tmp/codex-router".into(),
    );
    record.conversation_source = Some(source.clone());
    let cache = ConversationPreviewCache::default();

    let preview = cache.select_record(&record);

    assert_eq!(
        preview,
        SelectedConversationPreview {
            preview: SessionConversationPreview::unavailable("history loading"),
            load_request: Some(ConversationPreviewLoadRequest {
                key: ConversationPreviewKey {
                    identity: record.identity,
                    source,
                    generation: 0,
                },
            }),
        }
    );
    assert!(cache.is_empty(), "render decision should not mutate cache");
}

#[test]
fn conversation_preview_does_not_reuse_equal_ids_from_another_router() {
    // Arrange: two observed hosted sessions have equal native IDs but different sources.
    let mut first = picker_request().records.remove(0);
    let mut second = first.clone();
    first.identity = crate::sessions::SessionPickerIdentity::HostedCodex(
        serde_json::from_value(serde_json::json!({
            "endpoint": {"serviceId": "00000000-0000-4000-8000-000000000001", "endpointId": "codex-local"},
            "sessionId": first.session_id
        })).expect("first source identity")
    );
    second.identity = crate::sessions::SessionPickerIdentity::HostedCodex(
        serde_json::from_value(serde_json::json!({
            "endpoint": {"serviceId": "00000000-0000-4000-8000-000000000002", "endpointId": "codex-local"},
            "sessionId": second.session_id
        })).expect("second source identity")
    );
    first.conversation_source = Some(SessionConversationSource::new(
        "/tmp/source-one/sessions/history.jsonl",
        "/tmp/source-one".into(),
    ));
    second.conversation_source = Some(SessionConversationSource::new(
        "/tmp/source-two/sessions/history.jsonl",
        "/tmp/source-two".into(),
    ));
    let private_preview = SessionConversationPreview {
        snippets: vec!["ONLY_SOURCE_ONE".to_owned()],
        unavailable_reason: None,
    };
    let mut cache = ConversationPreviewCache::default();
    let load = cache
        .select_record(&first)
        .load_request
        .expect("first load");
    cache.start_loading(&load);
    cache.complete_load(&load, private_preview.clone());

    // Act: focus the same native ID from the other Router.
    let selected = cache.select_record(&second);

    // Assert: the first Router's text is never shown under the second source.
    assert_ne!(selected.preview, private_preview);
    assert!(selected.load_request.is_some());
}

#[test]
fn conversation_preview_rejects_a_late_load_after_view_invalidation() {
    let mut record = picker_request().records.remove(0);
    record.conversation_source = Some(SessionConversationSource::new(
        "/tmp/source/sessions/history.jsonl",
        "/tmp/source".into(),
    ));
    let mut cache = ConversationPreviewCache::default();
    let old_load = cache
        .select_record(&record)
        .load_request
        .expect("first load");
    cache.start_loading(&old_load);

    cache.invalidate();
    let current_load = cache
        .select_record(&record)
        .load_request
        .expect("current load");
    cache.start_loading(&current_load);
    cache.complete_load(
        &old_load,
        SessionConversationPreview {
            snippets: vec!["STALE_VIEW".to_owned()],
            unavailable_reason: None,
        },
    );

    assert_eq!(cache.select_record(&record).preview, record.conversation);
    let current_preview = SessionConversationPreview {
        snippets: vec!["CURRENT_VIEW".to_owned()],
        unavailable_reason: None,
    };
    cache.complete_load(&current_load, current_preview.clone());
    assert_eq!(cache.select_record(&record).preview, current_preview);
}

#[test]
fn conversation_preview_rejects_equal_local_ids_from_a_different_home() {
    let mut first = picker_request().records.remove(0);
    first.conversation_source = Some(SessionConversationSource::new(
        "/tmp/first-home/sessions/history.jsonl",
        "/tmp/first-home".into(),
    ));
    let mut second = first.clone();
    second.conversation_source = Some(SessionConversationSource::new(
        "/tmp/second-home/sessions/history.jsonl",
        "/tmp/second-home".into(),
    ));
    let mut cache = ConversationPreviewCache::default();
    let first_load = cache
        .select_record(&first)
        .load_request
        .expect("first home load");
    cache.start_loading(&first_load);
    let second_load = cache
        .select_record(&second)
        .load_request
        .expect("second home load");
    cache.start_loading(&second_load);
    cache.complete_load(
        &first_load,
        SessionConversationPreview {
            snippets: vec!["FIRST_HOME".to_owned()],
            unavailable_reason: None,
        },
    );

    assert_eq!(cache.select_record(&second).preview, second.conversation);
    assert!(
        cache.select_record(&second).load_request.is_none(),
        "second home stays loading"
    );
    assert!(
        cache.select_record(&first).load_request.is_some(),
        "homes never share a load"
    );
}

#[tokio::test]
async fn picker_preview_loads_real_history_without_leaking_between_equal_hosted_ids() {
    let home = tempfile::tempdir().unwrap();
    let mut request = picker_request();
    let template = request.records.remove(0);
    request.root = SessionsRoot::Any;
    request.records.clear();
    for (index, title, canary, service) in [
        (
            0,
            "First source",
            "ONLY_FIRST_HISTORY",
            "00000000-0000-4000-8000-000000000001",
        ),
        (
            1,
            "Second source",
            "ONLY_SECOND_HISTORY",
            "00000000-0000-4000-8000-000000000002",
        ),
    ] {
        let source_home = home.path().join(format!("home-{index}"));
        std::fs::create_dir_all(source_home.join("sessions")).unwrap();
        let history_path = source_home.join("sessions/history.jsonl");
        std::fs::write(
            &history_path,
            format!(
                "{}\n",
                serde_json::json!({
                    "type":"response_item", "payload":{"type":"message", "role":"user",
                    "content":[{"type":"input_text","text":canary}]}
                })
            ),
        )
        .unwrap();
        let mut record = template.clone();
        record.identity = crate::sessions::SessionPickerIdentity::HostedCodex(
            serde_json::from_value(serde_json::json!({
                "endpoint":{"serviceId":service,"endpointId":"codex-local"},
                "sessionId":record.session_id,
            }))
            .unwrap(),
        );
        record.title = title.to_owned();
        record.full_title = title.to_owned();
        record.recency_at_ms = Some(2000 - index);
        record.preview = None;
        record.conversation = SessionConversationPreview::unavailable("history loading");
        record.conversation_source = Some(SessionConversationSource::new(
            history_path.display().to_string(),
            source_home,
        ));
        request.records.push(record);
    }
    let (send_event, receive_event) = tokio::sync::mpsc::unbounded_channel();
    let events = futures_util::stream::unfold(receive_event, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let mut picker = element! {
        SessionsPickerComponent(request, width: 160usize, height: 40usize)
    };
    let frames = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(events));
    tokio::pin!(frames);
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut first_loaded = false;
        while let Some(canvas) = frames.next().await {
            let text = canvas.to_string();
            if !first_loaded && text.contains("ONLY_FIRST_HISTORY") {
                first_loaded = true;
                send_event
                    .send(TerminalEvent::Key(KeyEvent::new(
                        KeyEventKind::Press,
                        KeyCode::Down,
                    )))
                    .unwrap();
            } else if first_loaded && text.contains("❯ Second source") {
                assert!(
                    !text.contains("ONLY_FIRST_HISTORY"),
                    "foreign history leaked: {text}"
                );
                if text.contains("ONLY_SECOND_HISTORY") {
                    return;
                }
            }
        }
        panic!("both actual history loads must reach their own rendered source");
    })
    .await
    .expect("preview integration completes within bounded event wait");
}
