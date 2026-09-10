//! Replaying creation must observe the current skipped wake through the CLI wait path.
use automation_storage::{AutomationStore, WakeAction, WakeCreate, WakeMutation};
use communication_protocol::{OperationId, SavedMessage};
use communication_service::{LocalControlService, ManifestPublication, ServiceIdentity};
use serde_json::{Value, json};
use std::{os::unix::fs::DirBuilderExt, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn skipped_one_shot_cli_wait_reports_no_firing() -> Result<(), Box<dyn std::error::Error>> {
    // Arrange: real SQLite and Control transport, controlled clock, no native backend.
    let root = std::path::PathBuf::from(format!(
        "/tmp/skipped-cli-{}",
        OperationId::generate().as_str()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let mut store = AutomationStore::open(&root.join("automation.sqlite")).await?;
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let target = json!({"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"fixture"});
    let message: SavedMessage = serde_json::from_value(json!({
        "target":target,"content":{"kind":"humanUser","text":"Reminder"},
        "delivery":"auto","generationGuard":null
    }))?;
    let operation_id = OperationId::generate();
    let wake = store
        .create_wakeup(&WakeCreate {
            operation_id: operation_id.clone(),
            message,
            timing: agent_automation::TimingRule::After { seconds: 1 },
            expiry: agent_automation::ExpiryRule::None,
            now_ms: 0,
        })
        .await?;
    for (action, now_ms) in [(WakeAction::Pause, 500), (WakeAction::Resume, 2000)] {
        store
            .mutate_wakeup::<SavedMessage>(&WakeMutation {
                operation_id: OperationId::generate(),
                wakeup_id: wake.definition.wakeup_id.clone(),
                action,
                now_ms,
            })
            .await?;
    }
    let store = Arc::new(tokio::sync::Mutex::new(store));
    let digest = format!("sha256:{}", "a".repeat(64));
    let identity = ServiceIdentity::new(service_id, epoch, &digest)
        .map_err(std::io::Error::other)?
        .with_automation_store(Arc::clone(&store));
    let listener = LocalControlService::bind(&root.join("control.sock"), identity)?;
    let manifest = serde_json::from_value(json!({
        "version":1,"serviceId":service_id,"serviceEpoch":epoch,
        "control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest
    }))?;
    let publication = ManifestPublication::publish(&root, &manifest)?;
    let stop = CancellationToken::new();
    let server = tokio::spawn(listener.run(stop.clone()));

    // Act: recover the existing creation receipt, then wait on the current wake.
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
            .kill_on_drop(true)
            .args([
                "wake",
                "send",
                "--to",
                &target.to_string(),
                "--human-user",
                "--text",
                "Reminder",
                "--after",
                "1s",
                "--operation-id",
                operation_id.as_str(),
                "--wait-until-first-fire",
                "--json",
                "--service-directory",
            ])
            .arg(&root)
            .output(),
    )
    .await;
    stop.cancel();
    server.await??;
    drop(publication);
    let output = output??;

    // Assert: durable creation succeeded, but no firing or delivery was invented.
    if output.status.code() != Some(4) {
        return Err(format!(
            "expected terminal wait exit 4: {}",
            String::from_utf8_lossy(&output.stdout)
        )
        .into());
    }
    let records = String::from_utf8(output.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    if records.len() != 2
        || records
            .first()
            .and_then(|value| value.pointer("/result/definition/wakeupId"))
            != Some(&json!(wake.definition.wakeup_id))
    {
        return Err("creation replay did not return the original wake before waiting".into());
    }
    let error = records.last().ok_or("missing wait error")?;
    for (pointer, expected) in [
        ("/error/kind", "wakeFinishedWithoutFiring"),
        ("/error/nextAction", "createWakeup"),
        ("/error/effects/firstFire", "notRecorded"),
    ] {
        if error.pointer(pointer) != Some(&json!(expected)) {
            return Err(format!("incorrect terminal wait feedback at {pointer}: {error}").into());
        }
    }
    let current = store
        .lock()
        .await
        .read_wakeup::<SavedMessage>(&wake.definition.wakeup_id)
        .await?;
    if current.first_fire.is_some() || current.pending_delivery_id.is_some() {
        return Err("skipped wake invented a firing or delivery".into());
    }
    drop(store);
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            return Err("unexpected cleanup entry".into());
        }
        std::fs::remove_file(entry.path())?;
    }
    std::fs::remove_dir(root)?;
    Ok(())
}
