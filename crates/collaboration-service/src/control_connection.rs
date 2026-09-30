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
        crate::provider_session_observation_dispatch::ProviderSessionSubscription,
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
                        Err(error) => error.response(json!(id)),
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
                Ok(request)
                    if matches!(
                        request.method.as_str(),
                        "board/threadListen"
                            | "board/threadWait"
                            | "board/threadListenShow"
                            | "board/threadListenCancel"
                    ) =>
                {
                    let identity = identity.clone();
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::thread_listen_dispatch::dispatch(
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
                        let response = crate::schedule_preparation_dispatch::dispatch(
                            crate::schedule_preparation_dispatch::PreparationRequest {
                                configuration: &identity.configuration,
                                id: json!(id),
                                params: request.params,
                                service_id: &identity.service_id,
                                execution: identity.scheduled_run_execution.as_ref(),
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
                                execution: identity.scheduled_run_execution.as_ref(),
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
                    let endpoints = subscription.snapshot()?.endpoints;
                    pending.spawn(async move {
                        let id = request.id.clone();
                        let response = crate::native_control_dispatch::dispatch_native(
                            crate::native_control_request::NativeControlRequest {
                                method: &request.method,
                                params: request.params,
                                id: json!(id),
                                service_id: &identity.service_id,
                                display_names: &identity.display_names,
                                backend: identity.native_backend.as_ref(),
                                endpoints: &endpoints,
                                stored_observation: identity.journal.as_deref().map(|store| {
                                    crate::stored_inventory_observation::StoredInventoryObservation {
                                        store,
                                        observer_id: &identity.service_epoch,
                                    }
                                }),
                                access_routes: identity.approval_broker.as_deref(),
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
async fn dispatch_interaction(
    method: &str,
    params: Value,
    id: Value,
    identity: &ServiceIdentity,
) -> Value {
    let Some(broker) = identity.approval_broker.as_ref() else {
        return json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Approval service unavailable","data":{"kind":"unavailable","stage":"inspect","message":"Approval service unavailable"}}});
    };
    match method {
        "approval/list" => {
            let Ok(params) =
                serde_json::from_value::<collaboration_protocol::ApprovalListParams>(params)
            else {
                return error(id, -32602, "Invalid params");
            };
            if params.include_options {
                match broker.list_detailed(params.pending).await {
                    Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                    Err(_) => error(id, -32050, "Approval service unavailable"),
                }
            } else {
                json!({"jsonrpc":"2.0","id":id,"result":broker.list(params.pending).await})
            }
        }
        "approval/decide" => {
            let Ok(params) =
                serde_json::from_value::<collaboration_protocol::ApprovalDecideParams>(params)
            else {
                return error(id, -32602, "Invalid params");
            };
            match broker.decide(params).await {
                Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                Err(failure) => {
                    let mut data = json!({"kind":failure.code(),"stage":"inspect","message":"Approval decision rejected"});
                    if let (Some(data), serde_json::Value::Object(detail)) =
                        (data.as_object_mut(), failure.detail())
                    {
                        data.extend(detail);
                    }
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Approval decision rejected","data":data}})
                }
            }
        }
        "question/list" => {
            let Ok(params) =
                serde_json::from_value::<collaboration_protocol::QuestionListParams>(params)
            else {
                return error(id, -32602, "Invalid params");
            };
            let records = broker.list_questions(params.pending).await;
            let Ok(questions) = records
                .into_iter()
                .map(question_record_view)
                .collect::<Result<Vec<_>, _>>()
            else {
                return error(id, -32050, "Question service unavailable");
            };
            json!({"jsonrpc":"2.0","id":id,"result":collaboration_protocol::QuestionListResult { questions }})
        }
        "question/answer" => {
            let Ok(params) =
                serde_json::from_value::<collaboration_protocol::QuestionAnswerParams>(params)
            else {
                return error(id, -32602, "Invalid params");
            };
            let state = match &params.response {
                collaboration_protocol::QuestionResponse::Answered { .. } => {
                    collaboration_protocol::QuestionState::Answered
                }
                collaboration_protocol::QuestionResponse::Declined => {
                    collaboration_protocol::QuestionState::Declined
                }
                collaboration_protocol::QuestionResponse::Cancelled => {
                    collaboration_protocol::QuestionState::Cancelled
                }
            };
            match broker
                .respond_question(&params.request_id, &params.actor, params.response)
                .await
            {
                Ok(()) => {
                    json!({"jsonrpc":"2.0","id":id,"result":collaboration_protocol::QuestionAnswerResult { request_id: params.request_id, state }})
                }
                Err(failure) => {
                    let (kind, field_id) = match failure {
                        crate::interaction_broker::InteractionHistoryError::WrongActor => {
                            ("wrongActor", None)
                        }
                        crate::interaction_broker::InteractionHistoryError::NotPending => {
                            ("questionNotPending", None)
                        }
                        crate::interaction_broker::InteractionHistoryError::AlreadySettled => {
                            ("alreadySettled", None)
                        }
                        crate::interaction_broker::InteractionHistoryError::InvalidAnswer {
                            field_id,
                        } => ("invalidAnswer", Some(field_id)),
                        _ => ("unavailable", None),
                    };
                    let mut data = json!({"kind":kind,"stage":"inspect","message":"Question response rejected"});
                    if let Some(field_id) = field_id
                        && let Some(fields) = data.as_object_mut()
                    {
                        fields.insert("fieldId".to_owned(), json!(field_id));
                    }
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Question response rejected","data":data}})
                }
            }
        }
        _ => error(id, -32601, "Method not found"),
    }
}

fn question_record_view(
    record: crate::interaction_broker::InteractionHistoryRecord,
) -> Result<collaboration_protocol::QuestionRecord, ()> {
    use crate::interaction_broker::{InteractionHistoryRecord, QuestionHistoryState};
    let InteractionHistoryRecord::Question {
        requester,
        approver,
        request,
        state,
    } = record
    else {
        return Err(());
    };
    let requester =
        serde_json::from_value(serde_json::to_value(requester).map_err(|_| ())?).map_err(|_| ())?;
    let fields = request
        .fields
        .iter()
        .map(|field| {
            serde_json::from_value(serde_json::to_value(field).map_err(|_| ())?).map_err(|_| ())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let state = match state {
        QuestionHistoryState::Pending => collaboration_protocol::QuestionState::Pending,
        QuestionHistoryState::Answered { .. } => collaboration_protocol::QuestionState::Answered,
        QuestionHistoryState::Declined => collaboration_protocol::QuestionState::Declined,
        QuestionHistoryState::Cancelled { .. } => collaboration_protocol::QuestionState::Cancelled,
    };
    Ok(collaboration_protocol::QuestionRecord {
        request_id: request.request_id,
        requester,
        approver,
        prompt: request.prompt,
        fields,
        state,
    })
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
        let schema = collaboration_protocol::control_schema_document(None).unwrap();
        let methods = schema.get("x-methods").and_then(Value::as_object).unwrap();
        for method in methods.keys() {
            let response = admit_request(
                json!({"jsonrpc":"2.0","id":format!("overloaded-{method}"),"method":method,"params":{"wakeupId":collaboration_protocol::WakeupId::generate()}}),
                &mut admission,
            );
            let Err(response) = response else {
                panic!("overloaded request admitted");
            };
            assert!(
                collaboration_protocol::control_error_is_valid(method, &response),
                "invalid overload response for {method}: {response}"
            );
        }
    }

    #[test]
    fn provider_methods_report_overload_before_target_resolution() {
        let mut admission = ControlAdmission::default();
        admission.admit("init", "control/initialize").unwrap();
        admission.initialized();
        admission.complete("init");
        for number in 0..64 {
            admission
                .admit(&format!("pending-{number}"), "codex/sessionInspect")
                .unwrap();
        }
        let target = json!({
            "endpoint": {
                "serviceId": "00000000-0000-4000-8000-000000000001",
                "endpointId": "cursor-local"
            },
            "sessionId": "fixture-session"
        });
        let operation_id = collaboration_protocol::OperationId::generate();
        for method in [
            "provider/sessionInspect",
            "provider/sessionList",
            "conversation/resume",
            "conversation/close",
            "conversation/settingsSet",
            "conversation/settingsAccept",
        ] {
            for include_target in [false, true] {
                let params = if include_target {
                    json!({"target": target, "operationId": operation_id})
                } else {
                    json!({})
                };
                let response = match admit_request(
                    json!({"jsonrpc":"2.0","id":format!("{method}-{include_target}"),"method":method,"params":params}),
                    &mut admission,
                ) {
                    Err(response) => response,
                    Ok(_) => panic!("saturated admission accepted {method}"),
                };
                let data = &response["error"]["data"];
                assert_eq!(data["kind"], "overloaded", "{method}: {response}");
                assert_eq!(data["stage"], "discovery", "{method}: {response}");
                assert!(
                    collaboration_protocol::control_error_is_valid(method, &response),
                    "{method}: {response}"
                );
                if method == "provider/sessionList" {
                    assert!(data.get("target").is_none());
                } else if include_target {
                    assert_eq!(data["target"], target);
                } else {
                    assert!(data.get("target").is_none());
                }
            }
        }
    }

    #[test]
    fn saturated_message_admission_stops_before_any_route_dispatch() {
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
            json!({"jsonrpc":"2.0","id":"rejected-message","method":"message/send","params":{}}),
            &mut admission,
        );
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("saturated admission accepted work"),
        };
        // Assert: clients can distinguish overload from unknown or partial native submission.
        assert_eq!(error["error"]["data"]["kind"], "overloaded");
        assert!(error["error"]["data"].get("client").is_none());
        assert!(collaboration_protocol::control_error_is_valid(
            "message/send",
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
        assert!(collaboration_protocol::control_error_is_valid(
            "control/initialize",
            &initialization
        ));
    }
}
