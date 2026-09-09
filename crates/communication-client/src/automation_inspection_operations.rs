//! Bounded inspection methods share typed errors while preserving their distinct record types.
use crate::{ClientError, ControlClient};
use communication_protocol::{
    AutomationInspectionFailure, AutomationPage, AutomationPageRequest, DeliveryInspection,
    DeliveryListRequest, InstructionSnapshot, RevisionListRequest, RevisionRecord, RunListRequest,
    RunSnapshot, ScheduleSnapshot,
};

#[derive(Debug, thiserror::Error)]
pub enum AutomationInspectionClientError {
    #[error("automation inspection rejected: {0:?}")]
    Rejected(Box<AutomationInspectionFailure>),
    #[error(transparent)]
    Connection(#[from] ClientError),
}
impl ControlClient {
    pub async fn read_operation(
        &mut self,
        request: communication_protocol::OperationShowRequest,
    ) -> Result<communication_protocol::OperationSnapshot, AutomationInspectionClientError> {
        let expected = request.operation_id.clone();
        let result: communication_protocol::OperationSnapshot = self
            .automation_inspection_call("operation/show", request)
            .await?;
        if result.operation_id != expected || !result.has_consistent_outcome() {
            self.connection.retire();
            return Err(
                ClientError::Protocol("operation result identity or method mismatch").into(),
            );
        }
        Ok(result)
    }
    pub async fn read_automation_events(
        &mut self,
        request: communication_protocol::AutomationEventsRequest,
    ) -> Result<communication_protocol::AutomationEventsPage, AutomationInspectionClientError> {
        self.automation_inspection_call("automation/events", request)
            .await
    }
    pub async fn read_delivery_attempts(
        &mut self,
        request: communication_protocol::DeliveryAttemptsRequest,
    ) -> Result<
        communication_protocol::AttemptHistoryPage<communication_protocol::AttemptInspection>,
        AutomationInspectionClientError,
    > {
        self.automation_inspection_call("delivery/attempts", request)
            .await
    }
    pub async fn read_run_summaries(
        &mut self,
        request: communication_protocol::RunSummariesRequest,
    ) -> Result<
        communication_protocol::AttemptHistoryPage<communication_protocol::SummaryInspection>,
        AutomationInspectionClientError,
    > {
        self.automation_inspection_call("run/summaries", request)
            .await
    }
    pub async fn list_instructions(
        &mut self,
        request: AutomationPageRequest,
    ) -> Result<AutomationPage<InstructionSnapshot>, AutomationInspectionClientError> {
        self.automation_inspection_call("instruction/list", request)
            .await
    }
    pub async fn list_schedules(
        &mut self,
        request: AutomationPageRequest,
    ) -> Result<AutomationPage<ScheduleSnapshot>, AutomationInspectionClientError> {
        self.automation_inspection_call("schedule/list", request)
            .await
    }
    pub async fn list_runs(
        &mut self,
        request: RunListRequest,
    ) -> Result<AutomationPage<RunSnapshot>, AutomationInspectionClientError> {
        self.automation_inspection_call("run/list", request).await
    }
    pub async fn list_deliveries(
        &mut self,
        request: DeliveryListRequest,
    ) -> Result<AutomationPage<DeliveryInspection>, AutomationInspectionClientError> {
        self.automation_inspection_call("delivery/list", request)
            .await
    }
    pub async fn list_instruction_revisions(
        &mut self,
        request: RevisionListRequest,
    ) -> Result<AutomationPage<RevisionRecord>, AutomationInspectionClientError> {
        self.automation_inspection_call("revision/list", request)
            .await
    }
    pub(crate) async fn automation_inspection_call<
        TRequest: serde::Serialize,
        TResult: serde::de::DeserializeOwned,
    >(
        &mut self,
        method: &str,
        request: TRequest,
    ) -> Result<TResult, AutomationInspectionClientError> {
        let params = serde_json::to_value(request)
            .map_err(|_| ClientError::Protocol("invalid inspection request"))?;
        let result = match self.connection.call(method, params).await {
            Ok(result) => result,
            Err(ClientError::Rejected {
                code: -32050,
                data: Some(data),
            }) => {
                return Err(AutomationInspectionClientError::Rejected(
                    serde_json::from_value(data)
                        .map_err(|_| ClientError::Protocol("invalid inspection error"))?,
                ));
            }
            Err(error) => return Err(error.into()),
        };
        match serde_json::from_value(result) {
            Ok(result) => Ok(result),
            Err(_) => {
                self.connection.retire();
                Err(ClientError::Protocol("invalid inspection response").into())
            }
        }
    }
}
