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
    #[error("Claude Code session registry could not be read")]
    Unavailable(#[source] io::Error),
    #[error("Claude Code session registry contains a live unreadable record")]
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
        let target_id = String::from(target.clone());
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(PeerSessionLookup::Absent);
            }
            Err(error) => return Err(PeerRegistryError::Unavailable(error)),
        };
        let mut matched = None;
        for entry in entries {
            let entry = entry.map_err(PeerRegistryError::Unavailable)?;
            let Some(process_id) = process_id_from_filename(&entry.file_name()) else {
                continue;
            };
            if !process_is_live(process_id)? {
                continue;
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(PeerRegistryError::Unavailable)?;
            if !metadata.file_type().is_file() || metadata.len() > MAX_REGISTRY_RECORD_BYTES {
                return Err(PeerRegistryError::LiveUnreadable);
            }
            let bytes = fs::read(&path).map_err(PeerRegistryError::Unavailable)?;
            let envelope: Value =
                serde_json::from_slice(&bytes).map_err(|_| PeerRegistryError::LiveUnreadable)?;
            if envelope.get("sessionId").and_then(Value::as_str) != Some(target_id.as_str()) {
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
        Ok(matched.unwrap_or(PeerSessionLookup::Absent))
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
        Err(_) => Err(PeerRegistryError::LiveUnreadable),
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
        return unsupported("live registry peer protocol is unsupported");
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
