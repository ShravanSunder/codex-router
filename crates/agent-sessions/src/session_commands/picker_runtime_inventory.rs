//! Read-only runtime inventory joined with the existing stored picker catalog.
use super::{SessionPickerRecord, SessionRecord};
use crate::picker_runtime_status::{
    PickerRecordsSnapshot, PickerRuntimeCoverage, PickerRuntimeStatus,
};
use communication_client::{ClientError, ControlClient};
use communication_protocol::{
    ChannelDescription, CodexGeneration, EndpointAvailability, EndpointInventory, EndpointRef,
    NativeSessionListParams, NativeSessionObservation, NativeSessionView,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

const MAX_RUNTIME_ROWS: usize = 4096;

pub(super) async fn load_runtime_records(
    client: &mut ControlClient,
    metadata: &[SessionPickerRecord],
) -> Result<Vec<SessionPickerRecord>, ClientError> {
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
    Ok(rows)
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
    SessionPickerRecord::from_record(&SessionRecord {
        session_id: id.to_owned(),
        rollout_path: None,
        display_title: Some(title.clone()),
        cwd: Some(cwd.to_owned()),
        provider: text("modelProvider"),
        model: text("model"),
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
        title: Some(title),
        preview: None,
        first_user_message: None,
        created_at_ms: time("createdAt"),
        updated_at_ms: time("updatedAt"),
        recency_at_ms: time("updatedAt"),
    })
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
    ) -> PickerRecordsSnapshot {
        let mut combined: BTreeMap<String, SessionPickerRecord> = self
            .remembered
            .iter()
            .cloned()
            .chain(stored)
            .map(|mut row| {
                row.runtime_status = PickerRuntimeStatus::Unknown;
                row.recency = super::format_recency_at_ms(row.recency_at_ms);
                row.created = super::format_recency_at_ms(row.created_at_ms);
                (row.session_id.clone(), row)
            })
            .collect();
        let metadata: Vec<_> = combined.values().cloned().collect();
        let result = if let Some(directory) = directory {
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                let mut client =
                    ControlClient::connect(directory, "sessions-picker", env!("CARGO_PKG_VERSION"))
                        .await?;
                let result = load_runtime_records(&mut client, &metadata).await;
                let _closed = client.close().await;
                result
            })
            .await
            .ok()
            .and_then(Result::ok)
        } else {
            None
        };
        let runtime_coverage = if directory.is_none() {
            PickerRuntimeCoverage::LocalOnly
        } else if result.is_some() {
            PickerRuntimeCoverage::Available
        } else {
            PickerRuntimeCoverage::Unavailable
        };
        if let Some(runtime) = result {
            self.remembered = runtime.clone();
            for row in runtime {
                combined.insert(row.session_id.clone(), row);
            }
        } else {
            for row in &mut self.remembered {
                row.runtime_status = PickerRuntimeStatus::Unknown;
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
