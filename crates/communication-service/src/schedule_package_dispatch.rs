//! Local portable transfers never depend on native endpoint availability or allocate threads.
use crate::schedule_dispatch::{FailureContext, ScheduleRequest, failure, invalid};
use automation_storage::StorageError;
use communication_protocol::{
    EndpointRef, LocalMutationState, MAX_CONTROL_FRAME_BYTES, ScheduleExportResult,
    ScheduleImportRequest, ScheduleShowRequest, SessionRef,
};
use serde_json::{Value, json};

pub(crate) async fn dispatch(request: ScheduleRequest<'_>) -> Value {
    let context = FailureContext {
        operation_id: request
            .params
            .get("operationId")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
        schedule_id: request
            .params
            .get("scheduleId")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
    };
    let Some(store) = request.store else {
        return failure(
            request.id,
            context,
            StorageError::InvalidRecord,
            LocalMutationState::None,
            None,
        );
    };
    if request.method == "schedule/export" {
        let params = match serde_json::from_value::<ScheduleShowRequest>(request.params) {
            Ok(params) => params,
            Err(_) => return invalid(request.id, context),
        };
        let package = match store
            .lock()
            .await
            .export_schedule::<SessionRef, EndpointRef>(&params.schedule_id)
            .await
        {
            Ok(package) => package,
            Err(error) => {
                return failure(request.id, context, error, LocalMutationState::None, None);
            }
        };
        // Encode once without a text-only cap, then measure the actual escaped response envelope.
        let encoded = match agent_automation::encode_schedule_package(&package, usize::MAX) {
            Ok(encoded) => encoded,
            Err(error) => {
                return failure(
                    request.id,
                    context,
                    error.into(),
                    LocalMutationState::None,
                    None,
                );
            }
        };
        // Request IDs allow 128 UTF-8 bytes; a control character needs six JSON bytes.
        // Reserve a maximally escaped ID and the larger import envelope before exporting.
        let import = json!({
            "jsonrpc":"2.0", "id":"\u{0001}".repeat(128), "method":"schedule/import",
            "params":{"operationId":"00000000-0000-7000-8000-000000000000","packageUtf8":encoded,"overwrite":false}
        });
        let import_bytes = serde_json::to_vec(&import)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX);
        let response = json!({"jsonrpc":"2.0","id":request.id,"result":ScheduleExportResult { package_utf8: encoded }});
        let bytes = match serde_json::to_vec(&response) {
            Ok(bytes) => bytes,
            Err(_) => {
                return failure(
                    request.id,
                    context,
                    StorageError::InvalidRecord,
                    LocalMutationState::None,
                    None,
                );
            }
        };
        let required_bytes = bytes.len().max(import_bytes);
        if required_bytes > MAX_CONTROL_FRAME_BYTES {
            return json!({"jsonrpc":"2.0","id":request.id,"error":{"code":-32050,"message":"Schedule package too large","data":communication_protocol::ScheduleFailure::package_frame_limit(None, required_bytes)}});
        }
        return response;
    }
    let params = match serde_json::from_value::<ScheduleImportRequest>(request.params) {
        Ok(params) => params,
        Err(_) => return invalid(request.id, context),
    };
    match store
        .lock()
        .await
        .import_schedule::<SessionRef, EndpointRef>(&automation_storage::ScheduleImport {
            operation_id: params.operation_id,
            package_utf8: &params.package_utf8,
            overwrite: params.overwrite,
            now_ms: chrono::Utc::now().timestamp_millis(),
        })
        .await
    {
        Ok(record) => match crate::schedule_projection::snapshot(record) {
            Ok(snapshot) => json!({"jsonrpc":"2.0","id":request.id,"result":snapshot}),
            Err(()) => failure(
                request.id,
                context,
                StorageError::InvalidRecord,
                LocalMutationState::Committed,
                None,
            ),
        },
        Err(error) => {
            let mutation = if matches!(error, StorageError::Database(_)) {
                LocalMutationState::Unknown
            } else {
                LocalMutationState::None
            };
            failure(request.id, context, error, mutation, None)
        }
    }
}
