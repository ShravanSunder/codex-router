//! One non-spawning receiver: native policy/codec stays at its existing native owner.
use crate::NativeProbeError;
use codex_native_integration::run_app_server_probe;
use codex_router_descriptor_boundary::{
    DescriptorGate, OwnedSocket, PipeReader, PipeWriter, ReceiptByteStream, UnixReceipt,
    validate_pipe,
};
use codex_router_keeper_protocol::{
    BuildInfo, ComponentFingerprint, ComponentKind, JsonMessage, NativeProbeJob,
    NativeProbeJobWire, NativeProbeResult, NativeProbeResultWire, ReceiverHelloWire,
};

pub fn validate_native_probe_launch(
    parent_role: ComponentKind,
    projected: ComponentFingerprint,
    compiled: &BuildInfo,
) -> Result<(), NativeProbeError> {
    if parent_role != ComponentKind::Keeper {
        return Err(NativeProbeError::ParentRole);
    }
    if projected != compiled.fingerprints.keeper {
        return Err(NativeProbeError::Fingerprint);
    }
    Ok(())
}
/// Entrypoint supplies its real compiled BuildInfo; this function never spawns or chooses policy.
pub async fn run_native_probe_receiver(
    parent_role: ComponentKind,
    projected: ComponentFingerprint,
    compiled: &BuildInfo,
) -> Result<(), NativeProbeError> {
    validate_native_probe_launch(parent_role, projected, compiled)?;
    validate_pipe(rustix::stdio::stdin(), false)?;
    validate_pipe(rustix::stdio::stdout(), true)?;
    let gate = DescriptorGate::global();
    let mut output =
        codex_router_keeper_protocol::PipeFrameWriter::new(PipeWriter::inherit_stdout(gate).await?);
    let mut input =
        codex_router_keeper_protocol::PipeFrameReader::new(PipeReader::inherit_stdin(gate).await?);
    output
        .send(&JsonMessage::encode(&ReceiverHelloWire::Hello {
            parent_role,
            fingerprint: projected,
        })?)
        .await?;
    let record = input
        .receive()
        .await?
        .ok_or(NativeProbeError::RecordCount)?;
    let job = NativeProbeJob::from(record.decode::<NativeProbeJobWire>()?);
    // The job is one-shot. Even valid trailing JSON must not trigger a native effect.
    if input.receive().await?.is_some() {
        return Err(NativeProbeError::RecordCount);
    }
    let alias = job.alias().as_path().to_owned();
    let result = match run_app_server_probe(
        job.action(),
        job.native_readiness_wait(),
        job.remote_control_wait(),
        move || {
            let alias = alias.clone();
            async move {
                OwnedSocket::connect_unix(&alias, DescriptorGate::global())
                    .await
                    .map(|socket| ReceiptByteStream::new(UnixReceipt::new(socket)))
                    .map_err(std::io::Error::other)
            }
        },
    )
    .await
    {
        Ok(observation) => NativeProbeResult::Observed(observation),
        Err(failure) => NativeProbeResult::Failed(failure.into()),
    };
    output
        .send(&JsonMessage::encode(&NativeProbeResultWire::from(result))?)
        .await?;
    drop(output);
    Ok(())
}
