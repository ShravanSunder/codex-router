//! Read Claude Code's owner-local live-session registry by exact session identity.
use agent_automation::PeerProcessId;
use collaboration_protocol::SessionId;
use rustix::{io::Errno, process::Pid};
use serde_json::Value;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

const MAX_REGISTRY_RECORD_BYTES: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerSessionStatus {
    Busy,
    Idle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerSessionRecord {
    pub session_id: SessionId,
    pub process_id: PeerProcessId,
    pub status: PeerSessionStatus,
    pub socket_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PeerSessionLookup {
    Absent,
    Writable(PeerSessionRecord),
    LiveUnsupported { reason: String },
}

#[derive(Debug, thiserror::Error)]
pub enum PeerRegistryError {
    #[error("Claude Code session registry could not be read: {0}")]
    Unavailable(#[source] io::Error),
    #[error("Claude Code process liveness probe failed for PID {process_id}: {source}")]
    LivenessProbe { process_id: u32, source: Errno },
    #[error("Claude Code session registry contains a live record whose identity could not be read")]
    LiveUnreadable,
}

#[derive(Clone, Debug)]
pub struct ClaudeCodeSessionRegistry {
    directory: PathBuf,
}

impl ClaudeCodeSessionRegistry {
    #[must_use]
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    pub fn lookup(&self, target: &SessionId) -> Result<PeerSessionLookup, PeerRegistryError> {
        self.lookup_with_probe(target, process_is_live)
    }

    fn lookup_with_probe(
        &self,
        target: &SessionId,
        mut is_process_live: impl FnMut(u32) -> Result<bool, PeerRegistryError>,
    ) -> Result<PeerSessionLookup, PeerRegistryError> {
        let target_id = String::from(target.clone());
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(PeerSessionLookup::Absent);
            }
            Err(error) => return Err(PeerRegistryError::Unavailable(error)),
        };
        let mut matched = None;
        let mut has_unreadable_live_record = false;
        for entry in entries {
            let Ok(entry) = entry else {
                continue;
            };
            let Some(process_id) = process_id_from_filename(&entry.file_name()) else {
                continue;
            };
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                record_unreadable_live_process(
                    process_id,
                    &mut is_process_live,
                    &mut has_unreadable_live_record,
                );
                continue;
            };
            if !metadata.file_type().is_file() || metadata.len() > MAX_REGISTRY_RECORD_BYTES {
                record_unreadable_live_process(
                    process_id,
                    &mut is_process_live,
                    &mut has_unreadable_live_record,
                );
                continue;
            }
            let Ok(bytes) = fs::read(&path) else {
                record_unreadable_live_process(
                    process_id,
                    &mut is_process_live,
                    &mut has_unreadable_live_record,
                );
                continue;
            };
            let Ok(envelope) = serde_json::from_slice::<Value>(&bytes) else {
                record_unreadable_live_process(
                    process_id,
                    &mut is_process_live,
                    &mut has_unreadable_live_record,
                );
                continue;
            };
            if envelope.get("sessionId").and_then(Value::as_str) != Some(target_id.as_str()) {
                continue;
            }
            if !is_process_live(process_id)? {
                continue;
            }
            let candidate = decode_record(target, process_id, &envelope);
            if matched.is_some() {
                return Ok(PeerSessionLookup::LiveUnsupported {
                    reason: "multiple live Claude Code registry records claim this session"
                        .to_owned(),
                });
            }
            matched = Some(candidate);
        }
        match matched {
            Some(candidate) => Ok(candidate),
            None if has_unreadable_live_record => Err(PeerRegistryError::LiveUnreadable),
            None => Ok(PeerSessionLookup::Absent),
        }
    }

    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

fn process_id_from_filename(filename: &std::ffi::OsStr) -> Option<u32> {
    let name = filename.to_str()?;
    let stem = name.strip_suffix(".json")?;
    if stem.is_empty() || !stem.bytes().all(|byte| byte.is_ascii_digit()) || stem.starts_with('0') {
        return None;
    }
    stem.parse().ok()
}

fn process_is_live(process_id: u32) -> Result<bool, PeerRegistryError> {
    let Some(pid) = i32::try_from(process_id).ok().and_then(Pid::from_raw) else {
        return Ok(false);
    };
    match rustix::process::test_kill_process(pid) {
        Ok(()) | Err(Errno::PERM) => Ok(true),
        Err(Errno::SRCH) => Ok(false),
        Err(source) => Err(PeerRegistryError::LivenessProbe { process_id, source }),
    }
}

fn record_unreadable_live_process(
    process_id: u32,
    is_process_live: &mut impl FnMut(u32) -> Result<bool, PeerRegistryError>,
    has_unreadable_live_record: &mut bool,
) {
    if matches!(is_process_live(process_id), Ok(true)) {
        *has_unreadable_live_record = true;
    }
}

fn decode_record(target: &SessionId, filename_pid: u32, envelope: &Value) -> PeerSessionLookup {
    let Some(stored_pid) = envelope
        .get("pid")
        .and_then(Value::as_u64)
        .and_then(|pid| u32::try_from(pid).ok())
    else {
        return unsupported("live registry record has no valid process identity");
    };
    if stored_pid != filename_pid {
        return unsupported("live registry process identity differs from its filename");
    }
    let Some(protocol) = envelope.get("peerProtocol").and_then(Value::as_u64) else {
        return unsupported("live registry record has no peer protocol version");
    };
    if protocol != 1 {
        return unsupported(&format!(
            "live registry peer protocol {protocol} is unsupported"
        ));
    }
    let status = match envelope.get("status").and_then(Value::as_str) {
        Some("busy") => PeerSessionStatus::Busy,
        Some("idle") => PeerSessionStatus::Idle,
        _ => return unsupported("live registry session status is unsupported"),
    };
    let Some(socket_path) = envelope.get("messagingSocketPath").and_then(Value::as_str) else {
        return unsupported("live registry record has no peer socket path");
    };
    if socket_path.contains('\0') || !Path::new(socket_path).is_absolute() {
        return unsupported("live registry peer socket path is invalid");
    }
    let Ok(process_id) = PeerProcessId::try_from(stored_pid) else {
        return unsupported("live registry process identity is invalid");
    };
    PeerSessionLookup::Writable(PeerSessionRecord {
        session_id: target.clone(),
        process_id,
        status,
        socket_path: PathBuf::from(socket_path),
    })
}

fn unsupported(reason: &str) -> PeerSessionLookup {
    PeerSessionLookup::LiveUnsupported {
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{ClaudeCodeSessionRegistry, PeerRegistryError};
    use collaboration_protocol::SessionId;
    use rustix::io::Errno;
    use serde_json::json;

    #[test]
    fn liveness_probe_error_is_preserved_for_the_target() {
        let root = tempfile::tempdir().expect("registry directory");
        let process_id = std::process::id();
        let session_id = SessionId::try_from("fixture-target".to_owned()).expect("session ID");
        std::fs::write(
            root.path().join(format!("{process_id}.json")),
            json!({
                "pid": process_id,
                "sessionId": "fixture-target",
                "status": "busy",
                "peerProtocol": 1,
                "messagingSocketPath": "/private/tmp/peer.sock"
            })
            .to_string(),
        )
        .expect("target record");
        let registry = ClaudeCodeSessionRegistry::new(root.path().to_owned());

        let result = registry.lookup_with_probe(&session_id, |target_pid| {
            Err(PeerRegistryError::LivenessProbe {
                process_id: target_pid,
                source: Errno::IO,
            })
        });

        assert!(matches!(
            result,
            Err(PeerRegistryError::LivenessProbe {
                process_id: pid,
                source: Errno::IO
            }) if pid == process_id
        ));
    }
}
