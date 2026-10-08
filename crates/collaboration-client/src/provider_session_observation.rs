//! Select provider observation or native attachment from endpoint capabilities.
//!
//! A provider Session is observed through bounded calls that resume from the last event's
//! sequence within the hub's epoch, so following one is a loop of those calls. A Codex
//! Session is observed on its native carrier.

use std::{collections::VecDeque, time::Duration};

use collaboration_protocol::{
    BoundedObservationRequest, BoundedObservationResult, ChannelDescription, CodexGeneration,
    EndpointId, EndpointRef, ObservationEndReason, SessionId, SessionRef,
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::collaboration_access::EndpointDirectoryReader;
use crate::{
    ClientError, CollaborationAccess, CollaborationClient, NativeObservation, OperationError,
};

/// How long one provider observation call waits for new events while following.
const FOLLOW_CALL_SECONDS: u64 = 30;
const FOLLOW_MAX_EVENTS: usize = 4096;
const FOLLOW_MAX_BYTES: usize = 1_048_576;

pub enum SessionObservation {
    Native(NativeObservation),
    Provider(ProviderSessionFollow),
}

/// Follows one provider Session through bounded observation calls.
pub struct ProviderSessionFollow {
    access: ProviderObservationAccess,
    target: SessionRef,
    generation: CodexGeneration,
    epoch: Option<u64>,
    after_sequence: Option<u64>,
    pending: VecDeque<Value>,
    ended: bool,
}

enum ProviderObservationAccess {
    Api(CollaborationClient),
    Local(std::sync::Arc<dyn crate::LocalCollaboration>),
}

impl ProviderObservationAccess {
    async fn observe(
        &self,
        request: BoundedObservationRequest,
    ) -> Result<BoundedObservationResult, ClientError> {
        match self {
            Self::Api(client) => client.observe_provider_session(request).await,
            Self::Local(router) => router.observe_provider_session(request).await,
        }
    }
}

impl SessionObservation {
    pub async fn attach_by_ids_with_context(
        access: &CollaborationAccess,
        endpoint_id: EndpointId,
        session_id: SessionId,
    ) -> Result<Self, OperationError> {
        let endpoints = access
            .endpoint_directory("agent-collaboration-observer")
            .await
            .map_err(|error| OperationError::before_dispatch("observation-connect", None, error))?;
        let target = SessionRef {
            endpoint: EndpointRef {
                service_id: endpoints.service_id(),
                endpoint_id,
            },
            session_id,
        };
        if endpoint_is_provider(&endpoints, &target).await? {
            let provider = provider_access(access, endpoints);
            let first = provider
                .observe(BoundedObservationRequest {
                    target: target.clone(),
                    timeout_seconds: 1,
                    max_events: FOLLOW_MAX_EVENTS,
                    max_bytes: FOLLOW_MAX_BYTES,
                    after_sequence: None,
                    epoch: None,
                })
                .await
                .map_err(|error| {
                    OperationError::before_dispatch(
                        "observation-attach",
                        Some(target.clone()),
                        error,
                    )
                })?;
            let mut follow = ProviderSessionFollow {
                access: provider,
                generation: first.generation.clone(),
                target,
                epoch: None,
                after_sequence: None,
                pending: VecDeque::new(),
                ended: false,
            };
            follow.accept(first);
            Ok(Self::Provider(follow))
        } else {
            NativeObservation::attach_with_endpoints(access, endpoints, target)
                .await
                .map(Self::Native)
        }
    }

    pub async fn observe_bounded(
        access: &CollaborationAccess,
        request: BoundedObservationRequest,
        cancel: CancellationToken,
    ) -> Result<BoundedObservationResult, OperationError> {
        let target = request.target.clone();
        crate::observation_session::validate_observation_bounds(&request).map_err(|error| {
            OperationError::before_dispatch("observation-validation", Some(target.clone()), error)
        })?;
        let endpoints = access
            .endpoint_directory("agent-collaboration-observer")
            .await
            .map_err(|error| {
                OperationError::before_dispatch("observation-connect", Some(target.clone()), error)
            })?;
        if endpoint_is_provider(&endpoints, &target).await? {
            let provider = provider_access(access, endpoints);
            tokio::select! {
                () = cancel.cancelled() => Err(OperationError::before_dispatch(
                    "observation-collect", Some(target), ClientError::InvalidRequest("observation cancelled"))),
                result = provider.observe(request) => result.map_err(|error|
                    OperationError::before_dispatch("observation-collect", Some(target), error)),
            }
        } else {
            if request.after_sequence.is_some() || request.epoch.is_some() {
                return Err(OperationError::before_dispatch(
                    "observation-validation",
                    Some(target),
                    ClientError::InvalidRequest("paging is available only for provider Sessions"),
                ));
            }
            let deadline = tokio::time::Instant::now()
                .checked_add(Duration::from_secs(request.timeout_seconds))
                .ok_or_else(|| {
                    OperationError::before_dispatch(
                        "observation-validation",
                        Some(target.clone()),
                        ClientError::InvalidRequest("invalid observation deadline"),
                    )
                })?;
            let native =
                NativeObservation::attach_with_endpoints(access, endpoints, target.clone()).await?;
            native
                .collect_until(deadline, request.max_events, request.max_bytes, cancel)
                .await
                .map_err(|error| {
                    OperationError::after_dispatch("observation-collect", Some(target), None, error)
                })
        }
    }

    #[must_use]
    pub fn target(&self) -> &SessionRef {
        match self {
            Self::Native(native) => native.target(),
            Self::Provider(follow) => &follow.target,
        }
    }

    #[must_use]
    pub fn generation(&self) -> &CodexGeneration {
        match self {
            Self::Native(native) => native.generation(),
            Self::Provider(follow) => &follow.generation,
        }
    }

    pub async fn next_message(&mut self) -> Result<Value, ClientError> {
        match self {
            Self::Native(native) => native.next_message().await,
            Self::Provider(follow) => follow.next_event().await,
        }
    }

    #[must_use]
    pub const fn is_provider(&self) -> bool {
        matches!(self, Self::Provider(_))
    }
}

impl ProviderSessionFollow {
    /// The next event, resuming observation after the last one delivered.
    pub async fn next_event(&mut self) -> Result<Value, ClientError> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(event);
            }
            if self.ended {
                return Err(ClientError::Protocol("provider Session observation ended"));
            }
            let observed = self
                .access
                .observe(BoundedObservationRequest {
                    target: self.target.clone(),
                    timeout_seconds: FOLLOW_CALL_SECONDS,
                    max_events: FOLLOW_MAX_EVENTS,
                    max_bytes: FOLLOW_MAX_BYTES,
                    after_sequence: self.after_sequence,
                    epoch: self.epoch,
                })
                .await?;
            self.accept(observed);
        }
    }

    fn accept(&mut self, observed: BoundedObservationResult) {
        if observed.epoch.is_some() {
            self.epoch = observed.epoch;
        }
        for event in &observed.events {
            if let Some(sequence) = event.get("sequence").and_then(Value::as_u64) {
                self.after_sequence = Some(
                    self.after_sequence
                        .map_or(sequence, |after| after.max(sequence)),
                );
            }
        }
        self.pending.extend(observed.events);
        if matches!(
            observed.end_reason,
            ObservationEndReason::ResyncRequired | ObservationEndReason::BackendDisconnected
        ) {
            self.ended = true;
        }
    }
}

fn provider_access(
    access: &CollaborationAccess,
    endpoints: EndpointDirectoryReader,
) -> ProviderObservationAccess {
    match (access, endpoints) {
        (CollaborationAccess::Local { router, .. }, _) => {
            ProviderObservationAccess::Local(std::sync::Arc::clone(router))
        }
        (CollaborationAccess::Api { .. }, EndpointDirectoryReader::Api(client)) => {
            ProviderObservationAccess::Api(client)
        }
        (CollaborationAccess::Api { .. }, EndpointDirectoryReader::Local(router)) => {
            ProviderObservationAccess::Local(router)
        }
    }
}

async fn endpoint_is_provider(
    endpoints: &EndpointDirectoryReader,
    target: &SessionRef,
) -> Result<bool, OperationError> {
    let inventory = endpoints.endpoints().await.map_err(|error| {
        OperationError::before_dispatch("observation-discovery", Some(target.clone()), error)
    })?;
    let endpoint = inventory
        .endpoints
        .iter()
        .find(|entry| entry.endpoint == target.endpoint)
        .ok_or_else(|| {
            OperationError::before_dispatch(
                "observation-discovery",
                Some(target.clone()),
                ClientError::Protocol("observation endpoint missing"),
            )
        })?;
    Ok(endpoint
        .channels
        .iter()
        .any(|channel| matches!(channel, ChannelDescription::ExternalProvider { .. })))
}

impl CollaborationClient {
    pub async fn observe_provider_session(
        &self,
        request: BoundedObservationRequest,
    ) -> Result<BoundedObservationResult, ClientError> {
        let timeout = Duration::from_secs(request.timeout_seconds.saturating_add(5));
        let params = serde_json::to_value(request)
            .map_err(|_| ClientError::InvalidRequest("invalid provider observation request"))?;
        let response = self
            .connection
            .call_with_timeout("events_observe", params, timeout)
            .await?;
        serde_json::from_value(response)
            .map_err(|_| ClientError::Protocol("invalid provider observation response"))
    }
}
