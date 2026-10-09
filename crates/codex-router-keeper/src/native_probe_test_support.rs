use crate::{ImageLease, ImageRegistry, NativeProbeProcess};
use codex_native_integration::AppServerProbeAction;
use codex_router_keeper_protocol::{
    BuildInfo, ComponentFingerprint, ComponentFingerprints, GenerationAliasPath, NativeProbeJob,
};
use std::{
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};
pub(crate) type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
pub(crate) fn fixture_path() -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let path = PathBuf::from(
        std::env::var_os("KEEPER_NATIVE_PROBE_FIXTURE")
            .ok_or("explicit compiled native probe fixture path required")?,
    );
    if !path.is_absolute() || !path.is_file() {
        return Err("fixture must be a built absolute executable".into());
    }
    Ok(path)
}
pub(crate) fn build_info() -> Result<BuildInfo, Box<dyn std::error::Error + Send + Sync>> {
    let fp = ComponentFingerprint::from_bytes(&[0x11; 32])?;
    Ok(BuildInfo {
        package_version: "1.2.3".parse()?,
        fingerprints: ComponentFingerprints {
            keeper: fp,
            agent_collaboration_services: fp,
            agent_proxy_services: fp,
            agent_provider_services: fp,
        },
    })
}
pub(crate) async fn setup()
-> Result<(tempfile::TempDir, ImageRegistry, ImageLease), Box<dyn std::error::Error + Send + Sync>>
{
    #[cfg(target_os = "macos")]
    let fixture_temp_base = PathBuf::from("/private/tmp");
    #[cfg(not(target_os = "macos"))]
    let fixture_temp_base = std::env::temp_dir();
    let root = tempfile::Builder::new()
        .prefix("np-")
        .tempdir_in(fixture_temp_base)?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let mut registry = ImageRegistry::new(root.path()).await?;
    // Each scenario owns a distinct source inode. Shared hardlink creation/removal
    // legitimately changes the build artifact's ctime during parallel capture.
    let source = root.path().join("compiled-fixture-source");
    let original = fixture_path()?;
    let expected = std::fs::read(&original)?;
    std::fs::copy(&original, &source)?;
    if std::fs::read(&source)? != expected {
        return Err("fixture copy differs from explicitly built bytes".into());
    }
    let expected_info = build_info()?;
    let copied_metadata = std::fs::metadata(&source)?;
    let preparation_started = tokio::time::Instant::now();
    match crate::image_warmup::warmup(
        &source,
        &expected_info,
        crate::lifecycle_bounds::PREPARE_DEADLINE,
    )
    .await
    {
        crate::image_warmup::WarmupOutcome::Verified => {}
        crate::image_warmup::WarmupOutcome::Refused {
            reason,
            finished_pid,
        } => {
            eprintln!(
                "NATIVE_FIXTURE_ASSESSMENT_REFUSED elapsed={:?} pid={finished_pid:?} error={reason:?}",
                preparation_started.elapsed()
            );
            return Err(reason.into());
        }
        crate::image_warmup::WarmupOutcome::CleanupPending {
            mut cleanup,
            failure,
        } => {
            eprintln!(
                "NATIVE_FIXTURE_ASSESSMENT_DEBT elapsed={:?} original_failure={failure:?} stored={:?}",
                preparation_started.elapsed(),
                cleanup.reaped_status()
            );
            if let Err(cleanup_error) = drain_fixture_assessment(&mut cleanup).await {
                let preserved_root = root.keep();
                return Err(FixtureAssessmentDebt {
                    cleanup,
                    failure,
                    cleanup_error,
                    preserved_root,
                }
                .into());
            }
            return Err(failure.into());
        }
    }
    let assessed_bytes = std::fs::read(&source)?;
    let assessed_metadata = std::fs::metadata(&source)?;
    if assessed_bytes != expected
        || assessed_metadata.dev() != copied_metadata.dev()
        || assessed_metadata.ino() != copied_metadata.ino()
        || assessed_metadata.mode() != copied_metadata.mode()
    {
        return Err("assessed source bytes/device/inode/mode changed".into());
    }
    let lease = registry.pin(&source, &expected_info).await?;
    let retained_metadata = std::fs::metadata(lease.image().retained_path())?;
    let retained_bytes = std::fs::read(lease.image().retained_path())?;
    use sha2::Digest;
    let assessed_digest: [u8; 32] = sha2::Sha256::digest(&assessed_bytes).into();
    if retained_metadata.dev() != assessed_metadata.dev()
        || retained_metadata.ino() != assessed_metadata.ino()
        || retained_metadata.mode() != assessed_metadata.mode()
        || retained_bytes != assessed_bytes
        || *lease.image().file_sha256() != assessed_digest
        || lease.build_info() != &expected_info
    {
        return Err("retained lease does not preserve the actual assessed copy".into());
    }
    eprintln!(
        "NATIVE_FIXTURE_SOURCE_ASSESSED elapsed={:?} source={source:?} retained={:?} device={} inode={} bytes={} digest={assessed_digest:?} build_info_equal=true before_start_job=true",
        preparation_started.elapsed(),
        lease.image().retained_path(),
        assessed_metadata.dev(),
        assessed_metadata.ino(),
        assessed_bytes.len(),
    );
    Ok((root, registry, lease))
}
pub(crate) fn process(
    registry: &ImageRegistry,
    image: &ImageLease,
    mode: &str,
) -> NativeProbeProcess {
    NativeProbeProcess::fixture(registry, image, vec!["--fixture-mode".into(), mode.into()])
}
pub(crate) fn job(
    path: &Path,
    action: AppServerProbeAction,
    native: Duration,
    remote: Duration,
) -> Result<NativeProbeJob, Box<dyn std::error::Error + Send + Sync>> {
    Ok(NativeProbeJob::new(
        action,
        GenerationAliasPath::try_from(path.to_owned())?,
        native,
        remote,
    ))
}
pub(crate) fn fences(process: &NativeProbeProcess) -> TestResult {
    let pid = process.leader_pid().ok_or("owned PID absent")?;
    let group = process
        .original_process_group_id()
        .ok_or("original PGID absent")?;
    if !process.cleanup_fenced()
        || process.leader_exit_status().is_none()
        || rustix::process::test_kill_process(pid.as_pid()) != Err(rustix::io::Errno::SRCH)
        || rustix::process::test_kill_process_group(group.as_pid()) != Err(rustix::io::Errno::SRCH)
        || !matches!(
            rustix::process::waitpid(Some(pid.as_pid()), rustix::process::WaitOptions::NOHANG),
            Err(rustix::io::Errno::CHILD)
        )
    {
        return Err("native probe exact exit/process/group/wait fence missing".into());
    }
    eprintln!(
        "NATIVE_PROBE_FENCE pid={pid:?} group={group:?} stored={:?} process=ESRCH group=ESRCH wait=ECHILD",
        process.leader_exit_status()
    );
    Ok(())
}

/// Test caller drives existing cleanup debt; an errno observation is never a fence.
pub(crate) async fn drain_owned_probe(process: &mut NativeProbeProcess) -> TestResult {
    // Error-path callers can re-enter this driver, but cannot allocate another window.
    let started = process.test_cleanup_observation_start();
    let deadline =
        started + crate::lifecycle_bounds::STOP_GRACE + crate::lifecycle_bounds::GROUP_REAP_BOUND;
    let cancel = tokio_util::sync::CancellationToken::new();
    let mut attempt = 0usize;
    loop {
        attempt += 1;
        let observed = tokio::time::timeout_at(deadline, process.drain_cleanup(&cancel)).await;
        eprintln!(
            "NATIVE_CLEANUP_OBSERVATION attempt={attempt} elapsed={:?} pid={:?} original_group={:?} stored={:?} fenced={} result={observed:?}",
            started.elapsed(),
            process.leader_pid(),
            process.original_process_group_id(),
            process.leader_exit_status(),
            process.cleanup_fenced(),
        );
        match observed {
            Ok(Ok(())) => {
                if !process.cleanup_fenced() || process.leader_exit_status().is_none() {
                    return Err(crate::NativeProbeError::CleanupIncomplete.into());
                }
                fences(process)?;
                return Ok(());
            }
            Ok(Err(error)) => {
                if !matches!(
                    &error,
                    crate::NativeProbeError::Group(crate::GroupStopError::Probe(errno))
                        if *errno == rustix::io::Errno::PERM
                ) {
                    return Err(error.into());
                }
                eprintln!(
                    "NATIVE_CLEANUP_ERROR_RETAINED pid={:?} error={error:?}",
                    process.leader_pid()
                );
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => {
                        eprintln!("NATIVE_CLEANUP_BOUND_EXPIRED pid={:?} original_error={error:?}", process.leader_pid());
                        return Err(error.into());
                    },
                    _ = tokio::time::sleep(crate::lifecycle_bounds::GROUP_POLL_INTERVAL) => {},
                }
            }
            Err(bound_error) => {
                eprintln!(
                    "NATIVE_CLEANUP_BOUND_EXPIRED pid={:?} error={bound_error:?}",
                    process.leader_pid()
                );
                return Err(crate::NativeProbeError::CleanupIncomplete.into());
            }
        }
    }
}

#[derive(thiserror::Error)]
#[error(
    "fixture assessment debt preserved at {preserved_root:?}: {failure}; cleanup: {cleanup_error}"
)]
struct FixtureAssessmentDebt {
    cleanup: crate::image_warmup::WarmupCleanup,
    #[source]
    failure: crate::ImageError,
    cleanup_error: crate::ImageError,
    preserved_root: PathBuf,
}
impl std::fmt::Debug for FixtureAssessmentDebt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FixtureAssessmentDebt")
            .field("failure", &self.failure)
            .field("cleanup_error", &self.cleanup_error)
            .field("preserved_root", &self.preserved_root)
            .field("owned_reaped_status", &self.cleanup.reaped_status())
            .finish()
    }
}
async fn drain_fixture_assessment(
    cleanup: &mut crate::image_warmup::WarmupCleanup,
) -> Result<(), crate::ImageError> {
    let cancel = tokio_util::sync::CancellationToken::new();
    let (pid, group) = match cleanup {
        crate::image_warmup::WarmupCleanup::Running(owner) => {
            if matches!(owner.progress(), crate::GroupStopProgress::Running) {
                owner.begin_stop(
                    crate::GroupStopTiming::normal(),
                    tokio::time::Instant::now(),
                )?;
            }
            let status = owner.wait_for_stop(&cancel).await?;
            if !matches!(status, crate::GroupStopStatus::GroupEmpty { .. })
                || owner.leader_exit_status().is_none()
            {
                return Err(crate::ImageError::CleanupIncomplete);
            }
            (owner.leader_pid(), owner.process_group_id())
        }
        crate::image_warmup::WarmupCleanup::FailedLaunch(owner) => {
            owner.wait_for_cleanup(&cancel).await?;
            (
                owner
                    .leader_pid()
                    .ok_or(crate::ImageError::CleanupIncomplete)?,
                owner
                    .original_process_group_id()
                    .ok_or(crate::ImageError::CleanupIncomplete)?,
            )
        }
    };
    let stored = cleanup.reaped_status();
    let process_probe = rustix::process::test_kill_process(pid.as_pid());
    let group_probe = rustix::process::test_kill_process_group(group.as_pid());
    // This independent oracle runs only after the sole owner consumed its exit.
    let waiter = rustix::process::waitpid(Some(pid.as_pid()), rustix::process::WaitOptions::NOHANG);
    eprintln!(
        "NATIVE_FIXTURE_ASSESSMENT_CLEANUP pid={pid:?} group={group:?} stored={stored:?} process={process_probe:?} group_probe={group_probe:?} wait={waiter:?}"
    );
    if stored.is_none()
        || process_probe != Err(rustix::io::Errno::SRCH)
        || group_probe != Err(rustix::io::Errno::SRCH)
        || !matches!(waiter, Err(rustix::io::Errno::CHILD))
    {
        return Err(crate::ImageError::CleanupIncomplete);
    }
    Ok(())
}
