//! A lost service process cannot turn a replayed creation into a duplicate wake.
use super::serve_control_connection;
use automation_storage::AutomationStore;
use communication_client::ControlClient;
use communication_protocol::{AutomationPageRequest, OperationId, WakeSendRequest};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

const STAGE_ENV: &str = "WAKE_CREATION_TEST_CRASH_STAGE";
const ROOT_ENV: &str = "WAKE_CREATION_TEST_CRASH_ROOT";
const OPERATION_ENV: &str = "WAKE_CREATION_TEST_CRASH_OPERATION";
const SERVICE: &str = "00000000-0000-4000-8000-000000000001";
const CRASH_EXIT: i32 = 92;
type TestResult<TValue> = Result<TValue, Box<dyn std::error::Error>>;

pub(super) fn checkpoint(stage: &str, method: &str) {
    if method == "wake/send" && std::env::var(STAGE_ENV).as_deref() == Ok(stage) {
        std::process::exit(CRASH_EXIT);
    }
}

#[tokio::test]
async fn wake_creation_replay_survives_service_exit_before_response() -> TestResult<()> {
    for stage in ["before-dispatch", "after-dispatch"] {
        let operation = OperationId::generate();
        let root = PathBuf::from(format!("/tmp/wake-crash-{}", operation.as_str()));
        std::fs::create_dir(&root)?;
        let output = tokio::time::timeout(
            Duration::from_secs(15),
            tokio::process::Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "control_connection::wake_creation_crash_tests::wake_crash_child",
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
                "{stage}: child did not reach crash boundary: {:?}; {}; {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let (mut client, server) = connect(&root, "00000000-0000-4000-8000-000000000003").await?;
        let before = client.list_wakeups(page()?).await?;
        let expected_count = usize::from(stage == "after-dispatch");
        if before.records.len() != expected_count {
            return Err(
                format!("{stage}: incorrect committed creation count before replay").into(),
            );
        }
        let first = client.send_wakeup(request(operation.clone())?).await?;
        let second = client.send_wakeup(request(operation)?).await?;
        let after = client.list_wakeups(page()?).await?;
        if first.definition.wakeup_id != second.definition.wakeup_id
            || after.records.len() != 1
            || first.first_fire.is_some()
            || first.pending_delivery_id.is_some()
            || before
                .records
                .first()
                .is_some_and(|old| old.definition.wakeup_id != first.definition.wakeup_id)
        {
            return Err(
                format!("{stage}: replay duplicated creation or invented native delivery").into(),
            );
        }
        client.close().await?;
        server.await??;
        for entry in std::fs::read_dir(&root)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                return Err("unexpected fixture cleanup entry".into());
            }
            std::fs::remove_file(entry.path())?;
        }
        std::fs::remove_dir(root)?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "owned subprocess exits at a creation dispatch boundary without cleanup"]
async fn wake_crash_child() -> TestResult<()> {
    let operation: OperationId = std::env::var(OPERATION_ENV)?.try_into()?;
    let root = PathBuf::from(std::env::var(ROOT_ENV)?);
    if root.as_path() != Path::new(&format!("/tmp/wake-crash-{}", operation.as_str())) {
        return Err("refusing non-fixture creation root".into());
    }
    let (mut client, _server) = connect(&root, "00000000-0000-4000-8000-000000000002").await?;
    client.send_wakeup(request(operation)?).await?;
    Err("creation did not stop at selected checkpoint".into())
}

async fn connect(
    root: &Path,
    epoch: &str,
) -> TestResult<(ControlClient, tokio::task::JoinHandle<std::io::Result<()>>)> {
    let store = Arc::new(tokio::sync::Mutex::new(
        AutomationStore::open(&root.join("automation.sqlite")).await?,
    ));
    let identity =
        crate::ServiceIdentity::new(SERVICE, epoch, &format!("sha256:{}", "a".repeat(64)))
            .map_err(std::io::Error::other)?
            .with_automation_store(store);
    let (client, server) = tokio::net::UnixStream::pair()?;
    let server = tokio::spawn(serve_control_connection(server, identity));
    Ok((
        ControlClient::initialize(client, "wake-crash-proof", "1").await?,
        server,
    ))
}

fn page() -> TestResult<AutomationPageRequest> {
    Ok(AutomationPageRequest {
        cursor: None,
        limit: 50.try_into()?,
    })
}
fn request(operation_id: OperationId) -> TestResult<WakeSendRequest> {
    Ok(serde_json::from_value(json!({
        "operationId":operation_id,
        "message":{"target":{"endpoint":{"serviceId":SERVICE,"endpointId":"codex-local"},"sessionId":"fixture"},
            "content":{"kind":"humanUser","text":"Reminder"},"delivery":"auto","generationGuard":null},
        "timing":{"kind":"after","seconds":600},"expiry":{"kind":"none"}
    }))?)
}
