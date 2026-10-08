use crate::NativeProbeProcess;
use crate::native_probe_test_support::*;
use codex_native_integration::AppServerProbeAction;
use codex_router_descriptor_boundary::{DescriptorGate, OwnedPipe};
use std::{
    io::IoSlice,
    mem::MaybeUninit,
    os::fd::{AsFd, BorrowedFd},
    time::Duration,
};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;
fn malicious_rights(
    sender: BorrowedFd<'_>,
    rights: &[BorrowedFd<'_>],
) -> Result<usize, rustix::io::Errno> {
    // Same pinned safe sender facility as descriptor containment proof; independent128 capacity.
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(128))];
    let mut control = rustix::net::SendAncillaryBuffer::new(&mut space);
    if !control.push(rustix::net::SendAncillaryMessage::ScmRights(rights)) {
        return Err(rustix::io::Errno::INVAL);
    }
    rustix::net::sendmsg(
        sender,
        &[IoSlice::new(b"x")],
        &mut control,
        rustix::net::SendFlags::DONTWAIT,
    )
}
async fn poison(right_count: usize) -> TestResult {
    let (root, registry, image) = setup().await?;
    let alias = root.path().join("gen-12345678-1.sock");
    let listener = tokio::net::UnixListener::bind(&alias)?;
    let mut probe = NativeProbeProcess::new(&registry, &image);
    probe.start_job(job(
        &alias,
        AppServerProbeAction::Observe,
        Duration::from_secs(2),
        Duration::from_secs(1),
    )?)?;
    let scenario: TestResult = async {
        let token = CancellationToken::new();
        let mut collecting = Box::pin(probe.collect(&token));
        let (mut peer, _) = tokio::select! {result=&mut collecting=>return Err(format!("receiver settled before poison peer: {result:?}").into()),accepted=listener.accept()=>accepted?};
        let mut upgrade = [0; 512];
        tokio::select! {result=&mut collecting=>return Err(format!("receiver settled before native upgrade: {result:?}").into()),read=peer.read(&mut upgrade)=>{if read?==0{return Err("native peer closed before receipt".into());}}}
        let gate = DescriptorGate::global();
        let (witness_read, witness_write) = OwnedPipe::pair(gate).await?;
        let mut copies = Vec::new();
        {
            let _creation = gate.creation().await;
            for _ in 0..right_count {
                copies.push(rustix::io::dup(witness_write.as_fd())?);
            }
        }
        let rights: Vec<_> = copies.iter().map(AsFd::as_fd).collect();
        if malicious_rights(peer.as_fd(), &rights)? != 1 {
            return Err("poison rights send not literal one byte".into());
        }
        drop(rights);
        drop(copies);
        drop(witness_write);
        let result = collecting.await;
        eprintln!("NATIVE_POISON_RESULT rights={right_count} pid={:?} result={result:?}", probe.leader_pid());
        drain_owned_probe(&mut probe).await?;
        if !matches!(
            result,
            Err(crate::NativeProbeError::RecordCount | crate::NativeProbeError::AbnormalExit)
        ) {
            return Err(format!("poison receiver did not reach its fatal receipt result: {result:?}").into());
        }
        use std::os::unix::process::ExitStatusExt;
        let status = probe
            .leader_exit_status()
            .ok_or("poison exact exit missing")?;
        if status.code() != Some(70) && status.signal().is_none() {
            return Err(format!(
                "poison receiver not terminated by established fatal boundary: {status:?}"
            )
            .into());
        }
        fences(&probe)?;
        if tokio::time::timeout(Duration::from_secs(1), witness_read.read(&mut [0])).await?? != 0 {
            return Err("poison witness FD leaked after receiver reap".into());
        }
        // Original peer's remaining client upgrade bytes may precede EOF; drain only this fixture.
        let mut trailing = Vec::new();
        let closure =
            tokio::time::timeout(Duration::from_secs(1), peer.read_to_end(&mut trailing)).await?;
        if let Err(error) = closure
            && error.kind() != std::io::ErrorKind::ConnectionReset
        {
            return Err(error.into());
        }
        rustix::process::test_kill_process(rustix::process::getpid())?;
        eprintln!(
            "NATIVE_POISON rights={right_count} disposition={status:?} collector={result:?} result_unadmitted witness=EOF original_peer=closed parent=alive"
        );
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
#[tokio::test]
async fn actual_unexpected_right_kills_only_receiving_probe_without_result() -> TestResult {
    poison(1).await
}
#[tokio::test]
async fn actual_128_into64_ancillary_poison_reaps_probe_and_closes_witness() -> TestResult {
    poison(128).await
}
