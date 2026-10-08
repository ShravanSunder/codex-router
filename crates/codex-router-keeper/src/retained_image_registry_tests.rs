use super::*;
use crate::{GroupStopStatus, GroupStopTiming, ImageCommitRelease, SlotImageState};
use codex_router_keeper_protocol::ComponentFingerprints;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    time::{Instant, timeout},
};
use tokio_util::sync::CancellationToken;
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
const FIXTURE: &str = include_str!("../tests/support/retained_image_fixture.py");
fn expected_build(byte: u8) -> Result<BuildInfo, Box<dyn std::error::Error + Send + Sync>> {
    let digest = ComponentFingerprint::from_bytes(&[byte; 32])?;
    Ok(BuildInfo {
        package_version: "1.2.3".parse()?,
        fingerprints: ComponentFingerprints {
            keeper: digest,
            agent_collaboration_services: digest,
            agent_proxy_services: digest,
            agent_provider_services: digest,
        },
    })
}
fn root() -> Result<tempfile::TempDir, std::io::Error> {
    tempfile::tempdir()
}
fn private_root(directory: &tempfile::TempDir) -> Result<PathBuf, std::io::Error> {
    let path = std::fs::canonicalize(directory.path())?;
    std::fs::set_permissions(&path, Permissions::from_mode(0o700))?;
    Ok(path)
}
fn fixture(
    directory: &Path,
    name: &str,
    mode: &str,
    marker: &str,
    byte: u8,
) -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error + Send + Sync>> {
    let path = directory.join(name);
    let proof = directory.join(format!("{name}.pid"));
    let source = FIXTURE
        .replace(
            "FIXTURE_MODE = \"valid\"",
            &format!("FIXTURE_MODE = {mode:?}"),
        )
        .replace(
            "CODE_MARKER = \"IMAGE_A\"",
            &format!("CODE_MARKER = {marker:?}"),
        )
        .replace(
            "PROOF_PATH = None",
            &format!(
                "PROOF_PATH = {:?}",
                proof.to_str().ok_or("fixture path encoding")?
            ),
        )
        .replace(&"11".repeat(32), &format!("{byte:02x}").repeat(32));
    std::fs::write(&path, source)?;
    std::fs::set_permissions(&path, Permissions::from_mode(0o755))?;
    Ok((path, proof))
}
fn exdev(_source: &Path, _target: &Path) -> std::io::Result<()> {
    Err(std::io::Error::from_raw_os_error(
        rustix::io::Errno::XDEV.raw_os_error(),
    ))
}
async fn finished(group: &mut OwnedProcessGroup) -> TestResult {
    timeout(Duration::from_secs(3), async {
        let cadence = crate::lifecycle_bounds::GROUP_POLL_INTERVAL;
        let mut ticks = tokio::time::interval_at(Instant::now() + cadence, cadence);
        loop {
            ticks.tick().await;
            let status = group.tick(Instant::now())?;
            if matches!(status, GroupStopStatus::GroupEmpty { .. })
                && group.leader_exit_status().is_some()
            {
                return Ok::<_, Box<dyn std::error::Error + Send + Sync>>(());
            }
        }
    })
    .await?
}
async fn run_marker(registry: &ImageRegistry, lease: &ImageLease, marker: &str) -> TestResult {
    let (mut group, _running_image) = crate::owned_launch_test_support::require_image_launch(
        registry.spawn(lease, &[OsString::from("marker")]).await?,
    )
    .await?;
    let mut output = group.take_stdout().ok_or("stdout absent")?;
    let mut text = String::new();
    timeout(Duration::from_secs(3), output.read_to_string(&mut text)).await??;
    finished(&mut group).await?;
    if !text.starts_with(&format!("{marker} PID "))
        || !group
            .leader_exit_status()
            .is_some_and(|status| status.success())
        || rustix::process::test_kill_process_group(group.process_group_id().as_pid())
            != Err(rustix::io::Errno::SRCH)
    {
        return Err("retained executable marker/exit/group oracle failed".into());
    }
    Ok(())
}
#[tokio::test]
async fn hardlink_retains_inode_mode_bytes_and_executes_after_source_removal() -> TestResult {
    let temp = root()?;
    let root = private_root(&temp)?;
    let (source, proof) = fixture(&root, "source", "valid", "IMAGE_A", 1)?;
    let original = std::fs::metadata(&source)?;
    let bytes = std::fs::read(&source)?;
    let mut registry = ImageRegistry::new(&root).await?;
    let lease = registry.pin(&source, &expected_build(1)?).await?;
    let pinned = std::fs::metadata(lease.image().retained_path())?;
    if pinned.dev() != original.dev()
        || pinned.ino() != original.ino()
        || pinned.mode() != original.mode()
        || std::fs::read(lease.image().retained_path())? != bytes
        || proof.exists()
    {
        return Err("hardlink identity changed or redundant warmup ran".into());
    }
    std::fs::remove_file(&source)?;
    run_marker(&registry, &lease, "IMAGE_A").await?;
    if !registry.collect().await?.is_empty() {
        return Err("live lease collected".into());
    }
    let record = lease.image().clone();
    drop(lease);
    if registry.collect().await? != vec![record.clone()] || record.retained_path().exists() {
        return Err("unreferenced owned image not collected".into());
    }
    Ok(())
}
#[tokio::test]
async fn copied_image_runs_real_warmup_and_survives_source_removal() -> TestResult {
    let temp = root()?;
    let root = private_root(&temp)?;
    let (source, proof) = fixture(&root, "copy", "valid", "IMAGE_COPY", 1)?;
    let original = std::fs::metadata(&source)?;
    let mut registry = ImageRegistry::new(&root).await?;
    let lease = match registry
        .pin_with_link(&source, &expected_build(1)?, exdev, Duration::from_secs(2))
        .await
    {
        Ok(lease) => lease,
        Err(error) => {
            if let Some(pending) = registry.pending_warmups.first() {
                let group = pending.cleanup.running_group()?;
                eprintln!(
                    "VALID_COPY_WARMUP_FAILURE pid={:?} pgid={:?} leader={:?} progress={:?} process_probe={:?} group_probe={:?} getpgid={:?} reason={error:?}",
                    group.leader_pid(),
                    group.process_group_id(),
                    group.leader_exit_status(),
                    group.progress(),
                    rustix::process::test_kill_process(group.leader_pid().as_pid()),
                    rustix::process::test_kill_process_group(group.process_group_id().as_pid()),
                    rustix::process::getpgid(Some(group.leader_pid().as_pid()))
                );
            }
            if matches!(
                error,
                ImageError::Process(crate::GroupStopError::Probe(rustix::io::Errno::PERM))
            ) && let Some(pending) = registry.pending_warmups.first_mut()
            {
                let pid = pending.cleanup.running_group()?.leader_pid().as_pid();
                let observed = rustix::process::waitid(
                    rustix::process::WaitId::Pid(pid),
                    rustix::process::WaitIdOptions::EXITED
                        | rustix::process::WaitIdOptions::NOHANG
                        | rustix::process::WaitIdOptions::NOWAIT,
                );
                eprintln!("NOWAIT_DISCRIMINATION pid={pid:?} observation={observed:?}");
                let next = pending.cleanup.running_group_mut()?.tick(Instant::now());
                eprintln!(
                    "ONE_TICK_DISCRIMINATION pid={pid:?} result={next:?} leader={:?} progress={:?} process_probe={:?} group_probe={:?} getpgid={:?}",
                    pending.cleanup.running_group()?.leader_exit_status(),
                    pending.cleanup.running_group()?.progress(),
                    rustix::process::test_kill_process(pid),
                    rustix::process::test_kill_process_group(pid),
                    rustix::process::getpgid(Some(pid))
                );
            }
            // Diagnostic never upgrades the original refusal to a successful pin.
            return Err(error.into());
        }
    };
    let pinned = std::fs::metadata(lease.image().retained_path())?;
    if pinned.ino() == original.ino()
        || std::fs::read(&source)? != std::fs::read(lease.image().retained_path())?
        || !proof.exists()
    {
        return Err("copy/warmup was not real".into());
    }
    let pid =
        codex_router_keeper_protocol::ChildPid::new(std::fs::read_to_string(proof)?.parse()?)?;
    if rustix::process::test_kill_process(pid.as_pid()) != Err(rustix::io::Errno::SRCH) {
        return Err("warmup child not reaped".into());
    }
    std::fs::remove_file(source)?;
    run_marker(&registry, &lease, "IMAGE_COPY").await
}
#[tokio::test]
async fn every_copied_warmup_refusal_reaps_actual_process_and_exposes_no_image() -> TestResult {
    for mode in [
        "mismatch",
        "version",
        "tamper",
        "invalid",
        "nonzero",
        "oversized",
        "timeout",
        "trailing",
    ] {
        copied_refusal_tests::prove_copied_refusal(mode).await?;
    }
    Ok(())
}
#[tokio::test]
async fn missing_changed_and_replaced_images_refuse_without_ambient_fallback() -> TestResult {
    for defect in ["missing", "bytes", "inode"] {
        let temp = root()?;
        let root = private_root(&temp)?;
        let (source, _) = fixture(&root, "source", "valid", "IMAGE_A", 1)?;
        let mut registry = ImageRegistry::new(&root).await?;
        let lease = registry.pin(&source, &expected_build(1)?).await?;
        let path = lease.image().retained_path();
        match defect {
            "missing" => std::fs::remove_file(path)?,
            "bytes" => std::fs::write(path, b"#!/bin/sh\necho WRONG\n")?,
            "inode" => {
                std::fs::remove_file(path)?;
                std::fs::copy(&source, path)?;
            }
            _ => return Err("unknown defect".into()),
        }
        if registry.spawn(&lease, &[]).await.is_ok() {
            return Err(format!("{defect} image used ambient fallback").into());
        }
    }
    Ok(())
}
#[tokio::test]
async fn private_directory_symlink_and_foreign_node_checks_preserve_authority() -> TestResult {
    let temp = root()?;
    let root = private_root(&temp)?;
    let alias = root.join("alias");
    std::os::unix::fs::symlink(&root, &alias)?;
    if ImageRegistry::new(&alias).await.is_ok() {
        return Err("symlink root accepted".into());
    }
    let (source, _) = fixture(&root, "source", "valid", "IMAGE_A", 1)?;
    let mut registry = ImageRegistry::new(&root).await?;
    let captured = capture(&source, true).await?;
    let directory = registry.images_root.join(digest_hex(&captured.digest));
    ensure_private(&directory)?;
    let retained = directory.join("codex-router");
    std::os::unix::fs::symlink(&source, &retained)?;
    if !matches!(
        registry.pin(&source, &expected_build(1)?).await,
        Err(ImageError::ForeignNode)
    ) || !std::fs::symlink_metadata(&retained)?
        .file_type()
        .is_symlink()
    {
        return Err("foreign symlink overwritten".into());
    }
    std::fs::remove_file(retained)?;
    std::fs::set_permissions(&directory, Permissions::from_mode(0o755))?;
    if !matches!(
        registry.pin(&source, &expected_build(1)?).await,
        Err(ImageError::PrivateDirectory)
    ) {
        return Err("nonprivate directory accepted".into());
    }
    Ok(())
}
#[tokio::test]
async fn collection_preserves_replaced_nodes_and_unrecognized_entries() -> TestResult {
    let temp = root()?;
    let root = private_root(&temp)?;
    let (source, _) = fixture(&root, "source", "valid", "IMAGE_A", 1)?;
    let mut registry = ImageRegistry::new(&root).await?;
    let lease = registry.pin(&source, &expected_build(1)?).await?;
    let path = lease.image().retained_path().to_owned();
    drop(lease);
    std::fs::remove_file(&path)?;
    std::fs::write(&path, b"foreign")?;
    std::fs::set_permissions(&path, Permissions::from_mode(0o755))?;
    let unknown = registry.images_root.join("unrecognized");
    std::fs::write(&unknown, b"preserve")?;
    if registry.collect().await.is_ok()
        || std::fs::read(&path)? != b"foreign"
        || std::fs::read(&unknown)? != b"preserve"
    {
        return Err("foreign collection node removed".into());
    }
    Ok(())
}
#[tokio::test]
async fn handoff_reference_reacquires_before_collection_and_reuse_keeps_same_lease() -> TestResult {
    let temp = root()?;
    let root = private_root(&temp)?;
    let (source, _) = fixture(&root, "source", "valid", "IMAGE_A", 1)?;
    let mut registry = ImageRegistry::new(&root).await?;
    let lease = registry.pin(&source, &expected_build(1)?).await?;
    let same = registry.pin(&source, &expected_build(1)?).await?;
    if !Arc::ptr_eq(&lease.0, &same.0) {
        return Err("cache reuse split reference accounting".into());
    }
    let record = lease.image().clone();
    drop(lease);
    drop(same);
    drop(registry);
    let mut restored = ImageRegistry::new(&root).await?;
    let recovered = restored
        .reacquire(record.clone(), expected_build(1)?)
        .await?;
    if !restored.collect().await?.is_empty() {
        return Err("handoff reference collected".into());
    }
    std::fs::remove_file(source)?;
    run_marker(&restored, &recovered, "IMAGE_A").await?;
    drop(recovered);
    if restored.collect().await? != vec![record] {
        return Err("reacquired lease lifetime failed".into());
    }
    Ok(())
}
#[tokio::test]
async fn slot_refusal_and_commit_keep_exact_recovery_code_through_retiring_child() -> TestResult {
    let temp = root()?;
    let root = private_root(&temp)?;
    let (a_source, _) = fixture(&root, "A", "valid", "IMAGE_A", 1)?;
    let (b_source, _) = fixture(&root, "B", "valid", "IMAGE_B", 2)?;
    let mut registry = ImageRegistry::new(&root).await?;
    let a = registry.pin(&a_source, &expected_build(1)?).await?;
    let b = registry.pin(&b_source, &expected_build(2)?).await?;
    let mut slot = SlotImageState::new(ComponentKind::AgentProxyServices, a.clone());
    slot.prepare_candidate(b.clone())?;
    // Sending Deactivate is a future channel action; this local candidate alone cannot commit.
    if slot.committed().image() != a.image() {
        return Err("Prepare prematurely committed".into());
    }
    let candidate_child_ref = b.clone();
    drop(slot.refuse_candidate()?);
    drop(b);
    if !registry.collect().await?.is_empty() {
        return Err("candidate child reference ignored".into());
    }
    drop(candidate_child_ref);
    if registry.collect().await?.len() != 1 {
        return Err("refused candidate lifetime failed".into());
    }
    std::fs::remove_file(&a_source)?;
    run_marker(&registry, slot.committed(), "IMAGE_A").await?;
    let b = registry.pin(&b_source, &expected_build(2)?).await?;
    slot.prepare_candidate(b.clone())?;
    let retiring_ref = a.clone();
    let receiver_ref = a.clone();
    let (mut retiring, retiring_image) = crate::owned_launch_test_support::require_image_launch(
        registry.spawn(&a, &[OsString::from("hold")]).await?,
    )
    .await?;
    let mut output = BufReader::new(retiring.take_stdout().ok_or("retiring stdout absent")?);
    let mut ready = String::new();
    timeout(Duration::from_secs(3), output.read_line(&mut ready)).await??;
    if !ready.starts_with("IMAGE_A PID ") {
        return Err("retiring code marker failed".into());
    }
    drop(slot.commit_candidate(ImageCommitRelease::Deactivated)?);
    drop(a);
    drop(b);
    // Simulated Activate failure: recovery remains B; no actual role channel is claimed.
    std::fs::remove_file(b_source)?;
    run_marker(&registry, slot.committed(), "IMAGE_B").await?;
    if slot.committed_fingerprint() != ComponentFingerprint::from_bytes(&[2; 32])?
        || !registry.collect().await?.is_empty()
    {
        return Err("commit/fingerprint/retiring lease drift".into());
    }
    drop(receiver_ref);
    if !registry.collect().await?.is_empty() {
        return Err("live retiring child image removed".into());
    }
    retiring.begin_stop(GroupStopTiming::normal(), Instant::now())?;
    let stopped = retiring.wait_for_stop(&CancellationToken::new()).await?;
    if !matches!(stopped, GroupStopStatus::GroupEmpty { .. })
        || retiring.leader_exit_status().is_none()
    {
        return Err("retirement fence fabricated".into());
    }
    drop(retiring_ref);
    drop(retiring_image);
    if registry.collect().await?.len() != 1 {
        return Err("old image not collected after real child cleanup".into());
    }
    let (forced_source, _) = fixture(&root, "forced-A", "valid", "IMAGE_A", 1)?;
    let forced_a = registry.pin(&forced_source, &expected_build(1)?).await?;
    let mut forced_slot = SlotImageState::new(ComponentKind::AgentProxyServices, forced_a);
    forced_slot.prepare_candidate(slot.committed().clone())?;
    drop(forced_slot.commit_candidate(ImageCommitRelease::ForcedPredicateCompleted)?);
    if forced_slot.candidate().is_some()
        || forced_slot.committed_fingerprint() != ComponentFingerprint::from_bytes(&[2; 32])?
    {
        return Err("forced release did not commit and consume candidate B".into());
    }
    run_marker(&registry, forced_slot.committed(), "IMAGE_B").await
}

#[path = "retained_image_ownership_tests.rs"]
mod ownership_tests;

#[path = "retained_image_restart_tests.rs"]
mod restart_tests;

#[tokio::test]
async fn bounded_vm_teardown_warmup_requires_normal_exit_and_actual_reap_fence() -> TestResult {
    let temp = root()?;
    let root = private_root(&temp)?;
    let (source, proof) = fixture(&root, "vm-exit", "vm-teardown", "VM_IMAGE", 1)?;
    let mut registry = ImageRegistry::new(&root).await?;
    let lease = registry
        .pin_with_link(&source, &expected_build(1)?, exdev, Duration::from_secs(2))
        .await?;
    let pid =
        codex_router_keeper_protocol::ChildPid::new(std::fs::read_to_string(proof)?.parse()?)?;
    if rustix::process::test_kill_process(pid.as_pid()) != Err(rustix::io::Errno::SRCH)
        || rustix::process::test_kill_process_group(pid.as_pid()) != Err(rustix::io::Errno::SRCH)
        || !matches!(
            rustix::process::waitpid(Some(pid.as_pid()), rustix::process::WaitOptions::NOHANG),
            Err(rustix::io::Errno::CHILD)
        )
        || lease.build_info() != &expected_build(1)?
    {
        return Err(
            "VM fixture did not reach actual validated output/normal exit/reap/group fence".into(),
        );
    }
    eprintln!(
        "BOUNDED_VM_EXIT pid={pid:?} bytes=67108864 expected_info=verified process=ESRCH group=ESRCH wait=ECHILD"
    );
    std::fs::remove_file(source)?;
    run_marker(&registry, &lease, "VM_IMAGE").await
}

#[path = "retained_image_launch_failure_tests.rs"]
mod launch_failure_tests;

#[path = "copied_image_refusal_tests.rs"]
mod copied_refusal_tests;
