//! Reminder operations return durable wake identity, never an implied native submission receipt.
use crate::{ClientError, ControlClient};
use communication_protocol::{WakeFailure, WakeSendRequest, WakeShowRequest, WakeSnapshot};
use serde::Serialize;
#[derive(Debug, thiserror::Error)]
pub enum WakeClientError {
    #[error("wake operation rejected: {0:?}")]
    Rejected(WakeFailure),
    #[error(transparent)]
    Connection(#[from] ClientError),
}
impl ControlClient {
    pub async fn send_wakeup(
        &mut self,
        request: WakeSendRequest,
    ) -> Result<WakeSnapshot, WakeClientError> {
        self.wakeup_call("wake/send", request).await
    }
    pub async fn read_wakeup(
        &mut self,
        request: WakeShowRequest,
    ) -> Result<WakeSnapshot, WakeClientError> {
        self.wakeup_call("wake/show", request).await
    }
    async fn wakeup_call<TRequest: Serialize>(
        &mut self,
        method: &str,
        request: TRequest,
    ) -> Result<WakeSnapshot, WakeClientError> {
        let params = serde_json::to_value(request)
            .map_err(|_| ClientError::Protocol("invalid wake request"))?;
        let value = match self.connection.call(method, params).await {
            Ok(value) => value,
            Err(ClientError::Rejected {
                code: -32050,
                data: Some(data),
            }) => {
                let failure = serde_json::from_value(data)
                    .map_err(|_| ClientError::Protocol("invalid wake failure"))?;
                return Err(WakeClientError::Rejected(failure));
            }
            Err(error) => return Err(error.into()),
        };
        serde_json::from_value(value)
            .map_err(|_| ClientError::Protocol("invalid wake result").into())
    }
}
