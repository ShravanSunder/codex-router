//! Reminder operations return durable wake identity, never an implied native submission receipt.
use crate::{ClientError, CollaborationClient};
use collaboration_protocol::{WakeFailure, WakeSendRequest, WakeShowRequest, WakeSnapshot};
use serde::{Serialize, de::DeserializeOwned};
#[derive(Debug, thiserror::Error)]
pub enum WakeClientError {
    #[error("wake operation rejected: {0:?}")]
    Rejected(Box<WakeFailure>),
    #[error(transparent)]
    Connection(#[from] ClientError),
}
impl CollaborationClient {
    pub async fn send_wakeup(
        &self,
        request: WakeSendRequest,
    ) -> Result<WakeSnapshot, WakeClientError> {
        self.wakeup_call("wake_send", request).await
    }
    pub async fn read_wakeup(
        &self,
        request: WakeShowRequest,
    ) -> Result<WakeSnapshot, WakeClientError> {
        self.wakeup_call("wake_show", request).await
    }
    pub async fn pause_wakeup(
        &self,
        request: collaboration_protocol::WakeMutationRequest,
    ) -> Result<collaboration_protocol::WakeMutationResult, WakeClientError> {
        self.wakeup_call("wake_pause", request).await
    }
    pub async fn resume_wakeup(
        &self,
        request: collaboration_protocol::WakeMutationRequest,
    ) -> Result<collaboration_protocol::WakeMutationResult, WakeClientError> {
        self.wakeup_call("wake_resume", request).await
    }
    pub async fn cancel_wakeup(
        &self,
        request: collaboration_protocol::WakeMutationRequest,
    ) -> Result<collaboration_protocol::WakeMutationResult, WakeClientError> {
        self.wakeup_call("wake_cancel", request).await
    }
    pub async fn read_delivery(
        &self,
        request: collaboration_protocol::DeliveryShowRequest,
    ) -> Result<collaboration_protocol::DeliveryInspection, WakeClientError> {
        self.wakeup_call("delivery_show", request).await
    }
    pub async fn list_wakeups(
        &self,
        request: collaboration_protocol::AutomationPageRequest,
    ) -> Result<collaboration_protocol::AutomationPage<WakeSnapshot>, WakeClientError> {
        self.wakeup_call("wake_list", request).await
    }
    async fn wakeup_call<TRequest: Serialize, TResponse: DeserializeOwned>(
        &self,
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
