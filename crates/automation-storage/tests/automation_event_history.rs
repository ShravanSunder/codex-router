use agent_automation::{InstructionText, OperationId};
use automation_storage::{AutomationStore, EventHistoryQuery, EventHistoryRead, EventPosition};

#[tokio::test]
async fn event_pages_report_expiry_before_cleanup_and_preserve_navigation_after_cleanup()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "event-history-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let now = chrono::DateTime::parse_from_rfc3339("2026-08-31T12:00:00Z")?.timestamp_millis();
    let cutoff = chrono::DateTime::parse_from_rfc3339("2026-06-30T12:00:00Z")?.timestamp_millis();
    for timestamp in [cutoff - 1, cutoff, now] {
        store
            .create_instruction(
                &OperationId::generate(),
                &InstructionText::try_from("Recorded instruction".to_owned())?,
                timestamp,
            )
            .await?;
    }
    let first = store
        .read_event_history(&EventHistoryQuery {
            after: None,
            now_ms: now,
            limit: 1,
        })
        .await?;
    let EventHistoryRead::Page(first) = first else {
        return Err("fresh history incorrectly expired".into());
    };
    if first.records.len() != 1 || first.records[0].recorded_at_ms != cutoff {
        return Err("history included expired rows before cleanup".into());
    }
    let expired = store
        .read_event_history(&EventHistoryQuery {
            after: Some(EventPosition {
                sequence: 1,
                observed_at_ms: cutoff - 1,
            }),
            now_ms: now,
            limit: 50,
        })
        .await?;
    if !matches!(expired, EventHistoryRead::Expired { .. }) {
        return Err("expired history cursor was silently accepted".into());
    }
    store.prune_automation_events(now, 1000).await?;
    let continued = store
        .read_event_history(&EventHistoryQuery {
            after: Some(first.next),
            now_ms: now,
            limit: 1,
        })
        .await?;
    let EventHistoryRead::Page(continued) = continued else {
        return Err("valid continued page expired after cleanup".into());
    };
    if continued.records.len() != 1 || continued.records[0].recorded_at_ms != now {
        return Err("continued history skipped a retained event".into());
    }
    let forged = store
        .read_event_history(&EventHistoryQuery {
            after: Some(EventPosition {
                sequence: 2,
                observed_at_ms: now + 1,
            }),
            now_ms: now,
            limit: 50,
        })
        .await;
    if forged.is_ok() {
        return Err("future cursor observation was accepted".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
