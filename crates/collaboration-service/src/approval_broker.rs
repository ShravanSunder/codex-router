//! Service-owned routing for client-exposed Codex approval callbacks.
use crate::{EndpointDirectory, NativeControlBackend};
use codex_acp_adapter::{
    ApprovalBroker, ApprovalBrokerError, ApprovalRoute, BrokeredApprovalOutcome,
    BrokeredApprovalRequest,
};
use collaboration_protocol::{
    ApprovalDecideParams, ApprovalDecideResult, ApprovalDecision, ApprovalListResult,
    ApprovalRequestRecord, ApprovalState, MessageContent, MessageDelivery, NativeSendParams,
    SessionRef, UuidIdentity,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{Mutex, oneshot};

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);

struct PendingApproval {
    record: ApprovalRequestRecord,
    offered: BTreeMap<ApprovalDecision, String>,
    completion: oneshot::Sender<BrokeredApprovalOutcome>,
}

pub struct ServiceApprovalBroker {
    service_id: UuidIdentity,
    endpoints: EndpointDirectory,
    backend: NativeControlBackend,
    routes_path: PathBuf,
    routes: Mutex<BTreeMap<String, ApprovalRoute>>,
    pending: Arc<Mutex<BTreeMap<String, PendingApproval>>>,
    history_path: PathBuf,
    history: Arc<Mutex<Vec<ApprovalRequestRecord>>>,
}

impl ServiceApprovalBroker {
    pub async fn load(
        service_id: UuidIdentity,
        endpoints: EndpointDirectory,
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
        Ok(Arc::new(Self {
            service_id,
            endpoints,
            backend,
            routes_path,
            routes: Mutex::new(routes),
            pending: Arc::new(Mutex::new(BTreeMap::new())),
            history_path,
            history: Arc::new(Mutex::new(history)),
        }))
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

    async fn record(&self, record: ApprovalRequestRecord) -> Result<(), ApprovalBrokerError> {
        record_history(&self.history_path, &self.history, record).await
    }

    async fn transition_pending(
        &self,
        request_id: &str,
        state: ApprovalState,
    ) -> Result<(), ApprovalBrokerError> {
        let removed = self.pending.lock().await.remove(request_id);
        if let Some(mut pending) = removed {
            pending.record.state = state;
            self.record(pending.record).await?;
        }
        Ok(())
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

async fn record_history(
    history_path: &std::path::Path,
    history: &Mutex<Vec<ApprovalRequestRecord>>,
    record: ApprovalRequestRecord,
) -> Result<(), ApprovalBrokerError> {
    let mut history = history.lock().await;
    if let Some(existing) = history
        .iter_mut()
        .find(|item| item.request_id == record.request_id)
    {
        *existing = record;
    } else {
        history.push(record);
    }
    let bytes =
        serde_json::to_vec_pretty(&*history).map_err(|_| ApprovalBrokerError::Unavailable)?;
    let temporary = history_path.with_extension("json.tmp");
    tokio::fs::write(&temporary, bytes)
        .await
        .map_err(|_| ApprovalBrokerError::Unavailable)?;
    tokio::fs::rename(&temporary, history_path)
        .await
        .map_err(|_| ApprovalBrokerError::Unavailable)
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
                let _ = record_history(&history_path, &history, request.record).await;
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
            return Err("oldGeneration");
        }
        if params.actor == request.record.requester {
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
        let request = pending
            .remove(&params.request_id)
            .ok_or("approvalNotPending")?;
        let mut record = request.record.clone();
        record.state = ApprovalState::Decided;
        record.decision = Some(params.decision);
        drop(pending);
        self.record(record).await.map_err(|_| "unavailable")?;
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
        if String::from(record.approver.endpoint.endpoint_id.clone()) != "codex-local" {
            return Err(ApprovalBrokerError::RouteUnavailable);
        }
        let text = serde_json::to_string(record).map_err(|_| ApprovalBrokerError::Unavailable)?;
        let params = NativeSendParams {
            target: record.approver.clone(),
            generation: self
                .backend
                .gate
                .acquire()
                .map_err(|_| ApprovalBrokerError::Unavailable)?
                .generation()
                .clone(),
            message: MessageContent::Agent {
                sender: record.requester.clone(),
                text: text
                    .try_into()
                    .map_err(|_| ApprovalBrokerError::Unavailable)?,
            },
            delivery: MessageDelivery::Auto,
            client_user_message_id: None,
        };
        let endpoints = self
            .endpoints
            .subscribe()
            .and_then(|subscription| subscription.snapshot())
            .map_err(|_| ApprovalBrokerError::Unavailable)?
            .endpoints;
        let response = crate::native_message_dispatch::dispatch_message(
            crate::native_control_dispatch::NativeControlRequest {
                method: "codex/messageSend",
                params: serde_json::to_value(params)
                    .map_err(|_| ApprovalBrokerError::Unavailable)?,
                id: json!(record.request_id),
                service_id: &self.service_id,
                backend: Some(&self.backend),
                endpoints: &endpoints,
                stored_observation: None,
                access_routes: None,
            },
        )
        .await;
        response
            .get("result")
            .ok_or(ApprovalBrokerError::RouteUnavailable)
            .map(|_| ())
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
            let route = self
                .routes
                .lock()
                .await
                .get(&request.thread_id)
                .cloned()
                .ok_or(ApprovalBrokerError::RouteUnavailable)?;
            let requester = SessionRef {
                endpoint: self.backend.endpoint.clone(),
                session_id: request
                    .thread_id
                    .clone()
                    .try_into()
                    .map_err(|_| ApprovalBrokerError::Unavailable)?,
            };
            if requester == route.approver {
                return Ok(BrokeredApprovalOutcome::Cancelled);
            }
            let options = request
                .request
                .pointer("/params/options")
                .and_then(Value::as_array)
                .ok_or(ApprovalBrokerError::Unavailable)?;
            let offered = options
                .iter()
                .filter_map(|option| {
                    let id = option.get("optionId")?.as_str()?.to_owned();
                    let decision = match id.as_str() {
                        "native-accept" => ApprovalDecision::Allow,
                        "native-accept-session" => ApprovalDecision::AllowForSession,
                        "native-decline" => ApprovalDecision::Deny,
                        _ => return None,
                    };
                    Some((decision, id))
                })
                .collect::<BTreeMap<_, _>>();
            if offered.is_empty() {
                return Err(ApprovalBrokerError::Unavailable);
            }
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
                operation: request.request,
                expires_at,
                decision: None,
            };
            let (completion, receiver) = oneshot::channel();
            self.pending.lock().await.insert(
                request_id.clone(),
                PendingApproval {
                    record: record.clone(),
                    offered,
                    completion,
                },
            );
            self.record(record.clone()).await?;
            let mut cancellation = CancellationMarker {
                request_id: request_id.clone(),
                pending: Arc::clone(&self.pending),
                history: Arc::clone(&self.history),
                history_path: self.history_path.clone(),
                armed: true,
            };
            if self.deliver(&record).await.is_err() {
                self.pending.lock().await.remove(&request_id);
                let mut terminal = record;
                terminal.state = ApprovalState::ApproverUnreachable;
                self.record(terminal).await?;
                cancellation.armed = false;
                return Ok(BrokeredApprovalOutcome::Cancelled);
            }
            match tokio::time::timeout(APPROVAL_TIMEOUT, receiver).await {
                Ok(Ok(outcome)) => {
                    cancellation.armed = false;
                    Ok(outcome)
                }
                Ok(Err(_)) => {
                    cancellation.armed = false;
                    Ok(BrokeredApprovalOutcome::Cancelled)
                }
                Err(_) => {
                    self.pending.lock().await.remove(&request_id);
                    let mut terminal = record;
                    terminal.state = ApprovalState::TimedOut;
                    self.record(terminal).await?;
                    cancellation.armed = false;
                    Ok(BrokeredApprovalOutcome::Cancelled)
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
#[path = "approval_broker_tests.rs"]
mod tests;
