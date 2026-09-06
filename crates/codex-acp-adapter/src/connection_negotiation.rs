//! ACP version negotiation for the complete Codex conversation dispatcher.
use crate::{AcpSchemaCatalog, AcpSchemaError};
use serde_json::{Value, json};

#[derive(Default)]
pub struct AcpNegotiation {
    attempted: bool,
    initialized: bool,
}
#[derive(Debug, thiserror::Error)]
pub enum AcpNegotiationError {
    #[error("invalid ACP initialization request")]
    InvalidRequest,
    #[error("invalid ACP initialization parameters")]
    InvalidParameters,
    #[error("ACP initialization was already attempted")]
    AlreadyInitialized,
    #[error(transparent)]
    Schema(#[from] AcpSchemaError),
}
impl AcpNegotiation {
    /// Only the complete dispatcher may expose this advertised conversation capability set.
    pub fn initialize(
        &mut self,
        catalog: &mut AcpSchemaCatalog,
        request: &Value,
    ) -> Result<Value, AcpNegotiationError> {
        if self.attempted {
            return Err(AcpNegotiationError::AlreadyInitialized);
        }
        if request.get("jsonrpc") != Some(&json!("2.0"))
            || request.get("method") != Some(&json!("initialize"))
        {
            return Err(AcpNegotiationError::InvalidRequest);
        }
        let id = request
            .get("id")
            .ok_or(AcpNegotiationError::InvalidRequest)?;
        if !(id.is_null() || id.is_string() || id.as_i64().is_some()) {
            return Err(AcpNegotiationError::InvalidRequest);
        }
        let params = request
            .get("params")
            .ok_or(AcpNegotiationError::InvalidParameters)?;
        if !catalog.validate("InitializeRequest", params)? {
            return Err(AcpNegotiationError::InvalidParameters);
        }
        self.attempted = true;
        let result = json!({
            "protocolVersion":1,
            "agentCapabilities":{
                "loadSession":true,
                "promptCapabilities":{"image":false,"audio":false,"embeddedContext":false},
                "mcpCapabilities":{"http":false,"sse":false,"acp":false},
                "sessionCapabilities":{"list":{}}
            },
            "authMethods":[],
            "agentInfo":{"name":"codex-router","title":"Codex Router ACP","version":env!("CARGO_PKG_VERSION")}
        });
        if !catalog.validate("InitializeResponse", &result)? {
            return Err(AcpNegotiationError::InvalidParameters);
        }
        // ACP returns the supported version; the client decides whether it can continue.
        self.initialized = true;
        Ok(json!({"jsonrpc":"2.0","id":id,"result":result}))
    }
    #[must_use]
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}
