//! Inspect exact Runs and explicitly recover their summary work through the collaboration API client.
use crate::{ClientError, CollaborationClient};
use collaboration_protocol::{RunFailure, RunRecoveryRequest, RunShowRequest, RunSnapshot};
#[derive(Debug, thiserror::Error)]
pub enum RunClientError {
    #[error("Run request rejected: {0:?}")]
    Rejected(Box<RunFailure>),
    #[error(transparent)]
    Connection(#[from] ClientError),
}
impl CollaborationClient {
    pub async fn read_run(&self, request: RunShowRequest) -> Result<RunSnapshot, RunClientError> {
        self.run_call("run_show", request).await
    }
    pub async fn retry_summary(
        &self,
        request: RunRecoveryRequest,
    ) -> Result<RunSnapshot, RunClientError> {
        self.run_call("run_summary_retry", request).await
    }
    pub async fn skip_summary(
        &self,
        request: RunRecoveryRequest,
    ) -> Result<RunSnapshot, RunClientError> {
        self.run_call("run_summary_skip", request).await
    }
    async fn run_call<TRequest: serde::Serialize>(
        &self,
        method: &str,
        request: TRequest,
    ) -> Result<RunSnapshot, RunClientError> {
        let params = serde_json::to_value(request)
            .map_err(|_| ClientError::Protocol("invalid Run request"))?;
        let result = match self.connection.call(method, params).await {
            Ok(result) => result,
            Err(ClientError::Rejected {
                code: -32050,
                data: Some(data),
            }) => {
                return Err(RunClientError::Rejected(
                    serde_json::from_value(data)
                        .map_err(|_| ClientError::Protocol("invalid Run failure"))?,
                ));
            }
            Err(error) => return Err(error.into()),
        };
        serde_json::from_value(result)
            .map_err(|_| ClientError::Protocol("invalid Run result").into())
    }
}
