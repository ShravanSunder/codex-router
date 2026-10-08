//! Typed instruction operations; callers retain operation identity across an uncertain connection.
use crate::{ClientError, CollaborationClient};
use collaboration_protocol::{
    InstructionCreateParams, InstructionFailure, InstructionShowParams, InstructionSnapshot,
    InstructionUpdateParams,
};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum InstructionClientError {
    #[error("instruction operation rejected: {0:?}")]
    Rejected(InstructionFailure),
    #[error(transparent)]
    Connection(#[from] ClientError),
}
impl CollaborationClient {
    pub async fn create_instruction(
        &self,
        params: InstructionCreateParams,
    ) -> Result<InstructionSnapshot, InstructionClientError> {
        self.instruction_call("instruction_create", params).await
    }
    pub async fn update_instruction(
        &self,
        params: InstructionUpdateParams,
    ) -> Result<InstructionSnapshot, InstructionClientError> {
        self.instruction_call("instruction_update", params).await
    }
    pub async fn read_instruction(
        &self,
        params: InstructionShowParams,
    ) -> Result<InstructionSnapshot, InstructionClientError> {
        self.instruction_call("instruction_show", params).await
    }
    async fn instruction_call<TParams: Serialize>(
        &self,
        method: &str,
        params: TParams,
    ) -> Result<InstructionSnapshot, InstructionClientError> {
        let params = serde_json::to_value(params)
            .map_err(|_| ClientError::Protocol("invalid instruction request"))?;
        let shed = crate::admission_overload::ShedRequest::of(&params);
        let value = match self.connection.call(method, params).await {
            Ok(value) => value,
            Err(ClientError::Rejected {
                code: -32050,
                data: Some(data),
            }) => {
                let failure = serde_json::from_value(data)
                    .map_err(|_| ClientError::Protocol("invalid instruction error"))?;
                return Err(InstructionClientError::Rejected(failure));
            }
            Err(ClientError::Overloaded { message }) => {
                return Err(InstructionClientError::Rejected(
                    crate::admission_overload::instruction(message, &shed),
                ));
            }
            Err(error) => return Err(error.into()),
        };
        serde_json::from_value(value)
            .map_err(|_| ClientError::Protocol("invalid instruction result").into())
    }
}
