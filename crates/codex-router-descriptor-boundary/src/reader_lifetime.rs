//! Parent-owned waiter outlives the connection/codec future and reaps only its reader child.
use crate::BoundaryError;
use std::process::ExitStatus;
use tokio::sync::oneshot;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
pub struct ReaderLease {
    cancellation: CancellationToken,
    completion: Option<oneshot::Receiver<Result<ExitStatus, BoundaryError>>>,
}
pub fn supervise_reader(mut child: tokio::process::Child, tracker: &TaskTracker) -> ReaderLease {
    let cancellation = CancellationToken::new();
    let stopping = cancellation.clone();
    let (send, receive) = oneshot::channel();
    tracker.spawn(async move {
        let result = tokio::select! {
            biased;
            () = stopping.cancelled() => {
                let killed = child.start_kill();
                // Always reap, including the race where natural exit won before start_kill.
                let waited = child.wait().await;
                match waited { Ok(status) => Ok(status), Err(error) => Err(BoundaryError::Io(killed.err().unwrap_or(error))) }
            },
            result = child.wait() => result.map_err(BoundaryError::Io),
        };
        let _delivered = send.send(result);
    });
    ReaderLease {
        cancellation,
        completion: Some(receive),
    }
}
impl ReaderLease {
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }
    pub async fn wait(&mut self) -> Result<ExitStatus, BoundaryError> {
        let completion = self.completion.as_mut().ok_or(BoundaryError::WaitTask)?;
        let result = completion.await.map_err(|_| BoundaryError::WaitTask);
        self.completion = None;
        result?
    }
}
impl Drop for ReaderLease {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}
