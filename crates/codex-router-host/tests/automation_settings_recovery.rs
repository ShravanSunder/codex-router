//! Real filesystem/SQLite configuration boundaries, with no native process or model.
use automation_storage::AutomationStore;
use codex_router_host::AutomationSettingsFile;
use communication_protocol::{AutomationConfiguration, AutomationConfigureRequest, OperationId};
use communication_service::{AutomationConfigurationBackend, AutomationConfigurationHandle};
use serde_json::json;
use std::{
    os::unix::fs::{DirBuilderExt, MetadataExt},
    sync::Arc,
};
fn config(exec: u32, summary: u32) -> Result<AutomationConfiguration, Box<dyn std::error::Error>> {
    Ok(AutomationConfiguration {
        execution_timeout_seconds: exec.try_into()?,
        summary_timeout_seconds: summary.try_into()?,
    })
}
#[tokio::test]
async fn settings_recovery_reconciles_file_and_receipt_without_rolling_back_replays()
-> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "automation-settings-{}",
        OperationId::generate().as_str()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let store = Arc::new(tokio::sync::Mutex::new(
        AutomationStore::open(&root.join("automation.sqlite")).await?,
    ));
    let handle = AutomationConfigurationHandle::default();
    let backend = AutomationSettingsFile::new(&root, store.clone(), handle.clone());
    backend
        .recover()
        .await
        .map_err(|error| format!("default recovery: {error:?}"))?;
    let first = config(120, 30)?;
    let first_id = OperationId::generate();
    let request = AutomationConfigureRequest {
        operation_id: first_id.clone(),
        execution_timeout_seconds: first.execution_timeout_seconds,
        summary_timeout_seconds: first.summary_timeout_seconds,
    };
    backend
        .configure(request.clone())
        .await
        .map_err(|error| format!("initial configure: {error:?}"))?;
    // Crash point: admitted new operation, previous confirmed settings file remains.
    let second = config(240, 60)?;
    let second_id = OperationId::generate();
    store
        .lock()
        .await
        .admit_configuration(&second_id, &second, 1000)
        .await?;
    backend
        .recover()
        .await
        .map_err(|error| format!("pre-replace recovery: {error:?}"))?;
    if handle.current().await != Some(second) {
        return Err("admitted configuration was not recovered".into());
    }
    // Crash point: file replacement occurred but final SQLite receipt did not.
    let third = config(360, 90)?;
    let third_id = OperationId::generate();
    store
        .lock()
        .await
        .admit_configuration(&third_id, &third, 2000)
        .await?;
    let path = root.join("automation-settings.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({"formatVersion":1,"operationId":third_id,
            "executionTimeoutSeconds":third.execution_timeout_seconds,
            "summaryTimeoutSeconds":third.summary_timeout_seconds}))?,
    )?;
    let inode = std::fs::metadata(&path)?.ino();
    backend
        .recover()
        .await
        .map_err(|error| format!("post-replace recovery: {error:?}"))?;
    if handle.current().await != Some(third)
        || store
            .lock()
            .await
            .pending_configuration::<AutomationConfiguration>()
            .await?
            .is_some()
        || std::fs::metadata(&path)?.ino() != inode
    {
        return Err("post-replace recovery rewrote file or missed receipt".into());
    }
    let replay = backend
        .configure(request)
        .await
        .map_err(|error| format!("old replay: {error:?}"))?;
    if replay != first || handle.current().await != Some(third) {
        return Err("old command replay rolled back current settings".into());
    }
    let mut invalid: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    invalid["formatVersion"] = json!(2);
    std::fs::write(&path, serde_json::to_vec(&invalid)?)?;
    if backend.recover().await.is_ok() || handle.current().await.is_some() {
        return Err("unknown settings version admitted automation".into());
    }
    drop(backend);
    drop(store);
    for entry in std::fs::read_dir(&root)? {
        std::fs::remove_file(entry?.path())?;
    }
    std::fs::remove_dir(root)?;
    Ok(())
}
