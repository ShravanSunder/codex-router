//! Typed Control dispatch for Host-owned external provider conversations.
mod codex_conversation_inspection;
use crate::{ProviderConversationBackend, ServiceIdentity};
use codex_conversation_inspection::{
    codex_reconcile, codex_record, codex_snapshot_response, codex_wait,
};
use collaboration_protocol::{
    ConversationCancelRequest, ConversationCreateRequest, ConversationLoadRequest,
    ConversationOperationFailure, ConversationOperationFailureKind,
    ConversationOperationFailureStage, ConversationOperationReconcileRequest,
    ConversationOperationShowRequest, ConversationOperationWaitRequest, ConversationPromptRequest,
    EndpointAvailability, EndpointRef, NonEmptyText, OperationId, ProviderBindingIdentity,
    ProviderOperationEffect, SessionRef,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

pub(crate) async fn dispatch(
    id: Value,
    method: &str,
    params: Value,
    identity: &ServiceIdentity,
) -> Value {
    match method {
        "conversation/create" => dispatch_create(id, parse(params), identity).await,
        "conversation/load" => dispatch_load(id, parse(params), identity).await,
        "conversation/prompt" => dispatch_prompt(id, parse(params), identity).await,
        "conversation/cancel" => dispatch_cancel(id, parse(params), identity).await,
        "conversation/operationShow" => {
            let request = parse::<ConversationOperationShowRequest>(params);
            if let Ok(request) = &request {
                match codex_record(&request.operation_id, identity).await {
                    Ok(Some(record)) => return codex_snapshot_response(id, record),
                    Err(()) => {
                        return local_failure_response(
                            id,
                            LocalFailure::Unavailable,
                            request.operation_id.clone(),
                            None,
                        );
                    }
                    Ok(None) => {}
                }
            }
            dispatch_read(id, request, identity, |backend, request| {
                backend.show(request)
            })
            .await
        }
        "conversation/operationWait" => {
            let request = parse::<ConversationOperationWaitRequest>(params);
            if let Ok(request) = &request {
                match codex_record(&request.operation_id, identity).await {
                    Ok(Some(_)) => return codex_wait(id, request, identity).await,
                    Err(()) => {
                        return local_failure_response(
                            id,
                            LocalFailure::Unavailable,
                            request.operation_id.clone(),
                            None,
                        );
                    }
                    Ok(None) => {}
                }
            }
            dispatch_read(id, request, identity, |backend, request| {
                backend.wait(request)
            })
            .await
        }
        "conversation/operationReconcile" => {
            let request = parse::<ConversationOperationReconcileRequest>(params);
            if let Ok(request) = &request {
                match codex_record(&request.operation_id, identity).await {
                    Ok(Some(_)) => {
                        return codex_reconcile(id, &request.operation_id, identity).await;
                    }
                    Err(()) => {
                        return local_failure_response(
                            id,
                            LocalFailure::Unavailable,
                            request.operation_id.clone(),
                            None,
                        );
                    }
                    Ok(None) => {}
                }
            }
            dispatch_read(id, request, identity, |backend, request| {
                backend.reconcile(request)
            })
            .await
        }
        _ => json_rpc_error(id, -32601, "Method not found"),
    }
}

pub(crate) fn overloaded(id: Value, method: &str, params: Value) -> Value {
    let operation = match method {
        "conversation/create" => {
            parse::<ConversationCreateRequest>(params).map(|request| (request.operation_id, None))
        }
        "conversation/load" => parse::<ConversationLoadRequest>(params)
            .map(|request| (request.operation_id, Some(request.target))),
        "conversation/prompt" => parse::<ConversationPromptRequest>(params)
            .map(|request| (request.operation_id, Some(request.target))),
        "conversation/cancel" => parse::<ConversationCancelRequest>(params)
            .map(|request| (request.operation_id, Some(request.target))),
        "conversation/operationShow" => parse::<ConversationOperationShowRequest>(params)
            .map(|request| (request.operation_id, None)),
        "conversation/operationWait" => parse::<ConversationOperationWaitRequest>(params)
            .map(|request| (request.operation_id, None)),
        "conversation/operationReconcile" => parse::<ConversationOperationReconcileRequest>(params)
            .map(|request| (request.operation_id, None)),
        _ => return json_rpc_error(id, -32601, "Method not found"),
    };
    let Ok((operation_id, target)) = operation else {
        return invalid_params(id);
    };
    local_failure_response(id, LocalFailure::Busy, operation_id, target)
}

fn parse<T: DeserializeOwned>(params: Value) -> Result<T, ()> {
    serde_json::from_value(params).map_err(|_| ())
}

async fn dispatch_create(
    id: Value,
    request: Result<ConversationCreateRequest, ()>,
    identity: &ServiceIdentity,
) -> Value {
    let Ok(mut request) = request else {
        return invalid_params(id);
    };
    let target = None;
    let Some(backend) = identity.provider_conversations.as_deref() else {
        return unavailable_provider_response(
            id,
            request.operation_id,
            target,
            &request.endpoint,
            identity,
        );
    };
    if request.endpoint.service_id != identity.service_id
        || !actor_matches(&request.created_by, identity)
        || !actor_matches(&request.approver, identity)
    {
        return local_failure_response(
            id,
            LocalFailure::InvalidIdentity,
            request.operation_id,
            target,
        );
    }
    if let Some(response) = duplicate_submission(&id, backend, &request.operation_id).await {
        return response;
    }
    if !matches!(
        provider_unavailability(&request.endpoint, identity),
        Ok(None)
    ) {
        return unavailable_provider_response(
            id,
            request.operation_id,
            target,
            &request.endpoint,
            identity,
        );
    }
    let Some(binding) = backend.binding(&request.endpoint) else {
        return local_failure_response(
            id,
            LocalFailure::InvalidBinding,
            request.operation_id,
            target,
        );
    };
    if binding.endpoint != request.endpoint {
        return local_failure_response(
            id,
            LocalFailure::InvalidIdentity,
            request.operation_id,
            target,
        );
    }
    if request
        .generation
        .as_ref()
        .is_some_and(|expected| expected != &binding.generation)
    {
        return local_failure_response(
            id,
            LocalFailure::StaleGeneration,
            request.operation_id,
            target,
        );
    }
    request.generation = Some(binding.generation.clone());
    result_response(id, backend.create(request).await)
}

async fn dispatch_load(
    id: Value,
    request: Result<ConversationLoadRequest, ()>,
    identity: &ServiceIdentity,
) -> Value {
    let Ok(mut request) = request else {
        return invalid_params(id);
    };
    let target = Some(request.target.clone());
    let Some(backend) = identity.provider_conversations.as_deref() else {
        return unavailable_provider_response(
            id,
            request.operation_id,
            target,
            &request.target.endpoint,
            identity,
        );
    };
    if request.target.endpoint.service_id != identity.service_id
        || !actor_matches(&request.requested_by, identity)
        || !actor_matches(&request.approver, identity)
    {
        return local_failure_response(
            id,
            LocalFailure::InvalidIdentity,
            request.operation_id,
            target,
        );
    }
    if let Some(response) = duplicate_submission(&id, backend, &request.operation_id).await {
        return response;
    }
    if !matches!(
        provider_unavailability(&request.target.endpoint, identity),
        Ok(None)
    ) {
        return unavailable_provider_response(
            id,
            request.operation_id,
            target,
            &request.target.endpoint,
            identity,
        );
    }
    let Some(binding) = backend.binding(&request.target.endpoint) else {
        return local_failure_response(
            id,
            LocalFailure::InvalidBinding,
            request.operation_id,
            target,
        );
    };
    if !target_matches(&request.target, &binding, identity) {
        return local_failure_response(
            id,
            LocalFailure::InvalidIdentity,
            request.operation_id,
            target,
        );
    }
    if request
        .generation
        .as_ref()
        .is_some_and(|expected| expected != &binding.generation)
    {
        return local_failure_response(
            id,
            LocalFailure::StaleGeneration,
            request.operation_id,
            target,
        );
    }
    request.generation = Some(binding.generation.clone());
    result_response(id, backend.load(request).await)
}

async fn dispatch_prompt(
    id: Value,
    request: Result<ConversationPromptRequest, ()>,
    identity: &ServiceIdentity,
) -> Value {
    let Ok(mut request) = request else {
        return invalid_params(id);
    };
    let target = Some(request.target.clone());
    let Some(backend) = identity.provider_conversations.as_deref() else {
        return unavailable_provider_response(
            id,
            request.operation_id,
            target,
            &request.target.endpoint,
            identity,
        );
    };
    if request.target.endpoint.service_id != identity.service_id
        || !actor_matches(&request.requested_by, identity)
        || !actor_matches(&request.approver, identity)
    {
        return local_failure_response(
            id,
            LocalFailure::InvalidIdentity,
            request.operation_id,
            target,
        );
    }
    if let Some(response) = duplicate_submission(&id, backend, &request.operation_id).await {
        return response;
    }
    if !matches!(
        provider_unavailability(&request.target.endpoint, identity),
        Ok(None)
    ) {
        return unavailable_provider_response(
            id,
            request.operation_id,
            target,
            &request.target.endpoint,
            identity,
        );
    }
    let Some(binding) = backend.binding(&request.target.endpoint) else {
        return local_failure_response(
            id,
            LocalFailure::InvalidBinding,
            request.operation_id,
            target,
        );
    };
    if !target_matches(&request.target, &binding, identity) {
        return local_failure_response(
            id,
            LocalFailure::InvalidIdentity,
            request.operation_id,
            target,
        );
    }
    if request
        .generation
        .as_ref()
        .is_some_and(|expected| expected != &binding.generation)
    {
        return local_failure_response(
            id,
            LocalFailure::StaleGeneration,
            request.operation_id,
            target,
        );
    }
    request.generation = Some(binding.generation.clone());
    result_response(id, backend.prompt(request).await)
}

async fn dispatch_cancel(
    id: Value,
    request: Result<ConversationCancelRequest, ()>,
    identity: &ServiceIdentity,
) -> Value {
    let Ok(mut request) = request else {
        return invalid_params(id);
    };
    let target = Some(request.target.clone());
    let Some(backend) = identity.provider_conversations.as_deref() else {
        return unavailable_provider_response(
            id,
            request.operation_id,
            target,
            &request.target.endpoint,
            identity,
        );
    };
    if request.target.endpoint.service_id != identity.service_id
        || !actor_matches(&request.requested_by, identity)
        || !actor_matches(&request.approver, identity)
    {
        return local_failure_response(
            id,
            LocalFailure::InvalidIdentity,
            request.operation_id,
            target,
        );
    }
    if let Some(response) = duplicate_submission(&id, backend, &request.operation_id).await {
        return response;
    }
    if !matches!(
        provider_unavailability(&request.target.endpoint, identity),
        Ok(None)
    ) {
        return unavailable_provider_response(
            id,
            request.operation_id,
            target,
            &request.target.endpoint,
            identity,
        );
    }
    let Some(binding) = backend.binding(&request.target.endpoint) else {
        return local_failure_response(
            id,
            LocalFailure::InvalidBinding,
            request.operation_id,
            target,
        );
    };
    if !target_matches(&request.target, &binding, identity) {
        return local_failure_response(
            id,
            LocalFailure::InvalidIdentity,
            request.operation_id,
            target,
        );
    }
    if request
        .generation
        .as_ref()
        .is_some_and(|expected| expected != &binding.generation)
    {
        return local_failure_response(
            id,
            LocalFailure::StaleGeneration,
            request.operation_id,
            target,
        );
    }
    request.generation = Some(binding.generation.clone());
    result_response(id, backend.cancel(request).await)
}

async fn duplicate_submission(
    id: &Value,
    backend: &dyn ProviderConversationBackend,
    operation_id: &OperationId,
) -> Option<Value> {
    match backend.lookup_existing(operation_id.clone()).await {
        Ok(Some(operation)) => Some(result_response(
            id.clone(),
            Ok(collaboration_protocol::ConversationOperationSubmission {
                admission: collaboration_protocol::ConversationAdmissionState::Existing,
                operation,
            }),
        )),
        Ok(None) => None,
        Err(failure) => Some(result_response(
            id.clone(),
            Err::<collaboration_protocol::ConversationOperationSubmission, _>(failure),
        )),
    }
}

async fn dispatch_read<Request, Response, Call>(
    id: Value,
    request: Result<Request, ()>,
    identity: &ServiceIdentity,
    call: Call,
) -> Value
where
    Request: OperationRequest,
    Response: serde::Serialize,
    Call: for<'a> FnOnce(
        &'a dyn ProviderConversationBackend,
        Request,
    ) -> crate::ProviderConversationFuture<'a, Response>,
{
    let Ok(request) = request else {
        return invalid_params(id);
    };
    let operation_id = request.operation_id().clone();
    let Some(backend) = identity.provider_conversations.as_deref() else {
        return local_failure_response(id, LocalFailure::Unavailable, operation_id, None);
    };
    result_response(id, call(backend, request).await)
}

trait OperationRequest {
    fn operation_id(&self) -> &OperationId;
}
impl OperationRequest for ConversationOperationShowRequest {
    fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }
}
impl OperationRequest for ConversationOperationWaitRequest {
    fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }
}
impl OperationRequest for ConversationOperationReconcileRequest {
    fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }
}

fn actor_matches(actor: &SessionRef, identity: &ServiceIdentity) -> bool {
    actor.endpoint.service_id == identity.service_id
}

fn target_matches(
    target: &SessionRef,
    binding: &ProviderBindingIdentity,
    identity: &ServiceIdentity,
) -> bool {
    target.endpoint == binding.endpoint && target.endpoint.service_id == identity.service_id
}

fn provider_unavailability(
    endpoint: &EndpointRef,
    identity: &ServiceIdentity,
) -> Result<Option<EndpointAvailability>, std::io::Error> {
    let description = identity.endpoint_directory().read_endpoint(endpoint)?;
    Ok(
        description.and_then(|description| match description.availability {
            unavailable @ EndpointAvailability::Unavailable { .. } => Some(unavailable),
            EndpointAvailability::Available { .. } | EndpointAvailability::Unprobed => None,
        }),
    )
}

fn unavailable_provider_response(
    id: Value,
    operation_id: OperationId,
    target: Option<SessionRef>,
    endpoint: &EndpointRef,
    identity: &ServiceIdentity,
) -> Value {
    let availability = provider_unavailability(endpoint, identity).ok().flatten();
    let endpoint_id = String::from(endpoint.endpoint_id.clone());
    let message = match &availability {
        Some(EndpointAvailability::Unavailable { reason, .. }) => {
            format!(
                "provider conversation endpoint {endpoint_id} unavailable: {}",
                String::from(reason.clone())
            )
        }
        _ => format!("provider conversation endpoint {endpoint_id} unavailable"),
    };
    let message = NonEmptyText::try_from(message).or_else(|_| {
        NonEmptyText::try_from(format!("provider endpoint {endpoint_id} unavailable"))
    });
    let Ok(message) = message else {
        return json_rpc_error(id, -32603, "Internal error");
    };
    failure_response(
        id,
        ConversationOperationFailure {
            kind: ConversationOperationFailureKind::Unavailable,
            stage: ConversationOperationFailureStage::Binding,
            effect: ProviderOperationEffect::None,
            message,
            operation_id,
            provider_code: None,
            target,
            endpoint: Some(endpoint.clone()),
            availability,
        },
    )
}

fn local_failure_response(
    id: Value,
    failure: LocalFailure,
    operation_id: OperationId,
    target: Option<SessionRef>,
) -> Value {
    let (kind, stage, message) = match failure {
        LocalFailure::InvalidIdentity => (
            ConversationOperationFailureKind::InvalidRequest,
            ConversationOperationFailureStage::Validation,
            "provider conversation identity does not match the active binding",
        ),
        LocalFailure::InvalidBinding => (
            ConversationOperationFailureKind::InvalidRequest,
            ConversationOperationFailureStage::Binding,
            "provider conversation endpoint does not resolve to an active binding",
        ),
        LocalFailure::StaleGeneration => (
            ConversationOperationFailureKind::StaleGeneration,
            ConversationOperationFailureStage::Binding,
            "provider conversation generation is stale",
        ),
        LocalFailure::Busy => (
            ConversationOperationFailureKind::Busy,
            ConversationOperationFailureStage::Admission,
            "provider conversation request capacity exceeded",
        ),
        LocalFailure::Unavailable => (
            ConversationOperationFailureKind::Unavailable,
            ConversationOperationFailureStage::Binding,
            "provider conversation backend unavailable",
        ),
    };
    let Ok(message) = NonEmptyText::try_from(message.to_owned()) else {
        return json_rpc_error(id, -32603, "Internal error");
    };
    failure_response(
        id,
        ConversationOperationFailure {
            kind,
            stage,
            effect: ProviderOperationEffect::None,
            message,
            operation_id,
            provider_code: None,
            target,
            endpoint: None,
            availability: None,
        },
    )
}

enum LocalFailure {
    InvalidIdentity,
    InvalidBinding,
    StaleGeneration,
    Busy,
    Unavailable,
}

fn success(id: Value, result: impl serde::Serialize) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}
fn result_response(
    id: Value,
    result: Result<impl serde::Serialize, ConversationOperationFailure>,
) -> Value {
    match result {
        Ok(result) => success(id, result),
        Err(failure) => failure_response(id, failure),
    }
}
fn invalid_params(id: Value) -> Value {
    json_rpc_error(id, -32602, "Invalid params")
}
fn json_rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
fn failure_response(id: Value, failure: ConversationOperationFailure) -> Value {
    let message = String::from(failure.message.clone());
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":message,"data":failure}})
}
