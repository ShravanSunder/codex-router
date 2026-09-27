//! Control observation of the same provider Session hub used by attached faces.

use std::collections::VecDeque;

use collaboration_protocol::{
    BoundedObservationRequest, BoundedObservationResult, ChannelDescription, CodexGeneration,
    EndpointAvailability, ObservationEndReason, ProviderSessionListenReady,
    ProviderSessionListenRequest, SessionRef,
};
use serde_json::{Value, json};
use session_event_model::SessionEvent;
use tokio::sync::broadcast;

use crate::{HubEvent, ServiceIdentity, SessionEventAttachment, SessionEventHubError};

const MAX_EVENTS: usize = 4096;
const MAX_BYTES: usize = 1_048_576;
// The bounded result itself is one Control frame. Leave room for its envelope,
// target, generation and JSON framing when callers request the full byte bound.
const MAX_CONTROL_EVENT_BYTES: usize = MAX_BYTES - 65_536;

pub(crate) struct ProviderSessionSubscription {
    pub ready: ProviderSessionListenReady,
    snapshot: VecDeque<Value>,
    receiver: broadcast::Receiver<HubEvent>,
    closed: bool,
}

impl ProviderSessionSubscription {
    pub async fn next_value(&mut self) -> Option<Value> {
        if let Some(event) = self.snapshot.pop_front() {
            return Some(self.frame_bounded(event));
        }
        if self.closed {
            return None;
        }
        match self.receiver.recv().await {
            Ok(
                event @ HubEvent {
                    event: SessionEvent::ResyncRequired { .. },
                    ..
                },
            ) => {
                self.closed = true;
                Some(self.frame_bounded(event_value(event)))
            }
            Ok(event) => Some(self.frame_bounded(event_value(event))),
            Err(broadcast::error::RecvError::Lagged(_)) => {
                self.closed = true;
                Some(json!({"kind":"resyncRequired"}))
            }
            Err(broadcast::error::RecvError::Closed) => {
                self.closed = true;
                None
            }
        }
    }

    fn frame_bounded(&mut self, event: Value) -> Value {
        if serde_json::to_vec(&event).is_ok_and(|encoded| encoded.len() <= MAX_CONTROL_EVENT_BYTES)
        {
            event
        } else {
            self.snapshot.clear();
            self.closed = true;
            json!({"kind":"resyncRequired"})
        }
    }
}

pub(crate) async fn listen(
    params: Value,
    identity: &ServiceIdentity,
) -> Result<ProviderSessionSubscription, ObservationFailure> {
    let request: ProviderSessionListenRequest =
        serde_json::from_value(params).map_err(|_| ObservationFailure::InvalidField)?;
    let (attachment, generation) = attach(&request.target, identity).await?;
    Ok(ProviderSessionSubscription {
        ready: ProviderSessionListenReady {
            target: request.target,
            generation,
        },
        snapshot: attachment.snapshot.into_iter().map(event_value).collect(),
        receiver: attachment.receiver,
        closed: false,
    })
}

pub(crate) async fn observe(id: Value, params: Value, identity: &ServiceIdentity) -> Value {
    let request: BoundedObservationRequest = match serde_json::from_value(params) {
        Ok(request) => request,
        Err(_) => return ObservationFailure::InvalidField.response(id),
    };
    match collect(request, identity).await {
        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
        Err(error) => error.response(id),
    }
}

async fn collect(
    request: BoundedObservationRequest,
    identity: &ServiceIdentity,
) -> Result<BoundedObservationResult, ObservationFailure> {
    if request.timeout_seconds == 0
        || request.max_events == 0
        || request.max_events > MAX_EVENTS
        || request.max_bytes == 0
        || request.max_bytes > MAX_BYTES
    {
        return Err(ObservationFailure::InvalidField);
    }
    let deadline = tokio::time::Instant::now()
        .checked_add(std::time::Duration::from_secs(request.timeout_seconds))
        .ok_or(ObservationFailure::InvalidField)?;
    let (attachment, generation) =
        tokio::time::timeout_at(deadline, attach(&request.target, identity))
            .await
            .map_err(|_| ObservationFailure::Unavailable)??;
    let mut events = Vec::new();
    let mut bytes = 0_usize;
    let mut snapshot = attachment.snapshot.into_iter();
    let mut receiver = attachment.receiver;
    let end_reason = loop {
        let event = if let Some(event) = snapshot.next() {
            if matches!(&event.event, SessionEvent::ResyncRequired { .. }) {
                let _ = append_event(&mut events, &mut bytes, event_value(event), &request)?;
                break ObservationEndReason::ResyncRequired;
            }
            event_value(event)
        } else {
            let received = tokio::select! {
                () = tokio::time::sleep_until(deadline) => break ObservationEndReason::DeadlineReached,
                received = receiver.recv() => received,
            };
            match received {
                Ok(
                    event @ HubEvent {
                        event: SessionEvent::ResyncRequired { .. },
                        ..
                    },
                ) => {
                    let _ = append_event(&mut events, &mut bytes, event_value(event), &request)?;
                    break ObservationEndReason::ResyncRequired;
                }
                Ok(event) => event_value(event),
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let _ = append_event(
                        &mut events,
                        &mut bytes,
                        json!({"kind":"resyncRequired"}),
                        &request,
                    )?;
                    break ObservationEndReason::ResyncRequired;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break ObservationEndReason::BackendDisconnected;
                }
            }
        };
        if !append_event(&mut events, &mut bytes, event, &request)? {
            break ObservationEndReason::ResultLimitReached;
        }
        if events.len() == request.max_events || bytes == request.max_bytes {
            break ObservationEndReason::ResultLimitReached;
        }
    };
    Ok(BoundedObservationResult {
        target: request.target,
        generation,
        attached: true,
        events,
        end_reason,
        continuation_gap: true,
    })
}

fn append_event(
    events: &mut Vec<Value>,
    bytes: &mut usize,
    event: Value,
    request: &BoundedObservationRequest,
) -> Result<bool, ObservationFailure> {
    let encoded = serde_json::to_vec(&event).map_err(|_| ObservationFailure::Unavailable)?;
    if events.len() >= request.max_events
        || bytes
            .checked_add(encoded.len())
            .is_none_or(|total| total > request.max_bytes.min(MAX_CONTROL_EVENT_BYTES))
    {
        return Ok(false);
    }
    *bytes += encoded.len();
    events.push(event);
    Ok(true)
}

async fn attach(
    target: &SessionRef,
    identity: &ServiceIdentity,
) -> Result<(SessionEventAttachment, CodexGeneration), ObservationFailure> {
    if target.endpoint.service_id != identity.service_id {
        return Err(ObservationFailure::NotFound);
    }
    let endpoint = identity
        .directory
        .read_endpoint(&target.endpoint)
        .map_err(|_| ObservationFailure::Unavailable)?
        .ok_or(ObservationFailure::NotFound)?;
    if !matches!(
        endpoint.availability,
        EndpointAvailability::Available { .. }
    ) {
        return Err(ObservationFailure::Unavailable);
    }
    let generation = endpoint
        .channels
        .iter()
        .find_map(|channel| match channel {
            ChannelDescription::ExternalProvider {
                binding_generation, ..
            } => Some(CodexGeneration {
                service_epoch: identity.service_epoch.clone(),
                generation: *binding_generation,
            }),
            _ => None,
        })
        .ok_or(ObservationFailure::NotFound)?;
    let session = serde_json::from_value(
        serde_json::to_value(target).map_err(|_| ObservationFailure::InvalidField)?,
    )
    .map_err(|_| ObservationFailure::InvalidField)?;
    let hub = identity
        .provider_session_hub
        .as_ref()
        .ok_or(ObservationFailure::Unavailable)?;
    let attachment = hub.attach(session).await.map_err(|error| match error {
        SessionEventHubError::SessionNotFound => ObservationFailure::NotFound,
        SessionEventHubError::Unavailable => ObservationFailure::Unavailable,
    })?;
    Ok((attachment, generation))
}

fn event_value(event: HubEvent) -> Value {
    json!({"sequence":event.sequence,"event":event.event})
}

#[derive(Clone, Copy)]
pub(crate) enum ObservationFailure {
    InvalidField,
    NotFound,
    Unavailable,
}

impl ObservationFailure {
    pub(crate) fn response(self, id: Value) -> Value {
        let (code, kind, message) = match self {
            Self::InvalidField => (
                -32602,
                "invalidField",
                "Invalid provider observation request",
            ),
            Self::NotFound => (-32002, "notFound", "Provider Session was not found"),
            Self::Unavailable => (-32050, "unavailable", "Provider observation unavailable"),
        };
        json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message,"data":{"kind":kind,"stage":"inspect","message":message}}})
    }
}
