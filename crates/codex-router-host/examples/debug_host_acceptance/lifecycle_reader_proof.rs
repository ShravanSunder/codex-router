//! Real public lifecycle readers fed by the owned backend and normal read-only catalog.
use super::owned_thread_registry::OwnedThreadRegistry;
use codex_native_integration::NativeProtocolConnection;
use communication_client::ControlClient;
use communication_protocol::{
    AddressEntry, CoverageState, CoverageView, JournalPosition, JournalStatus, LifecycleChange,
    LifecycleSubject, NativeSessionListParams, NativeSessionView, NativeThreadStatus,
    ObservationSource, StatusOrdering,
};
use serde_json::json;
use std::{error::Error, path::Path, time::Duration};

pub async fn run_reader_proof(
    native: &mut NativeProtocolConnection,
    owned: &mut OwnedThreadRegistry,
    directory: &Path,
) -> Result<(), Box<dyn Error>> {
    let mut client =
        ControlClient::connect(directory, "lifecycle-proof", env!("CARGO_PKG_VERSION")).await?;
    let before = match client.journal_status().await? {
        JournalStatus::Available { bounds } => JournalPosition {
            journal_id: bounds.journal_id,
            sequence: bounds.last_sequence,
        },
        JournalStatus::Unavailable => {
            return Err("lifecycle storage unavailable before proof".into());
        }
    };
    let cwd = std::env::current_dir()?;
    let target = owned.create(native, &cwd).await?;
    let blank = owned.create(native, &cwd).await?;
    let receipt = owned
        .submit_text(
            native,
            &target,
            "Do not use tools. Reply exactly LIFECYCLE_READER_READY.",
        )
        .await?;
    if owned.observe_text(native, receipt).await?.trim() != "LIFECYCLE_READER_READY" {
        return Err("lifecycle preparation failed".into());
    }
    let endpoint = communication_protocol::EndpointRef {
        service_id: client.identity().service_id.clone(),
        endpoint_id: "codex-local".to_owned().try_into()?,
    };
    let stored = client
        .list_sessions(NativeSessionListParams {
            endpoint: endpoint.clone(),
            view: NativeSessionView::Stored,
            page_size: 100,
            cursor: None,
        })
        .await?;
    if !stored
        .sessions
        .iter()
        .any(|entry| String::from(entry.target.session_id.clone()) == target)
    {
        return Err("fresh materialized thread absent from native stored catalog".into());
    }
    let (entry, coverage, pages) = tokio::time::timeout(Duration::from_secs(10), async {
        let mut position = before.clone();
        loop {
            let mut cursor = None;
            let mut captured = None;
            let mut found = None;
            let mut pages = 0;
            loop {
                let page = client
                    .list_addresses(&endpoint, 1, cursor.as_deref())
                    .await?;
                let identity = (
                    page.snapshot_id.clone(),
                    serde_json::to_value(&page.watermark)?,
                    serde_json::to_value(&page.coverage)?,
                );
                if let Some(previous) = &captured {
                    if previous != &identity {
                        return Err::<_, Box<dyn Error>>(
                            "address snapshot changed while paging".into(),
                        );
                    }
                } else {
                    captured = Some(identity);
                }
                pages += 1;
                for entry in page.entries {
                    if String::from(entry.address.native_thread_id.clone()) == target {
                        found = Some(entry);
                    }
                }
                cursor = page.next_cursor;
                if cursor.is_none() {
                    if let Some(entry) = found
                        && fresh_idle(&entry, &page.coverage)
                        && pages > 1
                    {
                        return Ok((entry, page.coverage, pages));
                    }
                    break;
                }
                if pages >= 512 {
                    return Err("proof address scan exceeded bound".into());
                }
            }
            let changes = client.read_journal(&endpoint, position, 100, 1000).await?;
            position = changes.next;
        }
    })
    .await??;
    let mut position = before;
    let mut native_statuses = 0;
    let mut stored_discoveries = 0;
    loop {
        let page = client.read_journal(&endpoint, position, 100, 0).await?;
        for record in page.records {
            if !matches!(&record.observation.subject, LifecycleSubject::Thread { address } if String::from(address.native_thread_id.clone()) == target)
            {
                continue;
            }
            match (&record.observation.source, &record.observation.change) {
                (ObservationSource::NativeNotification, LifecycleChange::ThreadStatus { .. }) => {
                    native_statuses += 1
                }
                (ObservationSource::InventoryRead, LifecycleChange::ThreadDiscovered)
                    if record.observation.scope.generation.is_none() =>
                {
                    stored_discoveries += 1
                }
                _ => {}
            }
        }
        position = page.next;
        if page.caught_up {
            break;
        }
    }
    if native_statuses == 0 || stored_discoveries == 0 {
        return Err("public journal did not contain both actual observation sources".into());
    }
    client.close().await?;
    println!(
        "{}",
        json!({"kind":"ownedLifecycleReadersPassed","threadId":target,"blankThreadId":blank,"snapshotPages":pages,"entry":entry,"coverage":coverage,"nativeStatusRecords":native_statuses,"storedDiscoveryRecords":stored_discoveries})
    );
    Ok(())
}
fn fresh_idle(entry: &AddressEntry, coverage: &CoverageView) -> bool {
    coverage.state == CoverageState::Observing
        && entry.last_status == Some(NativeThreadStatus::Idle)
        && matches!(entry.status_ordering, StatusOrdering::Established)
        && entry.status_scope.as_ref().is_some_and(|scope| {
            Some(&scope.observer_id) == coverage.observer_id.as_ref()
                && scope.generation == coverage.generation
                && scope.generation.is_some()
        })
}
