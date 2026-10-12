//! Latest-view scheduling: bounded source reads, source validation and guarded publication.
use super::source_inventory_request::{PickerSourceContext, SourceInventoryRejection};
use super::source_reload_progress::{
    SourceReadProgress, SourceReadState, SourceRecordsUpdate, SourceReloadUpdate,
};
use super::{
    InventoryEndpointKind, SessionsPickerDataQuery, SessionsPickerRecordLoader,
    SourceInventoryBinding, SourceInventoryRequest,
};
use crate::{
    picker_runtime_status::{PickerRecordsSnapshot, PickerRuntimeCoverage},
    sessions::{SessionPickerIdentity, SessionRowProvenance},
};
use collaboration_client::protocol::EndpointRef;
use futures_util::{StreamExt, stream::FuturesUnordered};
use std::collections::{BTreeMap, VecDeque};

#[path = "source_alias_groups.rs"]
mod source_alias_groups;
use source_alias_groups::{QualifiedSourceRead, SourceAliasGroups};

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
        let mut pending_sources = request
            .sources
            .iter()
            .cloned()
            .map(|source_context| SourceInventoryRequest {
                source_context,
                query: request.query.clone(),
                request_generation: request.generation,
                continuation: None,
            })
            .collect::<VecDeque<_>>();
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
        let mut qualified_sources = Vec::new();
        let mut aliases = SourceAliasGroups::default();
        let mut qualification_pending = BTreeMap::new();
        for source in &request.sources {
            if let PickerSourceContext::ConfiguredHosted(profile) = source {
                *qualification_pending
                    .entry(profile.service_id.clone())
                    .or_insert(0usize) += 1;
            }
        }
        let mut pending_continuations = VecDeque::new();
        let mut last_rejection = SourceInventoryRejection::SourceUnavailable;
        let mut same_view_refresh = false;
        let mut superseded = false;
        loop {
            let ready = SourceAliasGroups::take_settled_qualifications(
                &mut qualified_sources,
                &qualification_pending,
            );
            if !ready.is_empty() {
                let (groups, queued, rejection) =
                    SourceAliasGroups::prepare(&request.sources, ready, &mut source_progress);
                aliases.extend(groups);
                pending_sources.extend(queued);
                if let Some(reason) = rejection {
                    last_rejection = reason;
                }
                let records_update = if let Some(reason) = rejection {
                    if snapshots.is_empty() {
                        if pending_sources.is_empty()
                            && pending_continuations.is_empty()
                            && unsettled_reads == 0
                        {
                            SourceRecordsUpdate::Rejected(reason)
                        } else {
                            SourceRecordsUpdate::Pending
                        }
                    } else {
                        SourceRecordsUpdate::Ready(merge_source_snapshots(
                            &request.sources,
                            &snapshots,
                        ))
                    }
                } else {
                    SourceRecordsUpdate::Pending
                };
                accept_records(
                    request.clone(),
                    SourceReloadUpdate {
                        source_progress: source_progress.clone(),
                        records_update,
                    },
                );
            }
            while unsettled_reads < MAX_SOURCE_READS {
                let Some(source_request) = pending_sources.pop_front().or_else(|| {
                    if qualification_pending.values().any(|count| *count > 0) {
                        None
                    } else {
                        pending_continuations.pop_front()
                    }
                }) else {
                    break;
                };
                if let Some(progress) = source_progress
                    .iter_mut()
                    .find(|progress| progress.source_context == source_request.source_context)
                    && let SourceReadState::Suspended { record_count } = progress.read_state
                {
                    progress.read_state = SourceReadState::LoadingMore { record_count };
                    aliases.synchronize(
                        &source_request.source_context,
                        &mut snapshots,
                        &mut source_progress,
                    );
                    accept_records(
                        request.clone(),
                        SourceReloadUpdate {
                            source_progress: source_progress.clone(),
                            records_update: SourceRecordsUpdate::Pending,
                        },
                    );
                }
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
                changed=receiver.changed()=>{
                    if changed.is_err() {return}
                    let next=receiver.borrow_and_update().clone();
                    if next==request {same_view_refresh=true;} else {pending_view=Some(next);superseded=true;break;}
                }
                completed=reads.next(),if !reads.is_empty()=>{
                    if let Some(result)=completed && result_sender.try_send(result).is_err() {return}
                }
                completed=results.recv()=>{
                    let Some((expected,result))=completed else {return};
                    unsettled_reads=unsettled_reads.saturating_sub(1);
                    let initial_qualification=expected.continuation.is_none() && matches!(expected.source_context,PickerSourceContext::ConfiguredHosted(_));
                    if initial_qualification && let PickerSourceContext::ConfiguredHosted(profile)=&expected.source_context
                        && let Some(count)=qualification_pending.get_mut(&profile.service_id) {*count=count.saturating_sub(1);}
                    let result=if matches!(result,super::SourceInventoryResult::Qualified {..}) {
                        match QualifiedSourceRead::from_result(&expected,result) {
                            Ok(qualified)=>{qualified_sources.push(qualified);continue;},
                            Err(reason)=>super::SourceInventoryResult::Rejected {request:expected.clone(),reason},
                        }
                    } else {result};
                    let validated=validate_source_page(&expected,result,&mut snapshots);
                    let progress=source_progress.iter_mut().find(|progress|progress.source_context==expected.source_context);
                    match validated {
                        Ok((record_count,page_progress))=>{
                            match page_progress {
                                super::InventoryPageProgress::Complete=>{if let Some(progress)=progress {progress.read_state=SourceReadState::Complete {record_count};}}
                                super::InventoryPageProgress::MoreAvailable {continuation}=>{
                                    if let Some(progress)=progress {progress.read_state=SourceReadState::Suspended {record_count};}
                                    let mut next=expected.clone();next.continuation=Some(continuation);pending_continuations.push_back(next);
                                }
                            }
                            aliases.synchronize(&expected.source_context,&mut snapshots,&mut source_progress);
                            accept_records(request.clone(),SourceReloadUpdate {source_progress:source_progress.clone(),records_update:SourceRecordsUpdate::Ready(merge_source_snapshots(&request.sources,&snapshots))});
                        }
                        Err(reason)=>{
                            last_rejection=reason;
                            let had_snapshot=snapshots.iter().any(|(source,_,_)|source==&expected.source_context);
                            snapshots.retain(|(source,_,_)|source!=&expected.source_context);
                            if let Some(progress)=progress {progress.read_state=SourceReadState::Rejected {reason};}
                            aliases.synchronize(&expected.source_context,&mut snapshots,&mut source_progress);
                            let records_update=if snapshots.is_empty() {
                                if unsettled_reads>0||!pending_sources.is_empty()||!pending_continuations.is_empty()||!qualified_sources.is_empty() {
                                    if had_snapshot {SourceRecordsUpdate::Ready(merge_source_snapshots(&request.sources,&snapshots))}
                                    else {SourceRecordsUpdate::Pending}
                                }
                                else {SourceRecordsUpdate::Rejected(reason)}
                            } else {SourceRecordsUpdate::Ready(merge_source_snapshots(&request.sources,&snapshots))};
                            accept_records(request.clone(),SourceReloadUpdate {source_progress:source_progress.clone(),records_update});
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

type BoundSourceSnapshots = Vec<(
    PickerSourceContext,
    Option<SourceInventoryBinding>,
    PickerRecordsSnapshot,
)>;

fn validate_source_page(
    expected: &SourceInventoryRequest,
    result: super::SourceInventoryResult,
    snapshots: &mut BoundSourceSnapshots,
) -> Result<(usize, super::InventoryPageProgress), SourceInventoryRejection> {
    if result.request() != expected {
        return Err(SourceInventoryRejection::StaleSnapshot);
    }
    let (binding, snapshot, progress) = match result {
        super::SourceInventoryResult::Ready {
            binding,
            snapshot,
            progress,
            ..
        } => (binding, snapshot, progress),
        super::SourceInventoryResult::Rejected { reason, .. } => return Err(reason),
        super::SourceInventoryResult::Qualified { .. } => {
            return Err(SourceInventoryRejection::InvalidInventory);
        }
    };
    if !source_snapshot_matches(&expected.source_context, binding.as_ref(), &snapshot) {
        return Err(SourceInventoryRejection::InvalidInventory);
    }
    if let Some((_, previous_binding, previous)) = snapshots
        .iter_mut()
        .find(|(source, _, _)| source == &expected.source_context)
    {
        if *previous_binding != binding {
            return Err(SourceInventoryRejection::StaleSnapshot);
        }
        if snapshot.records.iter().any(|row| {
            previous
                .records
                .iter()
                .any(|old| old.identity == row.identity)
        }) {
            return Err(SourceInventoryRejection::InvalidInventory);
        }
        previous.records.extend(snapshot.records);
        previous.runtime_coverage = snapshot.runtime_coverage;
        Ok((previous.records.len(), progress))
    } else {
        let count = snapshot.records.len();
        snapshots.push((expected.source_context.clone(), binding, snapshot));
        Ok((count, progress))
    }
}

fn source_snapshot_matches(
    source: &PickerSourceContext,
    binding: Option<&SourceInventoryBinding>,
    snapshot: &PickerRecordsSnapshot,
) -> bool {
    let PickerSourceContext::ConfiguredHosted(profile) = source else {
        return true;
    };
    let Some(binding) = binding else {
        return false;
    };
    if binding.service_id != profile.service_id {
        return false;
    }
    snapshot
        .records
        .iter()
        .all(|record| match &record.identity {
            SessionPickerIdentity::HostedCodex(target) => {
                binding.contains(&target.endpoint, InventoryEndpointKind::NativeCodex)
                    && record.provenance == SessionRowProvenance::ObservedHosted
                    && record.conversation_source.is_none()
                    && record.normalized_cwd.is_none()
            }
            SessionPickerIdentity::HostedProvider(target) => {
                binding.contains(&target.endpoint, InventoryEndpointKind::ExternalProvider)
                    && record.provenance == SessionRowProvenance::ObservedProvider
                    && record.conversation_source.is_none()
                    && record.normalized_cwd.is_none()
            }
            SessionPickerIdentity::LocalCodex(_) => false,
        })
}

fn merge_source_snapshots(
    source_order: &[PickerSourceContext],
    snapshots: &[(
        PickerSourceContext,
        Option<SourceInventoryBinding>,
        PickerRecordsSnapshot,
    )],
) -> PickerRecordsSnapshot {
    let mut records = BTreeMap::new();
    let mut coverage = PickerRuntimeCoverage::Unavailable;
    let labels = qualified_alias_projection::qualified_alias_labels(source_order, snapshots);
    for source in source_order {
        let Some((_, _, snapshot)) = snapshots.iter().find(|(context, _, _)| context == source)
        else {
            continue;
        };
        for record in &snapshot.records {
            records.entry(record.identity.clone()).or_insert_with(|| {
                let mut record = record.clone();
                record.source_context = Some(source.clone());
                record.machine_display_label = match (source, &record.identity) {
                    (
                        PickerSourceContext::ConfiguredHosted(_),
                        SessionPickerIdentity::HostedCodex(target)
                        | SessionPickerIdentity::HostedProvider(target),
                    ) => labels.get(&target.endpoint).cloned(),
                    _ => None,
                };
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

#[path = "qualified_alias_projection.rs"]
mod qualified_alias_projection;

#[cfg(test)]
#[path = "source_reload_worker_tests.rs"]
mod tests;
