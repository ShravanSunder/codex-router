//! Connection-wide output accounting retained until the writer finishes each frame.
use serde_json::Value;
use std::{io, sync::Arc};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct AcpOutputSender {
    sender: mpsc::Sender<QueuedAcpFrame>,
    bytes: Arc<Semaphore>,
    closed: CancellationToken,
    maximum_bytes: usize,
}
pub struct QueuedAcpFrame {
    value: Value,
    _bytes: OwnedSemaphorePermit,
}
impl std::ops::Deref for QueuedAcpFrame {
    type Target = Value;
    fn deref(&self) -> &Value {
        &self.value
    }
}
pub fn bounded_acp_output(
    closed: CancellationToken,
) -> (AcpOutputSender, mpsc::Receiver<QueuedAcpFrame>) {
    output_with_limits(closed, 64 * 1024 * 1024, 1024)
}
fn output_with_limits(
    closed: CancellationToken,
    maximum_bytes: usize,
    frames: usize,
) -> (AcpOutputSender, mpsc::Receiver<QueuedAcpFrame>) {
    let (sender, receiver) = mpsc::channel(frames);
    (
        AcpOutputSender {
            sender,
            bytes: Arc::new(Semaphore::new(maximum_bytes)),
            closed,
            maximum_bytes,
        },
        receiver,
    )
}
impl AcpOutputSender {
    /// Overflow closes this frontend connection. Never block native observation on a full queue.
    pub async fn send(&self, value: Value) -> io::Result<()> {
        if self.closed.is_cancelled() {
            return Err(io::Error::other("ACP output closed"));
        }
        let size = serde_json::to_vec(&value).map_err(io::Error::other)?.len();
        if size > self.maximum_bytes {
            self.closed.cancel();
            return Err(io::Error::other("ACP output frame exceeds limit"));
        }
        let count = u32::try_from(size).map_err(io::Error::other)?;
        let permit = match Arc::clone(&self.bytes).try_acquire_many_owned(count) {
            Ok(permit) => permit,
            Err(_) => {
                self.closed.cancel();
                return Err(io::Error::other("ACP output byte budget exceeded"));
            }
        };
        if self
            .sender
            .try_send(QueuedAcpFrame {
                value,
                _bytes: permit,
            })
            .is_err()
        {
            self.closed.cancel();
            return Err(io::Error::other("ACP output queue unavailable"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn writer_retains_bytes_until_frame_is_dropped() {
        let closed = CancellationToken::new();
        let (output, mut frames) = output_with_limits(closed.clone(), 8, 4);
        output
            .send(json!("1234"))
            .await
            .unwrap_or_else(|error| panic!("send: {error}"));
        let writing = frames.recv().await.unwrap_or_else(|| panic!("frame"));
        assert!(output.send(json!("5678")).await.is_err());
        assert!(closed.is_cancelled());
        drop(writing);
    }
    #[tokio::test]
    async fn completed_write_releases_capacity_for_next_frame() {
        let closed = CancellationToken::new();
        let (output, mut frames) = output_with_limits(closed.clone(), 8, 4);
        output
            .send(json!("1234"))
            .await
            .unwrap_or_else(|error| panic!("send: {error}"));
        drop(frames.recv().await.unwrap_or_else(|| panic!("frame")));
        output
            .send(json!("5678"))
            .await
            .unwrap_or_else(|error| panic!("second send: {error}"));
        assert!(!closed.is_cancelled());
    }
}
