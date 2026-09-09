//! Inspect exact Runs and explicitly recover their summary work through the public Control client.
use crate::{ClientError, ControlClient};
use communication_protocol::{RunFailure, RunRecoveryRequest, RunShowRequest, RunSnapshot};
#[derive(Debug, thiserror::Error)]
pub enum RunClientError {
    #[error("Run request rejected: {0:?}")]
    Rejected(Box<RunFailure>),
    #[error(transparent)]
    Connection(#[from] ClientError),
}
impl ControlClient {
    pub async fn read_run(
        &mut self,
        request: RunShowRequest,
    ) -> Result<RunSnapshot, RunClientError> {
        self.run_call("run/show", request).await
    }
    pub async fn retry_summary(
        &mut self,
        request: RunRecoveryRequest,
    ) -> Result<RunSnapshot, RunClientError> {
        self.run_call("run/summaryRetry", request).await
    }
    pub async fn skip_summary(
        &mut self,
        request: RunRecoveryRequest,
    ) -> Result<RunSnapshot, RunClientError> {
        self.run_call("run/summarySkip", request).await
    }
    async fn run_call<TRequest: serde::Serialize>(
        &mut self,
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
