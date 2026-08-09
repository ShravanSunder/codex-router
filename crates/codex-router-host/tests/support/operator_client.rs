use std::error::Error;
use std::path::Path;
use std::time::Duration;

use codex_router_host::HostProgress;
use codex_router_host::MAX_OPERATOR_FRAME_BYTES;
use codex_router_host::OperatorFrame;
use codex_router_host::OperatorRequest;
use codex_router_host::decode_operator_frame;
use codex_router_host::encode_operator_request;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

pub(crate) async fn send_operator_request(
    socket: &Path,
    request: OperatorRequest,
    deadline: Duration,
) -> Result<Vec<OperatorFrame>, Box<dyn Error>> {
    let deadline_at = tokio::time::Instant::now() + deadline;
    let mut stream = loop {
        match tokio::time::timeout_at(deadline_at, UnixStream::connect(socket)).await {
            Ok(Ok(stream)) => break stream,
            Ok(Err(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                if tokio::time::Instant::now() >= deadline_at {
                    return Err(std::io::Error::other("operator request timed out").into());
                }
                tokio::time::sleep_until(
                    deadline_at.min(tokio::time::Instant::now() + Duration::from_millis(20)),
                )
                .await;
            }
            Ok(Err(error)) => return Err(error.into()),
            Err(elapsed) => return Err(elapsed.into()),
        }
    };
    tokio::time::timeout_at(
        deadline_at,
        stream.write_all(&encode_operator_request(&request)?),
    )
    .await??;
    tokio::time::timeout_at(deadline_at, stream.shutdown()).await??;
    let response_bytes = read_bounded_to_end(&mut stream, deadline_at).await?;

    let mut frames = Vec::new();
    let mut terminal_seen = false;
    for response_line in response_bytes.split_inclusive(|byte| *byte == b'\n') {
        if response_line.is_empty() {
            continue;
        }
        if terminal_seen {
            return Err(
                std::io::Error::other("operator response continued after terminal frame").into(),
            );
        }
        let frame = decode_operator_frame(response_line)?;
        terminal_seen = matches!(frame, OperatorFrame::Terminal(_));
        frames.push(frame);
    }
    if !terminal_seen
        && matches!(
            frames.last(),
            Some(OperatorFrame::Progress(HostProgress::ReplacementStarting))
        )
    {
        return Ok(frames);
    }
    if !terminal_seen {
        return Err(std::io::Error::other("operator response omitted terminal frame").into());
    }
    Ok(frames)
}

async fn read_bounded_to_end(
    stream: &mut UnixStream,
    deadline_at: tokio::time::Instant,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut payload = Vec::new();
    let mut limited = stream.take(u64::try_from(MAX_OPERATOR_FRAME_BYTES).unwrap_or(u64::MAX) + 1);
    tokio::time::timeout_at(deadline_at, limited.read_to_end(&mut payload)).await??;
    if payload.len() > MAX_OPERATOR_FRAME_BYTES {
        return Err(std::io::Error::other("operator response exceeds protocol limit").into());
    }
    Ok(payload)
}
