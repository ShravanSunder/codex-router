//! App-server startup, thread and turn method handling for provider Sessions.
use super::*;

pub(super) fn handle_app_server_request(
    id: Value,
    method: &str,
    catalog: &[ProviderModelEntry],
) -> Value {
    let result = match method {
        "initialize" => Some(json!({})),
        "account/read" => Some(json!({"account":null,"requiresOpenaiAuth":false})),
        "model/list" => Some(render_model_list(catalog)),
        "configRequirements/read" => Some(json!({"requirements":null})),
        "collaborationMode/list" | "hooks/list" | "skills/list" => {
            Some(json!({"data":[],"nextCursor":null}))
        }
        _ => None,
    };
    match result {
        Some(result) => json!({"id":id,"result":result}),
        None => json!({"id":id,"error":{"code":-32601,"message":"Method not found"}}),
    }
}

pub(super) async fn handle_app_server_thread_request(
    method: &str,
    params: Value,
    commands: Arc<dyn SessionCommandPort>,
    events: Arc<dyn SessionEventHub>,
    endpoint: SessionEndpointRef,
    actor: Identity,
) -> Result<Value, ThreadMethodError> {
    match method {
        "thread/start" => {
            if params.get("ephemeral").and_then(Value::as_bool) == Some(true)
                || params
                    .pointer("/threadSource/feature")
                    .and_then(Value::as_str)
                    == Some("thread_title")
            {
                return Err(ThreadMethodError::InvalidParams);
            }
            let working_directory = match params.get("cwd").and_then(Value::as_str) {
                Some(cwd) => PathBuf::from(cwd),
                None => std::env::current_dir().map_err(|_| ThreadMethodError::Unavailable)?,
            };
            if !working_directory.is_absolute() {
                return Err(ThreadMethodError::InvalidParams);
            }
            let requested_model = params.get("model").and_then(Value::as_str);
            let model_override = requested_model.filter(|model| *model != "provider-default");
            let session = commands
                .create(CreateSessionCommand {
                    endpoint: endpoint.clone(),
                    working_directory,
                    settings: SessionSettingsCommand {
                        model: model_override.map(str::to_owned),
                        ..SessionSettingsCommand::default()
                    },
                    actor,
                })
                .await
                .map_err(|_| ThreadMethodError::Unavailable)?;
            let summary = events
                .sessions(endpoint)
                .await
                .map_err(|_| ThreadMethodError::Unavailable)?
                .into_iter()
                .find(|summary| summary.session == session)
                .ok_or(ThreadMethodError::Unavailable)?;
            Ok(thread_start_response(&summary))
        }
        "thread/list" => {
            let summaries = events
                .sessions(endpoint)
                .await
                .map_err(|_| ThreadMethodError::Unavailable)?;
            Ok(json!({
                "data": summaries.iter().map(render_thread).collect::<Vec<_>>(),
                "nextCursor": null, "backwardsCursor": null
            }))
        }
        "thread/read" | "thread/resume" => {
            let thread_id = params
                .get("threadId")
                .and_then(Value::as_str)
                .ok_or(ThreadMethodError::InvalidParams)?;
            let mut summary = events
                .sessions(endpoint)
                .await
                .map_err(|_| ThreadMethodError::Unavailable)?
                .into_iter()
                .find(|summary| thread_alias(&summary.session) == thread_id)
                .ok_or(ThreadMethodError::NotFound)?;
            if method == "thread/resume" {
                if summary.state == session_event_model::SessionState::Unloaded {
                    commands
                        .load_session(SessionTargetCommand {
                            target: summary.session.clone(),
                            actor,
                        })
                        .await
                        .map_err(|error| match error {
                            crate::CommandFailure::SessionNotFound => ThreadMethodError::NotFound,
                            _ => ThreadMethodError::Unavailable,
                        })?;
                    summary = events
                        .sessions(summary.session.endpoint.clone())
                        .await
                        .map_err(|_| ThreadMethodError::Unavailable)?
                        .into_iter()
                        .find(|current| current.session == summary.session)
                        .ok_or(ThreadMethodError::NotFound)?;
                }
                let attachment = events
                    .attach(summary.session.clone())
                    .await
                    .map_err(|error| match error {
                        SessionEventHubError::SessionNotFound => ThreadMethodError::NotFound,
                        SessionEventHubError::Unavailable => ThreadMethodError::Unavailable,
                    })?;
                let mut result = thread_start_response(&summary);
                if params.get("excludeTurns").and_then(Value::as_bool) != Some(true)
                    && let Some(thread) = result.get_mut("thread").and_then(Value::as_object_mut)
                {
                    thread.insert(
                        "turns".into(),
                        json!(historical_turns(&summary.session, &attachment.snapshot)),
                    );
                }
                Ok(result)
            } else {
                let mut thread = render_thread(&summary);
                if params.get("includeTurns").and_then(Value::as_bool) != Some(false)
                    && summary.state != session_event_model::SessionState::Unloaded
                {
                    let attachment = events
                        .attach(summary.session.clone())
                        .await
                        .map_err(|_| ThreadMethodError::Unavailable)?;
                    if let Some(thread) = thread.as_object_mut() {
                        thread.insert(
                            "turns".into(),
                            json!(historical_turns(&summary.session, &attachment.snapshot)),
                        );
                    }
                }
                Ok(json!({"thread":thread}))
            }
        }
        _ => Err(ThreadMethodError::InvalidParams),
    }
}

pub(crate) fn thread_alias(session: &message_board::SessionRef) -> String {
    let native_id = session.session_id.as_str();
    if let Ok(uuid) = Uuid::parse_str(native_id) {
        return uuid.hyphenated().to_string();
    }
    let name = format!(
        "codex-router:session:{}:{}:{}",
        session.endpoint.service_id.as_str(),
        session.endpoint.endpoint_id.as_str(),
        native_id
    );
    Uuid::new_v5(&Uuid::NAMESPACE_URL, name.as_bytes())
        .hyphenated()
        .to_string()
}

fn render_thread(summary: &HubSessionSummary) -> Value {
    let status = match &summary.state {
        session_event_model::SessionState::Unloaded | session_event_model::SessionState::Closed => {
            json!({"type":"notLoaded"})
        }
        session_event_model::SessionState::Idle => json!({"type":"idle"}),
        session_event_model::SessionState::Running => {
            json!({"type":"active","activeFlags":[]})
        }
        session_event_model::SessionState::RequiresAction { pending } => {
            let flag = match pending.first().kind() {
                session_event_model::InteractionKind::Approval => "waitingOnApproval",
                session_event_model::InteractionKind::Question => "waitingOnUserInput",
            };
            json!({"type":"active","activeFlags":[flag]})
        }
        session_event_model::SessionState::AuthenticationRequired => {
            json!({"type":"systemError"})
        }
    };
    let alias = thread_alias(&summary.session);
    json!({
        "id":alias,"sessionId":alias,
        "preview":summary.preview,"ephemeral":false,
        "modelProvider":summary.session.endpoint.endpoint_id.as_str(),
        "model":summary.model,
        // Router has no creation timestamp for provider Sessions, including legacy rows.
        "createdAt":summary.updated_at_seconds,
        "updatedAt":summary.updated_at_seconds,
        "status":status,
        "cwd":summary.working_directory,
        "cliVersion":env!("CARGO_PKG_VERSION"),
        "source":"cli",
        "name":summary.name,
        "turns":[]
    })
}

fn thread_start_response(summary: &HubSessionSummary) -> Value {
    let model = summary.model.as_deref().unwrap_or("provider-default");
    json!({
        "thread":render_thread(summary),
        "model":model,
        "modelProvider":summary.session.endpoint.endpoint_id.as_str(),
        "cwd":summary.working_directory,
        "approvalPolicy":"on-request",
        "approvalsReviewer":"user",
        "sandbox":{"type":"dangerFullAccess"},
        "reasoningEffort":null,
        "serviceTier":null,
        "collaborationMode":null
    })
}

pub(super) async fn handle_app_server_turn_request(
    method: &str,
    params: Value,
    commands: Arc<dyn SessionCommandPort>,
    events: Arc<dyn SessionEventHub>,
    endpoint: SessionEndpointRef,
    actor: Identity,
) -> Result<Value, ThreadMethodError> {
    let thread_id = params
        .get("threadId")
        .and_then(Value::as_str)
        .ok_or(ThreadMethodError::InvalidParams)?;
    let session = events
        .sessions(endpoint)
        .await
        .map_err(|_| ThreadMethodError::Unavailable)?
        .into_iter()
        .find(|summary| thread_alias(&summary.session) == thread_id)
        .map(|summary| summary.session)
        .ok_or(ThreadMethodError::NotFound)?;
    match method {
        "turn/start" => {
            let content = parse_turn_input(&params)?;
            let handle = commands
                .prompt(PromptSessionCommand {
                    target: session,
                    input_id: session_event_model::InputId::generate(),
                    content,
                    actor,
                })
                .await
                .map_err(|_| ThreadMethodError::Unavailable)?;
            Ok(json!({"turn":{
                "id":handle.turn_id,"items":[],"status":"inProgress",
                "error":null,"startedAt":null,"completedAt":null,"durationMs":null
            }}))
        }
        "turn/steer" => {
            let expected_turn_id = params
                .get("expectedTurnId")
                .and_then(Value::as_str)
                .ok_or(ThreadMethodError::InvalidParams)?
                .to_owned();
            let content = parse_turn_input(&params)?;
            let input_id = session_event_model::InputId::generate();
            let outcome = commands
                .steer(SteerSessionCommand {
                    target: session.clone(),
                    input_id: input_id.clone(),
                    expected_turn_id,
                    content: content.clone(),
                    actor: actor.clone(),
                })
                .await
                .map_err(|_| ThreadMethodError::Unavailable)?;
            let turn_id = match outcome {
                SessionSteerOutcome::Injected { turn_id }
                | SessionSteerOutcome::StartedNewTurn { turn_id } => turn_id,
                SessionSteerOutcome::PromptRequired => {
                    commands
                        .prompt(PromptSessionCommand {
                            target: session,
                            input_id,
                            content,
                            actor,
                        })
                        .await
                        .map_err(|_| ThreadMethodError::Unavailable)?
                        .turn_id
                }
                SessionSteerOutcome::Failed { .. } => return Err(ThreadMethodError::Unavailable),
            };
            Ok(json!({"turnId":turn_id}))
        }
        "turn/interrupt" => {
            if params.get("turnId").and_then(Value::as_str).is_none() {
                return Err(ThreadMethodError::InvalidParams);
            }
            commands
                .cancel(SessionTargetCommand {
                    target: session,
                    actor,
                })
                .await
                .map_err(|_| ThreadMethodError::Unavailable)?;
            Ok(json!({}))
        }
        _ => Err(ThreadMethodError::InvalidParams),
    }
}

fn parse_turn_input(params: &Value) -> Result<Vec<CommandContent>, ThreadMethodError> {
    let input = params
        .get("input")
        .and_then(Value::as_array)
        .ok_or(ThreadMethodError::InvalidParams)?;
    input
        .iter()
        .map(|entry| match entry.get("type").and_then(Value::as_str) {
            Some("text") => entry
                .get("text")
                .and_then(Value::as_str)
                .map(|text| CommandContent::Text(text.to_owned()))
                .ok_or(ThreadMethodError::InvalidParams),
            _ => Err(ThreadMethodError::InvalidParams),
        })
        .collect()
}
