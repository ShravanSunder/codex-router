//! One bounded source page; the picker owns accumulation, scheduling and publication.
use super::{
    SessionPickerRecord, SessionsSource, configured_inventory_query,
    configured_inventory_transport::ConfiguredInventoryClient,
    picker_runtime_inventory::native_inventory_pager::NativeInventoryPager,
    router_connection_registry::RouterConnectionProfile,
};
use crate::{
    picker_runtime_status::{PickerRecordsSnapshot, PickerRuntimeCoverage},
    presentation::session_picker::{
        InventoryEndpointKind, InventoryPageProgress, PickerSourceContext, SourceContinuation,
        SourceInventoryBinding, SourceInventoryRejection, SourceInventoryRequest,
        SourceInventoryResult,
    },
};
use collaboration_client::{
    ClientError,
    protocol::{
        EndpointInventory, EndpointRef, NativeSessionListParams, NativeSessionScope,
        NativeSessionSource, NativeSessionView, ProviderSessionListParams, SessionRef,
    },
};
use std::{
    collections::{BTreeSet, VecDeque},
    time::Duration,
};

const MAX_SOURCE_ROWS: usize = 4096;
const MAX_SOURCE_PAGES: usize = 65;
const SOURCE_READ_DEADLINE: Duration = Duration::from_secs(30);

pub(crate) struct StoredSourceContinuation {
    captured: SourceInventoryRequest,
    binding: SourceInventoryBinding,
    inventory: EndpointInventory,
    client: ConfiguredInventoryClient,
    pending: VecDeque<(EndpointRef, InventoryEndpointKind)>,
    native_pager: Option<NativeInventoryPager>,
    provider_cursor: Option<String>,
    provider_cursors: BTreeSet<String>,
    provider_pages: usize,
    seen: BTreeSet<SessionRef>,
}

enum ConfiguredReadOutcome {
    Qualified(SourceInventoryBinding, SourceContinuation),
    Page(
        SourceInventoryBinding,
        PickerRecordsSnapshot,
        InventoryPageProgress,
    ),
}

pub(super) async fn load(request: SourceInventoryRequest) -> SourceInventoryResult {
    let result = tokio::time::timeout(SOURCE_READ_DEADLINE, read_page(&request))
        .await
        .unwrap_or(Err(SourceInventoryRejection::SourceUnavailable));
    match result {
        Ok(ConfiguredReadOutcome::Qualified(binding, continuation)) => {
            SourceInventoryResult::Qualified {
                request,
                binding,
                continuation,
            }
        }
        Ok(ConfiguredReadOutcome::Page(binding, snapshot, progress)) => {
            SourceInventoryResult::Ready {
                request,
                binding: Some(binding),
                snapshot,
                progress,
            }
        }
        Err(reason) => SourceInventoryResult::Rejected { request, reason },
    }
}

async fn read_page(
    request: &SourceInventoryRequest,
) -> Result<ConfiguredReadOutcome, SourceInventoryRejection> {
    let PickerSourceContext::ConfiguredHosted(profile) = &request.source_context else {
        return Err(SourceInventoryRejection::InvalidInventory);
    };
    configured_inventory_query::qualify_configured_query(&request.query)?;
    if profile.credential.is_some() {
        return Err(SourceInventoryRejection::EndpointUnqualified);
    }
    let mut captured = request.clone();
    captured.continuation = None;
    let mut state = if let Some(cursor) = &request.continuation {
        let state = cursor
            .take()
            .await
            .ok_or(SourceInventoryRejection::InvalidInventory)?;
        if state.captured != captured {
            return Err(SourceInventoryRejection::StaleSnapshot);
        }
        state
    } else {
        let state = StoredSourceContinuation::start(profile, captured).await?;
        return Ok(ConfiguredReadOutcome::Qualified(
            state.binding.clone(),
            SourceContinuation::stored(state),
        ));
    };
    let rows = state.next_page().await?;
    // Every published page revalidates the source, not merely the end of a whole scan.
    let inventory = state.client.inventory().await.map_err(read_rejection)?;
    let final_binding = SourceInventoryBinding::from_inventory(&profile.service_id, &inventory)?;
    if final_binding != state.binding {
        return Err(SourceInventoryRejection::StaleSnapshot);
    }
    let binding = state.binding.clone();
    let progress = if state.pending.is_empty() {
        InventoryPageProgress::Complete
    } else {
        InventoryPageProgress::MoreAvailable {
            continuation: SourceContinuation::stored(state),
        }
    };
    Ok(ConfiguredReadOutcome::Page(
        binding,
        PickerRecordsSnapshot {
            records: rows,
            runtime_coverage: PickerRuntimeCoverage::Unavailable,
        },
        progress,
    ))
}

impl StoredSourceContinuation {
    async fn start(
        profile: &RouterConnectionProfile,
        captured: SourceInventoryRequest,
    ) -> Result<Self, SourceInventoryRejection> {
        let client = ConfiguredInventoryClient::new(profile).map_err(read_rejection)?;
        let inventory = client.inventory().await.map_err(read_rejection)?;
        let binding = SourceInventoryBinding::from_inventory(&profile.service_id, &inventory)?;
        configured_inventory_query::qualify_bound_source_filter(&captured.query, &binding)?;
        let pending = binding
            .endpoints
            .iter()
            .map(|(endpoint, kind)| (endpoint.clone(), *kind))
            .collect();
        Ok(Self {
            captured,
            binding,
            inventory,
            client,
            pending,
            native_pager: None,
            provider_cursor: None,
            provider_cursors: BTreeSet::new(),
            provider_pages: 0,
            seen: BTreeSet::new(),
        })
    }

    async fn next_page(&mut self) -> Result<Vec<SessionPickerRecord>, SourceInventoryRejection> {
        let (endpoint, kind) = self
            .pending
            .front()
            .cloned()
            .ok_or(SourceInventoryRejection::InvalidInventory)?;
        let source = match self.captured.query.source {
            SessionsSource::All => NativeSessionSource::All,
            SessionsSource::Interactive => NativeSessionSource::Interactive,
            SessionsSource::Subagents => NativeSessionSource::Subagents,
        };
        let (rows, complete) = match kind {
            InventoryEndpointKind::NativeCodex => {
                if self.native_pager.is_none() {
                    self.native_pager = Some(
                        NativeInventoryPager::new(
                            NativeSessionListParams {
                                endpoint,
                                view: NativeSessionView::Stored,
                                scope: NativeSessionScope::Any,
                                source,
                                include_empty_sessions: self.captured.query.include_empty_sessions,
                                query: None,
                                page_size: 100,
                                cursor: None,
                            },
                            None,
                        )
                        .map_err(read_rejection)?,
                    );
                }
                let page = self
                    .native_pager
                    .as_mut()
                    .ok_or(SourceInventoryRejection::InvalidInventory)?
                    .next_page(&mut self.client)
                    .await
                    .map_err(read_rejection)?
                    .ok_or(SourceInventoryRejection::InvalidInventory)?;
                let complete = page.next_cursor.is_none();
                let mut rows = Vec::new();
                for summary in page.sessions {
                    self.accept_target(summary.target.clone())?;
                    rows.push(SessionPickerRecord::from_native_summary(&summary));
                }
                (rows, complete)
            }
            InventoryEndpointKind::ExternalProvider => {
                if self.provider_pages >= MAX_SOURCE_PAGES {
                    return Err(SourceInventoryRejection::InvalidInventory);
                }
                let page = self
                    .client
                    .provider_page(&ProviderSessionListParams {
                        endpoint: endpoint.clone(),
                        view: NativeSessionView::Stored,
                        scope: NativeSessionScope::Any,
                        source,
                        query: None,
                        page_size: 100,
                        cursor: self.provider_cursor.clone(),
                    })
                    .await
                    .map_err(read_rejection)?;
                self.provider_pages += 1;
                if page.endpoint != endpoint
                    || page.sessions.len() > 100
                    || page.skipped_records.unwrap_or(0) > 0
                {
                    return Err(SourceInventoryRejection::InvalidInventory);
                }
                let description = self
                    .inventory
                    .endpoints
                    .iter()
                    .find(|entry| entry.endpoint == endpoint)
                    .ok_or(SourceInventoryRejection::InvalidInventory)?
                    .clone();
                let mut rows = Vec::new();
                for summary in page.sessions {
                    if summary.target().endpoint != endpoint {
                        return Err(SourceInventoryRejection::InvalidInventory);
                    }
                    self.accept_target(summary.target().clone())?;
                    rows.push(
                        SessionPickerRecord::from_source_provider_summary(
                            &summary,
                            &String::from(description.label.clone()),
                        )
                        .with_provider_endpoint(&summary, &description),
                    );
                }
                let complete = match page.next_cursor {
                    None => true,
                    Some(cursor)
                        if !cursor.is_empty()
                            && cursor.len() <= 1024
                            && self.provider_cursors.insert(cursor.clone()) =>
                    {
                        self.provider_cursor = Some(cursor);
                        false
                    }
                    Some(_) => return Err(SourceInventoryRejection::InvalidInventory),
                };
                (rows, complete)
            }
        };
        if complete {
            self.pending.pop_front();
            self.native_pager = None;
            self.provider_cursor = None;
            self.provider_cursors.clear();
            self.provider_pages = 0;
        }
        Ok(rows)
    }

    fn accept_target(&mut self, target: SessionRef) -> Result<(), SourceInventoryRejection> {
        if self.seen.len() >= MAX_SOURCE_ROWS || !self.seen.insert(target) {
            return Err(SourceInventoryRejection::InvalidInventory);
        }
        Ok(())
    }
}

fn read_rejection(error: ClientError) -> SourceInventoryRejection {
    match error {
        ClientError::Timeout
        | ClientError::Transport(_)
        | ClientError::Discovery { .. }
        | ClientError::Rejected { .. } => SourceInventoryRejection::SourceUnavailable,
        _ => SourceInventoryRejection::InvalidInventory,
    }
}
