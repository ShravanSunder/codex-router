//! Request-local two-attempt control; account and pin authority stay in the pipeline.

use bytes::Bytes;
use codex_router_core::attempt_outcome::AttemptOutcome;
use codex_router_core::attempt_outcome::TransportFailure;
use futures_util::future::BoxFuture;
use tokio_util::task::TaskTracker;

use super::forward::BufferedClaudeRequestBody;
use super::forward::ErrorBodyEvidence;
use super::forward::PreparedClaudeResponse;
use super::forward::inspect_response;
use crate::headers::HeaderCollection;
use crate::http_sse::AsyncStreamingHttpProxyResponse;
use crate::http_sse::HttpProxyError;

/// Existing pipeline operations used by the route's bounded attempt policy.
/// Implementations retain credential recovery, restrictions, and pin ownership.
pub(crate) trait ClaudeAttemptPipeline {
    /// Account, credential generation, and pin authority for one attempt.
    type Attempt: Clone + Send + Sync;

    /// Sends precisely these buffered bytes using the attempt's selected credentials.
    fn send<'a>(
        &'a self,
        attempt: &'a Self::Attempt,
        body: Bytes,
    ) -> BoxFuture<'a, Result<AsyncStreamingHttpProxyResponse, TransportFailure>>;

    /// Applies first-attempt restrictions/recovery and returns attempt 2's authority.
    fn recover_first<'a>(
        &'a self,
        attempt: &'a Self::Attempt,
        outcome: &'a AttemptOutcome,
    ) -> BoxFuture<'a, Result<Self::Attempt, HttpProxyError>>;

    /// Records a final attributed failure without recovery or another send.
    fn record_final<'a>(
        &'a self,
        attempt: &'a Self::Attempt,
        outcome: &'a AttemptOutcome,
    ) -> BoxFuture<'a, ()>;
}

/// Final result plus the exact attempt authority needed by pin publication.
pub(crate) struct ClaudeAttemptResult<TAttempt> {
    pub(crate) attempt: TAttempt,
    pub(crate) attempts: u8,
    pub(crate) outcome: AttemptOutcome,
    pub(crate) response: Option<PreparedClaudeResponse>,
}

/// Runs the client-carried route policy while retaining the pipeline's state authority.
pub(crate) async fn run_at_most_two<TPipeline, TClassifier>(
    pipeline: &TPipeline,
    first_attempt: TPipeline::Attempt,
    buffered_body: BufferedClaudeRequestBody,
    classifier: &TClassifier,
    task_tracker: TaskTracker,
) -> Result<ClaudeAttemptResult<TPipeline::Attempt>, HttpProxyError>
where
    TPipeline: ClaudeAttemptPipeline,
    TClassifier: Fn(u16, &HeaderCollection, ErrorBodyEvidence<'_>) -> AttemptOutcome,
{
    let buffered_body = buffered_body.into_bytes();
    let response = match pipeline.send(&first_attempt, buffered_body.clone()).await {
        Ok(response) => inspect_response(response, classifier, task_tracker.clone()).await,
        Err(reason) => {
            return Ok(ClaudeAttemptResult {
                attempt: first_attempt,
                attempts: 1,
                outcome: AttemptOutcome::NoResponse(reason),
                response: None,
            });
        }
    };
    if response.committed() || !is_account_rejection(response.outcome()) {
        return Ok(ClaudeAttemptResult {
            attempt: first_attempt,
            attempts: 1,
            outcome: response.outcome().clone(),
            response: Some(response),
        });
    }
    let first_outcome = response.outcome().clone();
    // Drop the response before recovery; no unread first-attempt bytes reach the client.
    drop(response);
    let second_attempt = pipeline
        .recover_first(&first_attempt, &first_outcome)
        .await?;
    let response = match pipeline.send(&second_attempt, buffered_body).await {
        Ok(response) => inspect_response(response, classifier, task_tracker).await,
        Err(reason) => {
            return Ok(ClaudeAttemptResult {
                attempt: second_attempt,
                attempts: 2,
                outcome: AttemptOutcome::NoResponse(reason),
                response: None,
            });
        }
    };
    let outcome = response.outcome().clone();
    if is_account_rejection(&outcome) {
        pipeline.record_final(&second_attempt, &outcome).await;
    }
    Ok(ClaudeAttemptResult {
        attempt: second_attempt,
        attempts: 2,
        outcome,
        response: Some(response),
    })
}

const fn is_account_rejection(outcome: &AttemptOutcome) -> bool {
    matches!(
        outcome,
        AttemptOutcome::SharedWindowExhausted { .. } | AttemptOutcome::CredentialRejected
    )
}

#[cfg(test)]
mod tests;
