//! Control connection admission and discovery bootstrap. Native dispatch is separate.
use crate::{EndpointSubscription, ServiceIdentity};
use communication_protocol::{AdmissionError, ControlAdmission, ControlFrameDecoder};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    jsonrpc: String,
    id: String,
    method: String,
    params: Value,
}
/// Serves admitted Control calls; native effects use the separately bound generation gate.
pub async fn serve_control_connection(
    mut stream: UnixStream,
    identity: ServiceIdentity,
) -> io::Result<()> {
    let mut subscription = identity.directory.subscribe()?;
    let mut wake_subscription: Option<crate::wakeup_subscription::WakeSubscriptionState> = None;
    let mut wake_poll = tokio::time::interval(std::time::Duration::from_millis(50));
    wake_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut decoder = ControlFrameDecoder::default();
    let mut admission = ControlAdmission::default();
    let mut buffer = [0_u8; 8192];
    let mut pending = tokio::task::JoinSet::<(String, Value)>::new();
    loop {
        let count = tokio::select! {
            result = stream.read(&mut buffer) => result?,
            _=wake_poll.tick(), if wake_subscription.is_some()=>{
                if let Some(wake)=&mut wake_subscription {
                    for change in wake.next_changes().await? {
                        let mut output=serde_json::to_vec(&json!({"jsonrpc":"2.0","method":"wake/changed","params":change})).map_err(io::Error::other)?;
                        output.push(b'\n');stream.write_all(&output).await?;
                    }
                }
                continue;
            },
            completion = pending.join_next(), if !pending.is_empty() => {
                let (id,response)=completion.ok_or_else(||io::Error::other("pending task missing"))?.map_err(|_|io::Error::other("Control task failed"))?;
                admission.complete(&id);
                let mut output=serde_json::to_vec(&response).map_err(io::Error::other)?;output.push(b'\n');stream.write_all(&output).await?;continue;
            },
            update = subscription.next(), if admission.is_initialized() => {
                let update = update?;
                let mut output = serde_json::to_vec(&json!({"jsonrpc":"2.0","method":"endpoint/changed","params":{"serviceEpoch":identity.service_epoch,"sequence":update.sequence,"endpoint":update.endpoint}})).map_err(io::Error::other)?;
                output.push(b'\n');
                stream.write_all(&output).await?;
                continue;
            }
        };
        if count == 0 {
            return decoder.finish().map_err(io::Error::other);
        }
        let input = buffer
            .get(..count)
            .ok_or_else(|| io::Error::other("read boundary"))?;
        let frames = decoder.push(input).map_err(io::Error::other)?;
        for frame in frames {
            // Notifications do not dispatch request-only methods and receive no response.
            if frame.get("id").is_none() {
                continue;
            }
            let response = match admit_request(frame, &mut admission) {
                Err(response) => response,
                Ok(request) if request.method == "wake/subscribe" => {
                    admission.complete(&request.id);
                    match serde_json::from_value::<communication_protocol::WakeShowRequest>(
                        request.params,
                    ) {
                        Err(_) => error(
                            json!(request.id),
                            -32602,
                            "Provide exact wakeupId for first-fire subscription",
                        ),
                        Ok(params) => {
                            let wakeup_id = params.wakeup_id.clone();
                            let permit = std::sync::Arc::clone(&identity.wake_wait_permits)
                                .try_acquire_owned();
                            if wake_subscription.is_some() {
                                crate::wakeup_subscription::unavailable(
                                    json!(request.id),
                                    wakeup_id,
                                )
                            } else if let (Some(store), Ok(permit)) =
                                (identity.automation.as_ref(), permit)
                            {
                                match crate::wakeup_subscription::start(
                                    std::sync::Arc::clone(store),
                                    identity.service_id.clone(),
                                    params,
                                    permit,
                                )
                                .await
                                {
                                    Ok((state, result)) => {
                                        wake_subscription = Some(state);
                                        json!({"jsonrpc":"2.0","id":request.id,"result":result})
                                    }
                                    Err(automation_storage::StorageError::WakeNotFound) => {
                                        crate::wakeup_subscription::not_found(
                                            json!(request.id),
                                            wakeup_id,
                                        )
                                    }
                                    Err(_) => crate::wakeup_subscription::unavailable(
                                        json!(request.id),
                                        wakeup_id,
                                    ),
                                }
                            } else {
                                crate::wakeup_subscription::unavailable(
                                    json!(request.id),
                                    wakeup_id,
                                )
                            }
                        }
                    }
                }
                Ok(request)
                    if matches!(
                        request.method.as_str(),
                        "wake/send"
                            | "wake/list"
                            | "wake/show"
                            | "wake/pause"
                            | "wake/resume"
                            | "wake/cancel"
                            | "delivery/show"
                    ) =>
                {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response =
                            crate::wakeup_dispatch::dispatch(crate::wakeup_dispatch::WakeRequest {
                                id: json!(id),
                                method: &request.method,
                                params: request.params,
                                service_id: &identity.service_id,
                                store: identity.automation.as_ref(),
                            })
                            .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request)
                    if matches!(
                        request.method.as_str(),
                        "automation/configure" | "automation/status"
                    ) =>
                {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::automation_configuration_dispatch::dispatch(
                            crate::automation_configuration_dispatch::ConfigurationRequest {
                                id: json!(id),
                                method: &request.method,
                                params: request.params,
                                handle: &identity.configuration,
                                backend: identity.configuration_backend.as_ref(),
                                store: identity.automation.as_ref(),
                                service_id: &identity.service_id,
                            },
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request)
                    if matches!(
                        request.method.as_str(),
                        "run/show" | "run/summaryRetry" | "run/summarySkip"
                    ) =>
                {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response =
                            crate::run_dispatch::dispatch(crate::run_dispatch::RunRequest {
                                configuration: &identity.configuration,
                                id: json!(id),
                                method: &request.method,
                                params: request.params,
                                store: identity.automation.as_ref(),
                            })
                            .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request)
                    if matches!(
                        request.method.as_str(),
                        "instruction/list"
                            | "schedule/list"
                            | "run/list"
                            | "revision/list"
                            | "delivery/list"
                    ) =>
                {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::automation_collection_dispatch::dispatch(
                            crate::automation_collection_dispatch::CollectionRequest {
                                id: json!(id),
                                method: &request.method,
                                params: request.params,
                                service_id: &identity.service_id,
                                store: identity.automation.as_ref(),
                            },
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request)
                    if matches!(
                        request.method.as_str(),
                        "delivery/attempts" | "run/summaries"
                    ) =>
                {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::attempt_history_dispatch::dispatch(
                            crate::attempt_history_dispatch::AttemptRequest {
                                id: json!(id),
                                method: &request.method,
                                params: request.params,
                                service_id: &identity.service_id,
                                store: identity.automation.as_ref(),
                            },
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request) if request.method == "automation/events" => {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::automation_event_dispatch::dispatch(
                            crate::automation_event_dispatch::EventRequest {
                                id: json!(id),
                                params: request.params,
                                service_id: &identity.service_id,
                                store: identity.automation.as_ref(),
                            },
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request)
                    if matches!(
                        request.method.as_str(),
                        "delivery/reconcile" | "run/reconcile"
                    ) =>
                {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = if request.method == "delivery/reconcile" {
                            crate::automation_reconciliation_dispatch::delivery(
                                json!(id),
                                request.params,
                                &identity,
                            )
                            .await
                        } else {
                            crate::automation_reconciliation_dispatch::run(
                                json!(id),
                                request.params,
                                &identity,
                            )
                            .await
                        };
                        (id, response)
                    });
                    continue;
                }
                Ok(request)
                    if matches!(
                        request.method.as_str(),
                        "operation/show" | "operation/reconcile"
                    ) =>
                {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::operation_inspection_dispatch::dispatch(
                            crate::operation_inspection_dispatch::OperationRequest {
                                reconcile: request.method == "operation/reconcile",
                                configuration_backend: identity.configuration_backend.as_ref(),
                                id: json!(id),
                                params: request.params,
                                service_id: &identity.service_id,
                                store: identity.automation.as_ref(),
                            },
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request) if request.method == "schedule/prepare" => {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::schedule_preparation_dispatch::dispatch(
                            crate::schedule_preparation_dispatch::PreparationRequest {
                                configuration: &identity.configuration,
                                id: json!(id),
                                params: request.params,
                                service_id: &identity.service_id,
                                backend: identity.native_backend.as_ref(),
                                store: identity.automation.as_ref(),
                            },
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request)
                    if matches!(
                        request.method.as_str(),
                        "schedule/create"
                            | "schedule/show"
                            | "schedule/update"
                            | "schedule/enable"
                            | "schedule/disable"
                            | "schedule/export"
                            | "schedule/import"
                    ) =>
                {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::schedule_dispatch::dispatch(
                            crate::schedule_dispatch::ScheduleRequest {
                                service_id: &identity.service_id,
                                backend: identity.native_backend.as_ref(),
                                id: json!(id),
                                method: &request.method,
                                params: request.params,
                                store: identity.automation.as_ref(),
                            },
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request)
                    if matches!(
                        request.method.as_str(),
                        "instruction/create" | "instruction/update" | "instruction/show"
                    ) =>
                {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::instruction_dispatch::dispatch(
                            crate::instruction_dispatch::InstructionRequest {
                                id: json!(id),
                                method: &request.method,
                                params: request.params,
                                store: identity.automation.as_ref(),
                            },
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request)
                    if matches!(
                        request.method.as_str(),
                        "codex/sessionInspect"
                            | "codex/turnInterrupt"
                            | "codex/messageSend"
                            | "codex/sessionList"
                    ) =>
                {
                    let identity = identity.clone();
                    let endpoints = subscription.snapshot()?.endpoints;
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::native_control_dispatch::dispatch_native(
                            crate::native_control_dispatch::NativeControlRequest {
                                method: &request.method,
                                params: request.params,
                                id: json!(id),
                                service_id: &identity.service_id,
                                backend: identity.native_backend.as_ref(),
                                endpoints: &endpoints,
                                stored_observation: identity.journal.as_deref().map(|store| {
                                    crate::stored_inventory_observation::StoredInventoryObservation {
                                        store,
                                        observer_id: &identity.service_epoch,
                                    }
                                }),
                            },
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request)
                    if matches!(
                        request.method.as_str(),
                        "lifecycleJournal/read" | "lifecycleJournal/status" | "addressBook/list"
                    ) =>
                {
                    let identity = identity.clone();
                    let endpoints = subscription.snapshot()?.endpoints;
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::journal_dispatch::dispatch_journal(
                            &request.method,
                            request.params,
                            json!(id),
                            identity.service_id,
                            identity.journal,
                            endpoints,
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request) => dispatch(request, &identity, &subscription, &mut admission),
            };
            let mut output = serde_json::to_vec(&response).map_err(io::Error::other)?;
            output.push(b'\n');
            stream.write_all(&output).await?;
        }
    }
}
fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
fn admit_request(frame: Value, admission: &mut ControlAdmission) -> Result<Request, Value> {
    let id = frame
        .get("id")
        .filter(|value| value.is_string())
        .cloned()
        .unwrap_or(Value::Null);
    let Ok(request) = serde_json::from_value::<Request>(frame) else {
        return Err(error(id, -32600, "Invalid request"));
    };
    if request.jsonrpc != "2.0" {
        return Err(error(id, -32600, "Invalid JSON-RPC version"));
    }
    if let Err(failure) = admission.admit(&request.id, &request.method) {
        if failure == AdmissionError::Overloaded {
            return Err(crate::control_overload_response::response(
                id,
                &request.method,
                &request.params,
            ));
        }
        return Err(error(id, -32600, &failure.to_string()));
    }
    Ok(request)
}
fn dispatch(
    request: Request,
    identity: &ServiceIdentity,
    subscription: &EndpointSubscription,
    admission: &mut ControlAdmission,
) -> Value {
    let id = json!(request.id);
    let response = match request.method.as_str() {
        "control/initialize" => initialize(request.params, id, identity, admission),
        "endpoint/list" if request.params == json!({}) => match subscription.snapshot() {
            Ok(snapshot) => {
                json!({"jsonrpc":"2.0","id":id,"result":communication_protocol::EndpointInventory {
                    service_epoch: identity.service_epoch.clone(), sequence: snapshot.sequence, endpoints: snapshot.endpoints,
                }})
            }
            Err(_) => error(id, -32603, "Endpoint directory unavailable"),
        },
        "endpoint/list" => error(id, -32602, "Invalid parameters"),
        _ => error(id, -32601, "Method not found"),
    };
    admission.complete(&request.id);
    response
}
fn initialize(
    params: Value,
    id: Value,
    identity: &ServiceIdentity,
    admission: &mut ControlAdmission,
) -> Value {
    let Ok(params) =
        serde_json::from_value::<communication_protocol::ControlInitializationParams>(params)
    else {
        return error(id, -32602, "Invalid initialization parameters");
    };
    if params.version.major > 9_007_199_254_740_991 || params.version.minor > 9_007_199_254_740_991
    {
        return error(id, -32602, "Invalid initialization parameters");
    }
    if params.version.major != 1 || params.version.minor != 0 {
        return json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Unsupported version","data":{"kind":"unsupportedVersion","stage":"initialize","message":"Control 1.0 required"}}});
    }
    admission.initialized();
    json!({"jsonrpc":"2.0","id":id,"result":communication_protocol::ControlInitializationResult {
        version: communication_protocol::ProtocolVersion { major: 1, minor: 0 },
        service_id: identity.service_id.clone(), service_epoch: identity.service_epoch.clone(),
        control_schema_digest: identity.schema_digest.clone(),
    }})
}

#[cfg(test)]
mod admission_error_tests {
    use super::*;

    #[test]
    fn saturated_methods_preserve_their_published_error_contracts() {
        let mut admission = ControlAdmission::default();
        admission.admit("init", "control/initialize").unwrap();
        admission.initialized();
        admission.complete("init");
        for number in 0..64 {
            admission
                .admit(&format!("pending-{number}"), "codex/sessionInspect")
                .unwrap();
        }
        let schema = communication_protocol::control_schema_document(None).unwrap();
        let methods = schema.get("x-methods").and_then(Value::as_object).unwrap();
        for method in methods.keys() {
            let response = admit_request(
                json!({"jsonrpc":"2.0","id":format!("overloaded-{method}"),"method":method,"params":{"wakeupId":communication_protocol::WakeupId::generate()}}),
                &mut admission,
            );
            let Err(response) = response else {
                panic!("overloaded request admitted");
            };
            assert!(
                communication_protocol::control_error_is_valid(method, &response),
                "invalid overload response for {method}: {response}"
            );
        }
    }

    #[test]
    fn saturated_message_admission_reports_that_no_native_effect_was_dispatched() {
        // Arrange: existing admitted work occupies every pending slot.
        let mut admission = ControlAdmission::default();
        admission.admit("init", "control/initialize").unwrap();
        admission.initialized();
        admission.complete("init");
        for number in 0..64 {
            admission
                .admit(&format!("pending-{number}"), "codex/sessionInspect")
                .unwrap();
        }
        // Act: this request is rejected by the real admission path before native dispatch.
        let result = admit_request(
            json!({"jsonrpc":"2.0","id":"rejected-message","method":"codex/messageSend","params":{}}),
            &mut admission,
        );
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("saturated admission accepted work"),
        };
        // Assert: clients can distinguish overload from unknown or partial native submission.
        assert_eq!(error["error"]["data"]["kind"], "overloaded");
        assert_eq!(
            error["error"]["data"]["effects"],
            json!({"resume":"notRequested","submission":"notDispatched"})
        );
        assert!(error["error"]["data"].get("clientUserMessageId").is_none());
        assert!(communication_protocol::control_error_is_valid(
            "codex/messageSend",
            &error
        ));
        let initialization = admit_request(
            json!({"jsonrpc":"2.0","id":"rejected-init","method":"control/initialize","params":{}}),
            &mut admission,
        );
        let initialization = match initialization {
            Err(error) => error,
            Ok(_) => panic!("saturated initialization accepted"),
        };
        assert!(communication_protocol::control_error_is_valid(
            "control/initialize",
            &initialization
        ));
    }
}
