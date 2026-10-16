//! Real dispatcher/HTTP continuation through the production reload worker.
//! Controlled peers supply timing and invalid frames, not a deployed Host proof.
use super::*;
use crate::sessions::configured_inventory_fault_tests::{ReadPeer, ReplyScenario};

#[tokio::test]
async fn healthy_first_page_publishes_while_an_unrelated_service_qualification_is_held() {
    let healthy = ReadPeer::start(ReplyScenario::RowBudget(1)).await;
    let held = ReadPeer::start_bound(
        ReplyScenario::HoldInventory,
        "00000000-0000-4000-8000-000000000003",
    )
    .await;
    let (sender, receiver) = tokio::sync::watch::channel(SessionRecordsReloadRequest {
        generation: 0,
        query: query(""),
        sources: vec![],
    });
    let (published_sender, mut published) = tokio::sync::mpsc::unbounded_channel();
    let loader = healthy.loader();
    let worker = tokio::spawn(async move {
        run_session_record_reload_worker(receiver, loader, move |_, update| {
            published_sender.send(update).unwrap();
        })
        .await;
    });
    sender.send_replace(SessionRecordsReloadRequest {
        generation: 1,
        query: query(""),
        sources: vec![
            healthy.request().source_context,
            held.request().source_context,
        ],
    });
    held.wait_for_held_page().await;
    let observed = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let update = published.recv().await.unwrap();
            if let SourceRecordsUpdate::Ready(snapshot) = update.records_update
                && snapshot.records.len() == 1
            {
                assert_eq!(snapshot.records[0].session_id, "source-0");
                assert!(
                    update
                        .source_progress
                        .iter()
                        .any(|progress| matches!(progress.read_state, SourceReadState::Loading))
                );
                break;
            }
        }
    })
    .await;
    drop(sender);
    worker.await.unwrap();
    healthy.finish().await;
    held.finish().await;
    assert!(
        observed.is_ok(),
        "a slow unrelated service cannot block a qualified first page"
    );
}

struct ActivePageGuard {
    active: Arc<AtomicUsize>,
    per_source: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    name: String,
}

impl Drop for ActivePageGuard {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
        assert!(self.per_source.lock().unwrap().remove(&self.name));
    }
}

enum PublishedRecordsWitness {
    Pending,
    Ready { record_count: usize },
    Rejected { reason: SourceInventoryRejection },
}

impl std::fmt::Debug for PublishedRecordsWitness {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => formatter.write_str("Pending"),
            Self::Ready { record_count } => formatter
                .debug_struct("Ready")
                .field("record_count", record_count)
                .finish(),
            Self::Rejected { reason } => formatter
                .debug_struct("Rejected")
                .field("reason", reason)
                .finish(),
        }
    }
}

#[tokio::test]
async fn five_real_sources_start_first_pages_before_repeating_and_hold_at_most_four_reads() {
    let mut peers = Vec::new();
    let mut selected = Vec::new();
    for index in 1..=5 {
        let peer = ReadPeer::start_bound(
            ReplyScenario::HoldSecondPage,
            &format!("00000000-0000-4000-8000-{index:012}"),
        )
        .await;
        let mut source = peer.request().source_context;
        let PickerSourceContext::ConfiguredHosted(profile) = &mut source else {
            panic!("configured peer")
        };
        profile.name = format!("Machine{index}").try_into().unwrap();
        selected.push(source);
        peers.push(peer);
    }
    let observation_started = std::time::Instant::now();
    let started_phases = Arc::new(std::sync::Mutex::new(Vec::new()));
    let last_publication = Arc::new(std::sync::Mutex::new(None));
    let actual = peers[0].loader();
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let per_source = Arc::new(std::sync::Mutex::new(std::collections::BTreeSet::new()));
    let invocation_counts = Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::<
        String,
        usize,
    >::new()));
    let (started_sender, mut started) = tokio::sync::mpsc::unbounded_channel();
    let loader: SessionsPickerRecordLoader = Arc::new({
        let invocation_counts = invocation_counts.clone();
        let started_phases = started_phases.clone();
        let active = active.clone();
        let maximum = maximum.clone();
        let per_source = per_source.clone();
        move |request| {
            let invocation_counts = invocation_counts.clone();
            let started_phases = started_phases.clone();
            let actual = actual.clone();
            let active = active.clone();
            let maximum = maximum.clone();
            let per_source = per_source.clone();
            let started_sender = started_sender.clone();
            Box::pin(async move {
                let PickerSourceContext::ConfiguredHosted(profile) = &request.source_context else {
                    panic!("configured source")
                };
                let name = profile.name.as_str().to_owned();
                assert!(
                    per_source.lock().unwrap().insert(name.clone()),
                    "only one outstanding page per source"
                );
                let active_count = active.fetch_add(1, Ordering::SeqCst) + 1;
                maximum.fetch_max(active_count, Ordering::SeqCst);
                let _guard = ActivePageGuard {
                    active,
                    per_source,
                    name: name.clone(),
                };
                let phase = {
                    let mut counts = invocation_counts.lock().unwrap();
                    let count = counts.entry(name.clone()).or_default();
                    let phase = *count;
                    *count += 1;
                    phase
                };
                started_phases.lock().unwrap().push((
                    observation_started.elapsed(),
                    name.clone(),
                    phase,
                    active_count,
                    maximum.load(Ordering::SeqCst),
                ));
                started_sender.send((name, phase)).unwrap();
                actual(request).await
            })
        }
    });
    let (sender, receiver) = tokio::sync::watch::channel(SessionRecordsReloadRequest {
        generation: 0,
        query: query(""),
        sources: vec![],
    });
    let (published_sender, mut published) = tokio::sync::mpsc::unbounded_channel();
    let publication_witness = last_publication.clone();
    let worker = tokio::spawn(async move {
        run_session_record_reload_worker(receiver, loader, move |_, update| {
            let records = match &update.records_update {
                SourceRecordsUpdate::Pending => PublishedRecordsWitness::Pending,
                SourceRecordsUpdate::Ready(snapshot) => PublishedRecordsWitness::Ready {
                    record_count: snapshot.records.len(),
                },
                SourceRecordsUpdate::Rejected(reason) => {
                    PublishedRecordsWitness::Rejected { reason: *reason }
                }
            };
            *publication_witness.lock().unwrap() = Some((
                observation_started.elapsed(),
                update
                    .source_progress
                    .iter()
                    .map(|progress| {
                        (
                            progress.source_name().to_owned(),
                            progress.read_state.clone(),
                        )
                    })
                    .collect::<Vec<_>>(),
                records,
            ));
            published_sender.send(update).unwrap();
        })
        .await;
    });
    sender.send_replace(SessionRecordsReloadRequest {
        generation: 1,
        query: query(""),
        sources: selected,
    });
    let mut first_loop_completed = false;
    let observed = tokio::time::timeout(Duration::from_secs(2), async {
        let mut first_names = std::collections::BTreeSet::new();
        loop {
            let (name, phase) = started.recv().await.unwrap();
            if phase == 0 {
                continue;
            }
            if phase == 2 {
                assert_eq!(
                    first_names.len(),
                    5,
                    "all initial pages must start before a repeated source page"
                );
                first_loop_completed = true;
                break;
            }
            assert!(first_names.insert(name));
        }
        loop {
            let update = published.recv().await.unwrap();
            if let SourceRecordsUpdate::Ready(snapshot) = update.records_update
                && snapshot.records.len() == 5
            {
                assert!(update.source_progress.iter().all(|progress| matches!(
                    progress.read_state,
                    SourceReadState::Suspended { record_count: 1 }
                        | SourceReadState::LoadingMore { record_count: 1 }
                )));
                assert_eq!(
                    snapshot
                        .records
                        .iter()
                        .map(|row| row.identity.clone())
                        .collect::<std::collections::BTreeSet<_>>()
                        .len(),
                    5
                );
                break;
            }
        }
    })
    .await;
    assert!(
        observed.is_ok(),
        "fairness observation failed: result={observed:?}; elapsed={:?}; \
         first_loop_completed={first_loop_completed}; starts(elapsed/source/phase/active/max)={:?}; \
         last_publication(elapsed/states/records)={:?}; active={}; maximum={}",
        observation_started.elapsed(),
        started_phases.lock().unwrap(),
        last_publication.lock().unwrap(),
        active.load(Ordering::SeqCst),
        maximum.load(Ordering::SeqCst),
    );
    assert_eq!(maximum.load(Ordering::SeqCst), 4);
    drop(sender);
    worker.await.unwrap();
    assert_eq!(
        active.load(Ordering::SeqCst),
        0,
        "closing view cancels every owned continuation"
    );
    for peer in peers {
        peer.finish().await;
    }
}

#[tokio::test]
async fn rejected_later_page_withdraws_its_rows_while_another_source_is_pending() {
    let peer = ReadPeer::start(ReplyScenario::HoldSecondWrongEndpoint).await;
    let source = peer.request().source_context;
    let (sender, receiver) = tokio::sync::watch::channel(SessionRecordsReloadRequest {
        generation: 0,
        query: query(""),
        sources: vec![],
    });
    let release_default = Arc::new(tokio::sync::Semaphore::new(0));
    let actual = peer.loader();
    let loader: SessionsPickerRecordLoader = Arc::new({
        let release_default = release_default.clone();
        move |request| {
            let actual = actual.clone();
            let release_default = release_default.clone();
            Box::pin(async move {
                if request.source_context == PickerSourceContext::DefaultHosted {
                    release_default.acquire().await.unwrap().forget();
                    SourceInventoryResult::Ready {
                        request,
                        binding: None,
                        progress: super::super::super::InventoryPageProgress::Complete,
                        snapshot: observed_records(vec![picker_record(
                            "healthy-default",
                            "Healthy default",
                            "/default",
                            "",
                            "cli",
                        )]),
                    }
                } else {
                    actual(request).await
                }
            })
        }
    });
    let (published_sender, mut published) = tokio::sync::mpsc::unbounded_channel();
    let worker = tokio::spawn(async move {
        run_session_record_reload_worker(receiver, loader, move |_, update| {
            published_sender.send(update).unwrap();
        })
        .await;
    });
    sender.send_replace(SessionRecordsReloadRequest {
        generation: 1,
        query: query(""),
        sources: vec![PickerSourceContext::DefaultHosted, source.clone()],
    });
    let mut displayed = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let update = published.recv().await.unwrap();
            if let SourceRecordsUpdate::Ready(snapshot) = update.records_update {
                displayed = snapshot.records;
                if displayed.len() == 1 {
                    break;
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(displayed[0].session_id, "source-0");
    peer.wait_for_held_page().await;
    peer.release_held_page().await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let update = published.recv().await.unwrap();
            let rejected = update.source_progress.iter().any(|progress| {
                progress.source_context == source
                    && matches!(
                        progress.read_state,
                        SourceReadState::Rejected {
                            reason: SourceInventoryRejection::InvalidInventory
                        }
                    )
            });
            if let SourceRecordsUpdate::Ready(snapshot) = update.records_update {
                displayed = snapshot.records;
            }
            if rejected {
                assert!(
                    displayed.is_empty(),
                    "invalid earlier rows must be withdrawn before the healthy source finishes"
                );
                break;
            }
        }
    })
    .await
    .unwrap();
    release_default.add_permits(1);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let SourceRecordsUpdate::Ready(snapshot) =
                published.recv().await.unwrap().records_update
            {
                assert_eq!(snapshot.records.len(), 1);
                assert_eq!(snapshot.records[0].session_id, "healthy-default");
                break;
            }
        }
    })
    .await
    .unwrap();
    drop(sender);
    worker.await.unwrap();
    peer.finish().await;
}
