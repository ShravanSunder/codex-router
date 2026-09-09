//! Typed schedule administration; operation identity recovers uncertain local mutations.
use crate::{ClientError, ControlClient};
use communication_protocol::{
    ScheduleCreateRequest, ScheduleEnableRequest, ScheduleFailure, ScheduleShowRequest,
    ScheduleSnapshot, ScheduleUpdateRequest,
};
#[derive(Debug, thiserror::Error)]
pub enum ScheduleClientError {
    #[error("schedule operation rejected: {0:?}")]
    Rejected(Box<ScheduleFailure>),
    #[error(transparent)]
    Connection(#[from] ClientError),
}
impl ControlClient {
    pub async fn create_schedule(
        &mut self,
        request: ScheduleCreateRequest,
    ) -> Result<ScheduleSnapshot, ScheduleClientError> {
        self.schedule_call("schedule/create", request).await
    }
    pub async fn update_schedule(
        &mut self,
        request: ScheduleUpdateRequest,
    ) -> Result<ScheduleSnapshot, ScheduleClientError> {
        self.schedule_call("schedule/update", request).await
    }
    pub async fn read_schedule(
        &mut self,
        request: ScheduleShowRequest,
    ) -> Result<ScheduleSnapshot, ScheduleClientError> {
        self.schedule_call("schedule/show", request).await
    }
    pub async fn enable_schedule(
        &mut self,
        request: ScheduleEnableRequest,
    ) -> Result<ScheduleSnapshot, ScheduleClientError> {
        self.schedule_call("schedule/enable", request).await
    }
    pub async fn disable_schedule(
        &mut self,
        request: ScheduleEnableRequest,
    ) -> Result<ScheduleSnapshot, ScheduleClientError> {
        self.schedule_call("schedule/disable", request).await
    }
    async fn schedule_call<TRequest: serde::Serialize>(
        &mut self,
        method: &str,
        request: TRequest,
    ) -> Result<ScheduleSnapshot, ScheduleClientError> {
        let params = serde_json::to_value(request)
            .map_err(|_| ClientError::Protocol("invalid schedule request"))?;
        let result = match self.connection.call(method, params).await {
            Ok(result) => result,
            Err(ClientError::Rejected {
                code: -32050,
                data: Some(data),
            }) => {
                return Err(ScheduleClientError::Rejected(
                    serde_json::from_value(data)
                        .map_err(|_| ClientError::Protocol("invalid schedule failure"))?,
                ));
            }
            Err(error) => return Err(error.into()),
        };
        serde_json::from_value(result)
            .map_err(|_| ClientError::Protocol("invalid schedule result").into())
    }
}
