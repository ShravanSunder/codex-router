//! App-server startup, thread and turn method handling for provider Sessions.
use super::*;

pub(super) fn handle_app_server_request(
    id: Value,
    method: &str,
    params: &Value,
    catalog: &[ProviderModelEntry],
    project_trust: Option<&dyn codex_native_integration::CodexProjectTrustLookup>,
) -> Value {
    let result = match method {
        "initialize" => Some(json!({})),
        "account/read" => Some(json!({"account":null,"requiresOpenaiAuth":false})),
        "model/list" => Some(render_model_list(catalog)),
        "configRequirements/read" => Some(json!({"requirements":null})),
        "config/read"
            if params.get("includeLayers") == Some(&Value::Bool(true))
                && params.get("cwd").and_then(Value::as_str).is_some() =>
        {
            let cwd = Path::new(
                params
                    .get("cwd")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
            if !cwd.is_absolute() || !cwd.is_dir() {
                return json!({"id":id,"error":{"code":-32602,"message":"config/read cwd must be an absolute existing directory"}});
            }
            let answer = project_trust.map_or_else(
                || codex_native_integration::ProjectTrustAnswer::ConfigUnavailable {
                    reason: "Codex trust lookup unavailable".into(),
                    trust_target: cwd.to_string_lossy().into_owned(),
                },
                |lookup| lookup.project_trust(cwd),
            );
            let (projects, layers) = match answer {
                codex_native_integration::ProjectTrustAnswer::Trusted {
                    matched_key,
                    match_kind: _,
                } => {
                    let layers = if matched_key == cwd.to_string_lossy() {
                        json!([])
                    } else {
                        json!([{"name":{"type":"project","dotCodexFolder":format!("{matched_key}/.codex")}}])
                    };
                    (json!({matched_key:{"trust_level":"trusted"}}), layers)
                }
                codex_native_integration::ProjectTrustAnswer::Untrusted {
                    trust_target,
                    explicitly_untrusted: true,
                } => {
                    let layers = if trust_target == cwd.to_string_lossy() {
                        json!([])
                    } else {
                        json!([{"name":{"type":"project","dotCodexFolder":format!("{trust_target}/.codex")},
                            "disabledReason": format!("{trust_target} is marked as untrusted in the effective configuration.")}])
                    };
                    (json!({trust_target:{"trust_level":"untrusted"}}), layers)
                }
                _ => (json!({}), json!([])),
            };
            Some(json!({"config":{"projects":projects},"origins":{},"layers":layers}))
        }
        "config/batchWrite" | "config/write" => {
            return json!({"id":id,"error":{"code":-32000,"message":"Provider session trust is read-only here; trust this project in Codex itself"}});
        }
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
    catalog: &[ProviderModelEntry],
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
            let working_directory = params
                .get("cwd")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .ok_or(ThreadMethodError::WorkingDirectoryRequired)?;
            if !working_directory.is_absolute() {
                return Err(ThreadMethodError::InvalidParams);
            }
            let metadata = tokio::fs::metadata(&working_directory)
                .await
                .map_err(|_| ThreadMethodError::InvalidParams)?;
            if !metadata.is_dir() {
                return Err(ThreadMethodError::InvalidParams);
            }
            let model_override =
                provider_model_override(params.get("model").and_then(Value::as_str), catalog);
            let session = commands
                .create(CreateSessionCommand {
                    endpoint: endpoint.clone(),
                    working_directory,
                    settings: SessionSettingsCommand {
                        model: model_override,
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
                if params.get("includeTurns").and_then(Value::as_bool) == Some(true)
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
    catalog: &[ProviderModelEntry],
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
            let content = parse_turn_input(&params).await?;
            if let Some(model) =
                provider_model_override(params.get("model").and_then(Value::as_str), catalog)
            {
                commands
                    .set_setting(SetSessionSettingCommand {
                        target: session.clone(),
                        setting_id: "model".into(),
                        value: model,
                        actor: actor.clone(),
                    })
                    .await
                    .map_err(|_| ThreadMethodError::Unavailable)?;
            }
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
            let content = parse_turn_input(&params).await?;
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

fn provider_model_override(
    requested: Option<&str>,
    catalog: &[ProviderModelEntry],
) -> Option<String> {
    let requested = requested?;
    if requested == "provider-default" {
        return None;
    }
    if catalog.iter().any(|model| model.id() == requested) {
        return Some(requested.to_owned());
    }
    tracing::warn!(
        model = requested,
        "model outside provider catalog; using provider default"
    );
    None
}

const MAX_IMAGE_BYTES: usize = 768 * 1024;

async fn parse_turn_input(params: &Value) -> Result<Vec<CommandContent>, ThreadMethodError> {
    let input = params
        .get("input")
        .and_then(Value::as_array)
        .ok_or(ThreadMethodError::InvalidParams)?;
    let mut blocks = Vec::with_capacity(input.len());
    let mut encoded_bytes = 0usize;
    for entry in input {
        let block = match entry.get("type").and_then(Value::as_str) {
            Some("text") => entry
                .get("text")
                .and_then(Value::as_str)
                .ok_or(ThreadMethodError::InvalidParams)
                .and_then(|text| {
                    CommandContent::text(text.to_owned())
                        .map_err(|_| ThreadMethodError::InvalidParams)
                }),
            Some("localImage") => {
                let path = entry
                    .get("path")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .ok_or(ThreadMethodError::InvalidParams)?;
                if !path.is_absolute() {
                    return Err(ThreadMethodError::InvalidParams);
                }
                let mime_type = image_mime_type(&path)?;
                let metadata = tokio::fs::metadata(&path)
                    .await
                    .map_err(|_| ThreadMethodError::ImageUnreadable)?;
                if metadata.len() > MAX_IMAGE_BYTES as u64 {
                    return Err(ThreadMethodError::ImageTooLarge);
                }
                let bytes = tokio::fs::read(&path)
                    .await
                    .map_err(|_| ThreadMethodError::ImageUnreadable)?;
                if bytes.len() > MAX_IMAGE_BYTES {
                    return Err(ThreadMethodError::ImageTooLarge);
                }
                use base64::Engine as _;
                CommandContent::image(
                    mime_type.into(),
                    base64::engine::general_purpose::STANDARD.encode(bytes),
                    None,
                )
                .map_err(|_| ThreadMethodError::InvalidParams)
            }
            Some("image") => {
                let url = entry
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or(ThreadMethodError::InvalidParams)?;
                let (media_type, encoded) = url
                    .split_once(",")
                    .ok_or(ThreadMethodError::InvalidParams)?;
                let mime_type = media_type
                    .strip_prefix("data:")
                    .and_then(|value| value.strip_suffix(";base64"))
                    .filter(|value| value.starts_with("image/"))
                    .ok_or(ThreadMethodError::ImageUnsupportedType)?;
                if encoded.len() > collaboration_protocol::MAX_CONTROL_FRAME_BYTES {
                    return Err(ThreadMethodError::ImageTooLarge);
                }
                use base64::Engine as _;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(|_| ThreadMethodError::InvalidParams)?;
                if bytes.len() > MAX_IMAGE_BYTES {
                    return Err(ThreadMethodError::ImageTooLarge);
                }
                CommandContent::image(mime_type.into(), encoded.to_owned(), None)
                    .map_err(|_| ThreadMethodError::InvalidParams)
            }
            _ => Err(ThreadMethodError::InvalidParams),
        }?;
        let block_bytes = match &block {
            CommandContent::Text { text } => text.as_str().len(),
            CommandContent::Image { data, .. } => data.as_str().len(),
            _ => 0,
        };
        encoded_bytes = encoded_bytes.saturating_add(block_bytes);
        if encoded_bytes > collaboration_protocol::MAX_CONTROL_FRAME_BYTES {
            return Err(ThreadMethodError::PromptTooLarge);
        }
        blocks.push(block);
    }
    Ok(blocks)
}

fn image_mime_type(path: &std::path::Path) -> Result<&'static str, ThreadMethodError> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => Ok("image/png"),
        Some("jpg" | "jpeg") => Ok("image/jpeg"),
        Some("webp") => Ok("image/webp"),
        Some("gif") => Ok("image/gif"),
        _ => Err(ThreadMethodError::ImageUnsupportedType),
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;

    #[tokio::test]
    #[allow(clippy::panic_in_result_fn)]
    async fn local_image_is_read_into_one_typed_image_block()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let image_path = directory.path().join("sample.png");
        std::fs::write(&image_path, [0x89, b'P', b'N', b'G', 1, 2, 3])?;
        let input = json!({"input":[{"type":"text","text":"look"},
            {"type":"localImage","path":image_path}]});
        let blocks = parse_turn_input(&input).await?;
        assert_eq!(
            blocks,
            vec![
                CommandContent::text("look".into())?,
                CommandContent::image("image/png".into(), "iVBORwECAw==".into(), None)?,
            ]
        );
        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::panic_in_result_fn)]
    async fn image_input_reports_unreadable_and_oversized_files()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let missing = directory.path().join("missing.png");
        let unreadable =
            parse_turn_input(&json!({"input":[{"type":"localImage","path":missing}]})).await;
        assert!(matches!(
            unreadable,
            Err(ThreadMethodError::ImageUnreadable)
        ));

        let too_large = directory.path().join("large.png");
        std::fs::write(&too_large, vec![0u8; MAX_IMAGE_BYTES + 1])?;
        let oversized =
            parse_turn_input(&json!({"input":[{"type":"localImage","path":too_large}]})).await;
        assert!(matches!(oversized, Err(ThreadMethodError::ImageTooLarge)));
        let inline = parse_turn_input(&json!({"input":[{"type":"image",
            "url":"data:image/png;base64,AQID"}]}))
        .await?;
        assert_eq!(
            inline,
            vec![CommandContent::image(
                "image/png".into(),
                "AQID".into(),
                None
            )?]
        );
        Ok(())
    }
}
