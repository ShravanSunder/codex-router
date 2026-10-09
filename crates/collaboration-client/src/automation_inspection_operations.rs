//! Bounded inspection methods share typed errors while preserving their distinct record types.
use crate::{ClientError, CollaborationClient};
use collaboration_protocol::{
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
impl CollaborationClient {
    /// Observe the exact recorded worker/summary turn without starting or interrupting native work.
    pub async fn reconcile_run(
        &self,
        request: collaboration_protocol::RunShowRequest,
    ) -> Result<RunSnapshot, AutomationInspectionClientError> {
        let expected = request.run_id.clone();
        let result: RunSnapshot = self
            .automation_inspection_call("run_reconcile", request)
            .await?;
        if result.run_id != expected {
            return Err(ClientError::Protocol("Run reconciliation identity mismatch").into());
        }
        Ok(result)
    }
    /// Observe an uncertain delivery without resending input. Absence remains uncertain.
    pub async fn reconcile_delivery(
        &self,
        request: collaboration_protocol::DeliveryShowRequest,
    ) -> Result<DeliveryInspection, AutomationInspectionClientError> {
        let expected = request.delivery_id.clone();
        let result: DeliveryInspection = self
            .automation_inspection_call("delivery_reconcile", request)
            .await?;
        if result.delivery_id != expected {
            return Err(ClientError::Protocol("delivery reconciliation identity mismatch").into());
        }
        Ok(result)
    }
    pub async fn read_operation(
        &self,
        request: collaboration_protocol::OperationShowRequest,
    ) -> Result<collaboration_protocol::OperationSnapshot, AutomationInspectionClientError> {
        self.inspect_operation("operation_show", request).await
    }
    /// Reconcile original evidence without replaying native input or allocating a thread.
    pub async fn reconcile_operation(
        &self,
        request: collaboration_protocol::OperationShowRequest,
    ) -> Result<collaboration_protocol::OperationSnapshot, AutomationInspectionClientError> {
        self.inspect_operation("operation_reconcile", request).await
    }
    async fn inspect_operation(
        &self,
        method: &str,
        request: collaboration_protocol::OperationShowRequest,
    ) -> Result<collaboration_protocol::OperationSnapshot, AutomationInspectionClientError> {
        let expected = request.operation_id.clone();
        let result: collaboration_protocol::OperationSnapshot =
            self.automation_inspection_call(method, request).await?;
        if result.operation_id != expected || !result.has_consistent_outcome() {
            return Err(
                ClientError::Protocol("operation result identity or method mismatch").into(),
            );
        }
        Ok(result)
    }
    pub async fn read_automation_events(
        &self,
        request: collaboration_protocol::AutomationEventsRequest,
    ) -> Result<collaboration_protocol::AutomationEventsPage, AutomationInspectionClientError> {
        self.automation_inspection_call("automation_events", request)
            .await
    }
    pub async fn read_delivery_attempts(
        &self,
        request: collaboration_protocol::DeliveryAttemptsRequest,
    ) -> Result<
        collaboration_protocol::AttemptHistoryPage<collaboration_protocol::AttemptInspection>,
        AutomationInspectionClientError,
    > {
        self.automation_inspection_call("delivery_attempts", request)
            .await
    }
    pub async fn read_run_summaries(
        &self,
        request: collaboration_protocol::RunSummariesRequest,
    ) -> Result<
        collaboration_protocol::AttemptHistoryPage<collaboration_protocol::SummaryInspection>,
        AutomationInspectionClientError,
    > {
        self.automation_inspection_call("run_summaries", request)
            .await
    }
    pub async fn list_instructions(
        &self,
        request: AutomationPageRequest,
    ) -> Result<AutomationPage<InstructionSnapshot>, AutomationInspectionClientError> {
        self.automation_inspection_call("instruction_list", request)
            .await
    }
    pub async fn list_schedules(
        &self,
        request: AutomationPageRequest,
    ) -> Result<AutomationPage<ScheduleSnapshot>, AutomationInspectionClientError> {
        self.automation_inspection_call("schedule_list", request)
            .await
    }
    pub async fn list_runs(
        &self,
        request: RunListRequest,
    ) -> Result<AutomationPage<RunSnapshot>, AutomationInspectionClientError> {
        self.automation_inspection_call("run_list", request).await
    }
    pub async fn list_deliveries(
        &self,
        request: DeliveryListRequest,
    ) -> Result<AutomationPage<DeliveryInspection>, AutomationInspectionClientError> {
        self.automation_inspection_call("delivery_list", request)
            .await
    }
    pub async fn list_instruction_revisions(
        &self,
        request: RevisionListRequest,
    ) -> Result<AutomationPage<RevisionRecord>, AutomationInspectionClientError> {
        self.automation_inspection_call("revision_list", request)
            .await
    }
    pub(crate) async fn automation_inspection_call<
        TRequest: serde::Serialize,
        TResult: serde::de::DeserializeOwned,
    >(
        &self,
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
            Err(ClientError::Overloaded { message }) => {
                return Err(AutomationInspectionClientError::Rejected(
                    crate::admission_overload::inspection(message).into(),
                ));
            }
            Err(error) => return Err(error.into()),
        };
        match serde_json::from_value(result) {
            Ok(result) => Ok(result),
            Err(_) => Err(ClientError::Protocol("invalid inspection response").into()),
        }
    }
}
