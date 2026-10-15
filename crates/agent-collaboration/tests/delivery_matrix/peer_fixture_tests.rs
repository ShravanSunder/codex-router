//! Permanent real-Unix protocol and artifact-lifetime scenarios; no Host or provider.
use super::{PeerFixture, PeerReadStage};
use crate::proof_context::ProofResult;
use collaboration_client::protocol::EndpointRef;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use std::{
    io,
    os::unix::fs::DirBuilderExt as _,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::AsyncWriteExt as _;

const WAIT: Duration = Duration::from_secs(5);
const PARTIAL_FRAME_STOP_WAIT: Duration = Duration::from_secs(1);
const AUTH_FRAME: &[u8] = b"{\"type\":\"auth\",\"token\":\"0123456789abcdef0123456789abcdef\"}\n";

struct PrivateRegistry {
    _root: tempfile::TempDir,
    directory: PathBuf,
    socket: PathBuf,
    record: PathBuf,
    key: PathBuf,
}

impl PrivateRegistry {
    fn create() -> ProofResult<Self> {
        let root = tempfile::Builder::new()
            .prefix("u6-peer-lifetime-")
            .tempdir_in("/tmp")?;
        let directory = root.path().join("peer-registry");
        std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
        let socket = directory.join("peer.sock");
        let pid = std::process::id();
        let record = directory.join(format!("{pid}.json"));
        // The registry protocol names a key by PID and the socket-path SHA256.
        // This independent fixture path is not obtained from the cleanup subject.
        let socket_digest = Sha256::digest(socket.to_string_lossy().as_bytes());
        let key = directory.join(format!("{pid}.{socket_digest:x}.key"));
        Ok(Self {
            _root: root,
            directory,
            socket,
            record,
            key,
        })
    }

    fn require_owned_artifacts_absent(&self) -> ProofResult<()> {
        require_absent(&self.socket, "socket")?;
        require_absent(&self.record, "registry record")?;
        require_absent(&self.key, "token key")
    }
}

fn fixture_endpoint() -> ProofResult<EndpointRef> {
    Ok(serde_json::from_value(json!({
        "serviceId":"00000000-0000-4000-8000-000000000001",
        "endpointId":"codex-local",
    }))?)
}

fn require_absent(path: &Path, artifact: &str) -> ProofResult<()> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err(format!("owned peer {artifact} remains after settled shutdown").into()),
    }
}

async fn shutdown_and_reap(peer: PeerFixture) -> ProofResult<()> {
    let abort = peer.task.abort_handle();
    let shutdown = peer.shutdown();
    tokio::pin!(shutdown);
    match tokio::time::timeout(WAIT, &mut shutdown).await {
        Ok(result) => result,
        Err(_) => {
            // Failure containment only: a forced stop can never make this test pass.
            abort.abort();
            let _reaped = shutdown.await;
            Err("peer shutdown exceeded its bound; owned task aborted and reaped".into())
        }
    }
}

fn preserve_scenario_and_cleanup(
    scenario: ProofResult<()>,
    cleanup: ProofResult<()>,
) -> ProofResult<()> {
    match (scenario, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(scenario), Err(cleanup)) => {
            Err(format!("peer scenario failed: {scenario}; cleanup also failed: {cleanup}").into())
        }
    }
}

#[tokio::test]
async fn settled_shutdown_removes_owned_artifacts_and_preserves_sentinel() -> ProofResult<()> {
    let registry = PrivateRegistry::create()?;
    let sentinel = registry.directory.join("unrelated-sentinel");
    std::fs::write(&sentinel, b"leave this unrelated file intact")?;
    let peer = PeerFixture::start_in_registry(&registry.directory, &fixture_endpoint()?).await?;
    // This await must settle the actual listener task before the literal path oracle.
    shutdown_and_reap(peer).await?;
    if std::fs::read(&sentinel)? != b"leave this unrelated file intact" {
        return Err("peer shutdown changed the unrelated sentinel".into());
    }
    registry.require_owned_artifacts_absent()
}

#[tokio::test]
async fn authenticated_user_frames_reach_the_actual_receiver() -> ProofResult<()> {
    let registry = PrivateRegistry::create()?;
    let mut peer =
        PeerFixture::start_in_registry(&registry.directory, &fixture_endpoint()?).await?;
    let scenario: ProofResult<()> = async {
        if serde_json::to_value(&peer.target)? != json!({
            "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},
            "sessionId":"fixture-session",
        }) {
            return Err("peer target changed service, endpoint or session identity".into());
        }
        let mut stream = tokio::net::UnixStream::connect(&registry.socket).await?;
        stream.write_all(AUTH_FRAME).await?;
        stream.write_all(b"{\"type\":\"user\",\"message\":{\"content\":\"literal peer input\"}}\n").await?;
        stream.shutdown().await?;
        let observed = peer.expect_text_with_timeout("literal peer input", WAIT).await?;
        if observed != "literal peer input" {
            return Err("authenticated peer user content did not survive the Unix exchange".into());
        }
        let mut second = tokio::net::UnixStream::connect(&registry.socket).await?;
        second.write_all(AUTH_FRAME).await?;
        second.write_all(b"{\"type\":\"user\",\"message\":{\"content\":\"marker-only roundtrip\"}}\n").await?;
        second.shutdown().await?;
        peer.expect_marker("marker-only roundtrip").await?;
        Ok(())
    }.await;
    let cleanup = shutdown_and_reap(peer).await;
    preserve_scenario_and_cleanup(scenario, cleanup)
}

#[tokio::test]
async fn malformed_user_frame_preserves_error_and_unlinks_after_task_settlement() -> ProofResult<()>
{
    let registry = PrivateRegistry::create()?;
    let mut peer =
        PeerFixture::start_in_registry(&registry.directory, &fixture_endpoint()?).await?;
    let scenario: ProofResult<()> = async {
        let mut stream = tokio::net::UnixStream::connect(&registry.socket).await?;
        stream.write_all(AUTH_FRAME).await?;
        stream.write_all(b"not-json\n").await?;
        stream.shutdown().await?;
        if tokio::time::timeout(WAIT, peer.received.recv())
            .await?
            .is_some()
        {
            return Err("malformed user JSON reached the peer content receiver".into());
        }
        Ok(())
    }
    .await;
    let cleanup = shutdown_and_reap(peer).await;
    scenario?;
    match cleanup {
        Err(error) if error.downcast_ref::<serde_json::Error>().is_some() => {}
        Err(error) => return Err(error),
        Ok(()) => return Err("malformed JSON error was erased by peer shutdown".into()),
    }
    registry.require_owned_artifacts_absent()
}

async fn wait_for_user_frame_read(peer: &mut PeerFixture) -> ProofResult<()> {
    tokio::time::timeout(WAIT, async {
        loop {
            let stage = *peer.read_stage.borrow_and_update();
            if stage == PeerReadStage::ReadingUser {
                return Ok::<(), Box<dyn std::error::Error + Send + Sync>>(());
            }
            peer.read_stage.changed().await?;
        }
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn accepted_partial_user_frame_cancels_without_waiting_for_client_eof() -> ProofResult<()> {
    let registry = PrivateRegistry::create()?;
    let mut peer =
        PeerFixture::start_in_registry(&registry.directory, &fixture_endpoint()?).await?;
    let mut client = None;
    let ready = async {
        let mut stream = tokio::net::UnixStream::connect(&registry.socket).await?;
        stream.write_all(AUTH_FRAME).await?;
        stream
            .write_all(b"{\"type\":\"user\",\"message\":{\"content\":\"unfinished")
            .await?;
        client = Some(stream);
        wait_for_user_frame_read(&mut peer).await
    }
    .await;
    if let Err(error) = ready {
        drop(client);
        let cleanup = shutdown_and_reap(peer).await;
        return preserve_scenario_and_cleanup(Err(error), cleanup);
    }
    let abort = peer.task.abort_handle();
    let shutdown = peer.shutdown();
    tokio::pin!(shutdown);
    let cancellation = tokio::time::timeout(PARTIAL_FRAME_STOP_WAIT, &mut shutdown).await;
    // Always release EOF before returning, including the expected old-code failure.
    drop(client);
    match cancellation {
        Ok(result) => result?,
        Err(_) => {
            let settlement = tokio::time::timeout(WAIT, &mut shutdown).await;
            if settlement.is_err() {
                abort.abort();
                let _reaped = shutdown.await;
                return Err("partial-frame shutdown required forced containment after client EOF; task reaped".into());
            }
            return Err(
                "partial-frame cancellation required client EOF before task settlement".into(),
            );
        }
    }
    registry.require_owned_artifacts_absent()
}

#[tokio::test]
async fn record_creation_failure_after_bind_removes_only_created_socket() -> ProofResult<()> {
    let registry = PrivateRegistry::create()?;
    std::fs::create_dir(&registry.record)?;
    let sentinel = registry.record.join("sentinel");
    std::fs::write(&sentinel, b"preexisting record directory")?;
    let result = PeerFixture::start_in_registry(&registry.directory, &fixture_endpoint()?).await;
    let rejected = match result {
        Ok(peer) => {
            shutdown_and_reap(peer).await?;
            false
        }
        Err(_) => true,
    };
    if !rejected {
        return Err("peer startup unexpectedly accepted an occupied record path".into());
    }
    if std::fs::read(&sentinel)? != b"preexisting record directory" {
        return Err("partial start removed a preexisting record directory sentinel".into());
    }
    require_absent(&registry.socket, "socket")?;
    require_absent(&registry.key, "token key")
}

#[tokio::test]
async fn key_creation_failure_after_bind_removes_created_socket_and_record() -> ProofResult<()> {
    let registry = PrivateRegistry::create()?;
    std::fs::create_dir(&registry.key)?;
    let sentinel = registry.key.join("sentinel");
    std::fs::write(&sentinel, b"preexisting key directory")?;
    let result = PeerFixture::start_in_registry(&registry.directory, &fixture_endpoint()?).await;
    let rejected = match result {
        Ok(peer) => {
            shutdown_and_reap(peer).await?;
            false
        }
        Err(_) => true,
    };
    if !rejected {
        return Err("peer startup unexpectedly accepted an occupied key path".into());
    }
    if std::fs::read(&sentinel)? != b"preexisting key directory" {
        return Err("partial start removed a preexisting key directory sentinel".into());
    }
    require_absent(&registry.socket, "socket")?;
    require_absent(&registry.record, "registry record")
}

#[tokio::test]
async fn preexisting_record_file_is_preserved_and_not_adopted() -> ProofResult<()> {
    let registry = PrivateRegistry::create()?;
    std::fs::write(&registry.record, b"preexisting record file")?;
    let result = PeerFixture::start_in_registry(&registry.directory, &fixture_endpoint()?).await;
    let rejected = match result {
        Ok(peer) => {
            shutdown_and_reap(peer).await?;
            false
        }
        Err(_) => true,
    };
    if std::fs::read(&registry.record)? != b"preexisting record file" {
        return Err("peer startup overwrote a preexisting record sentinel".into());
    }
    if !rejected {
        return Err("peer startup adopted a preexisting record file".into());
    }
    require_absent(&registry.socket, "socket")?;
    require_absent(&registry.key, "token key")
}

#[tokio::test]
async fn preexisting_key_file_is_preserved_and_not_adopted() -> ProofResult<()> {
    let registry = PrivateRegistry::create()?;
    std::fs::write(&registry.key, b"preexisting key file")?;
    let result = PeerFixture::start_in_registry(&registry.directory, &fixture_endpoint()?).await;
    let rejected = match result {
        Ok(peer) => {
            shutdown_and_reap(peer).await?;
            false
        }
        Err(_) => true,
    };
    if std::fs::read(&registry.key)? != b"preexisting key file" {
        return Err("peer startup overwrote a preexisting key sentinel".into());
    }
    if !rejected {
        return Err("peer startup adopted a preexisting key file".into());
    }
    require_absent(&registry.socket, "socket")?;
    require_absent(&registry.record, "registry record")
}
