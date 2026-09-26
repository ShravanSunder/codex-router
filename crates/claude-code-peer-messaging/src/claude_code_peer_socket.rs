//! Bounded newline-framed writes to a Claude Code peer socket.
use crate::PeerSessionRecord;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{
    fs,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{io::AsyncWriteExt as _, net::UnixStream};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_KEY_BYTES: u64 = 4096;
const MAX_USER_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PeerSocketWriteOutcome {
    Written,
    NotSubmitted { reason: &'static str },
    Unknown { reason: &'static str },
}

pub struct ClaudeCodePeerSocket {
    registry_directory: PathBuf,
}

impl ClaudeCodePeerSocket {
    #[must_use]
    pub fn new(registry_directory: PathBuf) -> Self {
        Self { registry_directory }
    }

    pub async fn write_user_message(
        &self,
        peer: &PeerSessionRecord,
        message: &str,
    ) -> PeerSocketWriteOutcome {
        let Some(token) = read_published_peer_token(&self.registry_directory, peer) else {
            return PeerSocketWriteOutcome::NotSubmitted {
                reason: "peer authentication key unavailable",
            };
        };
        let mut auth_line = match serde_json::to_vec(&json!({"type":"auth","token":token})) {
            Ok(line) => line,
            Err(_) => {
                return PeerSocketWriteOutcome::NotSubmitted {
                    reason: "peer auth frame could not be encoded",
                };
            }
        };
        auth_line.push(b'\n');
        let mut user_line = match serde_json::to_vec(
            &json!({"type":"user","message":{"role":"user","content":message}}),
        ) {
            Ok(line) if line.len() < MAX_USER_FRAME_BYTES => line,
            _ => {
                return PeerSocketWriteOutcome::NotSubmitted {
                    reason: "peer message exceeds the frame limit",
                };
            }
        };
        user_line.push(b'\n');
        let connection =
            tokio::time::timeout(CONNECT_TIMEOUT, UnixStream::connect(&peer.socket_path)).await;
        let mut stream = match connection {
            Ok(Ok(stream)) => stream,
            _ => {
                return PeerSocketWriteOutcome::NotSubmitted {
                    reason: "peer socket could not be connected",
                };
            }
        };
        let mut bytes_written = 0_usize;
        for frame in [&auth_line, &user_line] {
            let mut remaining = frame.as_slice();
            while !remaining.is_empty() {
                let write = tokio::time::timeout(WRITE_TIMEOUT, stream.write(remaining)).await;
                match write {
                    Ok(Ok(0)) | Ok(Err(_)) | Err(_) => {
                        return write_failure(bytes_written);
                    }
                    Ok(Ok(count)) => {
                        let Some(rest) = remaining.get(count..) else {
                            return write_failure(bytes_written);
                        };
                        bytes_written = bytes_written.saturating_add(count);
                        remaining = rest;
                    }
                }
            }
        }
        PeerSocketWriteOutcome::Written
    }
}

fn write_failure(bytes_written: usize) -> PeerSocketWriteOutcome {
    if bytes_written == 0 {
        PeerSocketWriteOutcome::NotSubmitted {
            reason: "peer socket write failed before any bytes",
        }
    } else {
        PeerSocketWriteOutcome::Unknown {
            reason: "peer socket write ended after some bytes",
        }
    }
}

fn read_published_peer_token(directory: &Path, peer: &PeerSessionRecord) -> Option<String> {
    let socket_path = peer.socket_path.to_str()?;
    if !Path::new(socket_path).is_absolute() {
        return None;
    }
    let digest = Sha256::digest(socket_path.as_bytes());
    let process_id = u32::from(peer.process_id);
    let key_path = directory.join(format!("{process_id}.{digest:x}.key"));
    let metadata = fs::symlink_metadata(&key_path).ok()?;
    if !metadata.file_type().is_file()
        || metadata.len() > MAX_KEY_BYTES
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.uid() != rustix::process::geteuid().as_raw()
    {
        return None;
    }
    let bytes = fs::read(&key_path).ok()?;
    let envelope: Value = serde_json::from_slice(&bytes).ok()?;
    let token = envelope.get("peerToken")?.as_str()?;
    if token.len() != 32 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(token.to_owned())
}
