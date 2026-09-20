//! Reusable ACP conversation client; native process and translation ownership stay server-side.
use crate::conversation_contract::{
    ConversationCreatePromptError, ConversationCreatePromptRequest, ConversationCreatePromptResult,
    ConversationCreateRequest, ConversationCreateResult, ConversationEnd, ConversationEvent,
    ConversationPromptRequest, ExistingConversationPromptError, ExistingConversationPromptRequest,
    ExistingConversationPromptResult,
};
use crate::{AcpTransportConnection, ClientError};
use collaboration_protocol::{
    AcpSchemaCatalog, EndpointId, EndpointRef, MessageContent, SessionRef, render_message,
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};
use tokio_util::sync::CancellationToken;

const FRAME_LIMIT: usize = 64 * 1024 * 1024;
const MAX_PROMPT_RESULT_UPDATES: usize = 1024;
const MAX_PROMPT_RESULT_BYTES: usize = FRAME_LIMIT;

struct BoundedPromptUpdates {
    updates: Vec<Value>,
    bytes: usize,
}

impl BoundedPromptUpdates {
    const fn new() -> Self {
        Self {
            updates: Vec::new(),
            bytes: 0,
        }
    }

    fn push(&mut self, update: Value) -> Result<(), ClientError> {
        let bytes = serde_json::to_vec(&update)
            .map_err(|_| ClientError::Protocol("ACP update encoding failed"))?
            .len();
        if self.updates.len() >= MAX_PROMPT_RESULT_UPDATES
            || self.bytes.saturating_add(bytes) > MAX_PROMPT_RESULT_BYTES
        {
            return Err(ClientError::Protocol("ACP prompt updates overflow"));
        }
        self.bytes += bytes;
        self.updates.push(update);
        Ok(())
    }

    fn into_updates(self) -> Vec<Value> {
        self.updates
    }
}
pub struct AcpConversation {
    stream: BufReader<UnixStream>,
    frame: Vec<u8>,
    schemas: AcpSchemaCatalog,
    endpoint: EndpointRef,
    service_directory: std::path::PathBuf,
    target: Option<SessionRef>,
    session_ready: bool,
    next_id: u64,
    callbacks: BTreeSet<String>,
    pending_updates: Vec<Value>,
    pending_bytes: usize,
    load_supported: bool,
    failed: bool,
}
impl AcpConversation {
    pub async fn create_and_prompt(
        directory: &Path,
        request: ConversationCreatePromptRequest,
        cancel: CancellationToken,
    ) -> Result<ConversationCreatePromptResult, ConversationCreatePromptError> {
        request
            .prompt
            .validate()
            .map_err(|source| crate::OperationError::before_dispatch("validation", None, source))?;
        validate_conversation_create_request(&request.create)
            .map_err(|source| crate::OperationError::before_dispatch("validation", None, source))?;
        if request.create.session.is_some() || request.create.fork.is_some() {
            return Err(crate::OperationError::before_dispatch(
                "validation",
                None,
                ClientError::InvalidRequest(
                    "create-and-prompt requires a fresh conversation request",
                ),
            ));
        }
        let endpoint_id = request.create.endpoint.endpoint_id.clone();
        let mut conversation = Self::connect_with_context(directory, endpoint_id).await?;
        validate_conversation_endpoint(conversation.endpoint(), &request.create.endpoint)
            .map_err(|source| crate::OperationError::before_dispatch("validation", None, source))?;
        let mut updates = BoundedPromptUpdates::new();
        let mut permission_required = false;
        let mut prompt_result = None;
        let mut emit = |event| {
            match event {
                ConversationEvent::SessionUpdate { update, .. } => updates.push(update)?,
                ConversationEvent::PermissionRequired(_) => permission_required = true,
                ConversationEvent::PromptResult { result, .. } => prompt_result = Some(result),
                ConversationEvent::SessionReady(_) => {}
            }
            Ok(())
        };
        let (target, end) = conversation
            .open_and_prompt(&request.create, request.prompt, cancel, &mut emit)
            .await?;
        Ok(ConversationCreatePromptResult {
            target,
            end,
            updates: updates.into_updates(),
            permission_required,
            result: prompt_result,
        })
    }

    pub async fn open_and_prompt(
        &mut self,
        create: &ConversationCreateRequest,
        prompt: ConversationPromptRequest,
        cancel: CancellationToken,
        emit: &mut impl FnMut(ConversationEvent) -> Result<(), ClientError>,
    ) -> Result<(SessionRef, ConversationEnd), ConversationCreatePromptError> {
        prompt
            .validate()
            .map_err(|source| crate::OperationError::before_dispatch("validation", None, source))?;
        let target = self.open_session_with_context(create, emit).await?;
        let end = self
            .prompt_and_wait(prompt, cancel, emit)
            .await
            .map_err(|source| {
                crate::OperationError::after_dispatch("prompt", Some(target.clone()), None, source)
            })?;
        Ok((target, end))
    }

    pub async fn prompt_existing(
        directory: &Path,
        request: ExistingConversationPromptRequest,
        cancel: CancellationToken,
    ) -> Result<ExistingConversationPromptResult, ExistingConversationPromptError> {
        let requested_target = request.target.clone();
        request.prompt.validate().map_err(|source| {
            crate::OperationError::before_dispatch(
                "validation",
                Some(requested_target.clone()),
                source,
            )
        })?;
        let endpoint = requested_target.endpoint.clone();
        let mut conversation = Self::connect_with_context(directory, endpoint.endpoint_id.clone())
            .await
            .map_err(|error| error.with_known_target(requested_target.clone()))?;
        validate_conversation_endpoint(conversation.endpoint(), &endpoint).map_err(|source| {
            crate::OperationError::before_dispatch(
                "validation",
                Some(requested_target.clone()),
                source,
            )
        })?;
        let mut updates = BoundedPromptUpdates::new();
        let mut permission_required = false;
        let mut prompt_result = None;
        let mut emit = |event| {
            match event {
                ConversationEvent::SessionUpdate { update, .. } => updates.push(update)?,
                ConversationEvent::PermissionRequired(_) => permission_required = true,
                ConversationEvent::PromptResult { result, .. } => prompt_result = Some(result),
                ConversationEvent::SessionReady(_) => {}
            }
            Ok(())
        };
        let create = ConversationCreateRequest {
            endpoint,
            cwd: request.cwd,
            session: Some(request.target.session_id.clone()),
            fork: None,
            model: None,
            effort: request.prompt.effort.clone(),
            access: None,
            created_by: None,
            approver: None,
            root_message_id: None,
        };
        let target = conversation
            .open_session_with_context(&create, &mut emit)
            .await?;
        let end = conversation
            .prompt_and_wait(request.prompt, cancel, &mut emit)
            .await
            .map_err(|source| {
                crate::OperationError::after_dispatch("prompt", Some(target.clone()), None, source)
            })?;
        Ok(ExistingConversationPromptResult {
            target,
            end,
            updates: updates.into_updates(),
            permission_required,
            result: prompt_result,
        })
    }

    #[must_use]
    pub fn endpoint(&self) -> &EndpointRef {
        &self.endpoint
    }
    pub async fn connect(directory: &Path, endpoint_id: EndpointId) -> Result<Self, ClientError> {
        Self::connect_with_context(directory, endpoint_id)
            .await
            .map_err(crate::OperationError::into_source)
    }

    pub async fn connect_with_context(
        directory: &Path,
        endpoint_id: EndpointId,
    ) -> Result<Self, crate::OperationError> {
        let transport = AcpTransportConnection::connect(directory, endpoint_id)
            .await
            .map_err(|source| crate::OperationError::before_dispatch("connect", None, source))?;
        if String::from(transport.schema_digest.clone())
            != format!("sha256:{}", collaboration_protocol::ACP_SCHEMA_DIGEST)
        {
            return Err(crate::OperationError::before_dispatch(
                "connect",
                None,
                ClientError::UnsupportedCapability("ACP schema profile"),
            ));
        }
        let schemas = AcpSchemaCatalog::load().map_err(|_| {
            crate::OperationError::before_dispatch(
                "initialize",
                None,
                ClientError::Protocol("ACP schema unavailable"),
            )
        })?;
        let mut client = Self {
            stream: BufReader::new(transport.stream),
            frame: Vec::new(),
            schemas,
            endpoint: transport.endpoint,
            service_directory: directory.to_owned(),
            target: None,
            session_ready: false,
            next_id: 0,
            callbacks: BTreeSet::new(),
            pending_updates: Vec::new(),
            pending_bytes: 0,
            load_supported: false,
            failed: false,
        };
        let initialize = json!({"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"agent-collaboration","version":env!("CARGO_PKG_VERSION")}});
        client
            .validate("InitializeRequest", &initialize)
            .map_err(|source| crate::OperationError::before_dispatch("initialize", None, source))?;
        let result = client
            .request(
                "initialize",
                initialize,
                "InitializeRequest",
                "InitializeResponse",
            )
            .await
            .map_err(|source| crate::OperationError::before_dispatch("initialize", None, source))?;
        if result.get("protocolVersion") != Some(&json!(1)) {
            return Err(crate::OperationError::before_dispatch(
                "initialize",
                None,
                ClientError::Protocol("unsupported ACP version"),
            ));
        }
        client.load_supported = result
            .pointer("/agentCapabilities/loadSession")
            .and_then(Value::as_bool)
            == Some(true);
        Ok(client)
    }
    /// Connects, initializes, and selects exactly one existing or newly-created
    /// Codex conversation. The returned client remains attached for a following
    /// prompt; dropping it never deletes the backend conversation.
    pub async fn create(
        directory: &Path,
        request: ConversationCreateRequest,
        emit: &mut impl FnMut(ConversationEvent) -> Result<(), ClientError>,
    ) -> Result<(Self, ConversationCreateResult), crate::OperationError> {
        validate_conversation_create_request(&request)
            .map_err(|source| crate::OperationError::before_dispatch("validation", None, source))?;
        let endpoint_id = request.endpoint.endpoint_id.clone();
        let mut client = Self::connect_with_context(directory, endpoint_id).await?;
        validate_conversation_endpoint(&client.endpoint, &request.endpoint)
            .map_err(|source| crate::OperationError::before_dispatch("validation", None, source))?;
        let target = client.open_session_with_context(&request, emit).await?;
        Ok((client, ConversationCreateResult { target }))
    }
    pub async fn open_session(
        &mut self,
        request: &ConversationCreateRequest,
        emit: &mut impl FnMut(ConversationEvent) -> Result<(), ClientError>,
    ) -> Result<SessionRef, ClientError> {
        self.open_session_with_context(request, emit)
            .await
            .map_err(crate::OperationError::into_source)
    }

    async fn open_session_with_context(
        &mut self,
        request: &ConversationCreateRequest,
        emit: &mut impl FnMut(ConversationEvent) -> Result<(), ClientError>,
    ) -> Result<SessionRef, crate::OperationError> {
        validate_conversation_create_request(request)
            .map_err(|source| crate::OperationError::before_dispatch("validation", None, source))?;
        // A requested switch retires the previous selection even when setup rejects
        // or its future is cancelled. Only completed setup may enable a prompt.
        self.session_ready = false;
        self.target = None;
        self.pending_updates.clear();
        self.pending_bytes = 0;
        if !request.cwd.is_absolute() {
            return Err(crate::OperationError::before_dispatch(
                "validation",
                None,
                ClientError::Protocol("ACP cwd must be absolute"),
            ));
        }
        let target = if let Some(id) = &request.session {
            let target = SessionRef {
                endpoint: self.endpoint.clone(),
                session_id: id.clone(),
            };
            if !self.load_supported {
                return Err(crate::OperationError::before_dispatch(
                    "load",
                    Some(target),
                    ClientError::UnsupportedCapability("ACP session/load"),
                ));
            }
            self.target = Some(target.clone());
            let params = json!({"sessionId":id,"cwd":request.cwd,"mcpServers":[],"_meta":{"codexRouter":router_metadata_effort(request.effort.as_deref())}});
            self.validate("LoadSessionRequest", &params)
                .map_err(|source| {
                    crate::OperationError::before_dispatch("load", Some(target.clone()), source)
                })?;
            self.request(
                "session/load",
                params,
                "LoadSessionRequest",
                "LoadSessionResponse",
            )
            .await
            .map_err(|source| {
                crate::OperationError::after_dispatch("load", Some(target.clone()), None, source)
            })?;
            target
        } else {
            // An absent model or effort is omitted, not sent as null: on fork the
            // adapter reads the source thread to fill it.
            let mut router_metadata = serde_json::Map::new();
            if let Some(model) = &request.model {
                router_metadata.insert("model".to_owned(), json!(model));
            }
            if let Some(effort) = &request.effort {
                router_metadata.insert("effort".to_owned(), json!(effort));
            }
            router_metadata.extend([
                ("access".to_owned(), json!(request.access)),
                ("createdBy".to_owned(), json!(request.created_by)),
                ("approver".to_owned(), json!(request.approver)),
            ]);
            let scratch_scope = request
                .root_message_id
                .as_ref()
                .map(|value| String::from(value.clone()))
                .unwrap_or_else(session_scratch_scope);
            let scratch_path = self
                .service_directory
                .parent()
                .ok_or(ClientError::Protocol("service directory has no owner root"))
                .map_err(|source| {
                    crate::OperationError::before_dispatch("preparation", None, source)
                })?
                .join("scratch")
                .join(&scratch_scope);
            prepare_project_write_areas(&request.cwd, request.access.as_deref())
                .map_err(ClientError::from)
                .map_err(|source| {
                    crate::OperationError::before_dispatch("preparation", None, source)
                })?;
            create_private_scratch(&scratch_path).map_err(|source| {
                crate::OperationError::before_dispatch("preparation", None, source)
            })?;
            router_metadata.insert("rootMessageId".into(), json!(request.root_message_id));
            router_metadata.insert("scratchScope".into(), json!(scratch_scope));
            router_metadata.insert("scratchPath".into(), json!(scratch_path));
            if let Some(source_thread_id) = &request.fork {
                router_metadata.insert("forkThreadId".into(), json!(source_thread_id));
            }
            let params =
                json!({"cwd":request.cwd,"mcpServers":[],"_meta":{"codexRouter":router_metadata}});
            let stage = if request.fork.is_some() {
                "fork"
            } else {
                "new"
            };
            self.validate("NewSessionRequest", &params)
                .map_err(|source| crate::OperationError::before_dispatch(stage, None, source))?;
            let result = self
                .request(
                    "session/new",
                    params,
                    "NewSessionRequest",
                    "NewSessionResponse",
                )
                .await
                .map_err(|source| {
                    crate::OperationError::after_dispatch(stage, None, None, source)
                })?;
            SessionRef {
                endpoint: self.endpoint.clone(),
                session_id: result
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .ok_or(ClientError::Protocol("ACP session ID missing"))
                    .map_err(|source| {
                        crate::OperationError::after_dispatch(stage, None, None, source)
                    })?
                    .to_owned()
                    .try_into()
                    .map_err(|_| {
                        crate::OperationError::after_dispatch(
                            stage,
                            None,
                            None,
                            ClientError::Protocol("invalid session ID"),
                        )
                    })?,
            }
        };
        self.target = Some(target.clone());
        emit(ConversationEvent::SessionReady(target.clone())).map_err(|source| {
            crate::OperationError::after_dispatch("setup", Some(target.clone()), None, source)
        })?;
        for update in std::mem::take(&mut self.pending_updates) {
            emit(ConversationEvent::SessionUpdate {
                target: target.clone(),
                update,
            })
            .map_err(|source| {
                crate::OperationError::after_dispatch("setup", Some(target.clone()), None, source)
            })?;
        }
        self.pending_bytes = 0;
        self.session_ready = true;
        Ok(target)
    }
    pub async fn prompt(
        &mut self,
        text: &str,
        effort: Option<&str>,
        timeout: Duration,
        cancel: CancellationToken,
        emit: &mut impl FnMut(ConversationEvent) -> Result<(), ClientError>,
    ) -> Result<ConversationEnd, ClientError> {
        if cancel.is_cancelled() {
            return Ok(ConversationEnd::Cancelled);
        }
        tokio::time::Instant::now()
            .checked_add(timeout)
            .ok_or(ClientError::InvalidRequest("invalid prompt deadline"))?;
        if !self.session_ready {
            return Err(ClientError::Protocol("ACP session setup has not completed"));
        }
        let target = self
            .target
            .clone()
            .ok_or(ClientError::Protocol("ACP session not opened"))?;
        let session = String::from(target.session_id.clone());
        let id = self
            .submit(
                "session/prompt",
                json!({"sessionId":session,"prompt":[{"type":"text","text":text}],"_meta":{"codexRouter":router_metadata_effort(effort)}}),
                "PromptRequest",
            )
            .await?;
        let mut deadline = tokio::time::Instant::now()
            .checked_add(timeout)
            .ok_or(ClientError::InvalidRequest("invalid prompt deadline"))?;
        let mut end = ConversationEnd::Completed;
        loop {
            let frame = tokio::select! {
                _=cancel.cancelled(),if end==ConversationEnd::Completed=>{
                    end=ConversationEnd::Cancelled;
                    self.write(&json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}})).await?;
                    deadline=tokio::time::Instant::now()+Duration::from_secs(30);continue;
                },
                _=tokio::time::sleep_until(deadline)=>{
                    if end!=ConversationEnd::Completed{return Err(ClientError::Timeout);}
                    end=ConversationEnd::TimedOut;
                    self.write(&json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}})).await?;
                    deadline=tokio::time::Instant::now()+Duration::from_secs(30);continue;
                },
                frame=self.read()=>frame?,
            };
            if frame.get("method").is_some() {
                if let Some(update) = self.handle_callback(&frame).await? {
                    emit(ConversationEvent::SessionUpdate {
                        target: target.clone(),
                        update,
                    })?;
                }
                if frame.get("method").and_then(Value::as_str) == Some("session/request_permission")
                {
                    emit(ConversationEvent::PermissionRequired(target.clone()))?;
                }
                continue;
            }
            let result = self.response(frame, &id, "PromptResponse")?;
            emit(ConversationEvent::PromptResult {
                target,
                end,
                result,
            })?;
            return Ok(end);
        }
    }
    /// Renders caller-declared public content once, then waits for the ACP
    /// response on the selected conversation connection.
    pub async fn prompt_and_wait(
        &mut self,
        request: ConversationPromptRequest,
        cancel: CancellationToken,
        emit: &mut impl FnMut(ConversationEvent) -> Result<(), ClientError>,
    ) -> Result<ConversationEnd, ClientError> {
        request.validate()?;
        let target = self
            .target
            .as_ref()
            .ok_or(ClientError::Protocol("ACP session not opened"))?;
        let message = MessageContent::from(request.message);
        let rendered = render_message(target, &message)
            .map_err(|_| ClientError::Protocol("conversation message rendering failed"))?;
        self.prompt(
            &rendered.text,
            request.effort.as_deref(),
            Duration::from_secs(request.timeout_seconds),
            cancel,
            emit,
        )
        .await
    }
    async fn request(
        &mut self,
        method: &str,
        params: Value,
        input: &str,
        output: &str,
    ) -> Result<Value, ClientError> {
        let id = self.submit(method, params, input).await?;
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let frame = self.read().await?;
                if frame.get("method").is_some() {
                    if let Some(update) = self.handle_callback(&frame).await? {
                        self.pending_bytes += serde_json::to_vec(&update)
                            .map_err(|_| ClientError::Protocol("ACP update encoding failed"))?
                            .len();
                        if self.pending_bytes > FRAME_LIMIT || self.pending_updates.len() >= 1024 {
                            return Err(ClientError::Protocol("ACP setup updates overflow"));
                        }
                        self.pending_updates.push(update);
                    }
                } else {
                    return self.response(frame, &id, output);
                }
            }
        })
        .await
        .map_err(|_| ClientError::Timeout)?
    }
    async fn submit(
        &mut self,
        method: &str,
        params: Value,
        input: &str,
    ) -> Result<String, ClientError> {
        if self.failed {
            return Err(ClientError::Protocol(
                "ACP connection has an unsettled request",
            ));
        }
        self.validate(input, &params)?;
        if self.next_id >= 65536 {
            return Err(ClientError::Protocol("ACP request budget exhausted"));
        }
        let id = format!("conversation-{}", self.next_id);
        self.next_id += 1;
        self.failed = true;
        self.write(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;
        Ok(id)
    }
    fn response(&mut self, frame: Value, id: &str, output: &str) -> Result<Value, ClientError> {
        if frame.get("id").and_then(Value::as_str) != Some(id) {
            return Err(ClientError::Protocol("unexpected ACP response ID"));
        }
        match (frame.get("result"), frame.get("error")) {
            (Some(value), None) => {
                self.validate(output, value)?;
                self.failed = false;
                Ok(value.clone())
            }
            (None, Some(error)) => {
                self.validate("Error", error)?;
                let code = error
                    .get("code")
                    .and_then(Value::as_i64)
                    .ok_or(ClientError::Protocol("invalid ACP error"))?;
                self.failed = false;
                Err(ClientError::Rejected {
                    code,
                    data: error.get("data").cloned(),
                })
            }
            _ => Err(ClientError::Protocol("invalid ACP response")),
        }
    }
    async fn handle_callback(&mut self, frame: &Value) -> Result<Option<Value>, ClientError> {
        let params = frame
            .get("params")
            .ok_or(ClientError::Protocol("missing ACP parameters"))?;
        match frame.get("method").and_then(Value::as_str) {
            Some("session/update") if frame.get("id").is_none() => {
                self.validate("SessionNotification", params)?;
                self.check_session(params)?;
                Ok(Some(
                    params
                        .get("update")
                        .ok_or(ClientError::Protocol("missing ACP update"))?
                        .clone(),
                ))
            }
            Some("session/request_permission") => {
                self.validate("RequestPermissionRequest", params)?;
                self.check_session(params)?;
                let id = frame
                    .get("id")
                    .filter(|id| id.is_string() || id.is_number())
                    .ok_or(ClientError::Protocol("invalid permission ID"))?;
                if self.callbacks.len() >= 65536 || !self.callbacks.insert(id.to_string()) {
                    return Err(ClientError::Protocol("reused ACP permission ID"));
                }
                self.write(
                    &json!({"jsonrpc":"2.0","id":id,"result":{"outcome":{"outcome":"cancelled"}}}),
                )
                .await?;
                Ok(None)
            }
            _ => Err(ClientError::Protocol("unsupported ACP callback")),
        }
    }
    fn check_session(&self, params: &Value) -> Result<(), ClientError> {
        if let Some(target) = &self.target
            && params.get("sessionId").and_then(Value::as_str)
                != Some(String::from(target.session_id.clone()).as_str())
        {
            return Err(ClientError::Protocol("ACP session scope mismatch"));
        }
        Ok(())
    }
    fn validate(&mut self, name: &str, value: &Value) -> Result<(), ClientError> {
        if self
            .schemas
            .validate(name, value)
            .map_err(|_| ClientError::Protocol("ACP schema unavailable"))?
        {
            Ok(())
        } else {
            Err(ClientError::Protocol("ACP payload rejected by schema"))
        }
    }
    async fn write(&mut self, value: &Value) -> Result<(), ClientError> {
        let mut bytes =
            serde_json::to_vec(value).map_err(|_| ClientError::Protocol("ACP encoding failed"))?;
        if bytes.len() > FRAME_LIMIT {
            return Err(ClientError::Protocol("ACP frame too large"));
        }
        bytes.push(b'\n');
        tokio::time::timeout(
            Duration::from_secs(30),
            self.stream.get_mut().write_all(&bytes),
        )
        .await
        .map_err(|_| ClientError::Timeout)??;
        Ok(())
    }
    async fn read(&mut self) -> Result<Value, ClientError> {
        loop {
            let buffer = self.stream.fill_buf().await?;
            if buffer.is_empty() {
                return Err(ClientError::Protocol("ACP connection closed"));
            }
            let end = buffer.iter().position(|b| *b == b'\n');
            let count = end.unwrap_or(buffer.len());
            if count > FRAME_LIMIT.saturating_sub(self.frame.len()) {
                return Err(ClientError::Protocol("ACP frame too large"));
            }
            self.frame.extend_from_slice(
                buffer
                    .get(..count)
                    .ok_or(ClientError::Protocol("ACP frame boundary"))?,
            );
            self.stream.consume(count + usize::from(end.is_some()));
            if end.is_some() {
                let value: Value = serde_json::from_slice(&std::mem::take(&mut self.frame))
                    .map_err(|_| ClientError::Protocol("invalid ACP JSON"))?;
                if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") || !value.is_object()
                {
                    return Err(ClientError::Protocol("invalid ACP envelope"));
                }
                return Ok(value);
            }
        }
    }
}

fn validate_conversation_endpoint(
    observed: &EndpointRef,
    requested: &EndpointRef,
) -> Result<(), ClientError> {
    if observed != requested {
        return Err(ClientError::InvalidRequest(
            "conversation endpoint belongs to another service",
        ));
    }
    Ok(())
}

fn validate_conversation_create_request(
    request: &ConversationCreateRequest,
) -> Result<(), ClientError> {
    if !request.cwd.is_absolute() {
        return Err(ClientError::InvalidRequest("ACP cwd must be absolute"));
    }
    if request.session.is_some() && request.fork.is_some() {
        return Err(ClientError::InvalidRequest(
            "session and fork cannot be selected together",
        ));
    }
    if request.session.is_some()
        && (request.model.is_some()
            || request.access.is_some()
            || request.created_by.is_some()
            || request.approver.is_some()
            || request.root_message_id.is_some())
    {
        return Err(ClientError::InvalidRequest(
            "existing session cannot change creation settings",
        ));
    }
    if request.session.is_none()
        && request.fork.is_none()
        && (request.model.is_none() || request.effort.is_none() || request.access.is_none())
    {
        return Err(ClientError::InvalidRequest(
            "new conversation requires model, effort, and access",
        ));
    }
    if request.session.is_none() {
        let created_by = request
            .created_by
            .as_ref()
            .ok_or(ClientError::InvalidRequest(
                "new or forked conversation requires createdBy",
            ))?;
        let approver = request
            .approver
            .as_ref()
            .ok_or(ClientError::InvalidRequest(
                "new or forked conversation requires approver",
            ))?;
        if created_by.endpoint != request.endpoint || approver.endpoint != request.endpoint {
            return Err(ClientError::InvalidRequest(
                "conversation identities belong to another endpoint",
            ));
        }
    }
    Ok(())
}

fn create_private_scratch(path: &Path) -> Result<(), ClientError> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700).create(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(ClientError::Protocol(
            "scratch directory is not owner-private",
        ));
    }
    Ok(())
}

/// Creates the project directories write-restricted access declares writable.
///
/// Workspace-write already owns the worktree, so Router must not materialize
/// `tmp/` or `docs/wip/` in it on that path.
fn prepare_project_write_areas(cwd: &Path, access: Option<&str>) -> std::io::Result<()> {
    if access != Some("write-restricted") {
        return Ok(());
    }
    std::fs::create_dir_all(cwd.join("tmp"))?;
    std::fs::create_dir_all(cwd.join("docs/wip"))
}

/// Omits the effort key entirely when the caller selected none, so the adapter
/// reads absence rather than a null it would have to interpret.
fn router_metadata_effort(effort: Option<&str>) -> Value {
    effort.map_or_else(|| json!({}), |effort| json!({"effort": effort}))
}

fn session_scratch_scope() -> String {
    format!("session-{}", uuid::Uuid::now_v7())
}

#[cfg(test)]
#[path = "acp_conversation_tests.rs"]
mod tests;
