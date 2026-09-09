//! Host-owned configuration file adapter; SQLite owns receipts, not filesystem replacement.
use automation_storage::{AutomationStore, ConfigurationAdmission};
use communication_protocol::{
    AutomationConfiguration, AutomationConfigureRequest, ConfigurationFailure,
    ConfigurationFailureKind, ConfigurationFileState, ConfigurationNextAction, OperationId,
};
use communication_service::{AutomationConfigurationBackend, AutomationConfigurationHandle};
use serde::{Deserialize, Serialize};
use std::{
    future::Future,
    io::{self, Write},
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};
use tokio::sync::Mutex;
#[cfg(test)]
#[path = "automation_settings_crash_tests.rs"]
mod crash_tests;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsDocument {
    operation_id: OperationId,
    configuration: AutomationConfiguration,
}
pub struct AutomationSettingsFile {
    path: PathBuf,
    store: Arc<Mutex<AutomationStore>>,
    handle: AutomationConfigurationHandle,
    serial: Mutex<()>,
}
impl AutomationSettingsFile {
    async fn reconcile_operation(&self, id: OperationId) -> Result<(), ConfigurationFailure> {
        let _serial = self.serial.lock().await;
        let record = self
            .store
            .lock()
            .await
            .read_operation(&id)
            .await
            .map_err(|_| unavailable(Some(id.clone())))?;
        if record.method != "automation/configure" {
            return Err(unavailable(Some(id)));
        }
        if matches!(
            record.state,
            automation_storage::StoredOperationState::Succeeded { .. }
                | automation_storage::StoredOperationState::Failed { .. }
        ) {
            return Ok(());
        }
        let pending = self
            .store
            .lock()
            .await
            .pending_configuration::<AutomationConfiguration>()
            .await
            .map_err(|_| unavailable(Some(id.clone())))?;
        if pending.is_none_or(|pending| pending.operation_id != id) {
            return Err(unavailable(Some(id)));
        }
        self.handle.suspend().await;
        self.recover_locked().await
    }
    pub fn new(
        directory: &Path,
        store: Arc<Mutex<AutomationStore>>,
        handle: AutomationConfigurationHandle,
    ) -> Self {
        Self {
            path: directory.join("automation-settings.json"),
            store,
            handle,
            serial: Mutex::new(()),
        }
    }
    /// The Host must own its listeners before recovery may replace a file or finish a receipt.
    pub async fn recover(&self) -> Result<(), ConfigurationFailure> {
        let _serial = self.serial.lock().await;
        self.handle.suspend().await;
        self.recover_locked().await
    }
    async fn recover_locked(&self) -> Result<(), ConfigurationFailure> {
        let pending = self
            .store
            .lock()
            .await
            .pending_configuration::<AutomationConfiguration>()
            .await
            .map_err(|_| unavailable(None))?;
        let latest = self
            .store
            .lock()
            .await
            .latest_configuration::<AutomationConfiguration>()
            .await
            .map_err(|_| unavailable(None))?;
        let file = read_document(&self.path).map_err(|_| unavailable(None))?;
        if let Some(pending) = pending {
            let matches_pending = file.as_ref().is_some_and(|file| {
                file.operation_id == pending.operation_id
                    && file.configuration == pending.configuration
            });
            let matches_previous = match (&file, &latest) {
                (None, None) => true,
                (Some(file), Some(latest)) => {
                    file.operation_id == latest.operation_id
                        && file.configuration == latest.configuration
                }
                _ => false,
            };
            if !matches_pending && !matches_previous {
                return Err(unavailable(Some(pending.operation_id)));
            }
            if !matches_pending {
                self.replace_file(&pending.operation_id, pending.configuration)
                    .await?;
            } else {
                sync_document(&self.path)
                    .map_err(|_| unavailable(Some(pending.operation_id.clone())))?;
            }
            self.finish_receipt(&pending.operation_id, pending.configuration)
                .await?;
            self.handle.publish(pending.configuration).await;
            return Ok(());
        }
        match (file, latest) {
            (None, None) => {
                self.handle
                    .publish(AutomationConfiguration::default())
                    .await
            }
            (Some(file), Some(latest))
                if file.operation_id == latest.operation_id
                    && file.configuration == latest.configuration =>
            {
                self.handle.publish(file.configuration).await
            }
            _ => return Err(unavailable(None)),
        }
        Ok(())
    }
    async fn apply(
        &self,
        request: AutomationConfigureRequest,
    ) -> Result<AutomationConfiguration, ConfigurationFailure> {
        let _serial = self.serial.lock().await;
        self.handle.suspend().await;
        self.recover_locked().await?;
        self.handle.suspend().await;
        let configuration = request.configuration();
        let admitted = self
            .store
            .lock()
            .await
            .admit_configuration(
                &request.operation_id,
                &configuration,
                chrono::Utc::now().timestamp_millis(),
            )
            .await;
        match admitted {
            Ok(ConfigurationAdmission::Existing(original)) => {
                self.recover_locked().await?;
                return Ok(original);
            }
            Ok(ConfigurationAdmission::Pending(_)) => {
                self.recover_locked().await?;
                return Ok(configuration);
            }
            Ok(ConfigurationAdmission::New) => {}
            Err(automation_storage::StorageError::OperationConflict) => {
                self.recover_locked().await?;
                return Err(ConfigurationFailure{kind:ConfigurationFailureKind::OperationConflict,message:"Operation identity belongs to another configuration request; inspect it before submitting new work.".into(),operation_id:Some(request.operation_id),file_state:ConfigurationFileState::NotReplaced,next_action:ConfigurationNextAction::InspectOperation});
            }
            Err(_) => return Err(unavailable(Some(request.operation_id))),
        }
        #[cfg(test)]
        crash_tests::checkpoint("receipt-admitted");
        self.replace_file(&request.operation_id, configuration)
            .await?;
        self.finish_receipt(&request.operation_id, configuration)
            .await?;
        #[cfg(test)]
        crash_tests::checkpoint("receipt-committed");
        self.handle.publish(configuration).await;
        Ok(configuration)
    }
    async fn replace_file(
        &self,
        id: &OperationId,
        configuration: AutomationConfiguration,
    ) -> Result<(), ConfigurationFailure> {
        let marked = self
            .store
            .lock()
            .await
            .record_configuration_progress(
                id,
                &configuration,
                automation_storage::ConfigurationProgress::Writing,
            )
            .await
            .map_err(|_| unavailable(Some(id.clone())))?;
        if !marked {
            return Err(unavailable(Some(id.clone())));
        }
        if let Err(error) = write_document(
            &self.path,
            &SettingsDocument {
                operation_id: id.clone(),
                configuration,
            },
        ) {
            let _ = self
                .store
                .lock()
                .await
                .record_configuration_progress(
                    id,
                    &configuration,
                    automation_storage::ConfigurationProgress::WriteUncertain,
                )
                .await;
            return Err(ConfigurationFailure {
                kind: ConfigurationFailureKind::AutomationUnavailable,
                message: format!(
                    "Settings file update failed ({:?}); inspect and reconcile this operation before another configuration change.",
                    error.kind()
                ),
                operation_id: Some(id.clone()),
                file_state: ConfigurationFileState::Unknown,
                next_action: ConfigurationNextAction::InspectOperation,
            });
        }
        Ok(())
    }
    async fn finish_receipt(
        &self,
        id: &OperationId,
        configuration: AutomationConfiguration,
    ) -> Result<(), ConfigurationFailure> {
        if self
            .store
            .lock()
            .await
            .complete_configuration(id, &configuration, chrono::Utc::now().timestamp_millis())
            .await
            .is_err()
        {
            let _ = self
                .store
                .lock()
                .await
                .record_configuration_progress(
                    id,
                    &configuration,
                    automation_storage::ConfigurationProgress::ReceiptUncertain,
                )
                .await;
            return Err(ConfigurationFailure { kind: ConfigurationFailureKind::OutcomeUnknown,
                message: "Settings file was replaced, but its receipt was not confirmed. Inspect or reconcile the same operation ID.".into(),
                operation_id: Some(id.clone()), file_state: ConfigurationFileState::Replaced, next_action: ConfigurationNextAction::InspectOperation });
        }
        Ok(())
    }
}
impl AutomationConfigurationBackend for AutomationSettingsFile {
    fn reconcile(
        &self,
        operation_id: OperationId,
    ) -> Pin<Box<dyn Future<Output = Result<(), ConfigurationFailure>> + Send + '_>> {
        Box::pin(self.reconcile_operation(operation_id))
    }
    fn configure(
        &self,
        request: AutomationConfigureRequest,
    ) -> Pin<
        Box<dyn Future<Output = Result<AutomationConfiguration, ConfigurationFailure>> + Send + '_>,
    > {
        Box::pin(self.apply(request))
    }
}
fn unavailable(operation_id: Option<OperationId>) -> ConfigurationFailure {
    ConfigurationFailure{kind:ConfigurationFailureKind::AutomationUnavailable,message:"Configuration file/receipt reconciliation is unavailable. New automation admission remains paused; inspect the operation before retrying.".into(),operation_id,file_state:ConfigurationFileState::Unknown,next_action:ConfigurationNextAction::InspectOperation}
}
fn read_document(path: &Path) -> io::Result<Option<SettingsDocument>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 8192 {
        return Err(io::Error::other("invalid automation settings file"));
    }
    serde_json::from_slice(&std::fs::read(path)?)
        .map(Some)
        .map_err(io::Error::other)
}
fn write_document(path: &Path, document: &SettingsDocument) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("settings parent missing"))?;
    let temporary = parent.join(format!(
        "automation-settings-{}.tmp",
        OperationId::generate().as_str()
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec(document).map_err(io::Error::other)?)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        #[cfg(test)]
        crash_tests::checkpoint("temporary-synced");
        std::fs::rename(&temporary, path)?;
        #[cfg(test)]
        crash_tests::checkpoint("file-renamed");
        std::fs::File::open(parent)?.sync_all()?;
        #[cfg(test)]
        crash_tests::checkpoint("directory-synced");
        Ok(())
    })();
    if result.is_err() {
        let _cleanup = std::fs::remove_file(&temporary);
    }
    result
}
fn sync_document(path: &Path) -> io::Result<()> {
    std::fs::File::open(path)?.sync_all()?;
    std::fs::File::open(
        path.parent()
            .ok_or_else(|| io::Error::other("settings parent missing"))?,
    )?
    .sync_all()
}
