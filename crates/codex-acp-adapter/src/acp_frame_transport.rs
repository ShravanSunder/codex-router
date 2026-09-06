//! Bounded ACP JSONL carrier; ACP IDs and envelopes are not Control's string-only dialect.
use serde_json::Value;
use std::io;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

const FRAME_LIMIT: usize = 64 * 1024 * 1024;

pub async fn read_acp_frame<TReader: AsyncBufRead + Unpin>(
    reader: &mut TReader,
) -> io::Result<Option<Value>> {
    let mut frame = Vec::new();
    loop {
        let buffer = reader.fill_buf().await?;
        if buffer.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::other("truncated ACP frame"))
            };
        }
        let end = buffer.iter().position(|byte| *byte == b'\n');
        let count = end.unwrap_or(buffer.len());
        if count > FRAME_LIMIT.saturating_sub(frame.len()) {
            return Err(io::Error::other("ACP frame exceeds limit"));
        }
        frame.extend_from_slice(
            buffer
                .get(..count)
                .ok_or_else(|| io::Error::other("ACP read boundary"))?,
        );
        reader.consume(count + usize::from(end.is_some()));
        if end.is_some() {
            break;
        }
    }
    let value: Value =
        serde_json::from_slice(&frame).map_err(|_| io::Error::other("invalid ACP JSON"))?;
    if !value.is_object() {
        return Err(io::Error::other("ACP requires an individual object"));
    }
    Ok(Some(value))
}
pub async fn write_acp_frame<TWriter: AsyncWrite + Unpin>(
    writer: &mut TWriter,
    value: &Value,
) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    if bytes.len() > FRAME_LIMIT {
        return Err(io::Error::other("ACP frame exceeds limit"));
    }
    bytes.push(b'\n');
    tokio::time::timeout(std::time::Duration::from_secs(30), writer.write_all(&bytes))
        .await
        .map_err(|_| io::Error::other("ACP slow reader"))??;
    Ok(())
}
