//! Safe ACP error-code classification at the provider client boundary.

use super::ExternalProviderRuntimeError;
use agent_client_protocol::schema::v1::ErrorCode;

pub(crate) fn acp_operation_error(
    error: agent_client_protocol::Error,
) -> ExternalProviderRuntimeError {
    if agent_client_protocol::is_incoming_transport_closed(&error) {
        return ExternalProviderRuntimeError::TransportFailure;
    }
    let code = i64::from(i32::from(error.code));
    match error.code {
        ErrorCode::AuthRequired => ExternalProviderRuntimeError::AuthenticationRequired { code },
        ErrorCode::ResourceNotFound => ExternalProviderRuntimeError::ResourceNotFound { code },
        ErrorCode::MethodNotFound => ExternalProviderRuntimeError::UnsupportedMethod { code },
        ErrorCode::InvalidParams => ExternalProviderRuntimeError::InvalidParams { code },
        ErrorCode::RequestCancelled => ExternalProviderRuntimeError::RequestCancelled { code },
        _ => ExternalProviderRuntimeError::ProviderRejected { code },
    }
}

pub(crate) fn acp_load_session_error(
    error: agent_client_protocol::Error,
) -> ExternalProviderRuntimeError {
    if error.code == ErrorCode::ResourceNotFound {
        return ExternalProviderRuntimeError::ProviderSessionNotFound {
            code: i64::from(i32::from(error.code)),
        };
    }
    acp_operation_error(error)
}
