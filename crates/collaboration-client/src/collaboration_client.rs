//! The typed client for the collaboration API on this machine.
//!
//! A client reads the published version 3 manifest for the Router's identity and version,
//! then calls tools over the owner-only API socket. There is no handshake: each call stands
//! alone, and nothing is replayed after an uncertain answer.
use crate::api_connection::{ApiConnection, DEFAULT_CALL_TIMEOUT, ToolAnswer};
use collaboration_protocol::{
    EndpointInventory, MachineLabel, SERVICE_MANIFEST_VERSION, SchemaDigest, ServiceManifest,
    UuidIdentity,
};
use serde_json::{Value, json};
use std::{
    io::{self, Read},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::net::UnixStream;

const PERMISSION_DIAGNOSTIC_MESSAGE: &str = "Request automated approval review through your tool, or ask the user to grant the required command/socket access. Retry only after access is granted.";

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("invalid collaboration request: {0}")]
    InvalidRequest(&'static str),
    #[error("unsupported protocol capability: {0}")]
    UnsupportedCapability(&'static str),
    #[error("collaboration API discovery failed at {stage}")]
    Discovery {
        stage: &'static str,
        source: std::io::Error,
    },
    #[error("collaboration API transport failed: {0}")]
    Transport(#[from] std::io::Error),
    #[error("collaboration API protocol violation: {0}")]
    Protocol(&'static str),
    #[error("collaboration API request timed out; no request was replayed")]
    Timeout,
    #[error("collaboration API request rejected with code {code}")]
    Rejected { code: i64, data: Option<Value> },
    /// The API was at its request limit: the request was not run and is safe to retry.
    #[error("collaboration API at capacity; the request was not run and is safe to retry")]
    Overloaded { message: String },
}

impl ClientError {
    /// The provider conversation failure this error carries: a conversation tool's typed
    /// rejection, or the API's admission overload in the conversation family's shape.
    #[must_use]
    pub fn conversation_failure(
        &self,
    ) -> Option<collaboration_protocol::ConversationOperationFailure> {
        match self {
            Self::Rejected {
                data: Some(data), ..
            } => serde_json::from_value(data.clone()).ok(),
            Self::Overloaded { message } => crate::admission_overload::conversation(message),
            _ => None,
        }
    }

    #[must_use]
    pub fn permission_diagnostic(&self) -> Option<collaboration_protocol::PermissionDiagnostic> {
        let Self::Discovery { stage, source } = self else {
            return None;
        };
        if source.kind() != std::io::ErrorKind::PermissionDenied {
            return None;
        }
        let stage = match *stage {
            "manifest-read" => collaboration_protocol::PermissionDiagnosticStage::ManifestRead,
            "directory-resolve" => {
                collaboration_protocol::PermissionDiagnosticStage::DirectoryResolve
            }
            "socket-resolve" => collaboration_protocol::PermissionDiagnosticStage::SocketResolve,
            "socket-connect" => collaboration_protocol::PermissionDiagnosticStage::SocketConnect,
            _ => return None,
        };
        Some(collaboration_protocol::PermissionDiagnostic {
            kind: collaboration_protocol::PermissionDiagnosticKind::PermissionDenied,
            stage,
            message: PERMISSION_DIAGNOSTIC_MESSAGE.to_owned(),
            next_action: collaboration_protocol::PermissionDiagnosticNextAction::RequestApproval,
        })
    }
}

/// The Router a client calls, as its published manifest names it.
#[derive(Clone, Debug)]
pub struct RouterIdentity {
    pub service_id: UuidIdentity,
    pub service_epoch: UuidIdentity,
    pub service_version: String,
    pub machine_label: MachineLabel,
    pub native_schema_digest: Option<SchemaDigest>,
}

/// A client of one Router's collaboration API.
#[derive(Clone)]
pub struct CollaborationClient {
    pub(crate) connection: ApiConnection,
    identity: RouterIdentity,
    directory: PathBuf,
}

impl CollaborationClient {
    /// Reads the manifest in `directory` and checks the API socket it names is reachable.
    pub async fn connect(directory: &Path, name: &str, version: &str) -> Result<Self, ClientError> {
        if !directory.is_absolute() {
            return Err(ClientError::Protocol("service directory must be absolute"));
        }
        let manifest = read_manifest(directory).map_err(|error| match error {
            ClientError::Transport(source) => ClientError::Discovery {
                stage: "manifest-read",
                source,
            },
            other => other,
        })?;
        let root = std::fs::canonicalize(directory).map_err(|source| ClientError::Discovery {
            stage: "directory-resolve",
            source,
        })?;
        let socket =
            std::fs::canonicalize(root.join(manifest.api.path.file_name())).map_err(|source| {
                ClientError::Discovery {
                    stage: "socket-resolve",
                    source,
                }
            })?;
        if socket.parent() != Some(root.as_path()) {
            return Err(ClientError::Protocol(
                "API socket escapes service directory",
            ));
        }
        let probe = tokio::time::timeout(DEFAULT_CALL_TIMEOUT, UnixStream::connect(&socket))
            .await
            .map_err(|_| ClientError::Timeout)?
            .map_err(|source| ClientError::Discovery {
                stage: "socket-connect",
                source,
            })?;
        drop(probe);
        let socket = socket
            .to_str()
            .ok_or(ClientError::Protocol("API socket path must be UTF-8"))?;
        let service_version = String::from(manifest.service_version);
        crate::warn_on_router_version_mismatch(name, version, &service_version);
        crate::record_service_version(&service_version);
        Ok(Self {
            connection: ApiConnection::new(socket),
            identity: RouterIdentity {
                service_id: manifest.service_id,
                service_epoch: manifest.service_epoch,
                service_version,
                machine_label: manifest.machine_label,
                native_schema_digest: manifest.native_schema_digest,
            },
            directory: root,
        })
    }

    #[must_use]
    pub fn identity(&self) -> &RouterIdentity {
        &self.identity
    }

    #[must_use]
    pub fn machine_label(&self) -> &MachineLabel {
        &self.identity.machine_label
    }

    /// The canonical service directory the manifest was read from.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Agent-originated communication; delivery defaults are carried by the typed request.
    pub async fn send_agent_message(
        &self,
        params: collaboration_protocol::SessionMessageSendParams,
    ) -> Result<collaboration_protocol::DeliveryReceipt, ClientError> {
        if !matches!(
            params.message,
            collaboration_protocol::MessageContent::Agent { .. }
        ) {
            return Err(ClientError::InvalidRequest("agent message required"));
        }
        self.submit_message_with_push(params)
            .await
            .map(|result| result.receipt)
    }

    /// Explicit human input; does not manufacture an authenticated human identity.
    pub async fn send_human_input(
        &self,
        params: collaboration_protocol::SessionMessageSendParams,
    ) -> Result<collaboration_protocol::DeliveryReceipt, ClientError> {
        if !matches!(
            params.message,
            collaboration_protocol::MessageContent::HumanUser { .. }
        ) {
            return Err(ClientError::InvalidRequest("human input required"));
        }
        self.submit_message_with_push(params)
            .await
            .map(|result| result.receipt)
    }

    /// Replies to one stored direct message using its push id or Router link.
    pub async fn message_reply(
        &self,
        params: collaboration_protocol::SessionMessageReplyParams,
    ) -> Result<collaboration_protocol::SessionMessageReplyResult, ClientError> {
        if params.caller.endpoint.service_id != self.identity().service_id {
            return Err(ClientError::InvalidRequest(
                "reply caller belongs to another service",
            ));
        }
        let value = self
            .delivery_receipt("message_reply", json!(params))
            .await?;
        serde_json::from_value(value)
            .map_err(|_| ClientError::Protocol("invalid message reply result; acceptance unknown"))
    }

    pub(crate) async fn submit_message_with_push(
        &self,
        params: collaboration_protocol::SessionMessageSendParams,
    ) -> Result<collaboration_protocol::PushMessageSendResult, ClientError> {
        use collaboration_protocol::{
            AcceptedResumeEffect, DeliveryClientReceipt, DeliveryOutcome, MessageDelivery,
            MessageInputKind, MessageRepresentation, NativeInputOperation, NativeSendAcceptance,
            SessionReachability,
        };
        let message = crate::PublicMessageContent::try_from(params.message.clone())?;
        let value = self
            .delivery_receipt(
                "message_send",
                json!({
                    "target": params.target,
                    "message": message,
                    "delivery": params.mode,
                    "generationGuard": params.generation_guard,
                }),
            )
            .await?;
        let decoded =
            serde_json::from_value::<collaboration_protocol::PushMessageSendResult>(value);
        let Ok(receipt) = decoded else {
            return Err(ClientError::Protocol(
                "invalid message receipt; acceptance unknown",
            ));
        };
        // The service turns either caller input kind into a Router-authored
        // push line. Native delivery renders that line as declared agent text.
        let kind = MessageInputKind::Agent;
        let representation = MessageRepresentation::DeclaredAgentText;
        let delivery_receipt = &receipt.receipt;
        let expected_link = collaboration_protocol::RouterLink::new(
            collaboration_protocol::MachineId::from(self.identity.service_id.clone()),
            receipt.push_id.clone(),
        )
        .to_string();
        let consistent = receipt.target == params.target
            && receipt.link == expected_link
            && match (&delivery_receipt.reachability, &delivery_receipt.client) {
                (None, None) => true,
                (
                    Some(SessionReachability::CodexAppServer),
                    Some(DeliveryClientReceipt::CodexAppServer(native)),
                ) => {
                    let acceptance_matches = matches!(
                        (&params.mode, &delivery_receipt.outcome, &native.acceptance),
                        (
                            MessageDelivery::Auto,
                            DeliveryOutcome::Started,
                            NativeSendAcceptance::NativeInputAccepted {
                                operation: NativeInputOperation::TurnStart,
                                ..
                            }
                        ) | (
                            MessageDelivery::Auto,
                            DeliveryOutcome::StartedOrSteered,
                            NativeSendAcceptance::NativeInputAccepted { .. }
                        ) | (
                            MessageDelivery::Auto,
                            DeliveryOutcome::Steered,
                            NativeSendAcceptance::SteerAccepted { .. }
                        ) | (
                            MessageDelivery::Queue,
                            DeliveryOutcome::Queued,
                            NativeSendAcceptance::QueueAccepted { .. }
                        ) | (
                            MessageDelivery::Steer,
                            DeliveryOutcome::Steered,
                            NativeSendAcceptance::SteerAccepted { .. }
                        )
                    );
                    native.target == params.target
                        && params
                            .generation_guard
                            .as_ref()
                            .is_none_or(|guard| guard == &native.generation)
                        && native.input_kind == kind
                        && native.representation == representation
                        && acceptance_matches
                        && (params.mode == MessageDelivery::Auto
                            || native.resume_effect == AcceptedResumeEffect::NotRequested)
                        && String::from(native.client_user_message_id.clone())
                            == receipt.push_id.as_str()
                }
                (
                    Some(SessionReachability::ProviderAcp),
                    Some(DeliveryClientReceipt::ProviderAcp { .. }),
                ) => {
                    matches!(
                        delivery_receipt.outcome,
                        DeliveryOutcome::Started
                            | DeliveryOutcome::Steered
                            | DeliveryOutcome::Queued
                    )
                }
                (
                    Some(SessionReachability::ClaudeCodePeer),
                    Some(DeliveryClientReceipt::ClaudeCodePeer),
                ) => {
                    matches!(
                        delivery_receipt.outcome,
                        DeliveryOutcome::PeerMessageWritten
                    )
                }
                (Some(_), None) => matches!(
                    delivery_receipt.outcome,
                    DeliveryOutcome::NotSubmitted { .. }
                        | DeliveryOutcome::Rejected(_)
                        | DeliveryOutcome::Unknown
                ),
                _ => false,
            };
        if !consistent {
            return Err(ClientError::Protocol(
                "inconsistent message receipt; acceptance unknown",
            ));
        }
        Ok(receipt)
    }

    /// A stored push and its delivery receipt. A delivery the tool reports as an error (not
    /// submitted, refused or unknown) still carries the stored push, which the caller judges.
    async fn delivery_receipt(&self, tool: &str, arguments: Value) -> Result<Value, ClientError> {
        match self.connection.answer(tool, arguments).await? {
            ToolAnswer::Success(value) => Ok(value),
            ToolAnswer::Failure(mut failure)
                if failure.get("pushId").is_some() && failure.get("receipt").is_some() =>
            {
                if let Some(fields) = failure.as_object_mut() {
                    for presentation in ["mcpResult", "kind", "message", "effect"] {
                        fields.remove(presentation);
                    }
                }
                Ok(failure)
            }
            ToolAnswer::Failure(failure) => {
                Err(crate::api_connection::rejection_from_tool_failure(failure))
            }
        }
    }

    pub async fn list_sessions(
        &self,
        params: collaboration_protocol::NativeSessionListParams,
    ) -> Result<collaboration_protocol::NativeSessionListResult, ClientError> {
        if !(1..=100).contains(&params.page_size) {
            return Err(ClientError::InvalidRequest("invalid session page size"));
        }
        let value = self.connection.call("sessions_list", json!(params)).await?;
        let result: collaboration_protocol::NativeSessionListResult = serde_json::from_value(value)
            .map_err(|_| ClientError::Protocol("invalid session inventory"))?;
        if result.endpoint != params.endpoint
            || result
                .sessions
                .iter()
                .any(|s| s.target.endpoint != params.endpoint)
            || result.sessions.len() > params.page_size as usize
        {
            return Err(ClientError::Protocol("inconsistent session inventory"));
        }
        Ok(result)
    }

    pub async fn list_provider_sessions(
        &self,
        params: collaboration_protocol::ProviderSessionListParams,
    ) -> Result<collaboration_protocol::ProviderSessionListResult, ClientError> {
        if !(1..=100).contains(&params.page_size) {
            return Err(ClientError::InvalidRequest(
                "invalid provider session page size",
            ));
        }
        let value = self
            .connection
            .call("provider_sessions_list", json!(params))
            .await?;
        let result: collaboration_protocol::ProviderSessionListResult =
            serde_json::from_value(value)
                .map_err(|_| ClientError::Protocol("invalid provider session inventory"))?;
        if result.endpoint != params.endpoint
            || result.sessions.len() > params.page_size as usize
            || result
                .sessions
                .iter()
                .any(|row| row.target().endpoint != params.endpoint)
        {
            return Err(ClientError::Protocol(
                "inconsistent provider session inventory",
            ));
        }
        Ok(result)
    }

    pub async fn inspect_session(
        &self,
        target: &collaboration_protocol::SessionRef,
    ) -> Result<collaboration_protocol::NativeInspectResult, ClientError> {
        let params = collaboration_protocol::NativeInspectParams {
            target: target.clone(),
        };
        let result = self
            .connection
            .call("session_inspect", json!(params))
            .await?;
        let result: collaboration_protocol::NativeInspectResult = serde_json::from_value(result)
            .map_err(|_| ClientError::Protocol("invalid inspection result"))?;
        if result.target != *target
            || result.generation.service_epoch != self.identity.service_epoch
            || result.thread.get("id").and_then(Value::as_str)
                != Some(String::from(target.session_id.clone()).as_str())
        {
            return Err(ClientError::Protocol("inconsistent inspection result"));
        }
        Ok(result)
    }

    pub async fn rename_session(
        &self,
        params: collaboration_protocol::NativeRenameParams,
    ) -> Result<collaboration_protocol::NativeRenameResult, ClientError> {
        let target = params.target.clone();
        let result = self
            .connection
            .call("session_rename", json!(params))
            .await?;
        let result: collaboration_protocol::NativeRenameResult = serde_json::from_value(result)
            .map_err(|_| ClientError::Protocol("invalid rename result"))?;
        if result.target != target {
            return Err(ClientError::Protocol("inconsistent rename result"));
        }
        Ok(result)
    }

    pub async fn interrupt_turn(
        &self,
        target: &collaboration_protocol::SessionRef,
        generation: &collaboration_protocol::CodexGeneration,
        turn_id: &str,
    ) -> Result<collaboration_protocol::NativeInterruptResult, ClientError> {
        let turn_id = collaboration_protocol::NonEmptyText::try_from(turn_id.to_owned())
            .map_err(|_| ClientError::InvalidRequest("invalid exact turn ID"))?;
        let params = collaboration_protocol::NativeInterruptParams {
            target: target.clone(),
            generation: generation.clone(),
            turn_id: turn_id.clone(),
        };
        let value = self
            .connection
            .call("turn_interrupt", json!(params))
            .await?;
        let result: collaboration_protocol::NativeInterruptResult =
            serde_json::from_value(value)
                .map_err(|_| ClientError::Protocol("invalid interruption result"))?;
        if result.target != *target || result.generation != *generation || result.turn_id != turn_id
        {
            return Err(ClientError::Protocol("inconsistent interruption result"));
        }
        Ok(result)
    }

    pub async fn list_pending_approvals(
        &self,
        pending_only: bool,
    ) -> Result<collaboration_protocol::ApprovalListResult, ClientError> {
        let value = self
            .connection
            .call(
                "approval_list",
                json!(collaboration_protocol::ApprovalListParams {
                    pending: pending_only,
                    include_options: false,
                }),
            )
            .await?;
        serde_json::from_value(value).map_err(|_| ClientError::Protocol("invalid approval list"))
    }

    pub async fn list_approvals_with_options(
        &self,
        pending_only: bool,
    ) -> Result<collaboration_protocol::ApprovalDetailedListResult, ClientError> {
        let value = self
            .connection
            .call(
                "approval_list",
                json!(collaboration_protocol::ApprovalListParams {
                    pending: pending_only,
                    include_options: true,
                }),
            )
            .await?;
        serde_json::from_value(value)
            .map_err(|_| ClientError::Protocol("invalid detailed approval list"))
    }

    pub async fn decide_approval(
        &self,
        params: collaboration_protocol::ApprovalDecideParams,
    ) -> Result<collaboration_protocol::ApprovalDecideResult, crate::OperationError> {
        let request_id = params.request_id.clone();
        let encoded = serde_json::to_value(params).map_err(|_| {
            crate::OperationError::before_dispatch(
                "approval-decision",
                None,
                ClientError::InvalidRequest("invalid approval decision"),
            )
        })?;
        let value = self
            .connection
            .call("approval_decide", encoded)
            .await
            .map_err(|source| {
                crate::OperationError::after_dispatch("approval-decision", None, None, source)
            })?;
        let result: collaboration_protocol::ApprovalDecideResult = serde_json::from_value(value)
            .map_err(|_| {
                crate::OperationError::after_dispatch(
                    "approval-decision",
                    None,
                    None,
                    ClientError::Protocol("invalid approval decision receipt"),
                )
            })?;
        if result.request_id != request_id
            || result.state != collaboration_protocol::ApprovalState::Decided
        {
            return Err(crate::OperationError::after_dispatch(
                "approval-decision",
                None,
                None,
                ClientError::Protocol("inconsistent approval decision receipt"),
            ));
        }
        Ok(result)
    }

    pub async fn list_questions(
        &self,
        pending_only: bool,
    ) -> Result<collaboration_protocol::QuestionListResult, ClientError> {
        let value = self
            .connection
            .call(
                "question_list",
                json!(collaboration_protocol::QuestionListParams {
                    pending: pending_only
                }),
            )
            .await?;
        serde_json::from_value(value).map_err(|_| ClientError::Protocol("invalid question list"))
    }

    pub async fn answer_question(
        &self,
        params: collaboration_protocol::QuestionAnswerParams,
    ) -> Result<collaboration_protocol::QuestionAnswerResult, crate::OperationError> {
        let request_id = params.request_id.clone();
        let encoded = serde_json::to_value(params).map_err(|_| {
            crate::OperationError::before_dispatch(
                "question-answer",
                None,
                ClientError::InvalidRequest("invalid question answer"),
            )
        })?;
        let value = self
            .connection
            .call("question_answer", encoded)
            .await
            .map_err(|source| {
                crate::OperationError::after_dispatch("question-answer", None, None, source)
            })?;
        let result: collaboration_protocol::QuestionAnswerResult = serde_json::from_value(value)
            .map_err(|_| {
                crate::OperationError::after_dispatch(
                    "question-answer",
                    None,
                    None,
                    ClientError::Protocol("invalid question result"),
                )
            })?;
        if result.request_id != request_id {
            return Err(crate::OperationError::after_dispatch(
                "question-answer",
                None,
                None,
                ClientError::Protocol("inconsistent question result"),
            ));
        }
        Ok(result)
    }

    pub async fn journal_status(&self) -> Result<crate::JournalStatus, ClientError> {
        let value = self.connection.call("journal_status", json!({})).await?;
        serde_json::from_value(value).map_err(|_| ClientError::Protocol("invalid journal status"))
    }

    /// Reads a captured historical address page without loading any native thread.
    pub async fn list_addresses(
        &self,
        endpoint: &collaboration_protocol::EndpointRef,
        page_size: u32,
        cursor: Option<&str>,
    ) -> Result<collaboration_protocol::AddressPage, ClientError> {
        if !(1..=100).contains(&page_size)
            || cursor.is_some_and(|cursor| cursor.is_empty() || cursor.len() > 1024)
        {
            return Err(ClientError::InvalidRequest(
                "invalid address snapshot arguments",
            ));
        }
        let params = json!(collaboration_protocol::AddressListParams {
            endpoint: endpoint.clone(),
            page_size,
            cursor: cursor.map(str::to_owned),
        });
        let value = self.connection.call("addresses_list", params).await?;
        let page: collaboration_protocol::AddressPage = serde_json::from_value(value)
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
            return Err(ClientError::Protocol("inconsistent address snapshot"));
        }
        Ok(page)
    }

    pub async fn read_journal(
        &self,
        endpoint: &collaboration_protocol::EndpointRef,
        after: collaboration_protocol::JournalPosition,
        page_size: u32,
        wait_milliseconds: u64,
    ) -> Result<collaboration_protocol::JournalPage, ClientError> {
        if !(1..=100).contains(&page_size) || wait_milliseconds > 30000 {
            return Err(ClientError::InvalidRequest(
                "invalid journal read arguments",
            ));
        }
        let value = self
            .connection
            .call_with_timeout(
                "journal_read",
                json!(collaboration_protocol::JournalReadParams {
                    endpoint: endpoint.clone(),
                    after: after.clone(),
                    page_size,
                    wait_milliseconds,
                }),
                DEFAULT_CALL_TIMEOUT.saturating_add(Duration::from_millis(wait_milliseconds)),
            )
            .await?;
        let page: collaboration_protocol::JournalPage = serde_json::from_value(value)
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

    pub async fn list_endpoints(&self) -> Result<EndpointInventory, ClientError> {
        let value = self.connection.call("endpoints_list", json!({})).await?;
        let result: EndpointInventory = serde_json::from_value(value)
            .map_err(|_| ClientError::Protocol("invalid endpoint inventory"))?;
        if result.service_epoch != self.identity.service_epoch
            || result.endpoints.len() > 64
            || result.sequence > 9_007_199_254_740_991
        {
            return Err(ClientError::Protocol("inconsistent endpoint inventory"));
        }
        Ok(result)
    }
}

/// Reads the service manifest; only version 3 is read.
pub(crate) fn read_manifest(directory: &Path) -> Result<ServiceManifest, ClientError> {
    let path = directory.join("service.json");
    let metadata = std::fs::symlink_metadata(&path)?;
    if !metadata.is_file() || metadata.len() > 65536 {
        return Err(ClientError::Protocol("invalid service manifest file"));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(65537)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err(ClientError::Protocol("service manifest too large"));
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
        ClientError::Transport(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid service manifest",
        ))
    })?;
    if value.get("version").and_then(serde_json::Value::as_u64)
        != Some(u64::from(SERVICE_MANIFEST_VERSION))
    {
        return Err(ClientError::Protocol(
            "unsupported service manifest version; expected version 3",
        ));
    }
    serde_json::from_value(value).map_err(|error| {
        ClientError::Transport(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid service manifest: {error}"),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::CollaborationClient;

    #[tokio::test]
    async fn a_version_two_manifest_is_refused_before_socket_resolution() {
        let directory = tempfile::tempdir().expect("temporary service directory");
        let manifest = serde_json::json!({
            "version":2,
            "serviceId":"00000000-0000-4000-8000-000000000001",
            "serviceEpoch":"00000000-0000-4000-8000-000000000002",
            "machineLabel":"fixture-host",
            "control":{"transport":"unixJsonLines","path":"control.sock"},
            "controlSchemaDigest":format!("sha256:{}", "a".repeat(64)),
            "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
        });
        std::fs::write(
            directory.path().join("service.json"),
            serde_json::to_vec(&manifest).expect("manifest JSON"),
        )
        .expect("manifest fixture");
        let error = match CollaborationClient::connect(directory.path(), "test-client", "1").await {
            Ok(_) => panic!("a version two manifest must fail discovery"),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "collaboration API protocol violation: unsupported service manifest version; expected version 3"
        );
    }
}
