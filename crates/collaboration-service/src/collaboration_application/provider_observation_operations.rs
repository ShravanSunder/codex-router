//! Provider Session observation: bounded, resumable reads of the same hub attached faces use.
//! Following a Session is a chain of these reads, each resuming after the last event.
use super::{CollaborationRejection, CollaborationRejectionReason, PublishedRejection};
use crate::{HubEvent, ServiceIdentity, SessionEventAttachment, SessionEventHubError};
use collaboration_protocol::{
    BoundedObservationRequest, BoundedObservationResult, ChannelDescription, CodexGeneration,
    EndpointAvailability, ObservationEndReason, ProviderObservationEventTooLarge,
    ProviderObservationEventTooLargeKind, SessionRef,
};
use serde_json::{Value, json};
use session_event_model::SessionEvent;
use tokio::sync::broadcast;

/// Observation operations over the provider Session event hub.
pub struct ObservationOperations<'service> {
    identity: &'service ServiceIdentity,
}

impl<'service> ObservationOperations<'service> {
    pub(crate) fn new(identity: &'service ServiceIdentity) -> Self {
        Self { identity }
    }

    /// Collects events after `afterSequence` (in `epoch`) until a bound or the deadline.
    pub async fn session_observe(
        &self,
        request: BoundedObservationRequest,
    ) -> Result<BoundedObservationResult, ObservationFailure> {
        collect(request, self.identity).await
    }
}

const MAX_EVENTS: usize = 4096;
const MAX_BYTES: usize = 1_048_576;
// A bounded result is one response of at most MAX_BYTES. Leave room for its envelope,
// target, generation and JSON framing when callers request the full byte bound.
const MAX_EVENT_BYTES: usize = MAX_BYTES - 65_536;

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
    if request.after_sequence.is_some() && request.epoch.is_none() {
        return Err(ObservationFailure::InvalidField);
    }
    let current_epoch = attachment.epoch;
    if request.epoch.is_some_and(|epoch| epoch != current_epoch) {
        return Ok(BoundedObservationResult {
            target: request.target,
            generation,
            attached: true,
            events: vec![json!({"kind":"resyncRequired"})],
            end_reason: ObservationEndReason::ResyncRequired,
            continuation_gap: true,
            epoch: Some(current_epoch),
        });
    }
    let mut events = Vec::new();
    let mut bytes = 0_usize;
    let mut snapshot = attachment.snapshot.into_iter().filter(|event| {
        request
            .after_sequence
            .is_none_or(|after| event.sequence > after)
    });
    let mut receiver = attachment.receiver;
    let end_reason = loop {
        let event = if let Some(event) = snapshot.next() {
            if matches!(&event.event, SessionEvent::ResyncRequired { .. }) {
                let _ = append_event(
                    &mut events,
                    &mut bytes,
                    event_value_bounded(event),
                    &request,
                )?;
                break ObservationEndReason::ResyncRequired;
            }
            event_value_bounded(event)
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
                    let _ = append_event(
                        &mut events,
                        &mut bytes,
                        event_value_bounded(event),
                        &request,
                    )?;
                    break ObservationEndReason::ResyncRequired;
                }
                Ok(event) => event_value_bounded(event),
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
        epoch: Some(current_epoch),
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
            .is_none_or(|total| total > request.max_bytes.min(MAX_EVENT_BYTES))
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

fn event_value_bounded(event: HubEvent) -> Value {
    let sequence = event.sequence;
    let item_id = match &event.event {
        SessionEvent::ItemStarted { item } | SessionEvent::ItemUpdated { item } => {
            Some(item.item_id.clone())
        }
        SessionEvent::ItemCompleted { item_id } => Some(item_id.clone()),
        _ => None,
    };
    let value = event_value(event);
    if serde_json::to_vec(&value).is_ok_and(|encoded| encoded.len() <= MAX_EVENT_BYTES) {
        value
    } else {
        json!(ProviderObservationEventTooLarge {
            kind: ProviderObservationEventTooLargeKind::EventTooLarge,
            sequence,
            item_id,
        })
    }
}

/// Why a provider observation failed: `{kind, stage, message}` on the wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ObservationFailure {
    #[error("Invalid provider observation request")]
    InvalidField,
    #[error("Provider Session was not found")]
    NotFound,
    #[error("Provider observation unavailable")]
    Unavailable,
}

impl ObservationFailure {
    #[must_use]
    pub const fn kind(self) -> &'static str {
        match self {
            Self::InvalidField => "invalidField",
            Self::NotFound => "notFound",
            Self::Unavailable => "unavailable",
        }
    }
}

impl serde::Serialize for ObservationFailure {
    fn serialize<TSerializer: serde::Serializer>(
        &self,
        serializer: TSerializer,
    ) -> Result<TSerializer::Ok, TSerializer::Error> {
        json!({"kind":self.kind(),"stage":"inspect","message":self.to_string()})
            .serialize(serializer)
    }
}

impl CollaborationRejection for ObservationFailure {
    fn rejection_reason(&self) -> Option<CollaborationRejectionReason> {
        match self {
            Self::InvalidField => Some(CollaborationRejectionReason::InvalidShape),
            Self::NotFound | Self::Unavailable => None,
        }
    }

    fn published_rejection(&self) -> PublishedRejection {
        let code = match self {
            Self::InvalidField => PublishedRejection::INVALID_PARAMS,
            Self::NotFound => PublishedRejection::NOT_FOUND,
            Self::Unavailable => PublishedRejection::OPERATION_FAILED,
        };
        PublishedRejection::typed(code, self.to_string(), self)
    }
}
