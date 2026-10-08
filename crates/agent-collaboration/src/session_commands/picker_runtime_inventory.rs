//! Read-only runtime inventory joined with the existing stored picker catalog.
use super::{SessionPickerIdentity, SessionPickerRecord, SessionRecord};
use crate::picker_runtime_status::{
    PickerRecordsSnapshot, PickerRuntimeCoverage, PickerRuntimeStatus,
};
use collaboration_client::protocol::{
    ChannelDescription, CodexGeneration, EndpointAvailability, EndpointInventory, EndpointRef,
    NativeSessionListParams, NativeSessionObservation, NativeSessionScope, NativeSessionSource,
    NativeSessionView, ProviderSessionListParams,
};
use collaboration_client::{ClientError, CollaborationClient};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

const MAX_RUNTIME_ROWS: usize = 4096;

pub(super) async fn load_runtime_records(
    client: &mut CollaborationClient,
    metadata: &[SessionPickerRecord],
    include_empty_sessions: bool,
) -> Result<(EndpointRef, Vec<SessionPickerRecord>), ClientError> {
    let (endpoint, generation) = selected_generation(client.list_endpoints().await?)?;
    let known: BTreeMap<_, _> = metadata
        .iter()
        .map(|row| (row.session_id.as_str(), row))
        .collect();
    let mut rows = Vec::new();
    let mut cursor = None;
    let mut seen_cursors = BTreeSet::new();
    let mut seen_ids = BTreeSet::new();
    loop {
        let page = client
            .list_sessions(NativeSessionListParams {
                endpoint: endpoint.clone(),
                view: NativeSessionView::Loaded,
                scope: collaboration_client::protocol::NativeSessionScope::Any,
                source: collaboration_client::protocol::NativeSessionSource::All,
                include_empty_sessions,
                query: None,
                page_size: 100,
                cursor,
            })
            .await?;
        if page.generation.as_ref() != Some(&generation) {
            return Err(ClientError::Protocol(
                "runtime inventory generation changed",
            ));
        }
        for summary in page.sessions {
            let target = summary.target.clone();
            let id = String::from(summary.target.session_id.clone());
            if !seen_ids.insert(id.clone()) || rows.len() >= MAX_RUNTIME_ROWS {
                return Err(ClientError::Protocol("runtime inventory exceeded bounds"));
            }
            let NativeSessionObservation::Runtime { status, .. } = summary.observation else {
                return Err(ClientError::Protocol("expected runtime observation"));
            };
            let mut row = if let Some(row) = known.get(id.as_str()) {
                (*row).clone()
            } else {
                let inspected = client.inspect_session(&summary.target).await?;
                if inspected.generation != generation {
                    return Err(ClientError::Protocol("runtime metadata generation changed"));
                }
                runtime_record(
                    &id,
                    &summary.title,
                    &String::from(summary.working_directory.clone()),
                    &inspected.thread,
                )
            };
            row.identity = SessionPickerIdentity::HostedCodex(target);
            row.runtime_status = PickerRuntimeStatus::from_native(&status);
            rows.push(row);
        }
        match page.next_cursor {
            Some(next) if seen_cursors.insert(next.clone()) && seen_cursors.len() <= 64 => {
                cursor = Some(next)
            }
            Some(_) => {
                return Err(ClientError::Protocol(
                    "runtime inventory cursor did not converge",
                ));
            }
            None => break,
        }
    }
    let (current_endpoint, current_generation) =
        selected_generation(client.list_endpoints().await?)?;
    if current_endpoint != endpoint || current_generation != generation {
        return Err(ClientError::Protocol(
            "runtime inventory generation changed",
        ));
    }
    Ok((endpoint, rows))
}

async fn load_provider_records(
    client: &mut CollaborationClient,
) -> Result<(Option<EndpointRef>, Vec<SessionPickerRecord>), ClientError> {
    let inventory = client.list_endpoints().await?;
    let native_endpoint = inventory
        .endpoints
        .iter()
        .find(|entry| {
            entry
                .channels
                .iter()
                .any(|channel| matches!(channel, ChannelDescription::NativeCodex { .. }))
        })
        .map(|entry| entry.endpoint.clone());
    let providers = inventory
        .endpoints
        .into_iter()
        .filter(|entry| {
            entry
                .channels
                .iter()
                .any(|channel| matches!(channel, ChannelDescription::ExternalProvider { .. }))
        })
        .collect::<Vec<_>>();
    let mut rows = Vec::new();
    for provider in providers {
        let mut cursor = None;
        let mut seen_cursors = BTreeSet::new();
        loop {
            let page = client
                .list_provider_sessions(ProviderSessionListParams {
                    endpoint: provider.endpoint.clone(),
                    view: NativeSessionView::Stored,
                    scope: NativeSessionScope::Any,
                    source: NativeSessionSource::All,
                    query: None,
                    page_size: 100,
                    cursor,
                })
                .await?;
            for summary in page.sessions {
                if rows.len() >= MAX_RUNTIME_ROWS {
                    return Err(ClientError::Protocol(
                        "provider picker inventory exceeded bounds",
                    ));
                }
                let label = String::from(provider.label.clone());
                rows.push(SessionPickerRecord::from_provider_summary(&summary, &label));
            }
            match page.next_cursor {
                Some(next) if seen_cursors.insert(next.clone()) && seen_cursors.len() <= 64 => {
                    cursor = Some(next)
                }
                Some(_) => {
                    return Err(ClientError::Protocol(
                        "provider picker cursor did not converge",
                    ));
                }
                None => break,
            }
        }
    }
    Ok((native_endpoint, rows))
}

fn selected_generation(
    inventory: EndpointInventory,
) -> Result<(EndpointRef, CodexGeneration), ClientError> {
    let endpoint = inventory
        .endpoints
        .into_iter()
        .find(|entry| String::from(entry.endpoint.endpoint_id.clone()) == "codex-local")
        .ok_or(ClientError::Protocol("runtime endpoint unavailable"))?;
    if !matches!(
        endpoint.availability,
        EndpointAvailability::Available { .. }
    ) {
        return Err(ClientError::Protocol("runtime endpoint unavailable"));
    }
    let generation = endpoint
        .channels
        .into_iter()
        .find_map(|channel| match channel {
            ChannelDescription::NativeCodex { generation, .. } => generation,
            _ => None,
        })
        .ok_or(ClientError::Protocol("runtime generation unavailable"))?;
    if generation.service_epoch != inventory.service_epoch {
        return Err(ClientError::Protocol("runtime inventory epoch mismatch"));
    }
    Ok((endpoint.endpoint, generation))
}

fn runtime_record(
    id: &str,
    fallback_title: &str,
    cwd: &str,
    thread: &serde_json::Value,
) -> SessionPickerRecord {
    let text = |key: &str| {
        thread
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    let time = |key: &str| {
        thread
            .get(key)
            .and_then(serde_json::Value::as_i64)
            .and_then(|value| value.checked_mul(1000))
    };
    let name = text("name");
    let title = name
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| {
            if fallback_title.is_empty() {
                id.to_owned()
            } else {
                fallback_title.to_owned()
            }
        });
    let mut row = SessionPickerRecord::from_record(&SessionRecord {
        session_id: id.to_owned(),
        rollout_path: None,
        cwd: Some(cwd.to_owned()),
        provider: text("modelProvider"),
        model: text("model"),
        reasoning_effort: text("reasoningEffort"),
        source: thread.get("source").and_then(|source| {
            source
                .as_str()
                .map(str::to_owned)
                .or_else(|| serde_json::to_string(source).ok())
        }),
        thread_source: if thread
            .get("parentThreadId")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|id| !id.is_empty())
        {
            Some("subagent".to_owned())
        } else {
            text("threadSource")
        },
        git_branch: thread
            .pointer("/gitInfo/branch")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        git_origin_url: thread
            .pointer("/gitInfo/originUrl")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        name,
        title: Some(title.clone()),
        preview: None,
        first_user_message: None,
        created_at_ms: time("createdAt"),
        updated_at_ms: time("updatedAt"),
        recency_at_ms: time("updatedAt"),
    });
    // Runtime inventory already chose its title; stored-session formatting is separate.
    row.title = title;
    row
}

/// Cache only remembered addresses/metadata; every refresh must re-establish live status.
#[derive(Default)]
pub(super) struct PickerRuntimeInventory {
    remembered: Vec<SessionPickerRecord>,
}
impl PickerRuntimeInventory {
    pub(super) async fn refresh(
        &mut self,
        directory: Option<&Path>,
        stored: Vec<SessionPickerRecord>,
        include_empty_sessions: bool,
    ) -> PickerRecordsSnapshot {
        if directory.is_none() {
            self.remembered.retain(|row| !row.identity.is_provider());
        }
        let (provider_result, native_result) = if let Some(directory) = directory {
            let metadata = stored.clone();
            let providers = async {
                let mut client = CollaborationClient::connect(
                    directory,
                    "sessions-picker-providers",
                    env!("CARGO_PKG_VERSION"),
                )
                .await?;
                load_provider_records(&mut client).await
            };
            let native = async {
                let mut client = CollaborationClient::connect(
                    directory,
                    "sessions-picker-native",
                    env!("CARGO_PKG_VERSION"),
                )
                .await?;

                load_runtime_records(&mut client, &metadata, include_empty_sessions).await
            };
            let deadline = std::time::Duration::from_secs(5);
            let (providers, native) = tokio::join!(
                tokio::time::timeout(deadline, providers),
                tokio::time::timeout(deadline, native)
            );
            (
                providers.ok().and_then(Result::ok),
                native.ok().and_then(Result::ok),
            )
        } else {
            (None, None)
        };
        let native_endpoint = native_result
            .as_ref()
            .map(|(endpoint, _)| endpoint)
            .or_else(|| {
                provider_result
                    .as_ref()
                    .and_then(|(endpoint, _)| endpoint.as_ref())
            });
        let provider_inventory_available = provider_result.is_some();
        let mut combined: BTreeMap<SessionPickerIdentity, SessionPickerRecord> = self
            .remembered
            .iter()
            .cloned()
            .chain(stored)
            .map(|mut row| {
                if let Some(endpoint) = native_endpoint
                    && matches!(row.identity, SessionPickerIdentity::LocalCodex(_))
                {
                    row = row.with_hosted_codex(endpoint);
                }
                row.runtime_status = PickerRuntimeStatus::Unknown;
                if row.identity.is_provider() && !provider_inventory_available {
                    row.provider_state = None;
                }
                row.recency = super::format_recency_at_ms(row.recency_at_ms);
                row.created = super::format_recency_at_ms(row.created_at_ms);
                (row.identity.clone(), row)
            })
            .collect();
        let runtime_coverage = if directory.is_none() {
            PickerRuntimeCoverage::LocalOnly
        } else if native_result.is_some() {
            PickerRuntimeCoverage::Available
        } else {
            PickerRuntimeCoverage::Unavailable
        };
        if native_result.is_some() || provider_result.is_some() {
            self.remembered.clear();
            if let Some((_, runtime)) = native_result {
                for row in runtime {
                    combined.insert(row.identity.clone(), row.clone());
                    self.remembered.push(row);
                }
            }
            if let Some((_, providers)) = provider_result {
                combined.retain(|identity, _| !identity.is_provider());
                for row in providers {
                    combined.insert(row.identity.clone(), row.clone());
                    self.remembered.push(row);
                }
            }
        } else {
            for row in &mut self.remembered {
                row.runtime_status = PickerRuntimeStatus::Unknown;
                if row.identity.is_provider() {
                    row.provider_state = None;
                }
            }
        }
        PickerRecordsSnapshot {
            records: combined.into_values().collect(),
            runtime_coverage,
        }
    }
}

#[cfg(test)]
#[path = "picker_runtime_inventory_tests.rs"]
mod tests;
