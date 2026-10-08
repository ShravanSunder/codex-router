//! The tool calls one listener is running, so its shutdown can stop the calls that outlast
//! their grace period and wait until every call has ended.
//!
//! rmcp runs each request's handler in a task of its own. A handler that does not watch its
//! cancellation keeps running after its caller or the listener is gone, still holding the
//! Router's stores. Every call runs through the registry: shutdown first lets calls finish on
//! their own, then aborts the rest by dropping them, and joins them all before the listener
//! reports that it stopped.
use std::future::Future;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[derive(Clone, Debug, Default)]
pub(crate) struct ToolCallRegistry {
    calls: TaskTracker,
    abort: CancellationToken,
}

impl ToolCallRegistry {
    /// Runs one call. If shutdown aborts the listener's calls first, the call is dropped where
    /// it stands and `aborted` answers in its place.
    pub(crate) async fn run<TOutput>(
        &self,
        call: impl Future<Output = TOutput>,
        aborted: impl FnOnce() -> TOutput,
    ) -> TOutput {
        let abort = self.abort.clone();
        self.calls
            .track_future(async move {
                tokio::select! {
                    biased;
                    () = abort.cancelled() => aborted(),
                    output = call => output,
                }
            })
            .await
    }

    /// Drops every running call and every call that starts from now on.
    pub(crate) fn abort_all(&self) {
        self.abort.cancel();
    }

    /// Calls running now.
    #[cfg(test)]
    pub(crate) fn running(&self) -> usize {
        self.calls.len()
    }

    /// Resolves once no call is running. Calls that start later are still waited for.
    pub(crate) async fn ended(&self) {
        self.calls.close();
        self.calls.wait().await;
    }
}

#[cfg(test)]
mod tests {
    use super::ToolCallRegistry;
    use std::time::Duration;

    #[tokio::test]
    async fn aborting_drops_a_call_that_ignores_cancellation_and_ends_the_registry() {
        // Arrange: a call that never finishes on its own.
        let registry = ToolCallRegistry::default();
        let running = {
            let registry = registry.clone();
            tokio::spawn(async move {
                registry
                    .run(std::future::pending::<&str>(), || "aborted")
                    .await
            })
        };
        tokio::task::yield_now().await;

        // Act
        registry.abort_all();
        let answer = tokio::time::timeout(Duration::from_secs(1), running).await;
        let ended = tokio::time::timeout(Duration::from_secs(1), registry.ended()).await;

        // Assert
        assert_eq!(answer.ok().and_then(Result::ok), Some("aborted"));
        assert!(ended.is_ok(), "the registry still had a running call");
    }

    #[tokio::test]
    async fn a_finished_call_answers_normally_and_leaves_nothing_running() {
        let registry = ToolCallRegistry::default();
        assert_eq!(registry.run(async { "done" }, || "aborted").await, "done");
        let ended = tokio::time::timeout(Duration::from_secs(1), registry.ended()).await;
        assert!(ended.is_ok());
    }
}
