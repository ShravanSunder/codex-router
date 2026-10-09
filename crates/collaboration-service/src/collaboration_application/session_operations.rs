//! Sessions, the lifecycle journal and the address book.
//!
//! Codex native operations are generation-scoped calls on the Codex app-server; this module owns
//! their admission and failure classification but no provider policy or native process. Session
//! inventories page through `codex_session_inventory` and `provider_session_inventory`.
use super::{
    CollaborationRejection, CollaborationRejectionReason, PublishedRejection, ResultByteBudget,
};
use crate::ServiceIdentity;
use codex_native_integration::{NativeConnectionError, NativeOperation, NativeProtocolConnection};
use collaboration_protocol::{
    ChannelDescription, CodexGeneration, EndpointDescription, EndpointInventory,
    NativeInspectParams, NativeInspectResult, NativeInterruptKind, NativeInterruptParams,
    NativeInterruptResult, NativeRenameParams, NativeRenameResult, NativeSessionListParams,
    NativeSessionListResult, ProviderSessionListParams, ProviderSessionListResult, RouterAccess,
    SessionRef, SettingsObservation, SettingsUnavailableReason,
};
use serde_json::{Map, Value, json};

/// Session, journal and address-book operations over the Router's endpoints.
pub struct SessionOperations<'service> {
    pub(super) identity: &'service ServiceIdentity,
}

impl<'service> SessionOperations<'service> {
    pub(crate) fn new(identity: &'service ServiceIdentity) -> Self {
        Self { identity }
    }

    /// Every endpoint this Router publishes, with the Host-wide publication sequence.
    pub fn endpoints_list(&self) -> Result<EndpointInventory, EndpointDirectoryUnavailable> {
        let inventory = self
            .identity
            .directory
            .inventory()
            .map_err(|_| EndpointDirectoryUnavailable)?;
        Ok(EndpointInventory {
            service_epoch: self.identity.service_epoch.clone(),
            sequence: inventory.sequence,
            endpoints: inventory.endpoints,
        })
    }

    /// Pages a Codex endpoint's stored catalog or its loaded and active threads.
    pub async fn codex_session_list(
        &self,
        request: NativeSessionListParams,
        budget: ResultByteBudget,
    ) -> Result<NativeSessionListResult, NativeSessionFailure> {
        crate::codex_session_inventory::list_codex_sessions(self.identity, request, budget).await
    }

    /// Reads one Codex thread without its turns.
    pub async fn codex_session_inspect(
        &self,
        request: NativeInspectParams,
        budget: ResultByteBudget,
    ) -> Result<NativeInspectResult, NativeSessionFailure> {
        let stage = NativeSessionStage::Inspect;
        let native_params = json!({"threadId":String::from(request.target.session_id.clone()),"includeTurns":false});
        let call = self
            .native_call(
                &request.target,
                None,
                stage,
                NativeOperation::ReadThread,
                native_params,
            )
            .await?;
        let effective_access = match self.identity.approval_broker.as_deref() {
            Some(routes) => {
                recorded_access(
                    routes,
                    String::from(request.target.session_id.clone()).as_str(),
                )
                .await
            }
            None => None,
        };
        let result = call.result.map_err(|failure| {
            classify_native_call_failure(stage, false, &failure.error, failure.native.as_ref())
        })?;
        let Some(thread) = result.get("thread") else {
            return Err(NativeSessionFailure::refused(
                NativeSessionFailureKind::OutcomeUnknown,
                stage,
            ));
        };
        if thread.get("id").and_then(Value::as_str)
            != Some(String::from(request.target.session_id.clone()).as_str())
        {
            return Err(NativeSessionFailure::refused(
                NativeSessionFailureKind::OutcomeUnknown,
                stage,
            ));
        }
        let inspection = NativeInspectResult {
            target: request.target,
            generation: call.generation,
            effective_access,
            settings_observation: SettingsObservation::Unavailable {
                reason: SettingsUnavailableReason::ThreadReadOmitsSettings,
            },
            thread: thread.clone(),
        };
        fit_native_result(inspection, budget, stage)
    }

    /// Interrupts one turn of a Codex thread in the generation the caller observed.
    pub async fn codex_turn_interrupt(
        &self,
        request: NativeInterruptParams,
        budget: ResultByteBudget,
    ) -> Result<NativeInterruptResult, NativeSessionFailure> {
        let stage = NativeSessionStage::Interrupt;
        let native_params = json!({"threadId":String::from(request.target.session_id.clone()),"turnId":String::from(request.turn_id.clone())});
        let call = self
            .native_call(
                &request.target,
                Some(&request.generation),
                stage,
                NativeOperation::InterruptTurn,
                native_params,
            )
            .await?;
        let result = call.result.map_err(|failure| {
            classify_native_call_failure(stage, true, &failure.error, failure.native.as_ref())
        })?;
        if result != json!({}) {
            return Err(NativeSessionFailure::refused(
                NativeSessionFailureKind::OutcomeUnknown,
                stage,
            ));
        }
        let interruption = NativeInterruptResult {
            target: request.target,
            generation: call.generation,
            turn_id: request.turn_id,
            kind: NativeInterruptKind::InterruptCompleted,
        };
        fit_native_result(interruption, budget, stage)
    }

    /// Renames a Codex thread and confirms the name the runtime echoes back.
    pub async fn codex_session_rename(
        &self,
        request: NativeRenameParams,
    ) -> Result<NativeRenameResult, NativeSessionFailure> {
        let stage = NativeSessionStage::Rename;
        let refused = |kind| NativeSessionFailure::refused(kind, stage);
        if !valid_session_rename_name(&request.name) {
            return Err(NativeSessionFailure::InvalidRequest(
                INVALID_NATIVE_PARAMETERS,
            ));
        }
        let route = self.native_route(&request.target, stage)?;
        let Ok(admission) = route.backend.gate.acquire() else {
            return Err(refused(NativeSessionFailureKind::Unavailable));
        };
        let Some(schemas) = admission.schemas() else {
            return Err(refused(NativeSessionFailureKind::UnsupportedCapability));
        };
        if route.advertised_generation.as_ref() != Some(admission.generation())
            || route.advertised_digest.as_deref() != Some(schemas.schema_digest())
        {
            return Err(refused(NativeSessionFailureKind::UnsupportedCapability));
        }
        if !schemas.supports_operation(NativeOperation::SetThreadName) {
            return Err(rename_method_unsupported());
        }
        let Ok(mut connection) = NativeProtocolConnection::connect(admission.backend_path()).await
        else {
            return Err(refused(NativeSessionFailureKind::Unavailable));
        };
        let thread_id = String::from(request.target.session_id.clone());
        let read_thread = json!({"threadId":thread_id,"includeTurns":false});
        let before = match connection
            .request_validated(&schemas, NativeOperation::ReadThread, read_thread.clone())
            .await
        {
            Ok(before) => before,
            Err(error) => {
                let native = connection.take_last_rejection();
                return Err(classify_native_call_failure(
                    stage,
                    false,
                    &error,
                    native.as_ref(),
                ));
            }
        };
        let previous_name = before
            .pointer("/thread/name")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Err(error) = connection
            .request_validated(
                &schemas,
                NativeOperation::SetThreadName,
                json!({"threadId":thread_id,"name":request.name}),
            )
            .await
        {
            let native = connection.take_last_rejection();
            return Err(classify_native_call_failure(
                stage,
                true,
                &error,
                native.as_ref(),
            ));
        }
        let after = match connection
            .request_validated(&schemas, NativeOperation::ReadThread, read_thread)
            .await
        {
            Ok(after) => after,
            Err(error) => {
                let native = connection.take_last_rejection();
                return Err(classify_native_call_failure(
                    stage,
                    true,
                    &error,
                    native.as_ref(),
                ));
            }
        };
        let Some(name) = after.pointer("/thread/name").and_then(Value::as_str) else {
            return Err(refused(NativeSessionFailureKind::OutcomeUnknown));
        };
        if name != request.name {
            return Err(NativeSessionFailure::NameMismatch {
                requested: request.name,
                effective: name.to_owned(),
            });
        }
        self.identity
            .display_names
            .remember(request.target.clone(), name);
        Ok(NativeRenameResult {
            target: request.target,
            name: name.to_owned(),
            previous_name,
        })
    }

    /// Pages a provider endpoint's hosted sessions, or Claude Code's live terminal sessions.
    pub async fn provider_session_list(
        &self,
        request: ProviderSessionListParams,
        budget: ResultByteBudget,
    ) -> Result<ProviderSessionListResult, ProviderInventoryFailure> {
        crate::provider_session_inventory::list_provider_sessions(self.identity, request, budget)
            .await
    }

    /// The local Codex endpoint, its advertised native channel and the backend serving it.
    fn native_route(
        &self,
        target: &SessionRef,
        stage: NativeSessionStage,
    ) -> Result<NativeRoute<'service>, NativeSessionFailure> {
        let refused = |kind| NativeSessionFailure::refused(kind, stage);
        if target.endpoint.service_id != self.identity.service_id {
            return Err(refused(NativeSessionFailureKind::WrongService));
        }
        let endpoint = match self.identity.directory.read_endpoint(&target.endpoint) {
            Ok(Some(endpoint)) => endpoint,
            Ok(None) => return Err(refused(NativeSessionFailureKind::EndpointNotFound)),
            Err(_) => return Err(refused(NativeSessionFailureKind::Unavailable)),
        };
        let Some((advertised_digest, advertised_generation)) = native_channel(&endpoint) else {
            return Err(refused(NativeSessionFailureKind::UnsupportedCapability));
        };
        let Some(backend) = self
            .identity
            .native_backend
            .as_ref()
            .filter(|backend| backend.endpoint == target.endpoint)
        else {
            return Err(refused(NativeSessionFailureKind::UnsupportedCapability));
        };
        Ok(NativeRoute {
            advertised_digest,
            advertised_generation,
            backend,
        })
    }

    /// Admits one native read or write and runs it, unless the backend retires first.
    async fn native_call(
        &self,
        target: &SessionRef,
        expected_generation: Option<&CodexGeneration>,
        stage: NativeSessionStage,
        operation: NativeOperation,
        native_params: Value,
    ) -> Result<NativeCall, NativeSessionFailure> {
        let refused = |kind| NativeSessionFailure::refused(kind, stage);
        let route = self.native_route(target, stage)?;
        let Ok(admission) = route.backend.gate.acquire() else {
            return Err(refused(NativeSessionFailureKind::Unavailable));
        };
        if expected_generation.is_some_and(|generation| generation != admission.generation()) {
            return Err(refused(NativeSessionFailureKind::StaleGeneration));
        }
        let Some(schemas) = admission.schemas() else {
            return Err(refused(NativeSessionFailureKind::UnsupportedCapability));
        };
        if route.advertised_generation.as_ref() != Some(admission.generation()) {
            return Err(refused(NativeSessionFailureKind::Unavailable));
        }
        if route.advertised_digest.as_deref() != Some(schemas.schema_digest()) {
            return Err(refused(NativeSessionFailureKind::UnsupportedCapability));
        }
        let retired = admission.retirement();
        let connection = tokio::select! {
            biased;
            _ = retired.cancelled() => return Err(refused(NativeSessionFailureKind::Unavailable)),
            connection = NativeProtocolConnection::connect(admission.backend_path()) => connection,
        };
        let Ok(mut connection) = connection else {
            return Err(refused(NativeSessionFailureKind::Unavailable));
        };
        if retired.is_cancelled() {
            return Err(refused(NativeSessionFailureKind::Unavailable));
        }
        let result = tokio::select! {
            biased;
            result = connection.request_validated(&schemas, operation, native_params) => result,
            _ = retired.cancelled() => Err(NativeConnectionError::OutcomeUnknown),
        };
        let result = result.map_err(|error| NativeCallError {
            error,
            native: connection.take_last_rejection(),
        });
        Ok(NativeCall {
            generation: admission.generation().clone(),
            result,
        })
    }
}

struct NativeRoute<'service> {
    advertised_digest: Option<String>,
    advertised_generation: Option<CodexGeneration>,
    backend: &'service crate::NativeControlBackend,
}

struct NativeCall {
    generation: CodexGeneration,
    result: Result<Value, NativeCallError>,
}

struct NativeCallError {
    error: NativeConnectionError,
    native: Option<Value>,
}

fn native_channel(
    endpoint: &EndpointDescription,
) -> Option<(Option<String>, Option<CodexGeneration>)> {
    endpoint.channels.iter().find_map(|channel| match channel {
        ChannelDescription::NativeCodex {
            schema_digest,
            generation,
            ..
        } => Some((
            schema_digest
                .as_ref()
                .map(|digest| String::from(digest.clone())),
            generation.clone(),
        )),
        _ => None,
    })
}

fn fit_native_result<TResult: serde::Serialize>(
    result: TResult,
    budget: ResultByteBudget,
    stage: NativeSessionStage,
) -> Result<TResult, NativeSessionFailure> {
    if budget.admits(&result) {
        Ok(result)
    } else {
        Err(NativeSessionFailure::refused(
            NativeSessionFailureKind::ResponseTooLarge,
            stage,
        ))
    }
}

/// A rename name is one trimmed line of 1..=120 characters that is a valid display name.
pub(crate) fn valid_session_rename_name(name: &str) -> bool {
    let scalar_count = name.chars().count();
    name.trim() == name
        && (1..=120).contains(&scalar_count)
        && collaboration_protocol::SessionDisplayName::try_from(name.to_owned()).is_ok()
}

/// The access Router recorded for this thread, if the broker holds one.
async fn recorded_access(
    routes: &crate::ServiceInteractionBroker,
    thread_id: &str,
) -> Option<RouterAccess> {
    use codex_acp_adapter::ApprovalBroker;
    routes
        .route(thread_id)
        .await
        .ok()
        .flatten()
        .map(|route| route.access)
}

/// Classifies one failed native call.
///
/// A refusal keeps its reason and corrective action from the shared closed set. After a
/// dispatched mutation, a lost or unreadable outcome is unknown, never a refusal; before one,
/// it is plain unavailability.
pub(crate) fn classify_native_call_failure(
    stage: NativeSessionStage,
    mutation: bool,
    error: &NativeConnectionError,
    native: Option<&Value>,
) -> NativeSessionFailure {
    let stage_name = stage.as_str();
    match error {
        NativeConnectionError::InvalidInput => {
            NativeSessionFailure::InvalidRequest(INVALID_NATIVE_PARAMETERS)
        }
        NativeConnectionError::Rejected { code } => {
            let (reason, next_action) =
                crate::message_effect_state::classify_native_rejection(*code, native);
            let message = native
                .and_then(|value| value.get("message"))
                .and_then(Value::as_str)
                .filter(|message| !message.is_empty())
                .unwrap_or(NATIVE_FAILURE_MESSAGE)
                .to_owned();
            NativeSessionFailure::NativeRejected {
                stage,
                message,
                reason,
                next_action,
                native_code: (reason == "unknown").then_some(*code),
            }
        }
        NativeConnectionError::Unavailable if !mutation => {
            NativeSessionFailure::refused(NativeSessionFailureKind::Unavailable, stage)
        }
        NativeConnectionError::UnavailableWithCause(cause) if !mutation => {
            NativeSessionFailure::Refused {
                kind: NativeSessionFailureKind::Unavailable,
                stage,
                message: format!("Native {stage_name} failed before dispatch: {cause}"),
            }
        }
        error if mutation => NativeSessionFailure::Refused {
            kind: NativeSessionFailureKind::OutcomeUnknown,
            stage,
            message: format!(
                "Native {stage_name} outcome is unknown after dispatch: {error}; no request was replayed"
            ),
        },
        error => NativeSessionFailure::Refused {
            kind: NativeSessionFailureKind::Unavailable,
            stage,
            message: format!("Native {stage_name} read did not complete: {error}"),
        },
    }
}

/// The cached app-server schema lacks the rename method; name it and the repair.
pub(crate) fn rename_method_unsupported() -> NativeSessionFailure {
    let method_name = NativeOperation::SetThreadName.method_name();
    NativeSessionFailure::Refused {
        kind: NativeSessionFailureKind::UnsupportedCapability,
        stage: NativeSessionStage::Rename,
        message: format!(
            "Codex app-server method `{method_name}` is missing from its cached schema; update Codex so its app-server schema defines ThreadSetNameParams and ThreadSetNameResponse, then restart the Router Host"
        ),
    }
}

const NATIVE_FAILURE_MESSAGE: &str = "Native control operation failed";
/// The correction for an invalid native session call.
pub(crate) const INVALID_NATIVE_PARAMETERS: &str = "Invalid native control parameters";
/// The correction for an invalid session inventory request.
pub(crate) const INVALID_INVENTORY_PARAMETERS: &str =
    "Invalid session inventory parameters or cursor";

/// Why a Codex native session operation failed.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum NativeSessionFailure {
    /// The request's fields or cursor are invalid; the message names the correction.
    #[error("{0}")]
    InvalidRequest(&'static str),
    /// The operation was refused or could not complete: `{kind, stage, message}`.
    #[error("{message}")]
    Refused {
        kind: NativeSessionFailureKind,
        stage: NativeSessionStage,
        message: String,
    },
    /// Codex refused the call, with the closed reason and corrective action.
    #[error("{message}")]
    NativeRejected {
        stage: NativeSessionStage,
        message: String,
        reason: &'static str,
        next_action: &'static str,
        /// The native error code, kept when the refusal matches no known reason.
        native_code: Option<i64>,
    },
    /// The name Codex echoed is not the name Router asked for.
    #[error("Native session rename failed")]
    NameMismatch {
        requested: String,
        effective: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeSessionFailureKind {
    WrongService,
    EndpointNotFound,
    UnsupportedCapability,
    Unavailable,
    StaleGeneration,
    OutcomeUnknown,
    /// The result exceeds the response budget.
    #[serde(rename = "overloaded")]
    ResponseTooLarge,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeSessionStage {
    Inspect,
    Interrupt,
    Rename,
    Discovery,
}

impl NativeSessionStage {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::Interrupt => "interrupt",
            Self::Rename => "rename",
            Self::Discovery => "discovery",
        }
    }
}

impl NativeSessionFailure {
    pub(crate) fn refused(kind: NativeSessionFailureKind, stage: NativeSessionStage) -> Self {
        let message = if matches!(stage, NativeSessionStage::Discovery) {
            "Session inventory unavailable"
        } else {
            NATIVE_FAILURE_MESSAGE
        };
        Self::Refused {
            kind,
            stage,
            message: message.to_owned(),
        }
    }
}

impl serde::Serialize for NativeSessionFailure {
    fn serialize<TSerializer: serde::Serializer>(
        &self,
        serializer: TSerializer,
    ) -> Result<TSerializer::Ok, TSerializer::Error> {
        let payload = match self {
            Self::InvalidRequest(_) => {
                json!({"kind":"invalidRequest","message":self.to_string()})
            }
            Self::Refused {
                kind,
                stage,
                message,
            } => json!({"kind":kind,"stage":stage,"message":message}),
            Self::NativeRejected {
                stage,
                message,
                reason,
                next_action,
                native_code,
            } => {
                let mut payload = Map::new();
                payload.insert("kind".to_owned(), json!("nativeRejected"));
                payload.insert("stage".to_owned(), json!(stage));
                payload.insert("message".to_owned(), json!(message));
                payload.insert("reason".to_owned(), json!(reason));
                payload.insert("nextAction".to_owned(), json!(next_action));
                if let Some(code) = native_code {
                    payload.insert("nativeCode".to_owned(), json!(code));
                }
                Value::Object(payload)
            }
            Self::NameMismatch {
                requested,
                effective,
            } => {
                json!({"kind":"nameMismatch","requested":requested,"effective":effective,"stage":"rename"})
            }
        };
        payload.serialize(serializer)
    }
}

impl CollaborationRejection for NativeSessionFailure {
    fn rejection_reason(&self) -> Option<CollaborationRejectionReason> {
        match self {
            Self::InvalidRequest(_) => Some(CollaborationRejectionReason::InvalidShape),
            // A result too large for its response budget is not the API's concurrency limit.
            Self::Refused { kind, .. } => match kind {
                NativeSessionFailureKind::WrongService
                | NativeSessionFailureKind::EndpointNotFound
                | NativeSessionFailureKind::UnsupportedCapability
                | NativeSessionFailureKind::Unavailable
                | NativeSessionFailureKind::StaleGeneration
                | NativeSessionFailureKind::OutcomeUnknown
                | NativeSessionFailureKind::ResponseTooLarge => None,
            },
            Self::NativeRejected { .. } | Self::NameMismatch { .. } => None,
        }
    }

    fn published_rejection(&self) -> PublishedRejection {
        match self {
            Self::InvalidRequest(message) => {
                PublishedRejection::bare(PublishedRejection::INVALID_PARAMS, *message)
            }
            Self::Refused { .. } | Self::NativeRejected { .. } | Self::NameMismatch { .. } => {
                PublishedRejection::typed(
                    PublishedRejection::OPERATION_FAILED,
                    self.to_string(),
                    self,
                )
            }
        }
    }
}

/// Why a provider session listing failed.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ProviderInventoryFailure {
    /// The request was refused before any read; the message names the correction.
    #[error("{0}")]
    InvalidRequest(&'static str),
    /// The listing could not be produced: `{kind, stage: discovery, message}`.
    #[error("Provider session inventory unavailable")]
    Unavailable(ProviderInventoryFailureKind),
    /// Claude Code terminal sessions are only live; there is no stored inventory.
    #[error("Claude Code terminal sessions have no stored inventory; use --view active")]
    NoStoredTerminalInventory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderInventoryFailureKind {
    WrongService,
    EndpointNotFound,
    UnsupportedCapability,
    Unavailable,
    /// The page or its cursor exceeds the response budget.
    #[serde(rename = "overloaded")]
    ResponseTooLarge,
}

impl serde::Serialize for ProviderInventoryFailure {
    fn serialize<TSerializer: serde::Serializer>(
        &self,
        serializer: TSerializer,
    ) -> Result<TSerializer::Ok, TSerializer::Error> {
        let message = self.to_string();
        let payload = match self {
            Self::InvalidRequest(_) => json!({"kind":"invalidRequest","message":message}),
            Self::Unavailable(kind) => {
                json!({"kind":kind,"stage":"discovery","message":message})
            }
            Self::NoStoredTerminalInventory => {
                json!({"kind":"unsupportedCapability","stage":"discovery","message":message})
            }
        };
        payload.serialize(serializer)
    }
}

impl CollaborationRejection for ProviderInventoryFailure {
    fn rejection_reason(&self) -> Option<CollaborationRejectionReason> {
        match self {
            Self::InvalidRequest(_) => Some(CollaborationRejectionReason::InvalidShape),
            // A page too large for its response budget is not the API's concurrency limit.
            Self::Unavailable(kind) => match kind {
                ProviderInventoryFailureKind::WrongService
                | ProviderInventoryFailureKind::EndpointNotFound
                | ProviderInventoryFailureKind::UnsupportedCapability
                | ProviderInventoryFailureKind::Unavailable
                | ProviderInventoryFailureKind::ResponseTooLarge => None,
            },
            Self::NoStoredTerminalInventory => None,
        }
    }

    fn published_rejection(&self) -> PublishedRejection {
        match self {
            Self::InvalidRequest(message) => {
                PublishedRejection::bare(PublishedRejection::INVALID_PARAMS, *message)
            }
            Self::Unavailable(_) | Self::NoStoredTerminalInventory => PublishedRejection::typed(
                PublishedRejection::OPERATION_FAILED,
                self.to_string(),
                self,
            ),
        }
    }
}

/// The endpoint directory could not be read.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("Endpoint directory unavailable")]
pub struct EndpointDirectoryUnavailable;

impl CollaborationRejection for EndpointDirectoryUnavailable {
    fn rejection_reason(&self) -> Option<CollaborationRejectionReason> {
        None
    }

    fn published_rejection(&self) -> PublishedRejection {
        PublishedRejection::bare(PublishedRejection::INTERNAL, self.to_string())
    }
}
