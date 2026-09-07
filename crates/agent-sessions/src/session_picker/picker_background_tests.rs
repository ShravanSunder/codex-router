use super::*;

#[tokio::test]
async fn session_record_reload_worker_runs_single_flight_and_keeps_only_latest_pending_query() {
    let initial_request = SessionRecordsReloadRequest {
        generation: 0,
        query: reload_query("initial"),
    };
    let (sender, receiver) = tokio::sync::watch::channel(initial_request);
    let (release_first_sender, release_first_receiver) = mpsc::channel::<()>();
    let release_first_receiver = Arc::new(Mutex::new(release_first_receiver));
    let (started_sender, mut started_receiver) = tokio::sync::mpsc::unbounded_channel();
    let active_loads = Arc::new(AtomicUsize::new(0));
    let maximum_active_loads = Arc::new(AtomicUsize::new(0));
    let loader: SessionsPickerRecordLoader = Arc::new({
        let active_loads = Arc::clone(&active_loads);
        let maximum_active_loads = Arc::clone(&maximum_active_loads);
        let release_first_receiver = Arc::clone(&release_first_receiver);
        move |query| {
            let active = active_loads.fetch_add(1, Ordering::SeqCst) + 1;
            maximum_active_loads.fetch_max(active, Ordering::SeqCst);
            let search = query.search;
            let _ = started_sender.send(search.clone());
            if search == "a" {
                release_first_receiver
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap_or_else(|error| panic!("first load release should arrive: {error}"));
            }
            active_loads.fetch_sub(1, Ordering::SeqCst);
            Ok(vec![picker_record(
                &format!("thread-{search}"),
                &format!("result {search}"),
                "/repo/project-a",
                "codex-router",
                "cli",
            )])
        }
    });
    let current_generation = Arc::new(AtomicU64::new(0));
    let (accepted_sender, mut accepted_receiver) = tokio::sync::mpsc::unbounded_channel::<String>();
    let worker = tokio::spawn({
        let current_generation = Arc::clone(&current_generation);
        async move {
            run_session_record_reload_worker(receiver, loader, move |request, _records| {
                if current_generation.load(Ordering::SeqCst) == request.generation {
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
    });
    assert_eq!(started_receiver.recv().await.as_deref(), Some("a"));

    current_generation.store(2, Ordering::SeqCst);
    sender.send_replace(SessionRecordsReloadRequest {
        generation: 2,
        query: reload_query("b"),
    });
    current_generation.store(3, Ordering::SeqCst);
    sender.send_replace(SessionRecordsReloadRequest {
        generation: 3,
        query: reload_query("c"),
    });
    release_first_sender
        .send(())
        .unwrap_or_else(|error| panic!("first load should release: {error}"));

    assert_eq!(started_receiver.recv().await.as_deref(), Some("c"));
    assert_eq!(accepted_receiver.recv().await.as_deref(), Some("c"));
    assert_eq!(maximum_active_loads.load(Ordering::SeqCst), 1);
    assert!(
        started_receiver.try_recv().is_err(),
        "obsolete B must not run"
    );

    worker.abort();
}

#[tokio::test]
async fn periodic_same_generation_refresh_does_not_starve_a_slow_result() {
    let (sender, receiver) = tokio::sync::watch::channel(SessionRecordsReloadRequest {
        generation: 0,
        query: reload_query("initial"),
    });
    let (release_slow_sender, release_slow_receiver) = mpsc::channel::<()>();
    let release_slow_receiver = Arc::new(Mutex::new(release_slow_receiver));
    let (started_sender, mut started_receiver) = tokio::sync::mpsc::unbounded_channel();
    let load_count = Arc::new(AtomicUsize::new(0));
    let active_loads = Arc::new(AtomicUsize::new(0));
    let maximum_active_loads = Arc::new(AtomicUsize::new(0));
    let loader: SessionsPickerRecordLoader = Arc::new({
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
            Ok(Vec::new())
        }
    });
    let (accepted_sender, mut accepted_receiver) = tokio::sync::mpsc::unbounded_channel();
    let worker = tokio::spawn(async move {
        run_session_record_reload_worker(receiver, loader, move |request, records| {
            if request.generation == 1 && records.is_ok() {
                let _ = accepted_sender.send(request.query.search);
            }
        })
        .await;
    });
    let refresh_request = SessionRecordsReloadRequest {
        generation: 1,
        query: reload_query("same-query"),
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
    let record_loader: SessionsPickerRecordLoader = Arc::new(move |query| {
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
        Ok(vec![record])
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
        actual.contains("◆ Blocked") && actual.contains("Reloaded SQL result"),
        "the Blocked view should show the blocked loaded result: {actual:?}"
    );
}

#[test]
fn selected_conversation_preview_requests_background_load_without_reading_jsonl() {
    let mut record = picker_request().records.remove(0);
    record.conversation = SessionConversationPreview::unavailable("history loading");
    let source = SessionConversationSource::for_test(
        "/tmp/codex-router-history.jsonl",
        "/tmp/codex-router".into(),
    );
    record.conversation_source = Some(source.clone());
    let cache = BTreeMap::new();

    let preview = selected_conversation_preview_for_record(&record, &cache);

    assert_eq!(
        preview,
        SelectedConversationPreview {
            preview: SessionConversationPreview::unavailable("history loading"),
            load_request: Some(ConversationPreviewLoadRequest {
                session_id: "thread-a".to_owned(),
                source,
            }),
        }
    );
    assert!(cache.is_empty(), "render decision should not mutate cache");
}
