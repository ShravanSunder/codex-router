//! Latest-view scheduling: bounded source reads, source validation and guarded publication.
use super::source_inventory_request::{PickerSourceContext, SourceInventoryRejection};
use super::source_reload_progress::{
    SourceReadProgress, SourceReadState, SourceRecordsUpdate, SourceReloadUpdate,
};
use super::{SessionsPickerDataQuery, SessionsPickerRecordLoader, SourceInventoryRequest};
use crate::{
    picker_runtime_status::{PickerRecordsSnapshot, PickerRuntimeCoverage},
    sessions::{SessionPickerIdentity, SessionRowProvenance},
};
use futures_util::{StreamExt, stream::FuturesUnordered};
use std::collections::{BTreeMap, VecDeque};

const MAX_SOURCE_READS: usize = 4;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SessionRecordsReloadRequest {
    pub(super) generation: u64,
    pub(super) query: SessionsPickerDataQuery,
    pub(super) sources: Vec<PickerSourceContext>,
}

pub(super) async fn run_session_record_reload_worker(
    mut receiver: tokio::sync::watch::Receiver<SessionRecordsReloadRequest>,
    loader: SessionsPickerRecordLoader,
    mut accept_records: impl FnMut(SessionRecordsReloadRequest, SourceReloadUpdate),
) {
    let mut pending_view = None;
    loop {
        let request = match pending_view.take() {
            Some(request) => request,
            None => {
                if receiver.changed().await.is_err() {
                    return;
                }
                receiver.borrow_and_update().clone()
            }
        };
        let mut pending_sources = VecDeque::from(request.sources.clone());
        let mut source_progress = request
            .sources
            .iter()
            .cloned()
            .map(|source_context| SourceReadProgress {
                source_context,
                read_state: SourceReadState::Loading,
            })
            .collect::<Vec<_>>();
        accept_records(
            request.clone(),
            SourceReloadUpdate {
                source_progress: source_progress.clone(),
                records_update: SourceRecordsUpdate::Pending,
            },
        );
        let mut reads = FuturesUnordered::new();
        let (result_sender, mut results) = tokio::sync::mpsc::channel(MAX_SOURCE_READS);
        let mut unsettled_reads = 0usize;
        let mut snapshots = Vec::new();
        let mut last_rejection = SourceInventoryRejection::SourceUnavailable;
        let mut same_view_refresh = false;
        let mut superseded = false;
        loop {
            while unsettled_reads < MAX_SOURCE_READS {
                let Some(source_context) = pending_sources.pop_front() else {
                    break;
                };
                let source_request = SourceInventoryRequest {
                    source_context,
                    query: request.query.clone(),
                    request_generation: request.generation,
                };
                let source_loader = loader.clone();
                reads.push(async move {
                    let result = source_loader(source_request.clone()).await;
                    (source_request, result)
                });
                unsettled_reads += 1;
            }
            if unsettled_reads == 0 {
                if snapshots.is_empty() {
                    accept_records(
                        request.clone(),
                        SourceReloadUpdate {
                            source_progress: source_progress.clone(),
                            records_update: SourceRecordsUpdate::Rejected(last_rejection),
                        },
                    );
                }
                break;
            }
            tokio::select! {
                changed = receiver.changed() => {
                    if changed.is_err() { return; }
                    let next = receiver.borrow_and_update().clone();
                    if next == request {
                        // Repeated refresh cannot cancel/starve its own slow read.
                        same_view_refresh = true;
                    } else {
                        pending_view = Some(next);
                        superseded = true;
                        break;
                    }
                }
                completed = reads.next(), if !reads.is_empty() => {
                    if let Some(result) = completed {
                        // Completed-but-unpublished reads still count toward the four-slot bound.
                        if result_sender.try_send(result).is_err() {
                            accept_records(request.clone(), SourceReloadUpdate { source_progress: source_progress.clone(), records_update: SourceRecordsUpdate::Rejected(last_rejection) });
                            return;
                        }
                    }
                }
                completed = results.recv() => {
                    let Some((expected, result)) = completed else { return; };
                    unsettled_reads = unsettled_reads.saturating_sub(1);
                    let validated = if result.request() != &expected {
                        Err(SourceInventoryRejection::StaleSnapshot)
                    } else {
                        result.into_snapshot().and_then(|snapshot| {
                            if source_snapshot_matches(&expected.source_context, &snapshot) { Ok(snapshot) }
                            else { Err(SourceInventoryRejection::InvalidInventory) }
                        })
                    };
                    let progress = source_progress.iter_mut().find(|progress| progress.source_context == expected.source_context);
                    match validated {
                        Ok(snapshot) => {
                            if let Some(progress) = progress { progress.read_state = SourceReadState::Ready { record_count: snapshot.records.len() }; }
                            snapshots.push((expected.source_context, snapshot));
                            accept_records(request.clone(), SourceReloadUpdate { source_progress: source_progress.clone(), records_update: SourceRecordsUpdate::Ready(merge_source_snapshots(&request.sources, &snapshots)) });
                        }
                        Err(reason) => {
                            last_rejection = reason;
                            if let Some(progress) = progress { progress.read_state = SourceReadState::Rejected { reason }; }
                            accept_records(request.clone(), SourceReloadUpdate { source_progress: source_progress.clone(), records_update: SourceRecordsUpdate::Pending });
                        }
                    }
                }
            }
        }
        // Dropping the owned FuturesUnordered cancels superseded read futures; no spawned tasks escape.
        drop(reads);
        if !superseded && same_view_refresh {
            pending_view = Some(request);
        }
    }
}

fn source_snapshot_matches(source: &PickerSourceContext, snapshot: &PickerRecordsSnapshot) -> bool {
    let PickerSourceContext::ConfiguredHosted(profile) = source else {
        return true;
    };
    snapshot
        .records
        .iter()
        .all(|record| match &record.identity {
            SessionPickerIdentity::HostedCodex(target) => {
                target.endpoint.service_id == profile.service_id
                    && record.provenance == SessionRowProvenance::ObservedHosted
                    && record.conversation_source.is_none()
            }
            SessionPickerIdentity::HostedProvider(target) => {
                target.endpoint.service_id == profile.service_id
                    && record.provenance == SessionRowProvenance::ObservedProvider
                    && record.conversation_source.is_none()
            }
            SessionPickerIdentity::LocalCodex(_) => false,
        })
}

fn merge_source_snapshots(
    source_order: &[PickerSourceContext],
    snapshots: &[(PickerSourceContext, PickerRecordsSnapshot)],
) -> PickerRecordsSnapshot {
    let mut records = BTreeMap::new();
    let mut coverage = PickerRuntimeCoverage::Unavailable;
    for source in source_order {
        let Some((_, snapshot)) = snapshots.iter().find(|(context, _)| context == source) else {
            continue;
        };
        for record in &snapshot.records {
            records.entry(record.identity.clone()).or_insert_with(|| {
                let mut record = record.clone();
                record.source_context = Some(source.clone());
                record
            });
        }
        if snapshot.runtime_coverage == PickerRuntimeCoverage::Available {
            coverage = PickerRuntimeCoverage::Available;
        } else if coverage != PickerRuntimeCoverage::Available {
            coverage = snapshot.runtime_coverage;
        }
    }
    PickerRecordsSnapshot {
        records: records.into_values().collect(),
        runtime_coverage: coverage,
    }
}

#[cfg(test)]
#[path = "source_reload_worker_tests.rs"]
mod tests;
