use agent_automation::OperationId;
use automation_storage::{AutomationStore, ExternalAdmission, ExternalAdmissionResult};
use serde_json::json;
#[tokio::test]
async fn preparation_replay_returns_admission_without_authorizing_second_effect()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "prepare-operation-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let request = ExternalAdmission {
        operation_id: OperationId::generate(),
        schedule_id: agent_automation::ScheduleId::generate(),
        canonical_request: br#"{"kind":"fresh"}"#.to_vec(),
        evidence: json!({"kind":"native","allocation":"notDispatched"}),
        now_ms: 0,
    };
    let first = store
        .admit_schedule_preparation::<_, serde_json::Value>(&request)
        .await?;
    if !matches!(first, ExternalAdmissionResult::New) {
        return Err("first admission missing".into());
    }
    store.close().await?;
    let mut store = AutomationStore::open(&path).await?;
    let replay = store
        .admit_schedule_preparation::<_, serde_json::Value>(&request)
        .await?;
    match replay {
        ExternalAdmissionResult::Existing(record)
            if record.status == "admitted" && record.result.is_none() => {}
        _ => return Err("reopen allowed repeated native preparation".into()),
    }
    let conflict = ExternalAdmission {
        canonical_request: br#"{"kind":"fork"}"#.to_vec(),
        ..request
    };
    if store
        .admit_schedule_preparation::<_, serde_json::Value>(&conflict)
        .await
        .is_ok()
    {
        return Err("operation identity accepted changed payload".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
