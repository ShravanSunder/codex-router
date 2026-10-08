//! Schedule Control dispatch: decodes each request and calls the typed schedule operations.
use crate::ServiceIdentity;
use crate::collaboration_application::{
    AutomationOperations, ScheduleFailureContext, ScheduleOperationFailure,
    invalid_schedule_request,
};
use collaboration_protocol::{
    ScheduleCreateRequest, ScheduleEnableRequest, ScheduleImportRequest, SchedulePrepareRequest,
    ScheduleShowRequest, ScheduleUpdateRequest,
};
use serde_json::{Value, json};

/// The operation and schedule identities a request names, read before it is decoded.
pub(crate) fn failure_context(params: &Value) -> ScheduleFailureContext {
    ScheduleFailureContext::new(
        params
            .get("operationId")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
        params
            .get("scheduleId")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
    )
}

pub(crate) async fn dispatch(
    id: Value,
    method: &str,
    params: Value,
    identity: &ServiceIdentity,
) -> Value {
    let automation = AutomationOperations::new(identity);
    let context = failure_context(&params);
    macro_rules! decode {
        ($request:ty) => {
            match serde_json::from_value::<$request>(params) {
                Ok(request) => request,
                Err(_) => return schedule_failure_response(id, invalid_schedule_request(context)),
            }
        };
    }
    let result = match method {
        "schedule/create" => automation
            .schedule_create(decode!(ScheduleCreateRequest))
            .await
            .map(|snapshot| json!(snapshot)),
        "schedule/show" => automation
            .schedule_show(decode!(ScheduleShowRequest))
            .await
            .map(|snapshot| json!(snapshot)),
        "schedule/update" => automation
            .schedule_update(decode!(ScheduleUpdateRequest))
            .await
            .map(|snapshot| json!(snapshot)),
        "schedule/enable" => automation
            .schedule_enable(decode!(ScheduleEnableRequest))
            .await
            .map(|snapshot| json!(snapshot)),
        "schedule/disable" => automation
            .schedule_disable(decode!(ScheduleEnableRequest))
            .await
            .map(|snapshot| json!(snapshot)),
        "schedule/export" => {
            let request = decode!(ScheduleShowRequest);
            let budget = crate::control_connection::control_result_budget(&id);
            automation
                .schedule_export(request, budget)
                .await
                .map(|package| json!(package))
        }
        "schedule/import" => automation
            .schedule_import(decode!(ScheduleImportRequest))
            .await
            .map(|snapshot| json!(snapshot)),
        _ => return schedule_failure_response(id, invalid_schedule_request(context)),
    };
    match result {
        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
        Err(failure) => schedule_failure_response(id, failure),
    }
}

/// Control entry point for `schedule/prepare`.
pub(crate) async fn prepare(id: Value, params: Value, identity: &ServiceIdentity) -> Value {
    let Ok(request) = serde_json::from_value::<SchedulePrepareRequest>(params) else {
        return json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Provide operationId, scheduleId and an explicit fresh/fork/existing destination."}});
    };
    match AutomationOperations::new(identity)
        .schedule_prepare(request)
        .await
    {
        Ok(snapshot) => json!({"jsonrpc":"2.0","id":id,"result":snapshot}),
        Err(failure) => schedule_failure_response(id, failure),
    }
}

fn schedule_failure_response(id: Value, failure: ScheduleOperationFailure) -> Value {
    crate::control_connection::rejection_response(id, &failure)
}
