use crate::native_probe_test_support::*;
use crate::{NativeProbeError, NativeProbeProcess};
use codex_native_integration::{AppServerProbeAction, RemoteControlObservation};
use codex_router_keeper_protocol::{NativeProbeFailure, NativeProbeResult, NativeProbeStage};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
async fn native_fixture(
    listener: tokio::net::UnixListener,
    action: AppServerProbeAction,
) -> Result<Vec<Value>, Box<dyn std::error::Error + Send + Sync>> {
    let (socket, _) = timeout(Duration::from_secs(5), listener.accept()).await??;
    let mut peer = tokio_tungstenite::accept_async(socket).await?;
    let mut transcript = Vec::new();
    let first = peer.next().await.ok_or("initialize absent")??;
    let initialize: Value = serde_json::from_str(first.to_text()?)?;
    let expected = json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"codex_router_host","title":"Codex Router Host","version":"0.1.63"},"capabilities":{"experimentalApi":true}}});
    if initialize != expected {
        return Err(format!("literal initialize request differs: {initialize}").into());
    }
    transcript.push(initialize);
    peer.send(Message::Text(
        r#"{"id":1,"result":{"userAgent":"codex/0.160.1-fixture native"}}"#.into(),
    ))
    .await?;
    let notification: Value =
        serde_json::from_str(peer.next().await.ok_or("initialized absent")??.to_text()?)?;
    if notification != json!({"method":"initialized"}) {
        return Err("literal initialized notification differs".into());
    }
    transcript.push(notification);
    let request: Value = serde_json::from_str(
        peer.next()
            .await
            .ok_or("remote request absent")??
            .to_text()?,
    )?;
    let expected = match action {
        AppServerProbeAction::EnableAndObserve => {
            json!({"id":2,"method":"remoteControl/enable","params":{"ephemeral":true}})
        }
        _ => json!({"id":2,"method":"remoteControl/status/read"}),
    };
    if request != expected {
        return Err(format!("literal remote request differs: {request}").into());
    }
    transcript.push(request);
    peer.send(Message::Text(r#"{"id":2,"result":{"status":"disabled","serverName":"SERVER_LITERAL","environmentId":null}}"#.into())).await?;
    if !matches!(peer.next().await, Some(Ok(Message::Close(_))) | None) {
        return Err("native codec did not close its exchange".into());
    }
    Ok(transcript)
}
async fn roundtrip(action: AppServerProbeAction, late_bind: bool) -> TestResult {
    let (root, registry, image) = setup().await?;
    let alias = root.path().join("gen-12345678-1.sock");
    let tracker = TaskTracker::new();
    let path = alias.clone();
    let bound = if late_bind {
        None
    } else {
        Some(tokio::net::UnixListener::bind(&alias)?)
    };
    let server = tracker.spawn(async move {
        let listener = match bound {
            Some(listener) => listener,
            None => {
                tokio::time::sleep(Duration::from_millis(80)).await;
                tokio::net::UnixListener::bind(path)?
            }
        };
        native_fixture(listener, action).await
    });
    let mut probe = NativeProbeProcess::new(&registry, &image);
    probe.start_job(job(
        &alias,
        action,
        Duration::from_secs(2),
        Duration::from_secs(1),
    )?)?;
    let result = probe.collect(&CancellationToken::new()).await;
    if result.is_err() {
        drain_owned_probe(&mut probe).await?;
    }
    let NativeProbeResult::Observed(observation) = result? else {
        return Err("native fixture did not yield observation".into());
    };
    if observation.running_version() != "0.160.1-fixture"
        || !matches!(observation.remote_control(),RemoteControlObservation::Disabled {server_name,environment_id} if server_name=="SERVER_LITERAL" && environment_id.is_none())
    {
        return Err("literal native observation differs".into());
    }
    fences(&probe)?;
    let transcript = timeout(Duration::from_secs(3), server).await???;
    tracker.close();
    tracker.wait().await;
    if transcript.len() != 3 {
        return Err("more than one receiver exchange/action admitted".into());
    }
    eprintln!(
        "NATIVE_REAL_EXCHANGE action={action:?} late_bind={late_bind} pid={:?} literal_requests={transcript:?}",
        probe.leader_pid()
    );
    Ok(())
}
#[tokio::test]
async fn retained_receiver_observe_reuses_native_codec_and_literal_status() -> TestResult {
    roundtrip(AppServerProbeAction::Observe, false).await
}
#[tokio::test]
async fn retained_receiver_enable_is_one_literal_ephemeral_request() -> TestResult {
    roundtrip(AppServerProbeAction::EnableAndObserve, false).await
}
#[tokio::test]
async fn wait_for_ready_keeps_one_process_for_connect_retry_budget() -> TestResult {
    roundtrip(AppServerProbeAction::WaitForReady, true).await
}
#[tokio::test]
async fn observe_missing_alias_reports_single_attempt_connect_failure() -> TestResult {
    let (root, registry, image) = setup().await?;
    let mut probe = NativeProbeProcess::new(&registry, &image);
    probe.start_job(job(
        &root.path().join("gen-12345678-1.sock"),
        AppServerProbeAction::Observe,
        Duration::from_secs(2),
        Duration::from_millis(100),
    )?)?;
    let outcome = probe.collect(&CancellationToken::new()).await;
    if outcome.is_err() {
        drain_owned_probe(&mut probe).await?;
        fences(&probe)?;
    }
    if outcome? != NativeProbeResult::Failed(NativeProbeFailure::Connect) {
        return Err("Observe missing alias was not a connect failure".into());
    }
    fences(&probe)
}
#[tokio::test]
async fn wait_ready_exhaustion_reports_connect_stage_within_original_window() -> TestResult {
    let (root, registry, image) = setup().await?;
    let mut probe = NativeProbeProcess::new(&registry, &image);
    let started = tokio::time::Instant::now();
    probe.start_job(job(
        &root.path().join("gen-12345678-1.sock"),
        AppServerProbeAction::WaitForReady,
        Duration::from_secs(2),
        Duration::from_millis(500),
    )?)?;
    let outcome = probe.collect(&CancellationToken::new()).await;
    if outcome.is_err() {
        drain_owned_probe(&mut probe).await?;
        fences(&probe)?;
    }
    let result = outcome?;
    if result
        != NativeProbeResult::Failed(NativeProbeFailure::Timeout {
            stage: NativeProbeStage::Connect,
        })
        || started.elapsed() > Duration::from_millis(2500)
    {
        return Err("connect retry reset/expanded the original collection budget".into());
    }
    fences(&probe)
}
#[tokio::test]
async fn wrong_compiled_fingerprint_refuses_before_any_native_peer_accept() -> TestResult {
    let (root, registry, image) = setup().await?;
    let alias = root.path().join("gen-12345678-1.sock");
    let listener = tokio::net::UnixListener::bind(&alias)?;
    let mut probe = process(&registry, &image, "wrong-compiled-fingerprint");
    probe.start_job(job(
        &alias,
        AppServerProbeAction::Observe,
        Duration::from_secs(1),
        Duration::from_secs(1),
    )?)?;
    let refusal = probe.collect(&CancellationToken::new()).await;
    eprintln!(
        "NATIVE_COMPILED_REFUSAL pid={:?} stored={:?} result={refusal:?}",
        probe.leader_pid(),
        probe.leader_exit_status()
    );
    drain_owned_probe(&mut probe).await?;
    fences(&probe)?;
    if !matches!(refusal, Err(NativeProbeError::Hello))
        || probe.leader_exit_status().and_then(|status| status.code()) != Some(1)
    {
        return Err(
            format!("compiled mismatch did not reach receiver refusal: {refusal:?}").into(),
        );
    }
    if timeout(Duration::from_millis(50), listener.accept())
        .await
        .is_ok()
    {
        return Err("wrong fingerprint reached Unix peer".into());
    }
    fences(&probe)
}
#[tokio::test]
async fn cancelled_pending_native_receipt_keeps_owner_then_stops_and_reaps() -> TestResult {
    let (root, mut registry, image) = setup().await?;
    let record = image.image().clone();
    let alias = root.path().join("gen-12345678-1.sock");
    let listener = tokio::net::UnixListener::bind(&alias)?;
    let mut probe = NativeProbeProcess::new(&registry, &image);
    drop(image);
    probe.start_job(job(
        &alias,
        AppServerProbeAction::Observe,
        Duration::from_secs(2),
        Duration::from_secs(1),
    )?)?;
    let scenario: TestResult = async {
        let cancel = CancellationToken::new();
        let mut collecting = Box::pin(probe.collect(&cancel));
        let (socket, _) = tokio::select! {result=&mut collecting=>return Err(format!("probe settled before native peer: {result:?}").into()),accepted=listener.accept()=>accepted?};
        cancel.cancel();
        if !matches!(collecting.await, Err(NativeProbeError::Cancelled)) {
            return Err("pending native receipt cancellation differs".into());
        }
        if !registry.collect().await?.is_empty() || !record.retained_path().exists() {
            return Err("pending native cleanup released image".into());
        }
        drain_owned_probe(&mut probe).await?;
        fences(&probe)?;
        drop(socket);
        Ok(())
    }.await;
    if scenario.is_err() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let _failure = probe.collect(&cancel).await;
        drain_owned_probe(&mut probe).await?;
        fences(&probe)?;
    }
    scenario
}
