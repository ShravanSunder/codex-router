//! Typed instruction operations; callers retain operation identity across an uncertain connection.
use crate::{ClientError, ControlClient};
use communication_protocol::{
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
impl ControlClient {
    pub async fn create_instruction(
        &mut self,
        params: InstructionCreateParams,
    ) -> Result<InstructionSnapshot, InstructionClientError> {
        self.instruction_call("instruction/create", params).await
    }
    pub async fn update_instruction(
        &mut self,
        params: InstructionUpdateParams,
    ) -> Result<InstructionSnapshot, InstructionClientError> {
        self.instruction_call("instruction/update", params).await
    }
    pub async fn read_instruction(
        &mut self,
        params: InstructionShowParams,
    ) -> Result<InstructionSnapshot, InstructionClientError> {
        self.instruction_call("instruction/show", params).await
    }
    async fn instruction_call<TParams: Serialize>(
        &mut self,
        method: &str,
        params: TParams,
    ) -> Result<InstructionSnapshot, InstructionClientError> {
        let params = serde_json::to_value(params)
            .map_err(|_| ClientError::Protocol("invalid instruction request"))?;
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
            Err(error) => return Err(error.into()),
        };
        serde_json::from_value(value)
            .map_err(|_| ClientError::Protocol("invalid instruction result").into())
    }
}
