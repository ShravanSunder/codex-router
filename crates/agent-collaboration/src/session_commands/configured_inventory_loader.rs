//! One captured machine read; inventory binding never qualifies a native action.
use super::{
    SessionPickerRecord, SessionsPickerDataQuery, SessionsSource, configured_inventory_query,
    configured_inventory_transport::ConfiguredInventoryClient,
    picker_runtime_inventory::native_inventory_pager::NativeInventoryPager,
    router_connection_registry::RouterConnectionProfile,
};
use crate::{
    picker_runtime_status::{PickerRecordsSnapshot, PickerRuntimeCoverage},
    presentation::session_picker::{
        InventoryEndpointKind, PickerSourceContext, SourceInventoryBinding,
        SourceInventoryRejection, SourceInventoryRequest, SourceInventoryResult,
    },
};
use collaboration_client::{
    ClientError,
    protocol::{
        NativeSessionListParams, NativeSessionScope, NativeSessionSource, NativeSessionView,
        ProviderSessionListParams, SessionRef,
    },
};
use std::{collections::BTreeSet, time::Duration};

const MAX_SOURCE_ROWS: usize = 4096;
const MAX_SOURCE_PAGES: usize = 65;
const SOURCE_READ_DEADLINE: Duration = Duration::from_secs(30);

pub(super) async fn load(request: SourceInventoryRequest) -> SourceInventoryResult {
    let result = match &request.source_context {
        PickerSourceContext::ConfiguredHosted(profile) => {
            if let Err(reason) =
                configured_inventory_query::qualify_configured_query(&request.query)
            {
                Err(reason)
            } else if profile.credential.is_some() {
                Err(SourceInventoryRejection::EndpointUnqualified)
            } else {
                tokio::time::timeout(
                    SOURCE_READ_DEADLINE,
                    read_inventory(profile, &request.query),
                )
                .await
                .unwrap_or(Err(SourceInventoryRejection::SourceUnavailable))
            }
        }
        _ => Err(SourceInventoryRejection::InvalidInventory),
    };
    match result {
        Ok((binding, snapshot)) => SourceInventoryResult::Ready {
            request,
            binding: Some(binding),
            snapshot,
        },
        Err(reason) => SourceInventoryResult::Rejected { request, reason },
    }
}

async fn read_inventory(
    profile: &RouterConnectionProfile,
    query: &SessionsPickerDataQuery,
) -> Result<(SourceInventoryBinding, PickerRecordsSnapshot), SourceInventoryRejection> {
    let mut client = ConfiguredInventoryClient::new(profile).map_err(read_rejection)?;
    let inventory = client.inventory().await.map_err(read_rejection)?;
    let binding = SourceInventoryBinding::from_inventory(&profile.service_id, &inventory)?;
    configured_inventory_query::qualify_bound_source_filter(query, &binding)?;
    let source_filter = match query.source {
        SessionsSource::All => NativeSessionSource::All,
        SessionsSource::Interactive => NativeSessionSource::Interactive,
        SessionsSource::Subagents => NativeSessionSource::Subagents,
    };
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for (endpoint, kind) in &binding.endpoints {
        match kind {
            InventoryEndpointKind::NativeCodex => {
                let mut pager = NativeInventoryPager::new(
                    NativeSessionListParams {
                        endpoint: endpoint.clone(),
                        view: NativeSessionView::Stored,
                        scope: NativeSessionScope::Any,
                        source: source_filter,
                        include_empty_sessions: query.include_empty_sessions,
                        query: None,
                        page_size: 100,
                        cursor: None,
                    },
                    None,
                )
                .map_err(read_rejection)?;
                while let Some(page) = pager.next_page(&mut client).await.map_err(read_rejection)? {
                    for summary in page.sessions {
                        accept_target(&mut seen, summary.target.clone())?;
                        rows.push(SessionPickerRecord::from_native_summary(&summary));
                    }
                }
            }
            InventoryEndpointKind::ExternalProvider => {
                let label = inventory
                    .endpoints
                    .iter()
                    .find(|entry| &entry.endpoint == endpoint)
                    .ok_or(SourceInventoryRejection::InvalidInventory)?;
                let mut request = ProviderSessionListParams {
                    endpoint: endpoint.clone(),
                    view: NativeSessionView::Stored,
                    scope: NativeSessionScope::Any,
                    source: source_filter,
                    query: None,
                    page_size: 100,
                    cursor: None,
                };
                let mut cursors = BTreeSet::new();
                let mut complete = false;
                for _ in 0..MAX_SOURCE_PAGES {
                    let page = client
                        .provider_page(&request)
                        .await
                        .map_err(read_rejection)?;
                    if page.endpoint != *endpoint
                        || page.sessions.len() > request.page_size as usize
                        || page.skipped_records.is_some_and(|skipped| skipped != 0)
                    {
                        return Err(SourceInventoryRejection::InvalidInventory);
                    }
                    for summary in page.sessions {
                        if summary.target().endpoint != *endpoint {
                            return Err(SourceInventoryRejection::InvalidInventory);
                        }
                        accept_target(&mut seen, summary.target().clone())?;
                        rows.push(SessionPickerRecord::from_source_provider_summary(
                            &summary,
                            &String::from(label.label.clone()),
                        ));
                    }
                    match page.next_cursor {
                        Some(cursor)
                            if !cursor.is_empty()
                                && cursor.len() <= 1024
                                && cursors.insert(cursor.clone()) =>
                        {
                            request.cursor = Some(cursor);
                        }
                        Some(_) => return Err(SourceInventoryRejection::InvalidInventory),
                        None => {
                            complete = true;
                            break;
                        }
                    }
                }
                if !complete {
                    return Err(SourceInventoryRejection::InvalidInventory);
                }
            }
        }
    }
    // A restart or changed endpoint membership invalidates the whole captured read.
    let final_inventory = client.inventory().await.map_err(read_rejection)?;
    let final_binding =
        SourceInventoryBinding::from_inventory(&profile.service_id, &final_inventory)?;
    if final_binding != binding {
        return Err(SourceInventoryRejection::StaleSnapshot);
    }
    Ok((
        binding,
        PickerRecordsSnapshot {
            records: rows,
            runtime_coverage: PickerRuntimeCoverage::Unavailable,
        },
    ))
}

fn accept_target(
    seen: &mut BTreeSet<SessionRef>,
    target: SessionRef,
) -> Result<(), SourceInventoryRejection> {
    if seen.len() >= MAX_SOURCE_ROWS || !seen.insert(target) {
        return Err(SourceInventoryRejection::InvalidInventory);
    }
    Ok(())
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
