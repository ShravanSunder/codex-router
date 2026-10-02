//! Safe ACP error-code classification at the provider client boundary.

use super::{ExternalProviderRuntimeError, ProviderErrorCorrelationId};
use agent_client_protocol::schema::v1::ErrorCode;

pub(crate) fn acp_operation_error(
    error: agent_client_protocol::Error,
) -> ExternalProviderRuntimeError {
    if agent_client_protocol::is_incoming_transport_closed(&error) {
        return ExternalProviderRuntimeError::TransportFailure;
    }
    let code = i64::from(i32::from(error.code));
    match error.code {
        ErrorCode::AuthRequired => {
            let correlation_id = log_provider_error(code, &error.message);
            ExternalProviderRuntimeError::AuthenticationRequired {
                code,
                correlation_id,
            }
        }
        ErrorCode::ResourceNotFound => {
            let correlation_id = log_provider_error(code, &error.message);
            ExternalProviderRuntimeError::ResourceNotFound {
                code,
                correlation_id,
            }
        }
        ErrorCode::MethodNotFound => {
            let correlation_id = log_provider_error(code, &error.message);
            ExternalProviderRuntimeError::UnsupportedMethod {
                code,
                correlation_id,
            }
        }
        ErrorCode::InvalidParams => {
            let correlation_id = log_provider_error(code, &error.message);
            ExternalProviderRuntimeError::InvalidParams {
                code,
                correlation_id,
            }
        }
        ErrorCode::RequestCancelled => {
            let correlation_id = log_provider_error(code, &error.message);
            ExternalProviderRuntimeError::RequestCancelled {
                code,
                correlation_id,
            }
        }
        _ => {
            let correlation_id = log_provider_error(code, &error.message);
            ExternalProviderRuntimeError::ProviderRejected {
                code,
                correlation_id,
            }
        }
    }
}

pub(crate) fn acp_load_session_error(
    error: agent_client_protocol::Error,
) -> ExternalProviderRuntimeError {
    if error.code == ErrorCode::ResourceNotFound {
        let code = i64::from(i32::from(error.code));
        let correlation_id = log_provider_error(code, &error.message);
        return ExternalProviderRuntimeError::ProviderSessionNotFound {
            code,
            correlation_id,
        };
    }
    acp_operation_error(error)
}

fn log_provider_error(code: i64, raw_provider_text: &str) -> ProviderErrorCorrelationId {
    let correlation_id = ProviderErrorCorrelationId::generate();
    tracing::warn!(
        provider_code = code,
        correlation_id = %correlation_id,
        raw_provider_text,
        "provider returned an ACP error"
    );
    correlation_id
}
