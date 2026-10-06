//! Bounded scheduling and namespace proof; simulated source replies are not remote qualification.
use super::super::source_inventory_request::{SourceInventoryRejection, SourceInventoryResult};
use super::super::test_support::{observed_records, picker_record};
use super::*;
use crate::sessions::router_connection_registry::{RouterConnectionRegistry, RouterRegistryError};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

fn sources(count: usize) -> Result<Vec<PickerSourceContext>, RouterRegistryError> {
    let profiles = (1..=count).map(|index| serde_json::json!({
        "name":format!("Machine{index}"),
        "connection":{"kind":"remote","serviceId":format!("00000000-0000-4000-8000-{index:012}"),
            "mcpUrl":"http://127.0.0.1:18788/mcp"},
        "defaultRemoteCwd":"/owned/project"
    })).collect::<Vec<_>>();
    let text = serde_json::json!({"version":1,"routers":profiles}).to_string();
    Ok(RouterConnectionRegistry::parse(&text)?
        .routers
        .into_iter()
        .map(PickerSourceContext::ConfiguredHosted)
        .collect())
}

fn query(search: &str) -> SessionsPickerDataQuery {
    SessionsPickerDataQuery {
        root: super::super::SessionsPickerRoot::Any,
        provider: crate::sessions::SessionsProvider::Any,
        source: crate::sessions::SessionsSource::All,
        sort: crate::sessions::SessionsSort::Updated,
        search: search.to_owned(),
        include_empty_sessions: false,
    }
}

#[tokio::test]
async fn rejected_source_progress_keeps_rows_from_the_successful_source() {
    let configured = sources(1).unwrap().remove(0);
    let requested_sources = vec![PickerSourceContext::DefaultHosted, configured.clone()];
    let (sender, receiver) = tokio::sync::watch::channel(SessionRecordsReloadRequest {
        generation: 0,
        query: query("initial"),
        sources: vec![],
    });
    let permit = Arc::new(tokio::sync::Semaphore::new(0));
    let loader: SessionsPickerRecordLoader = Arc::new({
        let permit = Arc::clone(&permit);
        move |request| {
            let permit = Arc::clone(&permit);
            Box::pin(async move {
                if matches!(
                    request.source_context,
                    PickerSourceContext::ConfiguredHosted(_)
                ) {
                    permit.acquire().await.unwrap().forget();
                    SourceInventoryResult::Rejected {
                        request,
                        reason: SourceInventoryRejection::SourceUnavailable,
                    }
                } else {
                    SourceInventoryResult::Ready {
                        request,
                        snapshot: observed_records(vec![picker_record(
                            "usable-default",
                            "Usable source",
                            "/owned/project",
                            "codex-router",
                            "cli",
                        )]),
                    }
                }
            })
        }
    });
    let (published_sender, mut published) = tokio::sync::mpsc::unbounded_channel();
    let worker = tokio::spawn(async move {
        run_session_record_reload_worker(receiver, loader, move |request, update| {
            let _ = published_sender.send((request, update));
        })
        .await;
    });
    sender.send_replace(SessionRecordsReloadRequest {
        generation: 9,
        query: query("all"),
        sources: requested_sources.clone(),
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut retained_rows = Vec::new();
        loop {
            let (request, update) = published.recv().await.unwrap();
            assert_eq!(request.generation, 9);
            assert_eq!(request.sources, requested_sources);
            let failed = update.source_progress.iter().any(|progress| {
                progress.source_context == configured
                    && matches!(
                        progress.read_state,
                        SourceReadState::Rejected {
                            reason: SourceInventoryRejection::SourceUnavailable
                        }
                    )
            });
            if let SourceRecordsUpdate::Ready(snapshot) = update.records_update {
                retained_rows = snapshot.records;
                assert!(update.source_progress.iter().any(|progress| matches!(
                    progress.read_state,
                    SourceReadState::Ready { record_count: 1 }
                )));
                permit.add_permits(1);
            }
            if failed {
                assert_eq!(retained_rows.len(), 1);
                assert_eq!(retained_rows[0].session_id, "usable-default");
                break;
            }
        }
    })
    .await
    .unwrap();
    drop(sender);
    tokio::time::timeout(Duration::from_secs(2), worker)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn all_source_reads_do_not_overwrite_each_other_and_run_at_most_four_at_once() {
    let requested_sources = sources(6).unwrap();
    let (sender, receiver) = tokio::sync::watch::channel(SessionRecordsReloadRequest {
        generation: 0,
        query: query("initial"),
        sources: vec![],
    });
    let permits = Arc::new(tokio::sync::Semaphore::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let (started_sender, mut started) = tokio::sync::mpsc::unbounded_channel();
    let loader: SessionsPickerRecordLoader = Arc::new({
        let permits = Arc::clone(&permits);
        let active = Arc::clone(&active);
        let maximum = Arc::clone(&maximum);
        move |request| {
            let permits = Arc::clone(&permits);
            let active = Arc::clone(&active);
            let maximum = Arc::clone(&maximum);
            let started_sender = started_sender.clone();
            Box::pin(async move {
                maximum.fetch_max(active.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                let _ = started_sender.send(request.source_context.clone());
                permits.acquire().await.unwrap().forget();
                active.fetch_sub(1, Ordering::SeqCst);
                let PickerSourceContext::ConfiguredHosted(profile) = &request.source_context else {
                    panic!("configured fixture source");
                };
                let mut row = picker_record(
                    "same-native-id",
                    profile.name.as_str(),
                    "/owned/project",
                    "codex-router",
                    "cli",
                );
                row.provenance = crate::sessions::SessionRowProvenance::ObservedHosted;
                row.identity = SessionPickerIdentity::HostedCodex(serde_json::from_value(serde_json::json!({
                    "endpoint":{"serviceId":String::from(profile.service_id.clone()),"endpointId":"codex-local"},
                    "sessionId":"same-native-id",
                })).unwrap());
                SourceInventoryResult::Ready {
                    request,
                    snapshot: observed_records(vec![row]),
                }
            })
        }
    });
    let (accepted_sender, mut accepted) = tokio::sync::mpsc::unbounded_channel();
    let worker = tokio::spawn(async move {
        run_session_record_reload_worker(receiver, loader, move |_request, snapshot| {
            if let Some(snapshot) = snapshot.into_snapshot() {
                let _ = accepted_sender.send(snapshot);
            }
        })
        .await;
    });
    sender.send_replace(SessionRecordsReloadRequest {
        generation: 1,
        query: query("all"),
        sources: requested_sources.clone(),
    });
    let observed = tokio::time::timeout(Duration::from_secs(2), async {
        let mut observed = Vec::new();
        for _ in 0..4 {
            observed.push(started.recv().await.unwrap());
        }
        assert!(
            started.try_recv().is_err(),
            "fifth read waits for one of the four slots"
        );
        permits.add_permits(6);
        for _ in 0..2 {
            observed.push(started.recv().await.unwrap());
        }
        loop {
            let snapshot = accepted.recv().await.unwrap().unwrap();
            if snapshot.records.len() == 6 {
                assert!(
                    snapshot
                        .records
                        .iter()
                        .all(|record| record.source_context.is_some())
                );
                break;
            }
        }
        observed
    })
    .await
    .unwrap();
    assert_eq!(maximum.load(Ordering::SeqCst), 4);
    assert!(
        requested_sources.iter().all(|source| observed
            .iter()
            .filter(|item| *item == source)
            .count()
            == 1)
    );
    worker.abort();
}

#[tokio::test]
async fn stale_request_and_foreign_service_rows_are_rejected_before_publication() {
    for stale_request in [false, true] {
        let mut selected = sources(1).unwrap();
        let source = selected.remove(0);
        let (sender, receiver) = tokio::sync::watch::channel(SessionRecordsReloadRequest {
            generation: 0,
            query: query("initial"),
            sources: vec![],
        });
        let loader: SessionsPickerRecordLoader = Arc::new(move |mut request| {
            Box::pin(async move {
                let mut row = picker_record(
                    "same-native-id",
                    "Wrong source",
                    "/owned/project",
                    "codex-router",
                    "cli",
                );
                row.identity = SessionPickerIdentity::HostedCodex(serde_json::from_value(serde_json::json!({
                "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000099","endpointId":"codex-local"},
                "sessionId":"same-native-id",
            })).unwrap());
                if stale_request {
                    request.request_generation += 1;
                }
                SourceInventoryResult::Ready {
                    request,
                    snapshot: observed_records(vec![row]),
                }
            })
        });
        let (accepted_sender, mut accepted) = tokio::sync::mpsc::unbounded_channel();
        let worker = tokio::spawn(async move {
            run_session_record_reload_worker(receiver, loader, move |_request, result| {
                if let Some(result) = result.into_snapshot() {
                    let _ = accepted_sender.send(result);
                }
            })
            .await;
        });
        sender.send_replace(SessionRecordsReloadRequest {
            generation: 1,
            query: query("selected"),
            sources: vec![source],
        });
        let result = tokio::time::timeout(Duration::from_secs(2), accepted.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            result.unwrap_err(),
            if stale_request {
                SourceInventoryRejection::StaleSnapshot
            } else {
                SourceInventoryRejection::InvalidInventory
            }
        );
        worker.abort();
    }
}

#[tokio::test]
async fn default_attribution_is_not_remote_observation_even_when_service_and_id_match() {
    let mut contexts = sources(1).unwrap();
    let source = contexts.remove(0);
    let (sender, receiver) = tokio::sync::watch::channel(SessionRecordsReloadRequest {
        generation: 0,
        query: query("initial"),
        sources: vec![],
    });
    let loader: SessionsPickerRecordLoader = Arc::new(move |request| {
        Box::pin(async move {
            let PickerSourceContext::ConfiguredHosted(profile) = &request.source_context else {
                panic!("configured source");
            };
            let endpoint: collaboration_client::protocol::EndpointRef =
                serde_json::from_value(serde_json::json!({
                    "serviceId":String::from(profile.service_id.clone()),"endpointId":"codex-local",
                }))
                .unwrap();
            let row = picker_record(
                "same-native-id",
                "Attributed local row",
                "/owned/project",
                "codex-router",
                "cli",
            )
            .with_hosted_codex(&endpoint);
            assert_eq!(
                row.provenance,
                crate::sessions::SessionRowProvenance::DefaultAttributed
            );
            SourceInventoryResult::Ready {
                request,
                snapshot: observed_records(vec![row]),
            }
        })
    });
    let (accepted_sender, mut accepted) = tokio::sync::mpsc::unbounded_channel();
    let worker = tokio::spawn(async move {
        run_session_record_reload_worker(receiver, loader, move |_request, result| {
            if let Some(result) = result.into_snapshot() {
                let _ = accepted_sender.send(result);
            }
        })
        .await;
    });
    sender.send_replace(SessionRecordsReloadRequest {
        generation: 1,
        query: query("configured"),
        sources: vec![source],
    });
    let result = tokio::time::timeout(Duration::from_secs(2), accepted.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        result.unwrap_err(),
        SourceInventoryRejection::InvalidInventory
    );
    worker.abort();
}

struct PendingReadGuard(Arc<AtomicUsize>);
impl Drop for PendingReadGuard {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn superseded_view_cancels_owned_reads_and_publishes_only_the_new_generation() {
    let (sender, receiver) = tokio::sync::watch::channel(SessionRecordsReloadRequest {
        generation: 0,
        query: query("initial"),
        sources: vec![PickerSourceContext::DefaultHosted],
    });
    let canceled = Arc::new(AtomicUsize::new(0));
    let (started_sender, mut started) = tokio::sync::mpsc::unbounded_channel();
    let loader: SessionsPickerRecordLoader = Arc::new({
        let canceled = Arc::clone(&canceled);
        move |request| {
            let canceled = Arc::clone(&canceled);
            let started_sender = started_sender.clone();
            Box::pin(async move {
                if request.query.search == "old" {
                    let _owned_read = PendingReadGuard(canceled);
                    let _ = started_sender.send(());
                    futures_util::future::pending::<()>().await;
                }
                SourceInventoryResult::Ready {
                    request,
                    snapshot: observed_records(vec![]),
                }
            })
        }
    });
    let (accepted_sender, mut accepted) = tokio::sync::mpsc::unbounded_channel();
    let worker = tokio::spawn(async move {
        run_session_record_reload_worker(receiver, loader, move |request, result| {
            if let Some(result) = result.into_snapshot() {
                let _ = accepted_sender.send((request, result));
            }
        })
        .await;
    });
    sender.send_replace(SessionRecordsReloadRequest {
        generation: 1,
        query: query("old"),
        sources: vec![PickerSourceContext::DefaultHosted],
    });
    tokio::time::timeout(Duration::from_secs(2), started.recv())
        .await
        .unwrap()
        .unwrap();
    sender.send_replace(SessionRecordsReloadRequest {
        generation: 2,
        query: query("new"),
        sources: vec![PickerSourceContext::DefaultHosted],
    });
    let (published, result) = tokio::time::timeout(Duration::from_secs(2), accepted.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(published.generation, 2);
    assert_eq!(published.query.search, "new");
    assert!(result.is_ok());
    assert_eq!(canceled.load(Ordering::SeqCst), 1);
    assert!(accepted.try_recv().is_err());
    worker.abort();
}
