use crate::native_probe_test_support::*;
use crate::{GroupStopStatus, GroupStopTiming, ImageLaunchOutcome};
use codex_native_integration::AppServerProbeAction;
use codex_router_descriptor_boundary::{DescriptorGate, PipeReader, PipeWriter};
use codex_router_keeper_protocol::{
    JsonMessage, NativeProbeJobWire, PipeFrameReader, ReceiverHelloWire,
};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
#[tokio::test]
async fn missing_partial_and_extra_job_refuse_before_native_peer_effect() -> TestResult {
    for mode in ["missing", "partial", "extra"] {
        let (root, registry, image) = setup().await?;
        let alias = root.path().join("gen-12345678-1.sock");
        let listener = tokio::net::UnixListener::bind(&alias)?;
        let (mut group, running_image) = match registry
            .native_probe_launcher(&image)
            .launch()
            .await?
        {
            ImageLaunchOutcome::Launched { group, image } => (group, image),
            ImageLaunchOutcome::Refused { reason } => return Err(reason.into()),
            ImageLaunchOutcome::CleanupPending {
                reason,
                mut cleanup,
                image,
            } => {
                let drained: TestResult = async {
                    cleanup.wait_for_cleanup(&CancellationToken::new()).await?;
                    let pid = cleanup.leader_pid().ok_or("failed launch PID missing")?;
                    let original_group = cleanup.original_process_group_id().ok_or("failed launch PGID missing")?;
                    if cleanup.leader_exit_status().is_none()
                        || rustix::process::test_kill_process(pid.as_pid()) != Err(rustix::io::Errno::SRCH)
                        || rustix::process::test_kill_process_group(original_group.as_pid()) != Err(rustix::io::Errno::SRCH)
                        || !matches!(rustix::process::waitpid(Some(pid.as_pid()), rustix::process::WaitOptions::NOHANG), Err(rustix::io::Errno::CHILD))
                    {
                        return Err("failed launch actual cleanup fence missing".into());
                    }
                    eprintln!("NATIVE_JOB_FAILED_LAUNCH_CLEANED pid={pid:?} original_group={original_group:?} stored={:?} original_refusal={reason:?} process=ESRCH group=ESRCH wait=ECHILD", cleanup.leader_exit_status());
                    Ok(())
                }.await;
                if let Err(cleanup_error) = drained {
                    return Err(DirectJobCleanupDebt {
                        ownership: ImageLaunchOutcome::CleanupPending {
                            reason,
                            cleanup,
                            image,
                        },
                        original_failure: None,
                        cleanup_error,
                        preserved_root: root.keep(),
                    }
                    .into());
                }
                return Err(reason.into());
            }
        };
        let pid = group.leader_pid();
        let original_group = group.process_group_id();
        let scenario: TestResult = async {
            let stdin = group
                .take_stdin()
                .ok_or("job pipe absent")?
                .into_owned_fd()?;
            let stdout = group
                .take_stdout()
                .ok_or("result pipe absent")?
                .into_owned_fd()?;
            let input = PipeWriter::from_owned(stdin, DescriptorGate::global()).await?;
            let mut output = PipeFrameReader::new(
                PipeReader::from_owned(stdout, DescriptorGate::global()).await?,
            );
            let hello = output
                .receive()
                .await?
                .ok_or("hello absent")?
                .decode::<ReceiverHelloWire>()?;
            if hello
                != (ReceiverHelloWire::Hello {
                    parent_role: codex_router_keeper_protocol::ComponentKind::Keeper,
                    fingerprint: image
                        .fingerprint(codex_router_keeper_protocol::ComponentKind::Keeper),
                })
            {
                return Err("literal receiver hello differs".into());
            }
            let job = NativeProbeJobWire::try_from(job(
                &alias,
                AppServerProbeAction::Observe,
                Duration::from_secs(1),
                Duration::from_secs(1),
            )?)?;
            if mode == "partial" {
                input.write_all(&100u32.to_be_bytes()).await?;
                input.write_all(b"{").await?;
            }
            if mode == "extra" {
                let bytes = JsonMessage::encode(&job)?;
                for _ in 0..2 {
                    input
                        .write_all(&u32::try_from(bytes.bytes().len())?.to_be_bytes())
                        .await?;
                    input.write_all(bytes.bytes()).await?;
                }
            }
            drop(input);
            if tokio::time::timeout(Duration::from_secs(2), output.receive())
                .await??
                .is_some()
            {
                return Err("invalid job produced a native result".into());
            }
            Ok(())
        }
        .await;
        eprintln!(
            "NATIVE_JOB_SCENARIO mode={mode} pid={pid:?} original_group={original_group:?} original_result={scenario:?}"
        );
        let drained: TestResult = async {
            group.begin_stop(GroupStopTiming::normal(), tokio::time::Instant::now())?;
            let status = group.wait_for_stop(&CancellationToken::new()).await?;
            if !matches!(status, GroupStopStatus::GroupEmpty { .. })
                || group.leader_exit_status().is_none()
                || rustix::process::test_kill_process(pid.as_pid()) != Err(rustix::io::Errno::SRCH)
                || rustix::process::test_kill_process_group(original_group.as_pid()) != Err(rustix::io::Errno::SRCH)
                || !matches!(rustix::process::waitpid(Some(pid.as_pid()), rustix::process::WaitOptions::NOHANG), Err(rustix::io::Errno::CHILD))
            {
                return Err("invalid job receiver not actually cleaned/reaped".into());
            }
            eprintln!("NATIVE_JOB_OWNER_FENCE mode={mode} pid={pid:?} original_group={original_group:?} stored={:?} process=ESRCH group=ESRCH wait=ECHILD", group.leader_exit_status());
            Ok(())
        }.await;
        if let Err(cleanup_error) = drained {
            return Err(DirectJobCleanupDebt {
                ownership: ImageLaunchOutcome::Launched {
                    group,
                    image: running_image,
                },
                original_failure: scenario.err(),
                cleanup_error,
                preserved_root: root.keep(),
            }
            .into());
        }
        scenario?;
        if tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_ok()
        {
            return Err("native peer effect occurred before exact job+EOF validation".into());
        }
        eprintln!(
            "NATIVE_JOB_REFUSAL mode={mode} pid={pid:?} native_peer=unaccepted result=absent exact_reap=true"
        );
    }
    Ok(())
}

#[derive(thiserror::Error)]
#[error(
    "direct job ownership debt preserved at {preserved_root:?}: original={original_failure:?}; cleanup={cleanup_error}"
)]
struct DirectJobCleanupDebt {
    ownership: ImageLaunchOutcome,
    original_failure: Option<Box<dyn std::error::Error + Send + Sync>>,
    #[source]
    cleanup_error: Box<dyn std::error::Error + Send + Sync>,
    preserved_root: std::path::PathBuf,
}
impl std::fmt::Debug for DirectJobCleanupDebt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = formatter.debug_struct("DirectJobCleanupDebt");
        debug
            .field("original_failure", &self.original_failure)
            .field("cleanup_error", &self.cleanup_error)
            .field("preserved_root", &self.preserved_root);
        match &self.ownership {
            ImageLaunchOutcome::Launched { group, image } => {
                debug
                    .field("pid", &group.leader_pid())
                    .field("original_group", &group.process_group_id())
                    .field("stored", &group.leader_exit_status())
                    .field("image", &image.image().retained_path());
            }
            ImageLaunchOutcome::CleanupPending {
                reason,
                cleanup,
                image,
            } => {
                debug
                    .field("original_refusal", reason)
                    .field("pid", &cleanup.leader_pid())
                    .field("original_group", &cleanup.original_process_group_id())
                    .field("stored", &cleanup.leader_exit_status())
                    .field("image", &image.image().retained_path());
            }
            ImageLaunchOutcome::Refused { reason } => {
                debug.field("refusal", reason);
            }
        }
        debug.finish()
    }
}
