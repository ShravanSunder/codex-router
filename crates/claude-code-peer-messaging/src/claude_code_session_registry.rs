//! Read Claude Code's owner-local live-session registry by exact session identity.
use agent_automation::PeerProcessId;
use collaboration_protocol::SessionId;
use rustix::{io::Errno, process::Pid};
use serde::{Serialize, Serializer};
use serde_json::Value;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

const MAX_REGISTRY_RECORD_BYTES: u64 = 256 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PeerSessionStatus {
    Busy,
    Idle,
    Waiting,
    Shell,
    Unreported,
    Other(String),
}

impl Serialize for PeerSessionStatus {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let status = match self {
            Self::Busy => "busy",
            Self::Idle => "idle",
            Self::Waiting => "waiting",
            Self::Shell => "shell",
            Self::Unreported => "unreported",
            Self::Other(_) => "other",
        };
        serializer.serialize_str(status)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerSessionSummary {
    pub session_id: SessionId,
    pub name: Option<String>,
    pub cwd: PathBuf,
    pub status: PeerSessionStatus,
    pub started_at: i64,
    pub updated_at: i64,
    pub status_updated_at: Option<i64>,
    pub kind: String,
    pub entrypoint: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerSessionInventory {
    pub sessions: Vec<PeerSessionSummary>,
    pub skipped_records: u32,
    #[serde(skip)]
    candidates: Vec<PeerSessionCandidate>,
    #[serde(skip)]
    has_unreadable_live_record: bool,
}

struct PeerSessionCandidate {
    session_id: SessionId,
    lookup: PeerSessionLookup,
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

    pub fn live_sessions(&self) -> Result<PeerSessionInventory, PeerRegistryError> {
        self.live_sessions_with_probe(process_is_live, None)
    }

    fn lookup_with_probe(
        &self,
        target: &SessionId,
        is_process_live: impl FnMut(u32) -> Result<bool, PeerRegistryError>,
    ) -> Result<PeerSessionLookup, PeerRegistryError> {
        let inventory = self.live_sessions_with_probe(is_process_live, Some(target))?;
        let mut matched = None;
        for candidate in inventory.candidates {
            if candidate.session_id != *target {
                continue;
            }
            if matched.is_some() {
                return Ok(PeerSessionLookup::LiveUnsupported {
                    reason: "multiple live Claude Code registry records claim this session"
                        .to_owned(),
                });
            }
            matched = Some(candidate.lookup);
        }
        match matched {
            Some(candidate) => Ok(candidate),
            None if inventory.has_unreadable_live_record => Err(PeerRegistryError::LiveUnreadable),
            None => Ok(PeerSessionLookup::Absent),
        }
    }

    fn live_sessions_with_probe(
        &self,
        mut is_process_live: impl FnMut(u32) -> Result<bool, PeerRegistryError>,
        strict_target: Option<&SessionId>,
    ) -> Result<PeerSessionInventory, PeerRegistryError> {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(PeerSessionInventory {
                    sessions: Vec::new(),
                    skipped_records: 0,
                    candidates: Vec::new(),
                    has_unreadable_live_record: false,
                });
            }
            Err(error) => return Err(PeerRegistryError::Unavailable(error)),
        };
        let mut sessions = Vec::new();
        let mut candidates = Vec::new();
        let mut skipped_records = 0_u32;
        let mut has_unreadable_live_record = false;
        for entry in entries {
            let Ok(entry) = entry else {
                skipped_records = skipped_records.saturating_add(1);
                continue;
            };
            let Some(process_id) = process_id_from_filename(&entry.file_name()) else {
                continue;
            };
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                if record_unreadable_live_process(
                    process_id,
                    &mut is_process_live,
                    &mut has_unreadable_live_record,
                ) {
                    skipped_records = skipped_records.saturating_add(1);
                }
                continue;
            };
            if !metadata.file_type().is_file() || metadata.len() > MAX_REGISTRY_RECORD_BYTES {
                if record_unreadable_live_process(
                    process_id,
                    &mut is_process_live,
                    &mut has_unreadable_live_record,
                ) {
                    skipped_records = skipped_records.saturating_add(1);
                }
                continue;
            }
            let Ok(bytes) = fs::read(&path) else {
                if record_unreadable_live_process(
                    process_id,
                    &mut is_process_live,
                    &mut has_unreadable_live_record,
                ) {
                    skipped_records = skipped_records.saturating_add(1);
                }
                continue;
            };
            let Ok(envelope) = serde_json::from_slice::<Value>(&bytes) else {
                if record_unreadable_live_process(
                    process_id,
                    &mut is_process_live,
                    &mut has_unreadable_live_record,
                ) {
                    skipped_records = skipped_records.saturating_add(1);
                }
                continue;
            };
            let Some(session_id) = envelope
                .get("sessionId")
                .and_then(Value::as_str)
                .and_then(|value| SessionId::try_from(value.to_owned()).ok())
            else {
                if matches!(is_process_live(process_id), Ok(true)) {
                    skipped_records = skipped_records.saturating_add(1);
                }
                continue;
            };
            let is_live = match is_process_live(process_id) {
                Ok(is_live) => is_live,
                Err(error) if strict_target.is_some_and(|target| target == &session_id) => {
                    return Err(error);
                }
                Err(_) => {
                    skipped_records = skipped_records.saturating_add(1);
                    continue;
                }
            };
            if !is_live {
                continue;
            }
            let lookup = decode_record(&session_id, process_id, &envelope);
            if matches!(lookup, PeerSessionLookup::Writable(_)) {
                if let Some(summary) = decode_summary(&session_id, &envelope) {
                    sessions.push(summary);
                } else {
                    skipped_records = skipped_records.saturating_add(1);
                }
            } else {
                skipped_records = skipped_records.saturating_add(1);
            }
            candidates.push(PeerSessionCandidate { session_id, lookup });
        }
        Ok(PeerSessionInventory {
            sessions,
            skipped_records,
            candidates,
            has_unreadable_live_record,
        })
    }

    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

fn decode_summary(session_id: &SessionId, envelope: &Value) -> Option<PeerSessionSummary> {
    let cwd = envelope.get("cwd")?.as_str()?;
    if !Path::new(cwd).is_absolute() {
        return None;
    }
    Some(PeerSessionSummary {
        session_id: session_id.clone(),
        name: envelope
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_owned),
        cwd: PathBuf::from(cwd),
        status: decode_status(envelope.get("status")),
        started_at: envelope.get("startedAt")?.as_i64()?,
        updated_at: envelope.get("updatedAt")?.as_i64()?,
        status_updated_at: envelope.get("statusUpdatedAt").and_then(Value::as_i64),
        kind: envelope.get("kind")?.as_str()?.to_owned(),
        entrypoint: envelope.get("entrypoint")?.as_str()?.to_owned(),
    })
}

fn decode_status(value: Option<&Value>) -> PeerSessionStatus {
    match value {
        None => PeerSessionStatus::Unreported,
        Some(Value::String(status)) => match status.as_str() {
            "busy" => PeerSessionStatus::Busy,
            "idle" => PeerSessionStatus::Idle,
            "waiting" => PeerSessionStatus::Waiting,
            "shell" => PeerSessionStatus::Shell,
            _ => PeerSessionStatus::Other(status.clone()),
        },
        Some(status) => PeerSessionStatus::Other(status.to_string()),
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
) -> bool {
    if matches!(is_process_live(process_id), Ok(true)) {
        *has_unreadable_live_record = true;
        true
    } else {
        false
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
    let status = decode_status(envelope.get("status"));
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
