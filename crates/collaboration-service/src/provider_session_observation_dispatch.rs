//! Provider observation Control dispatch: decodes each request and calls the typed
//! observation operations.
use crate::ServiceIdentity;
use crate::collaboration_application::{
    ObservationFailure, ObservationOperations, ProviderSessionSubscription,
};
use collaboration_protocol::{BoundedObservationRequest, ProviderSessionListenRequest};
use serde_json::{Value, json};

pub(crate) async fn listen(
    params: Value,
    identity: &ServiceIdentity,
) -> Result<ProviderSessionSubscription, ObservationFailure> {
    let request: ProviderSessionListenRequest =
        serde_json::from_value(params).map_err(|_| ObservationFailure::InvalidField)?;
    ObservationOperations::new(identity)
        .session_listen(request)
        .await
}

pub(crate) async fn observe(id: Value, params: Value, identity: &ServiceIdentity) -> Value {
    let request: BoundedObservationRequest = match serde_json::from_value(params) {
        Ok(request) => request,
        Err(_) => return failure_response(id, ObservationFailure::InvalidField),
    };
    match ObservationOperations::new(identity)
        .session_observe(request)
        .await
    {
        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
        Err(failure) => failure_response(id, failure),
    }
}

/// Control answers a missing Session with -32002 and an invalid request with -32602.
pub(crate) fn failure_response(id: Value, failure: ObservationFailure) -> Value {
    let code = match failure {
        ObservationFailure::InvalidField => -32602,
        ObservationFailure::NotFound => -32002,
        ObservationFailure::Unavailable => -32050,
    };
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":failure.to_string(),"data":failure}})
}
