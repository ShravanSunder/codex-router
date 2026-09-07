//! Dedicated frame readers retain partial input while the connection router handles other work.
use crate::{AcpOutputSender, QueuedAcpFrame, bounded_acp_output, read_acp_frame, write_acp_frame};
use std::io;
use tokio::{
    io::{AsyncRead, AsyncWrite, BufReader},
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;

pub struct AcpWireChannels {
    input: AcpOutputSender,
    output: mpsc::Receiver<QueuedAcpFrame>,
    closed: CancellationToken,
}
pub struct AcpRouterChannels {
    pub input: mpsc::Receiver<QueuedAcpFrame>,
    pub output: AcpOutputSender,
    pub closed: CancellationToken,
}
pub fn acp_connection_channels() -> (AcpWireChannels, AcpRouterChannels) {
    let closed = CancellationToken::new();
    let (input, input_receiver) = bounded_acp_output(closed.clone());
    let (output, output_receiver) = bounded_acp_output(closed.clone());
    (
        AcpWireChannels {
            input,
            output: output_receiver,
            closed: closed.clone(),
        },
        AcpRouterChannels {
            input: input_receiver,
            output,
            closed,
        },
    )
}
pub async fn run_acp_transport<TStream: AsyncRead + AsyncWrite + Unpin>(
    stream: TStream,
    mut channels: AcpWireChannels,
) -> io::Result<()> {
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let receive = async {
        while let Some(frame) = read_acp_frame(&mut read).await? {
            channels.input.send(frame).await?;
        }
        Ok::<_, io::Error>(())
    };
    let transmit = async {
        while let Some(frame) = channels.output.recv().await {
            // Keep the queued-byte permit until the complete socket write finishes.
            write_acp_frame(&mut write, &frame).await?;
        }
        Ok::<_, io::Error>(())
    };
    let result = tokio::select! {
        _=channels.closed.cancelled()=>Ok(()),
        result=receive=>result,
        result=transmit=>result,
    };
    channels.closed.cancel();
    result
}
