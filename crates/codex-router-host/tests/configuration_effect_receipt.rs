//! A failed filesystem write must not leave a receipt claiming no write was attempted.
use automation_storage::{AutomationStore, StoredOperationState};
use codex_router_host::AutomationSettingsFile;
use communication_protocol::{AutomationConfigureRequest, OperationId};
use communication_service::{AutomationConfigurationBackend, AutomationConfigurationHandle};
use std::{os::unix::fs::DirBuilderExt, sync::Arc};

#[tokio::test]
async fn settings_write_failure_retains_uncertainty_and_recovers_same_request()
-> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "configuration-effects-{}",
        OperationId::generate().as_str()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let store = Arc::new(tokio::sync::Mutex::new(
        AutomationStore::open(&root.join("automation.sqlite")).await?,
    ));
    let directory = root.join("configuration-files"); // Missing parent deterministically fails after SQLite admission.
    let handle = AutomationConfigurationHandle::default();
    let backend = AutomationSettingsFile::new(&directory, store.clone(), handle.clone());
    let request = AutomationConfigureRequest {
        operation_id: OperationId::generate(),
        execution_timeout_seconds: 120.try_into()?,
        summary_timeout_seconds: 90.try_into()?,
    };
    if backend.configure(request.clone()).await.is_ok() {
        return Err("settings write unexpectedly succeeded without its directory".into());
    }
    let record = store
        .lock()
        .await
        .read_operation(&request.operation_id)
        .await?;
    let StoredOperationState::Uncertain { effects, .. } = record.state else {
        return Err("failed filesystem operation retained a misleading admission state".into());
    };
    if effects.get("fileState").and_then(serde_json::Value::as_str) != Some("unknown")
        || handle.current().await.is_some()
    {
        return Err(
            "failed configuration claimed known no-write or published uncommitted defaults".into(),
        );
    }
    std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
    backend
        .reconcile(request.operation_id.clone())
        .await
        .map_err(|error| format!("configuration fixture recovery failed: {:?}", error.kind))?;
    let record = store
        .lock()
        .await
        .read_operation(&request.operation_id)
        .await?;
    if !matches!(record.state, StoredOperationState::Succeeded { .. })
        || handle.current().await != Some(request.configuration())
    {
        return Err("configuration recovery lost the original operation or intended values".into());
    }
    let newer = AutomationConfigureRequest {
        operation_id: OperationId::generate(),
        execution_timeout_seconds: 240.try_into()?,
        summary_timeout_seconds: 180.try_into()?,
    };
    backend
        .configure(newer.clone())
        .await
        .map_err(|error| format!("newer configuration failed: {:?}", error.kind))?;
    let installed = std::fs::read(directory.join("automation-settings.json"))?;
    backend
        .reconcile(request.operation_id.clone())
        .await
        .map_err(|error| format!("completed reconciliation failed: {:?}", error.kind))?;
    if handle.current().await != Some(newer.configuration())
        || std::fs::read(directory.join("automation-settings.json"))? != installed
    {
        return Err("reconciling old operation rolled back newer configuration".into());
    }
    drop(backend);
    drop(store);
    std::fs::remove_file(directory.join("automation-settings.json"))?;
    std::fs::remove_dir(directory)?;
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            return Err("unexpected fixture entry".into());
        }
        std::fs::remove_file(entry.path())?;
    }
    std::fs::remove_dir(root)?;
    Ok(())
}
