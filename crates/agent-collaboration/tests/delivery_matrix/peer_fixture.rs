//! Owned scripted peer socket lifetime for the opt-in delivery matrix.
use super::proof_context::{ProofContext, ProofResult};
use collaboration_client::protocol::{EndpointId, EndpointRef, SessionId, SessionRef};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{
    io::{self, Write as _},
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::{AsyncBufReadExt as _, BufReader};

const PEER_TOKEN: &str = "0123456789abcdef0123456789abcdef";

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PeerReadStage {
    WaitingForAccept,
    ReadingAuthentication,
    ReadingUser,
}

pub(super) struct PeerFixture {
    pub(super) target: SessionRef,
    received: tokio::sync::mpsc::Receiver<String>,
    stop: tokio_util::sync::CancellationToken,
    task: tokio::task::JoinHandle<ProofResult<()>>,
    created_artifacts: Vec<PathBuf>,
    #[cfg(test)]
    read_stage: tokio::sync::watch::Receiver<PeerReadStage>,
}

impl PeerFixture {
    pub(super) async fn start(proof: &ProofContext) -> ProofResult<Self> {
        let registry = super::delivery_matrix_support::selected_peer_registry(&proof.root)?;
        Self::start_in_registry(&registry, &proof.endpoint).await
    }

    pub(super) async fn start_in_registry(
        registry: &Path,
        endpoint: &EndpointRef,
    ) -> ProofResult<Self> {
        let mut target = SessionRef {
            endpoint: endpoint.clone(),
            session_id: SessionId::try_from("fixture-session".to_owned())?,
        };
        target.endpoint.endpoint_id = EndpointId::try_from("claude-local".to_owned())?;
        if !registry.is_dir() || std::fs::metadata(registry)?.permissions().mode() & 0o077 != 0 {
            return Err("Debug Host did not enable a private fixture peer registry".into());
        }
        let mut created_artifacts = Vec::new();
        let prepared: ProofResult<tokio::net::UnixListener> = (|| {
            let socket = registry.join("peer.sock");
            let listener = tokio::net::UnixListener::bind(&socket)?;
            created_artifacts.push(socket.clone());
            let pid = std::process::id();
            let record = registry.join(format!("{pid}.json"));
            let mut record_file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&record)?;
            created_artifacts.push(record);
            record_file.write_all(
                json!({
                    "pid":pid,"sessionId":"fixture-session","peerProtocol":1,
                    "messagingSocketPath":socket,"status":"busy"
                })
                .to_string()
                .as_bytes(),
            )?;
            let digest = Sha256::digest(socket.to_string_lossy().as_bytes());
            let key = registry.join(format!("{pid}.{digest:x}.key"));
            let mut key_file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&key)?;
            created_artifacts.push(key);
            key_file.write_all(json!({"peerToken":PEER_TOKEN}).to_string().as_bytes())?;
            Ok(listener)
        })();
        let listener = match prepared {
            Ok(listener) => listener,
            Err(error) => {
                return match remove_created_artifacts(&created_artifacts) {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(io::Error::other(format!(
                        "peer startup failed: {error}; peer artifact cleanup failed: {cleanup}"
                    ))
                    .into()),
                };
            }
        };
        let (transmit, received) = tokio::sync::mpsc::channel(16);
        let stop = tokio_util::sync::CancellationToken::new();
        let task_stop = stop.clone();
        #[cfg(test)]
        let (read_stage_transmit, read_stage) =
            tokio::sync::watch::channel(PeerReadStage::WaitingForAccept);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = task_stop.cancelled() => return Ok(()),
                    accepted = listener.accept() => {
                        let (stream, _) = accepted?;
                        tokio::select! {
                            () = task_stop.cancelled() => return Ok(()),
                            received = async {
                                let mut lines = BufReader::new(stream).lines();
                                #[cfg(test)]
                                let _observed = read_stage_transmit.send(PeerReadStage::ReadingAuthentication);
                                let auth: Value = serde_json::from_str(
                                    &lines.next_line().await?.ok_or("peer auth missing")?,
                                )?;
                                if auth["type"] != "auth" || auth["token"] != PEER_TOKEN {
                                    return Err("peer fixture received invalid authentication".into());
                                }
                                #[cfg(test)]
                                let _observed = read_stage_transmit.send(PeerReadStage::ReadingUser);
                                let user: Value = serde_json::from_str(&lines.next_line().await?.ok_or("peer user frame missing")?)?;
                                let text = user["message"]["content"].as_str().ok_or("peer text missing")?.to_owned();
                                transmit.send(text).await.map_err(|_| "peer receiver closed")?;
                                Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
                            } => received?,
                        }
                    }
                }
            }
        });
        Ok(Self {
            target,
            received,
            stop,
            task,
            created_artifacts,
            #[cfg(test)]
            read_stage,
        })
    }
    pub(super) async fn expect_marker(&mut self, marker: &str) -> ProofResult<()> {
        self.expect_marker_with_timeout(marker, Duration::from_secs(30))
            .await
    }
    pub(super) async fn expect_marker_with_timeout(
        &mut self,
        marker: &str,
        timeout: Duration,
    ) -> ProofResult<()> {
        self.expect_text_with_timeout(marker, timeout).await?;
        Ok(())
    }
    pub(super) async fn expect_text_with_timeout(
        &mut self,
        marker: &str,
        timeout: Duration,
    ) -> ProofResult<String> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let received = tokio::time::timeout_at(deadline, self.received.recv())
                .await?
                .ok_or("peer fixture stopped before message")?;
            if super::user_text_contains_marker(&received, marker) {
                return Ok(received);
            }
        }
    }
    pub(super) async fn shutdown(self) -> ProofResult<()> {
        let Self {
            received,
            stop,
            task,
            created_artifacts,
            ..
        } = self;
        stop.cancel();
        let task_result: ProofResult<()> = match task.await {
            Ok(result) => result,
            Err(error) => Err(error.into()),
        };
        // Keep the content receiver alive until the sending task has settled.
        drop(received);
        let cleanup_result = remove_created_artifacts(&created_artifacts);
        match (task_result, cleanup_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(error)) => Err(error.into()),
            (Err(task), Err(cleanup)) => Err(io::Error::other(format!(
                "peer task failed: {task}; peer artifact cleanup failed: {cleanup}"
            ))
            .into()),
        }
    }
}

fn remove_created_artifacts(created_artifacts: &[PathBuf]) -> io::Result<()> {
    let mut failure: Option<io::Error> = None;
    for path in created_artifacts.iter().rev() {
        if let Err(error) = std::fs::remove_file(path)
            && error.kind() != io::ErrorKind::NotFound
        {
            failure = Some(match failure {
                None => error,
                Some(previous) => io::Error::other(format!("{previous}; {error}")),
            });
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
#[path = "peer_fixture_tests.rs"]
mod tests;
