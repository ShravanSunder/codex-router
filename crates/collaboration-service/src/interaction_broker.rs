//! Service-owned routing for client-exposed Codex approval callbacks.
use crate::{
    DeliveryPrecondition, DeliveryRequest, LoadPolicy, NativeControlBackend,
    SessionMessageDelivery, session_delivery_contract::UnstoredAttemptEvidenceSink,
};
use codex_acp_adapter::{
    ApprovalBroker, ApprovalBrokerError, ApprovalRoute, BrokeredApprovalOutcome,
    BrokeredApprovalRequest,
};
use collaboration_protocol::{
    ApprovalDecideParams, ApprovalDecideResult, ApprovalDecision, ApprovalDetailedListResult,
    ApprovalDetailedRecord, ApprovalListResult, ApprovalOfferedOption, ApprovalOptionEffect,
    ApprovalOptionScope, ApprovalOptionView, ApprovalOptionViewScope, ApprovalPresentation,
    ApprovalRequestRecord, ApprovalState, DeliveryOutcome, EndpointRef, MessageContent,
    MessageDelivery, OperationId, SessionRef, UuidIdentity,
};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::sync::{Mutex, oneshot};

mod interaction_history;
mod legacy_provider_projection;
mod native_approval;
mod typed_interaction_notice;
mod typed_interactions;
use interaction_history::InteractionHistoryStore;
pub use interaction_history::{
    InteractionHistoryError, InteractionHistoryRecord, InteractionHistoryState,
    LegacyApprovalMetadata, QuestionHistoryState, QuestionResponse, RefusedApprovalOption,
    RefusedTypedApproval,
};
#[cfg(test)]
use legacy_provider_projection::legacy_presentation_from_typed;
use legacy_provider_projection::{
    legacy_choice_from_typed, legacy_option_view, project_typed_legacy_approval,
    typed_cancel_approval_state, typed_option_view,
};

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ApprovalDecisionError {
    #[error("{0}")]
    Code(&'static str),
    #[error("decision is not one of the offered options")]
    OptionNotOffered { offered: Vec<String> },
    #[error("persistent choice requires acknowledgement: {persistent_target}")]
    PersistentChoiceNotAcknowledged { persistent_target: String },
}

impl ApprovalDecisionError {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Code(code) => code,
            Self::OptionNotOffered { .. } => "decisionNotOffered",
            Self::PersistentChoiceNotAcknowledged { .. } => "persistentChoiceNotAcknowledged",
        }
    }

    #[must_use]
    pub fn detail(&self) -> serde_json::Value {
        match self {
            Self::Code(_) => serde_json::Value::Null,
            Self::OptionNotOffered { offered } => serde_json::json!({"offeredOptions":offered}),
            Self::PersistentChoiceNotAcknowledged { persistent_target } => {
                serde_json::json!({"persistentTarget":persistent_target})
            }
        }
    }
}

impl From<&'static str> for ApprovalDecisionError {
    fn from(value: &'static str) -> Self {
        Self::Code(value)
    }
}

impl From<InteractionHistoryError> for ApprovalDecisionError {
    fn from(value: InteractionHistoryError) -> Self {
        match value {
            InteractionHistoryError::OptionNotOffered { offered } => {
                Self::OptionNotOffered { offered }
            }
            InteractionHistoryError::PersistentChoiceNotAcknowledged { persistent_target } => {
                Self::PersistentChoiceNotAcknowledged { persistent_target }
            }
            InteractionHistoryError::WrongActor => Self::Code("wrongActor"),
            InteractionHistoryError::AlreadySettled => Self::Code("alreadySettled"),
            InteractionHistoryError::SelfApprover => Self::Code("selfDecision"),
            InteractionHistoryError::NotPending => Self::Code("approvalNotPending"),
            InteractionHistoryError::InvalidOptionId => Self::Code("invalidOptionId"),
            InteractionHistoryError::InvalidQuestion => Self::Code("invalidQuestion"),
            InteractionHistoryError::InvalidAnswer { .. } => Self::Code("invalidAnswer"),
            InteractionHistoryError::AlreadyExists | InteractionHistoryError::Unavailable => {
                Self::Code("unavailable")
            }
        }
    }
}

#[cfg(test)]
struct TypedAdmissionPause {
    recorded: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
}

struct PendingApproval {
    record: ApprovalRequestRecord,
    offered: BTreeMap<ApprovalDecision, String>,
    completion: oneshot::Sender<BrokeredApprovalOutcome>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeApprovalRefusalReason {
    MissingRoute,
}

impl NativeApprovalRefusalReason {
    const fn code(self) -> &'static str {
        match self {
            Self::MissingRoute => "nativeApprovalRouteUnavailable",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NativeApprovalRefusalDiagnostic {
    endpoint: String,
    provider_session_id: String,
    method: &'static str,
    reason_code: &'static str,
}

fn native_approval_refusal_diagnostic(
    endpoint: &EndpointRef,
    provider_session_id: &str,
    request: &Value,
    reason: NativeApprovalRefusalReason,
) -> NativeApprovalRefusalDiagnostic {
    let method = match request.get("method").and_then(Value::as_str) {
        Some("item/commandExecution/requestApproval") => "item/commandExecution/requestApproval",
        Some("item/fileChange/requestApproval") => "item/fileChange/requestApproval",
        Some("item/permissions/requestApproval") => "item/permissions/requestApproval",
        _ => "unknownNativeApprovalMethod",
    };
    NativeApprovalRefusalDiagnostic {
        endpoint: String::from(endpoint.endpoint_id.clone()),
        provider_session_id: provider_session_id.to_owned(),
        method,
        reason_code: reason.code(),
    }
}

pub struct ServiceInteractionBroker {
    service_id: UuidIdentity,
    backend: NativeControlBackend,
    session_delivery: OnceLock<Arc<dyn SessionMessageDelivery>>,
    routes_path: PathBuf,
    routes: Mutex<BTreeMap<String, ApprovalRoute>>,
    pending: Arc<Mutex<BTreeMap<String, PendingApproval>>>,
    history_path: PathBuf,
    history: Arc<Mutex<Vec<ApprovalRequestRecord>>>,
    interaction_history: InteractionHistoryStore,
    typed_operations: Mutex<()>,
    typed_pending_approvals: Mutex<BTreeMap<String, TypedPendingApproval>>,
    pending_questions: Mutex<BTreeMap<String, PendingQuestion>>,
    #[cfg(test)]
    typed_after_record: Mutex<Option<TypedAdmissionPause>>,
    #[cfg(test)]
    question_before_send: Mutex<Option<TypedAdmissionPause>>,
}

struct TypedPendingApproval {
    completion: oneshot::Sender<TypedApprovalResolution>,
    turn_cancellation: tokio_util::sync::CancellationToken,
    retirement: tokio_util::sync::CancellationToken,
    requester: message_board::SessionRef,
    notice_task: NoticeTask,
}

#[derive(Default)]
struct NoticeTask(Option<tokio::task::JoinHandle<()>>);

impl Drop for NoticeTask {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

struct PendingQuestion {
    completion: oneshot::Sender<QuestionResponse>,
    requester: message_board::SessionRef,
    retirement: Option<tokio_util::sync::CancellationToken>,
    notice_task: NoticeTask,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedApprovalSelection {
    pub option_id: session_event_model::OfferedOptionId,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedApprovalLegacyContext {
    pub operation_id: OperationId,
    pub target: SessionRef,
    pub generation: collaboration_protocol::CodexGeneration,
    pub requested_by: collaboration_protocol::ProviderIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypedApprovalResolution {
    Selected(TypedApprovalSelection),
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypedInteractionDecision {
    SelectApproval {
        option_id: String,
        acknowledge_persistent: bool,
        note: Option<String>,
    },
    Cancel,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypedInteractionDecisionOutcome {
    ApprovalSelected {
        option_id: session_event_model::OfferedOptionId,
    },
    ApprovalCancelled,
    QuestionCancelled,
}

impl ServiceInteractionBroker {
    fn participants_belong_to_service(
        &self,
        requester: &message_board::SessionRef,
        approver: &message_board::Identity,
    ) -> bool {
        let service_id = String::from(self.service_id.clone());
        requester.endpoint.service_id.as_str() == service_id
            && match approver {
                message_board::Identity::Session { session } => {
                    session.endpoint.service_id.as_str() == service_id
                }
                message_board::Identity::Human { .. } => true,
            }
    }

    pub async fn load(
        service_id: UuidIdentity,
        backend: NativeControlBackend,
        routes_path: PathBuf,
    ) -> Result<Arc<Self>, ApprovalBrokerError> {
        let routes = match tokio::fs::read(&routes_path).await {
            Ok(bytes) => serde_json::from_slice::<Vec<ApprovalRoute>>(&bytes)
                .map_err(|_| ApprovalBrokerError::Unavailable)?
                .into_iter()
                .map(|route| (route.thread_id.clone(), route))
                .collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(_) => return Err(ApprovalBrokerError::Unavailable),
        };
        let history_path = routes_path.with_file_name("approval-history.json");
        // Corrupt history is a lost decision record, not an empty one: fail closed
        // exactly as the routes file does. Only an absent file starts empty.
        let history = match tokio::fs::read(&history_path).await {
            Ok(bytes) => serde_json::from_slice::<Vec<ApprovalRequestRecord>>(&bytes)
                .map_err(|_| ApprovalBrokerError::Unavailable)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(_) => return Err(ApprovalBrokerError::Unavailable),
        };
        let interaction_history =
            InteractionHistoryStore::load(routes_path.with_file_name("interaction-history.json"))
                .await
                .map_err(|_| ApprovalBrokerError::Unavailable)?;
        Ok(Arc::new(Self {
            service_id,
            backend,
            session_delivery: OnceLock::new(),
            routes_path,
            routes: Mutex::new(routes),
            pending: Arc::new(Mutex::new(BTreeMap::new())),
            history_path,
            history: Arc::new(Mutex::new(history)),
            interaction_history,
            typed_operations: Mutex::new(()),
            typed_pending_approvals: Mutex::new(BTreeMap::new()),
            pending_questions: Mutex::new(BTreeMap::new()),
            #[cfg(test)]
            typed_after_record: Mutex::new(None),
            #[cfg(test)]
            question_before_send: Mutex::new(None),
        }))
    }

    pub fn install_session_delivery(
        &self,
        delivery: Arc<dyn SessionMessageDelivery>,
    ) -> Result<(), ApprovalBrokerError> {
        self.session_delivery
            .set(delivery)
            .map_err(|_| ApprovalBrokerError::Unavailable)
    }

    async fn persist_routes(&self) -> Result<(), ApprovalBrokerError> {
        let routes = self
            .routes
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let bytes =
            serde_json::to_vec_pretty(&routes).map_err(|_| ApprovalBrokerError::Unavailable)?;
        let temporary = self.routes_path.with_extension("json.tmp");
        tokio::fs::write(&temporary, bytes)
            .await
            .map_err(|_| ApprovalBrokerError::Unavailable)?;
        tokio::fs::rename(&temporary, &self.routes_path)
            .await
            .map_err(|_| ApprovalBrokerError::Unavailable)
    }

    pub async fn list(&self, pending_only: bool) -> ApprovalListResult {
        let mut approvals = self.history.lock().await.clone();
        approvals.extend(
            self.interaction_history
                .list_approvals(pending_only)
                .await
                .into_iter()
                .filter_map(project_typed_legacy_approval),
        );
        if pending_only {
            approvals.retain(|record| record.state == ApprovalState::PendingClientDecision);
        }
        ApprovalListResult { approvals }
    }

    pub async fn list_detailed(
        &self,
        pending_only: bool,
    ) -> Result<ApprovalDetailedListResult, ApprovalDecisionError> {
        let mut legacy = self.history.lock().await.clone();
        if pending_only {
            legacy.retain(|record| record.state == ApprovalState::PendingClientDecision);
        }
        let typed = self.interaction_history.list_approvals(pending_only).await;
        let mut approvals = Vec::with_capacity(legacy.len() + typed.len());
        for record in legacy {
            approvals.push(ApprovalDetailedRecord {
                request_id: record.request_id,
                requester: board_session_ref(&record.requester)?,
                approver: board_identity(&record.approver)?,
                state: record.state,
                reason: record.reason,
                title: record
                    .presentation
                    .as_ref()
                    .and_then(|value| value.title.clone()),
                description: None,
                options_origin: None,
                options: record
                    .offered_options
                    .into_iter()
                    .filter_map(legacy_option_view)
                    .collect(),
            });
        }
        for record in typed {
            let InteractionHistoryRecord::Approval {
                requester,
                approver,
                request,
                state,
                ..
            } = record
            else {
                if let InteractionHistoryRecord::RefusedApproval {
                    requester,
                    approver,
                    refusal,
                } = record
                {
                    let state = if matches!(&approver, message_board::Identity::Session { session } if session == &requester)
                    {
                        ApprovalState::ApproverIsRequester
                    } else if refusal.reason == "approverUnreachable" {
                        ApprovalState::ApproverUnreachable
                    } else {
                        ApprovalState::Refused
                    };
                    approvals.push(ApprovalDetailedRecord {
                        request_id: refusal.request_id,
                        requester,
                        approver,
                        state,
                        reason: Some(refusal.reason),
                        title: Some(refusal.title),
                        description: refusal.description,
                        options_origin: None,
                        options: Vec::new(),
                    });
                }
                continue;
            };
            let (state, reason) = match state {
                InteractionHistoryState::Pending => (ApprovalState::PendingClientDecision, None),
                InteractionHistoryState::Decided { .. } => (ApprovalState::Decided, None),
                InteractionHistoryState::Cancelled { reason } => {
                    let state = typed_cancel_approval_state(&reason);
                    (state, Some(reason.as_str().to_owned()))
                }
            };
            approvals.push(ApprovalDetailedRecord {
                request_id: request.request_id,
                requester,
                approver,
                state,
                reason,
                title: Some(request.title),
                description: request.description,
                options_origin: Some(request.options_origin),
                options: request.options.iter().map(typed_option_view).collect(),
            });
        }
        Ok(ApprovalDetailedListResult { approvals })
    }

    async fn record(&self, record: ApprovalRequestRecord) -> Result<(), ApprovalBrokerError> {
        record_history(&self.history_path, &self.history, record).await
    }

    async fn transition_pending(
        &self,
        request_id: &str,
        state: ApprovalState,
    ) -> Result<(), ApprovalBrokerError> {
        let reason = match state {
            ApprovalState::TimedOut => Some("approval expired before a decision"),
            ApprovalState::Cancelled => {
                Some("approval was cancelled because its provider generation ended")
            }
            _ => None,
        };
        self.finish_pending(request_id, state, reason)
            .await
            .map(|_| ())
    }

    async fn finish_pending(
        &self,
        request_id: &str,
        state: ApprovalState,
        reason: Option<&str>,
    ) -> Result<bool, ApprovalBrokerError> {
        let Some(mut pending) = self.pending.lock().await.remove(request_id) else {
            return Ok(false);
        };
        pending.record.state = state;
        pending.record.reason = reason.map(str::to_owned);
        if let Err(error) = self.record(pending.record.clone()).await {
            pending.record.reason = Some(
                "approval ended, but its history could not be persisted; inspect Router diagnostics"
                    .to_owned(),
            );
            let mut history = self.history.lock().await;
            if let Some(existing) = history
                .iter_mut()
                .find(|item| item.request_id == pending.record.request_id)
            {
                *existing = pending.record;
            }
            tracing::error!(%error, request_id, "failed to persist terminal approval state");
            return Err(error);
        }
        Ok(true)
    }

    async fn expire_or_cancel_stale(
        &self,
        request_id: &str,
        state: ApprovalState,
    ) -> Result<(), &'static str> {
        self.transition_pending(request_id, state)
            .await
            .map_err(|_| "unavailable")
    }
}

fn native_option_records(options: &[Value]) -> Vec<ApprovalOfferedOption> {
    options
        .iter()
        .filter_map(|option| {
            let option_id = option.get("optionId")?.as_str()?.to_owned();
            let scope = match option_id.as_str() {
                "native-accept" => ApprovalOptionScope::AllowOnce,
                "native-accept-session" => ApprovalOptionScope::AllowForSession,
                "native-decline" => ApprovalOptionScope::RejectOnce,
                _ => ApprovalOptionScope::Unsupported {
                    provider_kind: "native".to_owned(),
                },
            };
            Some(ApprovalOfferedOption {
                option_id,
                label: option
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|label| label.chars().take(120).collect()),
                scope,
            })
        })
        .collect()
}

fn map_native_options(
    options: &[Value],
) -> Result<BTreeMap<ApprovalDecision, String>, &'static str> {
    let mut identifiers = std::collections::BTreeSet::new();
    let mut offered = BTreeMap::new();
    for option in options {
        let option_id = option
            .get("optionId")
            .and_then(Value::as_str)
            .ok_or("native permission option has no valid optionId")?;
        if !identifiers.insert(option_id) {
            return Err("native permission options contain a duplicate optionId");
        }
        let decision = match option_id {
            "native-accept" => Some(ApprovalDecision::Allow),
            "native-accept-session" => Some(ApprovalDecision::AllowForSession),
            "native-decline" => Some(ApprovalDecision::Deny),
            _ => None,
        };
        if let Some(decision) = decision
            && offered.insert(decision, option_id.to_owned()).is_some()
        {
            return Err("native permission options contain duplicate decisions");
        }
    }
    if offered.is_empty() {
        return Err("native permission request has no supported decision option");
    }
    Ok(offered)
}

async fn record_history(
    history_path: &std::path::Path,
    history: &Mutex<Vec<ApprovalRequestRecord>>,
    record: ApprovalRequestRecord,
) -> Result<(), ApprovalBrokerError> {
    let mut history = history.lock().await;
    let mut updated_history = history.clone();
    if let Some(existing) = updated_history
        .iter_mut()
        .find(|item| item.request_id == record.request_id)
    {
        *existing = record;
    } else {
        updated_history.push(record);
    }
    let bytes = serde_json::to_vec_pretty(&updated_history)
        .map_err(|_| ApprovalBrokerError::Unavailable)?;
    let temporary = history_path.with_extension("json.tmp");
    tokio::fs::write(&temporary, bytes)
        .await
        .map_err(|_| ApprovalBrokerError::Unavailable)?;
    tokio::fs::rename(&temporary, history_path)
        .await
        .map_err(|_| ApprovalBrokerError::Unavailable)?;
    *history = updated_history;
    Ok(())
}

struct CancellationMarker {
    request_id: String,
    pending: Arc<Mutex<BTreeMap<String, PendingApproval>>>,
    history: Arc<Mutex<Vec<ApprovalRequestRecord>>>,
    history_path: PathBuf,
    armed: bool,
}

impl Drop for CancellationMarker {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let request_id = self.request_id.clone();
        let pending = Arc::clone(&self.pending);
        let history = Arc::clone(&self.history);
        let history_path = self.history_path.clone();
        tokio::spawn(async move {
            if let Some(mut request) = pending.lock().await.remove(&request_id) {
                request.record.state = ApprovalState::Cancelled;
                request.record.reason =
                    Some("approval handler ended before a decision was recorded".to_owned());
                if let Err(error) =
                    record_history(&history_path, &history, request.record.clone()).await
                {
                    request.record.reason = Some(
                        "approval ended, but its history could not be persisted; inspect Router diagnostics"
                            .to_owned(),
                    );
                    let mut history = history.lock().await;
                    if let Some(existing) = history
                        .iter_mut()
                        .find(|item| item.request_id == request_id)
                    {
                        *existing = request.record;
                    }
                    tracing::error!(%error, request_id, "failed to persist dropped approval state");
                }
            }
        });
    }
}

fn board_session_ref(
    session: &SessionRef,
) -> Result<message_board::SessionRef, ApprovalDecisionError> {
    let encoded =
        serde_json::to_value(session).map_err(|_| ApprovalDecisionError::Code("unavailable"))?;
    serde_json::from_value(encoded).map_err(|_| ApprovalDecisionError::Code("unavailable"))
}

fn board_identity(session: &SessionRef) -> Result<message_board::Identity, ApprovalDecisionError> {
    Ok(message_board::Identity::Session {
        session: board_session_ref(session)?,
    })
}

impl ServiceInteractionBroker {
    pub async fn decide(
        &self,
        params: ApprovalDecideParams,
    ) -> Result<ApprovalDecideResult, ApprovalDecisionError> {
        if params.option_id.is_some() == params.decision.is_some() {
            return Err(ApprovalDecisionError::Code("invalidSelection"));
        }
        let typed_pending = self
            .typed_pending_approvals
            .lock()
            .await
            .contains_key(&params.request_id);
        if typed_pending {
            let record = self
                .interaction_history
                .interaction(&params.request_id)
                .await
                .ok_or(ApprovalDecisionError::Code("approvalNotPending"))?;
            let option_id = match (&params.option_id, params.decision) {
                (Some(option_id), None) => option_id.clone(),
                (None, Some(decision)) => legacy_choice_from_typed(
                    record
                        .approval_request()
                        .ok_or(ApprovalDecisionError::Code("approvalNotPending"))?,
                    decision,
                )?,
                _ => return Err(ApprovalDecisionError::Code("invalidSelection")),
            };
            let selected = self
                .decide_typed_interaction(
                    &params.request_id,
                    &params.actor,
                    TypedInteractionDecision::SelectApproval {
                        option_id,
                        acknowledge_persistent: params.acknowledge_persistent,
                        note: params.note.clone(),
                    },
                )
                .await
                .map_err(ApprovalDecisionError::from)?;
            let TypedInteractionDecisionOutcome::ApprovalSelected {
                option_id: selected,
            } = selected
            else {
                return Err(ApprovalDecisionError::Code("approvalNotPending"));
            };
            return Ok(ApprovalDecideResult {
                request_id: params.request_id,
                state: ApprovalState::Decided,
                decision: params.decision,
                option_id: params.option_id.map(|_| selected.as_str().to_owned()),
                scope: None,
            });
        }
        if let Some(InteractionHistoryRecord::Approval { state, .. }) = self
            .interaction_history
            .interaction(&params.request_id)
            .await
            && state != InteractionHistoryState::Pending
        {
            return Err(ApprovalDecisionError::Code("alreadySettled"));
        }
        let mut pending = self.pending.lock().await;
        let request = pending
            .get(&params.request_id)
            .ok_or(ApprovalDecisionError::Code("approvalNotPending"))?;
        if chrono::DateTime::parse_from_rfc3339(&request.record.expires_at)
            .map(|expiry| expiry <= chrono::Utc::now())
            .unwrap_or(true)
        {
            drop(pending);
            self.expire_or_cancel_stale(&params.request_id, ApprovalState::TimedOut)
                .await?;
            return Err(ApprovalDecisionError::Code("expired"));
        }
        let current_generation = self
            .backend
            .gate
            .acquire()
            .map_err(|_| "unavailable")?
            .generation()
            .clone();
        if current_generation != request.record.generation {
            drop(pending);
            self.expire_or_cancel_stale(&params.request_id, ApprovalState::Cancelled)
                .await?;
            return Err(ApprovalDecisionError::Code("oldGeneration"));
        }
        let requester = board_identity(&request.record.requester)?;
        let approver = board_identity(&request.record.approver)?;
        if params.actor == requester {
            return Err(ApprovalDecisionError::Code("selfDecision"));
        }
        if params.actor != approver {
            return Err(ApprovalDecisionError::Code("wrongActor"));
        }
        let option_id = match (&params.option_id, params.decision) {
            (Some(option_id), None) => option_id.clone(),
            (None, Some(decision)) => request.offered.get(&decision).cloned().ok_or_else(|| {
                ApprovalDecisionError::OptionNotOffered {
                    offered: request
                        .record
                        .offered_options
                        .iter()
                        .map(|option| option.option_id.clone())
                        .collect(),
                }
            })?,
            _ => return Err(ApprovalDecisionError::Code("invalidSelection")),
        };
        let decision = request
            .offered
            .iter()
            .find_map(|(decision, offered_id)| (offered_id == &option_id).then_some(*decision))
            .ok_or_else(|| ApprovalDecisionError::OptionNotOffered {
                offered: request
                    .record
                    .offered_options
                    .iter()
                    .map(|option| option.option_id.clone())
                    .collect(),
            })?;
        let mut record = request.record.clone();
        record.state = ApprovalState::Decided;
        record.reason = None;
        record.decision = Some(decision);
        self.record(record).await.map_err(|_| "unavailable")?;
        let request = pending
            .remove(&params.request_id)
            .ok_or("approvalNotPending")?;
        drop(pending);
        request
            .completion
            .send(BrokeredApprovalOutcome::Selected {
                option_id: option_id.clone(),
            })
            .map_err(|_| "approvalNotPending")?;
        Ok(ApprovalDecideResult {
            request_id: params.request_id,
            state: ApprovalState::Decided,
            decision: Some(decision),
            option_id: params.option_id.map(|_| option_id),
            scope: (decision == ApprovalDecision::AllowForSession)
                .then(|| "nativeSession".to_owned()),
        })
    }

    async fn deliver(&self, record: &ApprovalRequestRecord) -> Result<(), ApprovalBrokerError> {
        let text = serde_json::to_string(record).map_err(|_| ApprovalBrokerError::Unavailable)?;
        self.deliver_message(record.requester.clone(), record.approver.clone(), text)
            .await
    }

    async fn deliver_message(
        &self,
        requester: SessionRef,
        approver: SessionRef,
        text: String,
    ) -> Result<(), ApprovalBrokerError> {
        let delivery = self
            .session_delivery
            .get()
            .ok_or(ApprovalBrokerError::Unavailable)?;
        deliver_message_via(delivery.as_ref(), requester, approver, text).await
    }
}

async fn deliver_message_via(
    delivery: &dyn SessionMessageDelivery,
    requester: SessionRef,
    approver: SessionRef,
    text: String,
) -> Result<(), ApprovalBrokerError> {
    let request = DeliveryRequest {
        target: approver,
        message: MessageContent::Agent {
            sender: requester,
            text: text
                .try_into()
                .map_err(|_| ApprovalBrokerError::Unavailable)?,
        },
        mode: MessageDelivery::Auto,
        load_policy: LoadPolicy::MayLoad,
        precondition: DeliveryPrecondition::Unpinned,
        correlation: collaboration_protocol::DeliveryCorrelationId::generate(),
        attempt: agent_automation::AttemptId::generate(),
    };
    let receipt = delivery
        .deliver(request, &UnstoredAttemptEvidenceSink)
        .await
        .map_err(|_| ApprovalBrokerError::Unavailable)?;
    match receipt.outcome {
        DeliveryOutcome::Started
        | DeliveryOutcome::Steered
        | DeliveryOutcome::StartedOrSteered
        | DeliveryOutcome::Queued
        | DeliveryOutcome::PeerMessageWritten
        | DeliveryOutcome::Unknown => Ok(()),
        DeliveryOutcome::NotSubmitted { .. } | DeliveryOutcome::Rejected(_) => {
            Err(ApprovalBrokerError::RouteUnavailable)
        }
    }
}

#[cfg(test)]
#[path = "interaction_broker_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "interaction_broker/turn_cancellation_tests.rs"]
mod turn_cancellation_tests;
