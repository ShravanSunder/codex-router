//! Typed schedule administration; operation identity recovers uncertain local mutations.
use crate::{ClientError, CollaborationClient};
use collaboration_protocol::{
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
impl CollaborationClient {
    pub async fn import_schedule(
        &self,
        request: collaboration_protocol::ScheduleImportRequest,
    ) -> Result<ScheduleSnapshot, ScheduleClientError> {
        self.schedule_call("schedule_import", request).await
    }
    pub async fn export_schedule(
        &self,
        request: ScheduleShowRequest,
    ) -> Result<collaboration_protocol::ScheduleExportResult, ScheduleClientError> {
        self.schedule_call("schedule_export", request).await
    }
    pub async fn create_schedule(
        &self,
        request: ScheduleCreateRequest,
    ) -> Result<ScheduleSnapshot, ScheduleClientError> {
        self.schedule_call("schedule_create", request).await
    }
    pub async fn update_schedule(
        &self,
        request: ScheduleUpdateRequest,
    ) -> Result<ScheduleSnapshot, ScheduleClientError> {
        self.schedule_call("schedule_update", request).await
    }
    pub async fn read_schedule(
        &self,
        request: ScheduleShowRequest,
    ) -> Result<ScheduleSnapshot, ScheduleClientError> {
        self.schedule_call("schedule_show", request).await
    }
    pub async fn enable_schedule(
        &self,
        request: ScheduleEnableRequest,
    ) -> Result<ScheduleSnapshot, ScheduleClientError> {
        self.schedule_call("schedule_enable", request).await
    }
    pub async fn disable_schedule(
        &self,
        request: ScheduleEnableRequest,
    ) -> Result<ScheduleSnapshot, ScheduleClientError> {
        self.schedule_call("schedule_disable", request).await
    }
    pub async fn prepare_schedule(
        &self,
        request: collaboration_protocol::SchedulePrepareRequest,
    ) -> Result<ScheduleSnapshot, ScheduleClientError> {
        self.schedule_call("schedule_prepare", request).await
    }
    async fn schedule_call<TRequest: serde::Serialize, TResult: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        request: TRequest,
    ) -> Result<TResult, ScheduleClientError> {
        let params = serde_json::to_value(request)
            .map_err(|_| ClientError::Protocol("invalid schedule request"))?;
        if method == "schedule_import" {
            let encoded_bytes = serde_json::to_vec(&params)
                .map(|bytes| bytes.len())
                .map_err(|_| ClientError::Protocol("invalid schedule request"))?;
            if encoded_bytes > collaboration_protocol::MAX_CONTROL_FRAME_BYTES {
                let operation_id = params
                    .get("operationId")
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok());
                return Err(ScheduleClientError::Rejected(Box::new(
                    ScheduleFailure::package_frame_limit(operation_id, encoded_bytes),
                )));
            }
        }
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
