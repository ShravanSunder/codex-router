//! Reminder operations return durable wake identity, never an implied native submission receipt.
use crate::{ClientError, ControlClient};
use communication_protocol::{WakeFailure, WakeSendRequest, WakeShowRequest, WakeSnapshot};
use serde::{Serialize, de::DeserializeOwned};
#[derive(Debug, thiserror::Error)]
pub enum WakeClientError {
    #[error("wake operation rejected: {0:?}")]
    Rejected(Box<WakeFailure>),
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
    pub async fn pause_wakeup(
        &mut self,
        request: communication_protocol::WakeMutationRequest,
    ) -> Result<communication_protocol::WakeMutationResult, WakeClientError> {
        self.wakeup_call("wake/pause", request).await
    }
    pub async fn resume_wakeup(
        &mut self,
        request: communication_protocol::WakeMutationRequest,
    ) -> Result<communication_protocol::WakeMutationResult, WakeClientError> {
        self.wakeup_call("wake/resume", request).await
    }
    pub async fn cancel_wakeup(
        &mut self,
        request: communication_protocol::WakeMutationRequest,
    ) -> Result<communication_protocol::WakeMutationResult, WakeClientError> {
        self.wakeup_call("wake/cancel", request).await
    }
    pub async fn read_delivery(
        &mut self,
        request: communication_protocol::DeliveryShowRequest,
    ) -> Result<communication_protocol::DeliveryInspection, WakeClientError> {
        self.wakeup_call("delivery/show", request).await
    }
    async fn wakeup_call<TRequest: Serialize, TResponse: DeserializeOwned>(
        &mut self,
        method: &str,
        request: TRequest,
    ) -> Result<TResponse, WakeClientError> {
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
