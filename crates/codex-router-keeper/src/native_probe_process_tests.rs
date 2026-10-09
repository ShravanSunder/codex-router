use crate::native_probe_test_support::*;
use crate::{NativeProbeError, NativeProbeProcess};
use codex_native_integration::AppServerProbeAction;
use codex_router_keeper_protocol::NativeProbeResult;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
#[tokio::test]
async fn malformed_hello_result_extra_and_abnormal_exit_never_admit_observation() -> TestResult {
    for mode in [
        "missing-hello",
        "partial-hello",
        "oversized-hello",
        "invalid-hello",
        "wrong-hello",
        "missing-result",
        "partial-result",
        "oversized-result",
        "extra-result",
        "trailing-result",
        "nonzero",
        "signal",
    ] {
        let (root, mut registry, image) = setup().await?;
        let record = image.image().clone();
        let mut probe = process(&registry, &image, mode);
        drop(image);
        let job = job(
            &root.path().join("gen-12345678-1.sock"),
            AppServerProbeAction::Observe,
            Duration::from_secs(2),
            Duration::from_secs(1),
        )?;
        probe.start_job(job.clone())?;
        let result = probe.collect(&CancellationToken::new()).await;
        eprintln!(
            "NATIVE_PRODUCER_RESULT mode={mode} pid={:?} stored={:?} result={result:?}",
            probe.leader_pid(),
            probe.leader_exit_status()
        );
        use codex_router_descriptor_boundary::BoundaryError;
        let intended_refusal = matches!(
            (mode, &result),
            (
                "missing-hello" | "wrong-hello",
                Err(NativeProbeError::Hello)
            ) | (
                "missing-result" | "extra-result",
                Err(NativeProbeError::RecordCount)
            ) | (
                "partial-hello" | "partial-result" | "trailing-result",
                Err(NativeProbeError::Frame(BoundaryError::UnexpectedEof)),
            ) | (
                "oversized-hello" | "oversized-result",
                Err(NativeProbeError::Frame(BoundaryError::TooLarge)),
            ) | (
                "invalid-hello",
                Err(NativeProbeError::Frame(BoundaryError::Json(_)))
            ) | ("nonzero" | "signal", Err(NativeProbeError::AbnormalExit))
        );
        if !probe.cleanup_fenced() && !matches!(probe.start_job(job), Err(NativeProbeError::Busy)) {
            return Err("new job entered before cleanup".into());
        }
        if !registry.collect().await?.is_empty() || !record.retained_path().exists() {
            return Err("error lost image reference".into());
        }
        drain_owned_probe(&mut probe).await?;
        fences(&probe)?;
        use std::os::unix::process::ExitStatusExt;
        if mode == "nonzero"
            && probe.leader_exit_status().and_then(|status| status.code()) != Some(23)
        {
            return Err("nonzero producer did not preserve literal exit23".into());
        }
        if mode == "signal"
            && probe
                .leader_exit_status()
                .and_then(|status| status.signal())
                != Some(9)
        {
            return Err("signal producer did not preserve literal SIGKILL9".into());
        }
        if !intended_refusal {
            return Err(
                format!("producer {mode} did not reach its intended refusal: {result:?}").into(),
            );
        }
        eprintln!("NATIVE_PRODUCER_REFUSED mode={mode} cause={result:?}");
        drop(probe);
        if registry.collect().await? != vec![record] {
            return Err("fenced unreferenced image not collected".into());
        }
    }
    Ok(())
}
#[tokio::test]
async fn large_valid_result_and_excess_stderr_are_drained_before_normal_exit() -> TestResult {
    for mode in ["large-result", "large-stderr"] {
        let (root, registry, image) = setup().await?;
        let mut probe = process(&registry, &image, mode);
        let capacity_listener = if mode == "large-result" {
            Some(tokio::net::UnixListener::bind(
                root.path().join("gen-12345678-1.sock"),
            )?)
        } else {
            None
        };
        probe.start_job(job(
            &root.path().join("gen-12345678-1.sock"),
            AppServerProbeAction::Observe,
            Duration::from_secs(2),
            Duration::from_secs(1),
        )?)?;
        let outcome = if mode == "large-result" {
            collect_capacity_result(
                &mut probe,
                capacity_listener.ok_or("capacity listener absent")?,
            )
            .await?
        } else {
            probe.collect(&CancellationToken::new()).await?
        };
        let NativeProbeResult::Observed(observed) = outcome else {
            return Err("large fixture result failed".into());
        };
        if observed.running_version() != "0.160.1-fixture"
            || !probe
                .leader_exit_status()
                .is_some_and(|status| status.success())
        {
            return Err("literal result/normal exit differs".into());
        }
        if mode == "large-result"
            && !matches!(observed.remote_control(),codex_native_integration::RemoteControlObservation::Disabled {server_name,..} if server_name.len()==65_536)
        {
            return Err("large valid result was truncated".into());
        }
        if mode == "large-stderr"
            && (probe.stderr_bytes().len() != 4096
                || probe.stderr_bytes().iter().any(|byte| *byte != b'e'))
        {
            return Err("stderr drainage/capture bound differs".into());
        }
        fences(&probe)?;
    }
    Ok(())
}
#[tokio::test]
async fn valid_result_without_eof_or_with_live_group_stays_unadmitted() -> TestResult {
    for mode in ["missing-eof", "live-group"] {
        let (root, registry, image) = setup().await?;
        let mut probe = process(&registry, &image, mode);
        probe.start_job(job(
            &root.path().join("gen-12345678-1.sock"),
            AppServerProbeAction::Observe,
            Duration::from_millis(500),
            Duration::from_millis(500),
        )?)?;
        let refusal = probe.collect(&CancellationToken::new()).await;
        drain_owned_probe(&mut probe).await?;
        fences(&probe)?;
        if !matches!(refusal, Err(NativeProbeError::CollectionExpired)) {
            return Err(format!(
                "valid {mode} producer did not reach its collection fence: {refusal:?}"
            )
            .into());
        }
        if mode == "live-group"
            && !probe
                .leader_exit_status()
                .is_some_and(|status| status.success())
        {
            return Err("live descendant case did not preserve its normal leader exit".into());
        }
    }
    Ok(())
}
#[tokio::test]
async fn cancelled_queued_launch_retains_future_image_and_refuses_new_job_until_fenced()
-> TestResult {
    let (root, mut registry, image) = setup().await?;
    let record = image.image().clone();
    let mut probe = NativeProbeProcess::new(&registry, &image);
    drop(image);
    let job = job(
        &root.path().join("gen-12345678-1.sock"),
        AppServerProbeAction::Observe,
        Duration::from_secs(1),
        Duration::from_secs(1),
    )?;
    let gate = codex_router_descriptor_boundary::DescriptorGate::global()
        .spawn()
        .await;
    probe.start_job(job.clone())?;
    let cancel = CancellationToken::new();
    let mut collecting = Box::pin(probe.collect(&cancel));
    tokio::select! {biased;result=&mut collecting=>return Err(format!("launch unexpectedly settled before gate release: {result:?}").into()),()=tokio::task::yield_now()=>{}}
    drop(collecting);
    cancel.cancel();
    if !matches!(
        probe.collect(&cancel).await,
        Err(NativeProbeError::Cancelled)
    ) || !matches!(probe.start_job(job), Err(NativeProbeError::Busy))
    {
        return Err("queued launch cancellation lost ownership".into());
    }
    drop(gate);
    if !registry.collect().await?.is_empty() || !record.retained_path().exists() {
        return Err("queued launch image collected".into());
    }
    drain_owned_probe(&mut probe).await?;
    fences(&probe)?;
    Ok(())
}
#[tokio::test]
async fn expired_original_budget_cannot_succeed_after_delayed_launch() -> TestResult {
    let (root, registry, image) = setup().await?;
    let mut probe = NativeProbeProcess::new(&registry, &image);
    let gate = codex_router_descriptor_boundary::DescriptorGate::global()
        .spawn()
        .await;
    probe.start_job(job(
        &root.path().join("gen-12345678-1.sock"),
        AppServerProbeAction::Observe,
        Duration::from_millis(20),
        Duration::from_millis(100),
    )?)?;
    let result = probe.collect(&CancellationToken::new()).await;
    if !matches!(result, Err(NativeProbeError::NativeBudgetExpired)) {
        return Err(format!("original launch deadline differs: {result:?}").into());
    }
    drop(gate);
    drain_owned_probe(&mut probe).await?;
    fences(&probe)?;
    Ok(())
}

#[tokio::test]
async fn dropped_collect_then_cancel_during_real_pipe_backpressure_retains_cleanup() -> TestResult {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (root, mut registry, image) = setup().await?;
    let record = image.image().clone();
    let alias = root.path().join("gen-12345678-1.sock");
    let listener = tokio::net::UnixListener::bind(&alias)?;
    let mut probe = process(&registry, &image, "backpressure");
    drop(image);
    probe.start_job(job(
        &alias,
        AppServerProbeAction::Observe,
        Duration::from_secs(3),
        Duration::from_secs(3),
    )?)?;
    let scenario: TestResult = async {
        let token = CancellationToken::new();
        let mut collecting = Box::pin(probe.collect(&token));
        let (mut control, _) = tokio::select! {result=&mut collecting=>return Err(format!("probe settled before pressure fixture ready: {result:?}").into()),accepted=listener.accept()=>accepted?};
        let mut ready = [0; 5];
        tokio::select! {result=&mut collecting=>return Err(format!("probe settled before readiness: {result:?}").into()),read=control.read_exact(&mut ready)=>{read?;}}
        if &ready != b"READY" {
            return Err("pressure fixture not ready before writer permit".into());
        }
        drop(collecting);
        control.write_all(b"W").await?;
        let mut completed = [0; 4];
        if tokio::time::timeout(
            Duration::from_millis(50),
            control.read_exact(&mut completed),
        )
        .await
        .is_ok()
        {
            return Err("fixture write completed while parent pipe reader was not being polled".into());
        }
        token.cancel();
        if !matches!(
            probe.collect(&token).await,
            Err(NativeProbeError::Cancelled)
        ) {
            return Err("pressure cancellation was not retained failure".into());
        }
        if !registry.collect().await?.is_empty() || !record.retained_path().exists() {
            return Err("pressure cleanup lost image lease".into());
        }
        drain_owned_probe(&mut probe).await?;
        fences(&probe)?;
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

async fn collect_capacity_result(
    probe: &mut NativeProbeProcess,
    listener: tokio::net::UnixListener,
) -> Result<NativeProbeResult, Box<dyn std::error::Error + Send + Sync>> {
    let result = collect_capacity_inner(probe, listener).await;
    if result.is_err() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let _failure = probe.collect(&cancel).await;
        drain_owned_probe(probe).await?;
        fences(probe)?;
    }
    result
}

async fn collect_capacity_inner(
    probe: &mut NativeProbeProcess,
    listener: tokio::net::UnixListener,
) -> Result<NativeProbeResult, Box<dyn std::error::Error + Send + Sync>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let token = CancellationToken::new();
    let mut collecting = Box::pin(probe.collect(&token));
    let (mut control, _) = tokio::select! {
        result=&mut collecting=>return Err(format!("valid producer settled before READY: {result:?}").into()),
        accepted=listener.accept()=>accepted?,
    };
    let mut ready = [0; 5];
    tokio::select! {
        result=&mut collecting=>return Err(format!("valid producer settled before permit: {result:?}").into()),
        read=control.read_exact(&mut ready)=>{read?;},
    }
    if &ready != b"READY" {
        return Err("valid producer readiness differs".into());
    }
    drop(collecting);
    control.write_all(b"W").await?;
    let mut premature = [0];
    if tokio::time::timeout(Duration::from_millis(50), control.read(&mut premature))
        .await
        .is_ok()
    {
        return Err("valid result fit entirely in undrained kernel pipe".into());
    }
    let mut completed = [0; 4];
    let (outcome, completion) = tokio::join!(
        probe.collect(&token),
        tokio::time::timeout(Duration::from_secs(3), control.read_exact(&mut completed)),
    );
    completion??;
    let outcome = outcome?;
    if &completed != b"DONE" {
        return Err("valid producer completion differs".into());
    }
    eprintln!(
        "NATIVE_VALID_CAPACITY READY→permit→write-blocked-without-parent-read→resumed-DONE→result/EOF/normal-exit/group-empty"
    );
    Ok(outcome)
}
