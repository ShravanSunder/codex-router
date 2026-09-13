use crate::proof_context::ProofResult;
use collaboration_client::protocol::SessionRef;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{io::Write, os::unix::fs::OpenOptionsExt, path::Path};

pub(super) const STATE_FILENAME: &str = "messaging-skill-dx-state.json";

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MessagingProofState {
    pub prior_host_pid: u64,
    pub recipient: SessionRef,
    pub recipient_cwd: String,
    pub sender_cwd: String,
    pub skill_path: String,
    pub cli_path: String,
    pub service_directory: String,
    pub readiness_marker: String,
    pub message_marker: String,
    pub reply_marker: String,
}

pub(super) fn read(root: &Path) -> ProofResult<MessagingProofState> {
    Ok(serde_json::from_slice(&std::fs::read(
        root.join(STATE_FILENAME),
    )?)?)
}

pub(super) fn write(root: &Path, state: &MessagingProofState) -> ProofResult<()> {
    write_private_json(&root.join(STATE_FILENAME), &serde_json::to_value(state)?)
}

pub(super) fn host_pid(root: &Path) -> ProofResult<u64> {
    let marker: Value =
        serde_json::from_slice(&std::fs::read(root.join("debug-host-context.json"))?)?;
    marker
        .get("hostPid")
        .and_then(Value::as_u64)
        .filter(|host_pid| *host_pid > 0)
        .ok_or_else(|| "Debug Host marker omitted its process identity".into())
}

pub(super) fn write_private_json(path: &Path, value: &Value) -> ProofResult<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    writeln!(file)?;
    Ok(())
}
