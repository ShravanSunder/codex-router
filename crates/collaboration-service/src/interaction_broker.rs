//! Service-owned routing for client-exposed Codex approval callbacks.
use crate::{
    DeliveryPrecondition, DeliveryRequest, NativeControlBackend, SessionMessageDelivery,
    session_delivery_contract::UnstoredAttemptEvidenceSink,
};
use codex_acp_adapter::{
    ApprovalBroker, ApprovalBrokerError, ApprovalRoute, BrokeredApprovalOutcome,
    BrokeredApprovalRequest,
};
use collaboration_protocol::{
    ApprovalDecideParams, ApprovalDecideResult, ApprovalDecision, ApprovalListResult,
    ApprovalOfferedOption, ApprovalOptionScope, ApprovalPresentation, ApprovalRequestRecord,
    ApprovalState, DeliveryOutcome, EndpointRef, MessageContent, MessageDelivery, SessionRef,
    UuidIdentity,
};
use serde::Serialize;
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

mod external_requests;
mod interaction_history;
#[cfg(test)]
use external_requests::map_external_options;
use interaction_history::InteractionHistoryStore;
pub use interaction_history::{
    InteractionHistoryError, InteractionHistoryRecord, InteractionHistoryState,
};

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExternalApprovalOptionScope {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    Unsupported { provider_kind: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalApprovalOption {
    pub option_id: String,
    pub label: Option<String>,
    pub scope: ExternalApprovalOptionScope,
}

#[derive(Clone, Debug)]
pub struct ExternalApprovalRequest {
    pub requester: SessionRef,
    pub approver: SessionRef,
    pub generation: collaboration_protocol::CodexGeneration,
    pub retirement: tokio_util::sync::CancellationToken,
    pub cancellation: tokio_util::sync::CancellationToken,
    pub operation_metadata: ExternalApprovalOperationMetadata,
    pub presentation: Option<ApprovalPresentation>,
    pub options: Vec<ExternalApprovalOption>,
}

#[derive(Clone, Debug)]
pub struct ExternalApprovalRefusal {
    pub requester: SessionRef,
    pub approver: SessionRef,
    pub generation: collaboration_protocol::CodexGeneration,
    pub operation_metadata: ExternalApprovalOperationMetadata,
    pub offered_options: Vec<ExternalApprovalOption>,
    pub presentation: Option<ApprovalPresentation>,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalApprovalOperationMetadata {
    pub operation_id: collaboration_protocol::OperationId,
    pub target: SessionRef,
    pub binding_generation: collaboration_protocol::CodexGeneration,
    pub method: &'static str,
}

impl Serialize for ExternalApprovalOperationMetadata {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct SerializedMetadata<'a> {
            kind: &'static str,
            operation_id: &'a collaboration_protocol::OperationId,
            target: &'a SessionRef,
            binding_generation: &'a collaboration_protocol::CodexGeneration,
            method: &'static str,
        }

        SerializedMetadata {
            kind: "externalProviderPermission",
            operation_id: &self.operation_id,
            target: &self.target,
            binding_generation: &self.binding_generation,
            method: self.method,
        }
        .serialize(serializer)
    }
}

struct PendingApproval {
    record: ApprovalRequestRecord,
    offered: BTreeMap<ApprovalDecision, String>,
    completion: oneshot::Sender<BrokeredApprovalOutcome>,
    generation_authority: ApprovalGenerationAuthority,
}

#[derive(Clone)]
enum ApprovalGenerationAuthority {
    Native,
    External {
        generation: collaboration_protocol::CodexGeneration,
        retirement: tokio_util::sync::CancellationToken,
    },
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

pub struct ServiceApprovalBroker {
    service_id: UuidIdentity,
    backend: NativeControlBackend,
    session_delivery: OnceLock<Arc<dyn SessionMessageDelivery>>,
    routes_path: PathBuf,
    routes: Mutex<BTreeMap<String, ApprovalRoute>>,
    pending: Arc<Mutex<BTreeMap<String, PendingApproval>>>,
    history_path: PathBuf,
    history: Arc<Mutex<Vec<ApprovalRequestRecord>>>,
    interaction_history: InteractionHistoryStore,
}

impl ServiceApprovalBroker {
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
        if pending_only {
            approvals.retain(|record| record.state == ApprovalState::PendingClientDecision);
        }
        ApprovalListResult { approvals }
    }

    /// New typed interaction records use their own history file. The legacy
    /// approval record and reader remain byte-shape compatible with 0.1.38.
    pub async fn record_typed_interaction(
        &self,
        record: InteractionHistoryRecord,
    ) -> Result<(), InteractionHistoryError> {
        self.interaction_history.record(record).await
    }

    pub async fn decide_typed_interaction(
        &self,
        request_id: &str,
        actor: &message_board::Identity,
        option_id: &str,
    ) -> Result<(), InteractionHistoryError> {
        self.interaction_history
            .decide(request_id, actor, option_id)
            .await
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

impl ServiceApprovalBroker {
    pub async fn decide(
        &self,
        params: ApprovalDecideParams,
    ) -> Result<ApprovalDecideResult, &'static str> {
        let mut pending = self.pending.lock().await;
        let request = pending
            .get(&params.request_id)
            .ok_or("approvalNotPending")?;
        if chrono::DateTime::parse_from_rfc3339(&request.record.expires_at)
            .map(|expiry| expiry <= chrono::Utc::now())
            .unwrap_or(true)
        {
            drop(pending);
            self.expire_or_cancel_stale(&params.request_id, ApprovalState::TimedOut)
                .await?;
            return Err("expired");
        }
        let current_generation = match &request.generation_authority {
            ApprovalGenerationAuthority::Native => self
                .backend
                .gate
                .acquire()
                .map_err(|_| "unavailable")?
                .generation()
                .clone(),
            ApprovalGenerationAuthority::External {
                generation,
                retirement,
            } => {
                if retirement.is_cancelled() {
                    drop(pending);
                    self.expire_or_cancel_stale(&params.request_id, ApprovalState::Cancelled)
                        .await?;
                    return Err("oldGeneration");
                }
                generation.clone()
            }
        };
        if current_generation != request.record.generation {
            drop(pending);
            self.expire_or_cancel_stale(&params.request_id, ApprovalState::Cancelled)
                .await?;
            return Err("oldGeneration");
        }
        if matches!(
            &request.generation_authority,
            ApprovalGenerationAuthority::Native
        ) && params.actor == request.record.requester
        {
            return Err("selfDecision");
        }
        if params.actor != request.record.approver {
            return Err("wrongActor");
        }
        let option_id = request
            .offered
            .get(&params.decision)
            .cloned()
            .ok_or("decisionNotOffered")?;
        let mut record = request.record.clone();
        record.state = ApprovalState::Decided;
        record.reason = None;
        record.decision = Some(params.decision);
        self.record(record).await.map_err(|_| "unavailable")?;
        let request = pending
            .remove(&params.request_id)
            .ok_or("approvalNotPending")?;
        drop(pending);
        request
            .completion
            .send(BrokeredApprovalOutcome::Selected { option_id })
            .map_err(|_| "approvalNotPending")?;
        Ok(ApprovalDecideResult {
            request_id: params.request_id,
            state: ApprovalState::Decided,
            decision: params.decision,
            scope: (params.decision == ApprovalDecision::AllowForSession)
                .then(|| "nativeSession".to_owned()),
        })
    }

    async fn deliver(&self, record: &ApprovalRequestRecord) -> Result<(), ApprovalBrokerError> {
        let text = serde_json::to_string(record).map_err(|_| ApprovalBrokerError::Unavailable)?;
        let request = DeliveryRequest {
            target: record.approver.clone(),
            message: MessageContent::Agent {
                sender: record.requester.clone(),
                text: text
                    .try_into()
                    .map_err(|_| ApprovalBrokerError::Unavailable)?,
            },
            mode: MessageDelivery::Auto,
            precondition: DeliveryPrecondition::Unpinned,
            correlation: collaboration_protocol::DeliveryCorrelationId::generate(),
            attempt: agent_automation::AttemptId::generate(),
        };
        let delivery = self
            .session_delivery
            .get()
            .ok_or(ApprovalBrokerError::Unavailable)?;
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
}

impl ApprovalBroker for ServiceApprovalBroker {
    fn register_route(
        &self,
        route: ApprovalRoute,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), ApprovalBrokerError>> + Send + '_>,
    > {
        Box::pin(async move {
            if route.created_by.endpoint.service_id != self.service_id
                || route.approver.endpoint.service_id != self.service_id
            {
                return Err(ApprovalBrokerError::RouteUnavailable);
            }
            self.routes
                .lock()
                .await
                .insert(route.thread_id.clone(), route);
            self.persist_routes().await
        })
    }

    fn request(
        &self,
        request: BrokeredApprovalRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<BrokeredApprovalOutcome, ApprovalBrokerError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            let requester = SessionRef {
                endpoint: self.backend.endpoint.clone(),
                session_id: request
                    .thread_id
                    .clone()
                    .try_into()
                    .map_err(|_| ApprovalBrokerError::Unavailable)?,
            };
            let route = self.routes.lock().await.get(&request.thread_id).cloned();
            let Some(route) = route else {
                let diagnostic = native_approval_refusal_diagnostic(
                    &self.backend.endpoint,
                    &request.thread_id,
                    &request.request,
                    NativeApprovalRefusalReason::MissingRoute,
                );
                tracing::warn!(
                    endpoint = %diagnostic.endpoint,
                    provider_session_id = %diagnostic.provider_session_id,
                    method = diagnostic.method,
                    reason_code = diagnostic.reason_code,
                    "native approval request refused before route lookup",
                );
                return Err(ApprovalBrokerError::RouteUnavailable);
            };
            let native_options = request
                .request
                .pointer("/params/options")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let offered_options = native_option_records(native_options);
            if requester == route.approver {
                let request_id = format!(
                    "approval-{}",
                    String::from(
                        crate::new_service_uuid().map_err(|_| ApprovalBrokerError::Unavailable)?
                    )
                );
                self.record(ApprovalRequestRecord {
                    request_id,
                    requester,
                    approver: route.approver,
                    generation: request.generation,
                    state: ApprovalState::ApproverIsRequester,
                    reason: Some("set a different approver".to_owned()),
                    offered_options,
                    presentation: None,
                    decision: None,
                    operation: request.request,
                    expires_at: chrono::Utc::now().to_rfc3339(),
                })
                .await?;
                return Ok(BrokeredApprovalOutcome::Cancelled);
            }
            let offered = match map_native_options(native_options) {
                Ok(offered) => offered,
                Err(reason) => {
                    let request_id = format!(
                        "approval-{}",
                        String::from(
                            crate::new_service_uuid()
                                .map_err(|_| { ApprovalBrokerError::Unavailable })?
                        )
                    );
                    self.record(ApprovalRequestRecord {
                        request_id,
                        requester,
                        approver: route.approver,
                        generation: request.generation,
                        state: ApprovalState::Cancelled,
                        reason: Some(reason.to_owned()),
                        offered_options,
                        presentation: None,
                        decision: None,
                        operation: request.request,
                        expires_at: chrono::Utc::now().to_rfc3339(),
                    })
                    .await?;
                    return Ok(BrokeredApprovalOutcome::Cancelled);
                }
            };
            let request_id = format!(
                "approval-{}",
                String::from(
                    crate::new_service_uuid().map_err(|_| ApprovalBrokerError::Unavailable)?
                )
            );
            let expires_at = (chrono::Utc::now()
                + chrono::Duration::seconds(APPROVAL_TIMEOUT.as_secs() as i64))
            .to_rfc3339();
            let record = ApprovalRequestRecord {
                request_id: request_id.clone(),
                requester,
                approver: route.approver,
                generation: request.generation,
                state: ApprovalState::PendingClientDecision,
                reason: None,
                offered_options,
                presentation: None,
                operation: request.request,
                expires_at,
                decision: None,
            };
            self.record(record.clone()).await?;
            let (completion, receiver) = oneshot::channel();
            self.pending.lock().await.insert(
                request_id.clone(),
                PendingApproval {
                    record: record.clone(),
                    offered,
                    completion,
                    generation_authority: ApprovalGenerationAuthority::Native,
                },
            );
            let mut cancellation = CancellationMarker {
                request_id: request_id.clone(),
                pending: Arc::clone(&self.pending),
                history: Arc::clone(&self.history),
                history_path: self.history_path.clone(),
                armed: true,
            };
            let deadline = tokio::time::Instant::now() + APPROVAL_TIMEOUT;
            let delivery = self.deliver(&record);
            tokio::pin!(delivery);
            let delivery_result = tokio::select! {
                biased;
                () = tokio::time::sleep_until(deadline) => {
                    self.finish_pending(
                        &request_id,
                        ApprovalState::TimedOut,
                        Some("approval timed out before the notice reached its approver"),
                    ).await?;
                    cancellation.armed = false;
                    return Ok(BrokeredApprovalOutcome::Cancelled);
                }
                result = &mut delivery => result,
            };
            if delivery_result.is_err() {
                self.finish_pending(
                    &request_id,
                    ApprovalState::ApproverUnreachable,
                    Some("approval notice could not be delivered to the configured approver"),
                )
                .await?;
                cancellation.armed = false;
                return Ok(BrokeredApprovalOutcome::Cancelled);
            }
            tokio::select! {
                biased;
                () = tokio::time::sleep_until(deadline) => {
                    self.finish_pending(
                        &request_id,
                        ApprovalState::TimedOut,
                        Some("approval timed out before an approver decided"),
                    ).await?;
                    cancellation.armed = false;
                    Ok(BrokeredApprovalOutcome::Cancelled)
                }
                result = receiver => match result {
                Ok(outcome) => {
                    cancellation.armed = false;
                    Ok(outcome)
                }
                Err(_) => {
                    cancellation.armed = false;
                    Ok(BrokeredApprovalOutcome::Cancelled)
                }
            }
            }
        })
    }

    fn route(
        &self,
        thread_id: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Option<ApprovalRoute>, ApprovalBrokerError>>
                + Send
                + '_,
        >,
    > {
        let thread_id = thread_id.to_owned();
        Box::pin(async move { Ok(self.routes.lock().await.get(&thread_id).cloned()) })
    }
}

#[cfg(test)]
#[path = "interaction_broker_tests.rs"]
mod tests;
