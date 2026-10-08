//! Caller-held frame futures retain partial IO across a dropped collection operation.
use crate::{ImageLease, NativeProbeError, OwnedProcessGroup};
use codex_router_descriptor_boundary::{DescriptorGate, PipeReader, PipeWriter};
use codex_router_keeper_protocol::{
    ComponentFingerprint, ComponentKind, JsonMessage, NativeProbeJobWire, NativeProbeResult,
    NativeProbeResultWire, PipeFrameReader, PipeFrameWriter, ReceiverHelloWire,
};
use std::{future::Future, pin::Pin};
use tokio::io::AsyncReadExt;
pub(crate) type FrameReadFuture = Pin<
    Box<
        dyn Future<Output = Result<(PipeFrameReader, Option<JsonMessage>), NativeProbeError>>
            + Send,
    >,
>;
pub(crate) type FrameSendFuture =
    Pin<Box<dyn Future<Output = Result<(), NativeProbeError>> + Send>>;
pub(crate) type StderrFuture =
    Pin<Box<dyn Future<Output = Result<Vec<u8>, NativeProbeError>> + Send>>;
pub(crate) type AttachmentFuture = Pin<Box<dyn Future<Output = AttachmentOutcome> + Send>>;
pub(crate) enum AttachmentOutcome {
    Attached(ProbeChannel),
    Failed {
        reason: NativeProbeError,
        group: OwnedProcessGroup,
        image: ImageLease,
    },
}
pub(crate) enum ReadPhase {
    Hello,
    Result,
    Eof,
    Ended,
}
pub(crate) enum JobWrite {
    AwaitHello(PipeFrameWriter),
    Sending(FrameSendFuture),
    Closed,
}
pub(crate) struct ProbeChannel {
    pub(crate) group: OwnedProcessGroup,
    pub(crate) image: ImageLease,
    pub(crate) reading: Option<FrameReadFuture>,
    pub(crate) writing: JobWrite,
    pub(crate) stderr: Option<StderrFuture>,
    pub(crate) stderr_bytes: Vec<u8>,
    pub(crate) read_phase: ReadPhase,
    pub(crate) result: Option<NativeProbeResult>,
}
struct PreparedPipes {
    reader: PipeFrameReader,
    writer: PipeFrameWriter,
    stderr: StderrFuture,
}
pub(crate) fn attach(mut group: OwnedProcessGroup, image: ImageLease) -> AttachmentFuture {
    Box::pin(async move {
        let pipes = async {
            let input = group.take_stdin().ok_or(NativeProbeError::MissingPipe)?;
            let output = group.take_stdout().ok_or(NativeProbeError::MissingPipe)?;
            let mut errors = group.take_stderr().ok_or(NativeProbeError::MissingPipe)?;
            let input = input.into_owned_fd()?;
            let output = output.into_owned_fd()?;
            let gate = DescriptorGate::global();
            let writer = PipeFrameWriter::new(PipeWriter::from_owned(input, gate).await?);
            let reader = PipeFrameReader::new(PipeReader::from_owned(output, gate).await?);
            let stderr: StderrFuture = Box::pin(async move {
                let mut captured = Vec::new();
                let mut chunk = [0; 4096];
                loop {
                    let count = errors.read(&mut chunk).await?;
                    if count == 0 {
                        return Ok(captured);
                    }
                    let keep = count.min(4096usize.saturating_sub(captured.len()));
                    let bytes = chunk.get(..keep).ok_or(NativeProbeError::MissingPipe)?;
                    captured.extend_from_slice(bytes);
                    // Continue draining after the capture bound, without buffering more.
                }
            });
            Ok::<_, NativeProbeError>(PreparedPipes {
                reader,
                writer,
                stderr,
            })
        }
        .await;
        match pipes {
            Ok(pipes) => AttachmentOutcome::Attached(ProbeChannel {
                group,
                image,
                reading: Some(read_frame(pipes.reader)),
                writing: JobWrite::AwaitHello(pipes.writer),
                stderr: Some(pipes.stderr),
                stderr_bytes: Vec::new(),
                read_phase: ReadPhase::Hello,
                result: None,
            }),
            Err(reason) => AttachmentOutcome::Failed {
                reason,
                group,
                image,
            },
        }
    })
}
fn read_frame(mut reader: PipeFrameReader) -> FrameReadFuture {
    Box::pin(async move {
        let record = reader.receive().await?;
        Ok((reader, record))
    })
}
impl ProbeChannel {
    /// Returns true only for the first validated hello; job transmission is then admitted.
    pub(crate) fn accept_frame(
        &mut self,
        reader: PipeFrameReader,
        record: Option<JsonMessage>,
        projected: ComponentFingerprint,
    ) -> Result<bool, NativeProbeError> {
        self.reading = None;
        let hello = match self.read_phase {
            ReadPhase::Hello => {
                let frame = record.ok_or(NativeProbeError::Hello)?;
                let ReceiverHelloWire::Hello {
                    parent_role,
                    fingerprint,
                } = frame.decode()?;
                if parent_role != ComponentKind::Keeper || fingerprint != projected {
                    return Err(NativeProbeError::Hello);
                }
                self.read_phase = ReadPhase::Result;
                true
            }
            ReadPhase::Result => {
                let frame = record.ok_or(NativeProbeError::RecordCount)?;
                self.result = Some(NativeProbeResult::try_from(
                    frame.decode::<NativeProbeResultWire>()?,
                )?);
                self.read_phase = ReadPhase::Eof;
                false
            }
            ReadPhase::Eof => {
                if record.is_some() {
                    return Err(NativeProbeError::RecordCount);
                }
                self.read_phase = ReadPhase::Ended;
                return Ok(false);
            }
            ReadPhase::Ended => return Err(NativeProbeError::RecordCount),
        };
        self.reading = Some(read_frame(reader));
        Ok(hello)
    }
    pub(crate) fn send_job(&mut self, job: NativeProbeJobWire) -> Result<(), NativeProbeError> {
        let message = JsonMessage::encode(&job)?;
        let JobWrite::AwaitHello(mut writer) =
            std::mem::replace(&mut self.writing, JobWrite::Closed)
        else {
            return Err(NativeProbeError::RecordCount);
        };
        self.writing = JobWrite::Sending(Box::pin(async move {
            writer.send(&message).await?;
            drop(writer);
            Ok(())
        }));
        Ok(())
    }
    pub(crate) fn io_finished(&self) -> bool {
        matches!(self.read_phase, ReadPhase::Ended)
            && matches!(self.writing, JobWrite::Closed)
            && self.stderr.is_none()
    }
}
pub(crate) async fn next_frame(
    reading: &mut Option<FrameReadFuture>,
) -> Result<(PipeFrameReader, Option<JsonMessage>), NativeProbeError> {
    match reading {
        Some(future) => future.await,
        None => std::future::pending().await,
    }
}
pub(crate) async fn send_frame(writing: &mut JobWrite) -> Result<(), NativeProbeError> {
    match writing {
        JobWrite::Sending(future) => future.await,
        _ => std::future::pending().await,
    }
}
pub(crate) async fn drain_stderr(
    stderr: &mut Option<StderrFuture>,
) -> Result<Vec<u8>, NativeProbeError> {
    match stderr {
        Some(future) => future.await,
        None => std::future::pending().await,
    }
}
