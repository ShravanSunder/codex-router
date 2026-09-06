//! Validate both sides of a typed native call without changing its wire semantics.
use crate::{
    NativeConnectionError, NativeOperation, NativePayloadSchemas, NativeProtocolConnection,
};
use serde_json::Value;

impl NativeProtocolConnection {
    pub async fn request_validated(
        &mut self,
        schemas: &NativePayloadSchemas,
        operation: NativeOperation,
        params: Value,
    ) -> Result<Value, NativeConnectionError> {
        let (input_schema, result_schema) = schemas
            .validators(operation)
            .ok_or(NativeConnectionError::InvalidInput)?;
        if !input_schema.is_valid(&params) {
            return Err(NativeConnectionError::InvalidInput);
        }
        let result = self.request(operation.contract().0, params).await?;
        if !result_schema.is_valid(&result) {
            self.retire();
            return Err(if operation.has_effects() {
                NativeConnectionError::OutcomeUnknown
            } else {
                NativeConnectionError::Protocol
            });
        }
        Ok(result)
    }
}
