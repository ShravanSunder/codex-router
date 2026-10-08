//! Control connection admission and discovery bootstrap. Native dispatch is separate.
use crate::{EndpointSubscription, ServiceIdentity};
use collaboration_protocol::{AdmissionError, ControlAdmission, ControlFrameDecoder};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
#[cfg(test)]
#[path = "wake_creation_crash_tests.rs"]
mod wake_creation_crash_tests;

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
    let mut provider_subscription: Option<
        crate::collaboration_application::ProviderSessionSubscription,
    > = None;
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
            provider_event = async {
                match provider_subscription.as_mut() {
                    Some(subscription) => subscription.next_value().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(event) = provider_event {
                    let mut output=serde_json::to_vec(&json!({"jsonrpc":"2.0","method":"provider/sessionEvent","params":event})).map_err(io::Error::other)?;
                    output.push(b'\n');
                    stream.write_all(&output).await?;
                } else {
                    provider_subscription = None;
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
                    match serde_json::from_value::<collaboration_protocol::WakeShowRequest>(
                        request.params,
                    ) {
                        Err(_) => error(
                            json!(request.id),
                            -32602,
                            "Provide exact wakeupId for first-fire subscription",
                        ),
                        Ok(params) => {
                            if wake_subscription.is_some() {
                                // One first-fire wait per Control connection.
                                crate::wakeup_subscription::unavailable(
                                    json!(request.id),
                                    params.wakeup_id,
                                )
                            } else {
                                match crate::collaboration_application::WakeOperations::new(
                                    &identity.service_id,
                                    identity.automation.as_ref(),
                                )
                                .with_wait_capacity(&identity.wake_wait_permits)
                                .wake_wait_start(params)
                                .await
                                {
                                    Ok((state, result)) => {
                                        wake_subscription = Some(state);
                                        json!({"jsonrpc":"2.0","id":request.id,"result":result})
                                    }
                                    Err(failure) => {
                                        crate::wakeup_subscription::wait_failure_response(
                                            json!(request.id),
                                            failure,
                                        )
                                    }
                                }
                            }
                        }
                    }
                }
                Ok(request) if request.method == "message/send" => {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::session_message_dispatch::dispatch(
                            json!(id),
                            request.params,
                            &identity,
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request) if request.method == "message/reply" => {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::session_message_reply_dispatch::dispatch(
                            json!(id),
                            request.params,
                            &identity,
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request) if request.method == "router/show" => {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response =
                            crate::push_record_resolver::show(json!(id), request.params, &identity)
                                .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request) if request.method == "message/inbox" => {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::push_record_resolver::inbox(
                            json!(id),
                            request.params,
                            &identity,
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request) if request.method == "message/history" => {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::push_record_resolver::history(
                            json!(id),
                            request.params,
                            &identity,
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request) if request.method == "provider/sessionListen" => {
                    admission.complete(&request.id);
                    let id = request.id.clone();
                    match crate::provider_session_observation_dispatch::listen(
                        request.params,
                        &identity,
                    )
                    .await
                    {
                        Ok(subscription) => {
                            let response =
                                json!({"jsonrpc":"2.0","id":id,"result":subscription.ready});
                            provider_subscription = Some(subscription);
                            response
                        }
                        Err(error) => {
                            crate::provider_session_observation_dispatch::failure_response(
                                json!(id),
                                error,
                            )
                        }
                    }
                }
                Ok(request) if request.method == "provider/sessionObserve" => {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::provider_session_observation_dispatch::observe(
                            json!(id),
                            request.params,
                            &identity,
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request)
                    if matches!(
                        request.method.as_str(),
                        "conversation/create"
                            | "conversation/load"
                            | "conversation/resume"
                            | "conversation/close"
                            | "conversation/prompt"
                            | "conversation/cancel"
                            | "conversation/settingsSet"
                            | "conversation/settingsAccept"
                            | "provider/sessionInspect"
                            | "conversation/operationShow"
                            | "conversation/operationWait"
                            | "conversation/operationReconcile"
                    ) =>
                {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::provider_conversation_dispatch::dispatch(
                            json!(id),
                            &request.method,
                            request.params,
                            &identity,
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request)
                    if matches!(
                        request.method.as_str(),
                        "approval/list" | "approval/decide" | "question/list" | "question/answer"
                    ) =>
                {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = dispatch_interaction(
                            &request.method,
                            request.params,
                            json!(id),
                            &identity,
                        )
                        .await;
                        (id, response)
                    });
                    continue;
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
                        #[cfg(test)]
                        wake_creation_crash_tests::checkpoint("before-dispatch", &request.method);
                        let response =
                            crate::wakeup_dispatch::dispatch(crate::wakeup_dispatch::WakeRequest {
                                id: json!(id),
                                method: &request.method,
                                params: request.params,
                                service_id: &identity.service_id,
                                store: identity.automation.as_ref(),
                            })
                            .await;
                        #[cfg(test)]
                        wake_creation_crash_tests::checkpoint("after-dispatch", &request.method);
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
                            json!(id),
                            &request.method,
                            request.params,
                            &identity,
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
                        let response = crate::run_dispatch::dispatch(
                            json!(id),
                            &request.method,
                            request.params,
                            &identity,
                        )
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
                            json!(id),
                            &request.method,
                            request.params,
                            &identity,
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
                            json!(id),
                            &request.method,
                            request.params,
                            &identity,
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
                            json!(id),
                            request.params,
                            &identity,
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
                            json!(id),
                            request.method == "operation/reconcile",
                            request.params,
                            &identity,
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request) if request.method.starts_with("board/") => {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::board_request_dispatch::dispatch(
                            json!(id),
                            &request.method,
                            request.params,
                            &identity,
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
                        let response =
                            crate::schedule_dispatch::prepare(json!(id), request.params, &identity)
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
                            json!(id),
                            &request.method,
                            request.params,
                            &identity,
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
                            json!(id),
                            &request.method,
                            request.params,
                            &identity,
                        )
                        .await;
                        (id, response)
                    });
                    continue;
                }
                Ok(request) if request.method == "provider/sessionList" => {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::provider_session_inventory_dispatch::dispatch(
                            json!(id),
                            request.params,
                            &identity,
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
                            | "codex/sessionRename"
                            | "codex/turnInterrupt"
                            | "codex/sessionList"
                    ) =>
                {
                    let identity = identity.clone();
                    // A connection whose endpoint subscription overflowed closes before serving
                    // a directory-backed read.
                    subscription.snapshot()?;
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::native_control_dispatch::dispatch_native(
                            json!(id),
                            &request.method,
                            request.params,
                            &identity,
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
                    // A connection whose endpoint subscription overflowed closes before serving
                    // a directory-backed read.
                    subscription.snapshot()?;
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::journal_dispatch::dispatch_journal(
                            &request.method,
                            request.params,
                            json!(id),
                            &identity,
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
async fn dispatch_interaction(
    method: &str,
    params: Value,
    id: Value,
    identity: &ServiceIdentity,
) -> Value {
    let interactions = crate::collaboration_application::InteractionOperations::new(identity);
    let result = match method {
        "approval/list" => {
            let Ok(params) =
                serde_json::from_value::<collaboration_protocol::ApprovalListParams>(params)
            else {
                return error(id, -32602, "Invalid params");
            };
            interactions
                .approval_list(params)
                .await
                .map(|result| json!(result))
        }
        "approval/decide" => {
            let Ok(params) =
                serde_json::from_value::<collaboration_protocol::ApprovalDecideParams>(params)
            else {
                return error(id, -32602, "Invalid params");
            };
            interactions
                .approval_decide(params)
                .await
                .map(|result| json!(result))
        }
        "question/list" => {
            let Ok(params) =
                serde_json::from_value::<collaboration_protocol::QuestionListParams>(params)
            else {
                return error(id, -32602, "Invalid params");
            };
            interactions
                .question_list(params)
                .await
                .map(|result| json!(result))
        }
        "question/answer" => {
            let Ok(params) =
                serde_json::from_value::<collaboration_protocol::QuestionAnswerParams>(params)
            else {
                return error(id, -32602, "Invalid params");
            };
            interactions
                .question_answer(params)
                .await
                .map(|result| json!(result))
        }
        _ => return error(id, -32601, "Method not found"),
    };
    match result {
        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
        Err(failure) => rejection_response(id, &failure),
    }
}

/// Control answers a failed operation with the failure's published rejection.
pub(crate) fn rejection_response(
    id: Value,
    failure: &impl crate::collaboration_application::CollaborationRejection,
) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":failure.published_rejection()})
}

/// The response bound for a result sent back to `id`: one Control frame, envelope included.
pub(crate) fn control_result_budget(id: &Value) -> crate::ResultByteBudget {
    let envelope_bytes = serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"result":null}))
        .map_or(collaboration_protocol::MAX_CONTROL_FRAME_BYTES, |bytes| {
            bytes.len().saturating_sub("null".len())
        });
    crate::ResultByteBudget::new(
        collaboration_protocol::MAX_CONTROL_FRAME_BYTES,
        envelope_bytes,
    )
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
            if request.method.starts_with("conversation/")
                || request.method == "provider/sessionInspect"
            {
                return Err(crate::provider_conversation_dispatch::overloaded(
                    id,
                    &request.method,
                    request.params,
                ));
            }
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
                json!({"jsonrpc":"2.0","id":id,"result":collaboration_protocol::EndpointInventory {
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
        serde_json::from_value::<collaboration_protocol::ControlInitializationParams>(params)
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
    json!({"jsonrpc":"2.0","id":id,"result":collaboration_protocol::ControlInitializationResult {
        version: collaboration_protocol::ProtocolVersion { major: 1, minor: 0 },
        service_id: identity.service_id.clone(), service_epoch: identity.service_epoch.clone(),
        control_schema_digest: identity.schema_digest.clone(),
        service_version: std::env::var("CODEX_ROUTER_DEBUG_RUNNING_VERSION")
            .ok()
            .filter(|_| cfg!(debug_assertions))
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned()),
    }})
}

#[cfg(test)]
#[path = "control_connection/admission_error_tests.rs"]
mod admission_error_tests;

#[cfg(test)]
#[path = "control_connection/result_budget_tests.rs"]
mod result_budget_tests;
