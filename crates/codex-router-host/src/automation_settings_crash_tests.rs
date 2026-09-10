//! Abrupt test-process exit at the actual settings adapter's persistence boundaries.
use super::*;
use automation_storage::StoredOperationState;
use std::{os::unix::fs::DirBuilderExt, time::Duration};

const STAGE_ENV: &str = "AUTOMATION_SETTINGS_TEST_CRASH_STAGE";
const ROOT_ENV: &str = "AUTOMATION_SETTINGS_TEST_CRASH_ROOT";
const OPERATION_ENV: &str = "AUTOMATION_SETTINGS_TEST_CRASH_OPERATION";
const CRASH_EXIT: i32 = 91;
type TestResult = Result<(), Box<dyn std::error::Error>>;

pub(super) fn checkpoint(stage: &str) {
    if std::env::var(STAGE_ENV).as_deref() == Ok(stage) {
        // No unwinding, file cleanup, SQLite close, or handle publication after this boundary.
        std::process::exit(CRASH_EXIT);
    }
}

#[tokio::test]
async fn settings_recover_after_each_interrupted_persistence_boundary() -> TestResult {
    for stage in [
        "receipt-admitted",
        "temporary-synced",
        "file-renamed",
        "directory-synced",
        "receipt-committed",
    ] {
        let operation = OperationId::generate();
        let root = PathBuf::from(format!("/tmp/settings-crash-{}", operation.as_str()));
        std::fs::DirBuilder::new().mode(0o700).create(&root)?;
        let output = tokio::time::timeout(
            Duration::from_secs(15),
            tokio::process::Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "automation_settings_file::crash_tests::settings_crash_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env(STAGE_ENV, stage)
                .env(ROOT_ENV, &root)
                .env(OPERATION_ENV, operation.as_str())
                .kill_on_drop(true)
                .output(),
        )
        .await??;
        if output.status.code() != Some(CRASH_EXIT) {
            return Err(format!(
                "checkpoint {stage} did not exit at boundary: {:?}; {}; {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let store = Arc::new(Mutex::new(
            AutomationStore::open(&root.join("automation.sqlite")).await?,
        ));
        let before = store.lock().await.read_operation(&operation).await?;
        let file_exists = root.join("automation-settings.json").exists();
        if file_exists
            != matches!(
                stage,
                "file-renamed" | "directory-synced" | "receipt-committed"
            )
        {
            return Err(
                format!("checkpoint {stage} did not leave the expected file boundary").into(),
            );
        }
        if matches!(before.state, StoredOperationState::Succeeded { .. })
            != (stage == "receipt-committed")
        {
            return Err(
                format!("checkpoint {stage} did not leave the expected receipt boundary").into(),
            );
        }
        let handle = AutomationConfigurationHandle::default();
        let backend = AutomationSettingsFile::new(&root, Arc::clone(&store), handle.clone());
        backend
            .recover()
            .await
            .map_err(|error| format!("{stage}: {error:?}"))?;
        let request = request(operation.clone())?;
        let installed = std::fs::read(root.join("automation-settings.json"))?;
        let document: SettingsDocument = serde_json::from_slice(&installed)?;
        let after = store.lock().await.read_operation(&operation).await?;
        if document.operation_id != operation
            || document.configuration() != request.configuration()
            || handle.current().await != Some(request.configuration())
            || !matches!(after.state, StoredOperationState::Succeeded { .. })
        {
            return Err(format!("{stage}: recovery lost intended configuration or receipt").into());
        }
        let replay = backend
            .configure(request.clone())
            .await
            .map_err(|error| format!("{stage}: replay: {error:?}"))?;
        if replay != request.configuration()
            || std::fs::read(root.join("automation-settings.json"))? != installed
        {
            return Err(format!("{stage}: replay changed recovered settings").into());
        }
        drop(backend);
        drop(store);
        for entry in std::fs::read_dir(&root)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                return Err("unexpected settings fixture cleanup entry".into());
            }
            std::fs::remove_file(entry.path())?;
        }
        std::fs::remove_dir(root)?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "owned subprocess exits without cleanup at the parent's selected checkpoint"]
async fn settings_crash_child() -> TestResult {
    let root = PathBuf::from(std::env::var(ROOT_ENV)?);
    let operation: OperationId = std::env::var(OPERATION_ENV)?.try_into()?;
    if root.as_path() != Path::new(&format!("/tmp/settings-crash-{}", operation.as_str())) {
        return Err("refusing non-fixture settings root".into());
    }
    let store = Arc::new(Mutex::new(
        AutomationStore::open(&root.join("automation.sqlite")).await?,
    ));
    let backend =
        AutomationSettingsFile::new(&root, store, AutomationConfigurationHandle::default());
    backend
        .configure(request(operation)?)
        .await
        .map_err(|error| format!("configure before checkpoint: {error:?}"))?;
    Err("selected crash checkpoint was not reached".into())
}

fn request(
    operation_id: OperationId,
) -> Result<AutomationConfigureRequest, Box<dyn std::error::Error>> {
    Ok(AutomationConfigureRequest {
        operation_id,
        execution_timeout_seconds: 120.try_into()?,
        summary_timeout_seconds: 30.try_into()?,
    })
}
