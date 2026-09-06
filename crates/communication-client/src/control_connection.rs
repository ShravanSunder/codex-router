//! Bounded sequential Control calls. No request replay or native process ownership.
use communication_protocol::{
    ControlFrameDecoder, ControlInitializationResult, EndpointChange, EndpointInventory,
};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("unsupported protocol capability: {0}")]
    UnsupportedCapability(&'static str),
    #[error("Control discovery failed at {stage}")]
    Discovery {
        stage: &'static str,
        source: std::io::Error,
    },
    #[error("Control transport failed: {0}")]
    Transport(#[from] std::io::Error),
    #[error("Control protocol violation: {0}")]
    Protocol(&'static str),
    #[error("Control request timed out; no request was replayed")]
    Timeout,
    #[error("Control request rejected with code {code}")]
    Rejected { code: i64, data: Option<Value> },
}
pub struct ControlClient {
    connection: ClientConnection,
    identity: ControlInitializationResult,
    notification_sequence: u64,
}
struct ClientConnection {
    stream: UnixStream,
    decoder: ControlFrameDecoder,
    incoming: VecDeque<Value>,
    notifications: VecDeque<Value>,
    next_id: u64,
    failed: bool,
}
impl ControlClient {
    pub async fn initialize(
        stream: UnixStream,
        name: &str,
        version: &str,
    ) -> Result<Self, ClientError> {
        let mut connection = ClientConnection {
            stream,
            decoder: ControlFrameDecoder::default(),
            incoming: VecDeque::new(),
            notifications: VecDeque::new(),
            next_id: 0,
            failed: false,
        };
        let value = connection
            .call(
                "control/initialize",
                json!(communication_protocol::ControlInitializationParams {
                    version: communication_protocol::ProtocolVersion { major: 1, minor: 0 },
                    client: communication_protocol::ControlClientInfo {
                        name: name
                            .to_owned()
                            .try_into()
                            .map_err(|_| ClientError::Protocol("invalid client name"))?,
                        version: version
                            .to_owned()
                            .try_into()
                            .map_err(|_| ClientError::Protocol("invalid client version"))?,
                    },
                }),
            )
            .await?;
        let identity: ControlInitializationResult = serde_json::from_value(value)
            .map_err(|_| ClientError::Protocol("invalid initialization result"))?;
        if identity.version.major != 1 || identity.version.minor != 0 {
            return Err(ClientError::Protocol("unsupported negotiated version"));
        }
        Ok(Self {
            connection,
            identity,
            notification_sequence: 0,
        })
    }
    #[must_use]
    pub fn identity(&self) -> &ControlInitializationResult {
        &self.identity
    }
    /// Agent-originated communication; delivery defaults are carried by the typed request.
    pub async fn send_agent_message(
        &mut self,
        params: communication_protocol::NativeSendParams,
    ) -> Result<communication_protocol::NativeSendReceipt, ClientError> {
        if !matches!(
            params.message,
            communication_protocol::MessageContent::Agent { .. }
        ) {
            return Err(ClientError::Protocol("agent message required"));
        }
        self.submit_message(params).await
    }
    /// Explicit human input; does not manufacture an authenticated human identity.
    pub async fn send_human_input(
        &mut self,
        params: communication_protocol::NativeSendParams,
    ) -> Result<communication_protocol::NativeSendReceipt, ClientError> {
        if !matches!(
            params.message,
            communication_protocol::MessageContent::HumanUser { .. }
        ) {
            return Err(ClientError::Protocol("human input required"));
        }
        self.submit_message(params).await
    }
    async fn submit_message(
        &mut self,
        params: communication_protocol::NativeSendParams,
    ) -> Result<communication_protocol::NativeSendReceipt, ClientError> {
        use communication_protocol::{
            AcceptedResumeEffect, MessageContent, MessageDelivery, MessageInputKind,
            MessageRepresentation, NativeSendAcceptance,
        };
        let value = self
            .connection
            .call("codex/messageSend", json!(params))
            .await?;
        let decoded = serde_json::from_value::<communication_protocol::NativeSendReceipt>(value);
        let Ok(receipt) = decoded else {
            self.connection.failed = true;
            return Err(ClientError::Protocol(
                "invalid message receipt; acceptance unknown",
            ));
        };
        let (kind, representation) = match params.message {
            MessageContent::Agent { .. } => (
                MessageInputKind::Agent,
                MessageRepresentation::DeclaredAgentText,
            ),
            MessageContent::HumanUser { .. } => (
                MessageInputKind::HumanUser,
                MessageRepresentation::HumanUserText,
            ),
        };
        let delivery_matches = matches!(
            (&params.delivery, &receipt.acceptance),
            (
                MessageDelivery::Auto,
                NativeSendAcceptance::NativeInputAccepted { .. }
                    | NativeSendAcceptance::SteerAccepted { .. }
            ) | (
                MessageDelivery::Queue,
                NativeSendAcceptance::QueueAccepted { .. }
            ) | (
                MessageDelivery::Steer,
                NativeSendAcceptance::SteerAccepted { .. }
            )
        );
        if receipt.target != params.target
            || receipt.generation != params.generation
            || receipt.input_kind != kind
            || receipt.representation != representation
            || !delivery_matches
            || (params.delivery != MessageDelivery::Auto
                && receipt.resume_effect != AcceptedResumeEffect::NotRequested)
            || params
                .client_user_message_id
                .as_ref()
                .is_some_and(|id| id != &receipt.client_user_message_id)
        {
            self.connection.failed = true;
            return Err(ClientError::Protocol(
                "inconsistent message receipt; acceptance unknown",
            ));
        }
        Ok(receipt)
    }
    pub async fn list_sessions(
        &mut self,
        params: communication_protocol::NativeSessionListParams,
    ) -> Result<communication_protocol::NativeSessionListResult, ClientError> {
        if !(1..=100).contains(&params.page_size) {
            return Err(ClientError::Protocol("invalid session page size"));
        }
        let value = self
            .connection
            .call("codex/sessionList", json!(params))
            .await?;
        let result: communication_protocol::NativeSessionListResult = serde_json::from_value(value)
            .map_err(|_| ClientError::Protocol("invalid session inventory"))?;
        if result.endpoint != params.endpoint
            || result
                .sessions
                .iter()
                .any(|s| s.target.endpoint != params.endpoint)
            || result.sessions.len() > params.page_size as usize
        {
            self.connection.failed = true;
            return Err(ClientError::Protocol("inconsistent session inventory"));
        }
        Ok(result)
    }
    pub async fn inspect_session(
        &mut self,
        target: &communication_protocol::SessionRef,
    ) -> Result<communication_protocol::NativeInspectResult, ClientError> {
        let params = communication_protocol::NativeInspectParams {
            target: target.clone(),
        };
        let result = self
            .connection
            .call("codex/sessionInspect", json!(params))
            .await?;
        let result: communication_protocol::NativeInspectResult = serde_json::from_value(result)
            .map_err(|_| ClientError::Protocol("invalid inspection result"))?;
        if result.target != *target
            || result.generation.service_epoch != self.identity.service_epoch
            || result.thread.get("id").and_then(Value::as_str)
                != Some(String::from(target.session_id.clone()).as_str())
        {
            self.connection.failed = true;
            return Err(ClientError::Protocol("inconsistent inspection result"));
        }
        Ok(result)
    }
    pub async fn interrupt_turn(
        &mut self,
        target: &communication_protocol::SessionRef,
        generation: &communication_protocol::CodexGeneration,
        turn_id: &str,
    ) -> Result<communication_protocol::NativeInterruptResult, ClientError> {
        let turn_id = communication_protocol::NonEmptyText::try_from(turn_id.to_owned())
            .map_err(|_| ClientError::Protocol("invalid exact turn ID"))?;
        let params = communication_protocol::NativeInterruptParams {
            target: target.clone(),
            generation: generation.clone(),
            turn_id: turn_id.clone(),
        };
        let value = self
            .connection
            .call("codex/turnInterrupt", json!(params))
            .await?;
        let result: communication_protocol::NativeInterruptResult =
            serde_json::from_value(value)
                .map_err(|_| ClientError::Protocol("invalid interruption result"))?;
        if result.target != *target || result.generation != *generation || result.turn_id != turn_id
        {
            self.connection.failed = true;
            return Err(ClientError::Protocol("inconsistent interruption result"));
        }
        Ok(result)
    }
    pub async fn journal_status(&mut self) -> Result<crate::JournalStatus, ClientError> {
        let value = self
            .connection
            .call("lifecycleJournal/status", json!({}))
            .await?;
        serde_json::from_value(value).map_err(|_| ClientError::Protocol("invalid journal status"))
    }
    /// Reads a captured historical address page without loading any native thread.
    pub async fn list_addresses(
        &mut self,
        endpoint: &communication_protocol::EndpointRef,
        page_size: u32,
        cursor: Option<&str>,
    ) -> Result<communication_protocol::AddressPage, ClientError> {
        if !(1..=100).contains(&page_size)
            || cursor.is_some_and(|cursor| cursor.is_empty() || cursor.len() > 1024)
        {
            return Err(ClientError::Protocol("invalid address snapshot arguments"));
        }
        let params = json!(communication_protocol::AddressListParams {
            endpoint: endpoint.clone(),
            page_size,
            cursor: cursor.map(str::to_owned),
        });
        let value = self.connection.call("addressBook/list", params).await?;
        let page: communication_protocol::AddressPage = serde_json::from_value(value)
            .map_err(|_| ClientError::Protocol("invalid address snapshot"))?;
        if page.coverage.endpoint != *endpoint
            || page.entries.len() > page_size as usize
            || page.watermark.sequence > 9_007_199_254_740_991
            || page.entries.iter().any(|entry| {
                entry.address.endpoint != *endpoint
                    || entry.last_observation.journal_id != page.watermark.journal_id
                    || entry.last_observation.sequence > page.watermark.sequence
            })
            || page
                .next_cursor
                .as_ref()
                .is_some_and(|cursor| cursor.is_empty() || cursor.len() > 1024)
        {
            self.connection.failed = true;
            return Err(ClientError::Protocol("inconsistent address snapshot"));
        }
        Ok(page)
    }
    pub async fn read_journal(
        &mut self,
        endpoint: &communication_protocol::EndpointRef,
        after: communication_protocol::JournalPosition,
        page_size: u32,
        wait_milliseconds: u64,
    ) -> Result<communication_protocol::JournalPage, ClientError> {
        if !(1..=100).contains(&page_size) || wait_milliseconds > 30000 {
            return Err(ClientError::Protocol("invalid journal read arguments"));
        }
        let value = self
            .connection
            .call(
                "lifecycleJournal/read",
                json!(communication_protocol::JournalReadParams {
                    endpoint: endpoint.clone(),
                    after: after.clone(),
                    page_size,
                    wait_milliseconds,
                }),
            )
            .await?;
        let page: communication_protocol::JournalPage = serde_json::from_value(value)
            .map_err(|_| ClientError::Protocol("invalid journal page"))?;
        if page.bounds.journal_id != after.journal_id
            || page.next.journal_id != after.journal_id
            || page.next.sequence < after.sequence
            || page.next.sequence > page.bounds.last_sequence
            || page.records.len() > page_size as usize
        {
            return Err(ClientError::Protocol("inconsistent journal page"));
        }
        let mut previous = after.sequence;
        for record in &page.records {
            if record.journal_id != after.journal_id
                || record.schema_version != 1
                || record.sequence <= previous
                || record.sequence > page.next.sequence
                || record.observation.scope.endpoint != *endpoint
                || record.observation.validate().is_err()
            {
                return Err(ClientError::Protocol("inconsistent journal record"));
            }
            previous = record.sequence;
        }
        Ok(page)
    }
    pub async fn list_endpoints(&mut self) -> Result<EndpointInventory, ClientError> {
        let value = self.connection.call("endpoint/list", json!({})).await?;
        let result: EndpointInventory = serde_json::from_value(value)
            .map_err(|_| ClientError::Protocol("invalid endpoint inventory"))?;
        if result.service_epoch != self.identity.service_epoch
            || result.endpoints.len() > 64
            || result.sequence > 9_007_199_254_740_991
        {
            self.connection.failed = true;
            return Err(ClientError::Protocol("inconsistent endpoint inventory"));
        }
        Ok(result)
    }
    /// Waits for a scoped endpoint notification without submitting another request.
    pub async fn next_notification(&mut self) -> Result<Value, ClientError> {
        if self.connection.failed {
            return Err(ClientError::Protocol("connection is retired"));
        }
        let result = self.receive_notification().await;
        if result.is_err() {
            self.connection.failed = true;
        }
        result
    }
    async fn receive_notification(&mut self) -> Result<Value, ClientError> {
        let frame = loop {
            if let Some(frame) = self.connection.notifications.pop_front() {
                break frame;
            }
            if let Some(frame) = self.connection.incoming.pop_front() {
                break frame;
            }
            self.connection.read_frames().await?;
        };
        if frame.get("jsonrpc") != Some(&json!("2.0"))
            || frame.get("method") != Some(&json!("endpoint/changed"))
            || frame.get("id").is_some()
            || frame.as_object().is_none_or(|value| value.len() != 3)
        {
            return Err(ClientError::Protocol("invalid endpoint notification"));
        }
        let params: EndpointChange = serde_json::from_value(
            frame
                .get("params")
                .cloned()
                .ok_or(ClientError::Protocol("missing notification parameters"))?,
        )
        .map_err(|_| ClientError::Protocol("invalid notification parameters"))?;
        if params.service_epoch != self.identity.service_epoch
            || params.endpoint.endpoint.service_id != self.identity.service_id
            || params.sequence > 9_007_199_254_740_991
            || params.sequence != self.notification_sequence + 1
        {
            return Err(ClientError::Protocol(
                "notification scope or sequence mismatch",
            ));
        }
        self.notification_sequence = params.sequence;
        Ok(frame)
    }
    pub async fn close(mut self) -> Result<(), ClientError> {
        self.connection.stream.shutdown().await?;
        Ok(())
    }
}
impl ClientConnection {
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, ClientError> {
        if self.failed || self.next_id >= 65_536 {
            return Err(ClientError::Protocol("connection is retired"));
        }
        // Set before awaiting: a dropped future leaves the exchange retired.
        self.failed = true;
        let result =
            tokio::time::timeout(Duration::from_secs(30), self.exchange(method, params)).await;
        match result {
            Ok(Ok(value)) => {
                self.failed = false;
                Ok(value)
            }
            Ok(Err(error)) => {
                self.failed = true;
                Err(error)
            }
            Err(_) => {
                self.failed = true;
                Err(ClientError::Timeout)
            }
        }
    }
    async fn exchange(&mut self, method: &str, params: Value) -> Result<Value, ClientError> {
        let id = format!("client-{}", self.next_id);
        self.next_id += 1;
        let mut bytes =
            serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
                .map_err(|_| ClientError::Protocol("request encoding"))?;
        if bytes.len() > communication_protocol::MAX_CONTROL_FRAME_BYTES {
            return Err(ClientError::Protocol("request too large"));
        }
        bytes.push(b'\n');
        self.stream.write_all(&bytes).await?;
        loop {
            if let Some(frame) = self.incoming.pop_front() {
                if frame.get("jsonrpc") != Some(&json!("2.0")) {
                    return Err(ClientError::Protocol("invalid envelope version"));
                }
                if frame.get("id").is_none() && frame.get("method").is_some() {
                    if self.notifications.len() >= 1024 {
                        return Err(ClientError::Protocol("notification overflow"));
                    }
                    self.notifications.push_back(frame);
                    continue;
                }
                if frame.get("id") != Some(&json!(id)) {
                    return Err(ClientError::Protocol("response ID mismatch"));
                }
                match (frame.get("result"), frame.get("error")) {
                    (Some(result), None) => return Ok(result.clone()),
                    (None, Some(error)) => {
                        if !communication_protocol::control_error_is_valid(method, &frame) {
                            return Err(ClientError::Protocol("invalid method error response"));
                        }
                        return Err(ClientError::Rejected {
                            code: error
                                .get("code")
                                .and_then(Value::as_i64)
                                .ok_or(ClientError::Protocol("invalid error code"))?,
                            data: error.get("data").cloned(),
                        });
                    }
                    _ => {
                        return Err(ClientError::Protocol(
                            "response must contain result or error",
                        ));
                    }
                }
            }
            self.read_frames().await?;
        }
    }
    async fn read_frames(&mut self) -> Result<(), ClientError> {
        let mut buffer = [0_u8; 8192];
        let count = self.stream.read(&mut buffer).await?;
        if count == 0 {
            return Err(ClientError::Protocol("connection closed"));
        }
        let input = buffer
            .get(..count)
            .ok_or(ClientError::Protocol("read boundary"))?;
        self.incoming.extend(
            self.decoder
                .push(input)
                .map_err(|_| ClientError::Protocol("invalid Control framing"))?,
        );
        Ok(())
    }
}
