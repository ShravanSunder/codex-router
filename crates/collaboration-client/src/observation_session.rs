//! Explicit native attachment and buffered observation, independent of terminal UI.
use crate::{ClientError, ControlClient};
use codex_native_integration::NativeProtocolConnection;
use collaboration_protocol::{
    ChannelDescription, CodexGeneration, EndpointAvailability, EndpointId, EndpointRef, SessionId,
    SessionRef,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::Path, time::Duration};
use tokio_util::sync::CancellationToken;

const MAX_BOUNDED_EVENTS: usize = 4096;
const MAX_BOUNDED_BYTES: usize = 1_048_576;

#[derive(JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ObservationEndReason {
    DeadlineReached,
    ResultLimitReached,
    CallerCancelled,
    BackendDisconnected,
}

#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BoundedObservationResult {
    pub target: SessionRef,
    pub generation: CodexGeneration,
    pub attached: bool,
    pub events: Vec<Value>,
    pub end_reason: ObservationEndReason,
    pub continuation_gap: bool,
}

#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BoundedObservationRequest {
    pub target: SessionRef,
    #[schemars(range(min = 1))]
    pub timeout_seconds: u64,
    #[schemars(range(min = 1, max = 4096))]
    pub max_events: usize,
    #[schemars(range(min = 1, max = 1048576))]
    pub max_bytes: usize,
}

pub struct NativeObservation {
    connection: NativeProtocolConnection,
    target: SessionRef,
    generation: CodexGeneration,
}
impl NativeObservation {
    pub async fn observe_bounded(
        directory: &Path,
        request: BoundedObservationRequest,
        cancel: CancellationToken,
    ) -> Result<BoundedObservationResult, ClientError> {
        validate_observation_bounds(&request)?;
        let deadline = tokio::time::Instant::now()
            .checked_add(Duration::from_secs(request.timeout_seconds))
            .ok_or(ClientError::InvalidRequest("invalid observation deadline"))?;
        let observation = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                return Err(ClientError::InvalidRequest("observation cancelled before attachment"));
            }
            _ = tokio::time::sleep_until(deadline) => {
                return Err(ClientError::Timeout);
            }
            observation = Self::attach(directory, request.target) => observation?,
        };
        observation
            .collect_until(deadline, request.max_events, request.max_bytes, cancel)
            .await
    }
    /// Binds the endpoint to the discovered service before attaching the native session ID.
    pub async fn attach_by_ids(
        directory: &Path,
        endpoint_id: EndpointId,
        session_id: SessionId,
    ) -> Result<Self, ClientError> {
        let control = Self::connect_control(directory).await?;
        let target = SessionRef {
            endpoint: EndpointRef {
                service_id: control.identity().service_id.clone(),
                endpoint_id,
            },
            session_id,
        };
        Self::attach_with_control(directory, control, target).await
    }

    /// May load the target through native resume; readiness is returned only after attachment.
    pub async fn attach(directory: &Path, target: SessionRef) -> Result<Self, ClientError> {
        let control = Self::connect_control(directory).await?;
        Self::attach_with_control(directory, control, target).await
    }

    async fn connect_control(directory: &Path) -> Result<ControlClient, ClientError> {
        ControlClient::connect(
            directory,
            "agent-collaboration-observer",
            env!("CARGO_PKG_VERSION"),
        )
        .await
    }

    async fn attach_with_control(
        directory: &Path,
        mut control: ControlClient,
        target: SessionRef,
    ) -> Result<Self, ClientError> {
        let inventory = control.list_endpoints().await?;
        let endpoint = inventory
            .endpoints
            .into_iter()
            .find(|e| e.endpoint == target.endpoint)
            .ok_or(ClientError::Protocol("observation endpoint missing"))?;
        if !matches!(
            endpoint.availability,
            EndpointAvailability::Available { .. }
        ) {
            return Err(ClientError::Protocol("observation endpoint unavailable"));
        }
        let (path, generation) = endpoint
            .channels
            .into_iter()
            .find_map(|c| match c {
                ChannelDescription::NativeCodex {
                    path,
                    generation: Some(g),
                    ..
                } => Some((String::from(path), g)),
                _ => None,
            })
            .ok_or(ClientError::Protocol("native observation unsupported"))?;
        let relative = Path::new(&path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(ClientError::Protocol("invalid native observation path"));
        }
        let root = std::fs::canonicalize(directory)?;
        let socket = std::fs::canonicalize(root.join(relative))?;
        if socket.parent() != Some(root.as_path()) {
            return Err(ClientError::Protocol("observation path escaped service"));
        }
        let mut connection = NativeProtocolConnection::connect(&socket)
            .await
            .map_err(|_| ClientError::Protocol("native observation connection failed"))?;
        connection
            .resume_thread(&String::from(target.session_id.clone()))
            .await
            .map_err(|_| ClientError::Protocol("native attachment failed or uncertain"))?;
        let current = control.list_endpoints().await?;
        let unchanged = current.endpoints.iter().find(|e| e.endpoint == target.endpoint).is_some_and(|e| e.channels.iter().any(|c| matches!(c, ChannelDescription::NativeCodex { generation:Some(g),.. } if g == &generation)));
        control.close().await?;
        if !unchanged {
            return Err(ClientError::Protocol("backend changed during attachment"));
        }
        Ok(Self {
            connection,
            target,
            generation,
        })
    }
    #[must_use]
    pub fn target(&self) -> &SessionRef {
        &self.target
    }
    #[must_use]
    pub fn generation(&self) -> &CodexGeneration {
        &self.generation
    }
    /// Returns native messages already buffered during attachment before reading new frames.
    /// Dropping observation never interrupts the native turn or answers an approval callback.
    pub async fn next_message(&mut self) -> Result<Value, ClientError> {
        self.connection
            .next_message()
            .await
            .map_err(|_| ClientError::Protocol("native observation closed"))
    }

    /// Collects a bounded call-local observation. A later call is a fresh attachment,
    /// not a cursor-based continuation, and cancellation never interrupts the turn.
    pub async fn collect_bounded(
        self,
        timeout: Duration,
        max_events: usize,
        max_bytes: usize,
        cancel: CancellationToken,
    ) -> Result<BoundedObservationResult, ClientError> {
        if timeout.is_zero()
            || max_events == 0
            || max_events > MAX_BOUNDED_EVENTS
            || max_bytes == 0
            || max_bytes > MAX_BOUNDED_BYTES
        {
            return Err(ClientError::InvalidRequest("invalid observation bound"));
        }
        let deadline = tokio::time::Instant::now()
            .checked_add(timeout)
            .ok_or(ClientError::InvalidRequest("invalid observation deadline"))?;
        self.collect_until(deadline, max_events, max_bytes, cancel)
            .await
    }

    async fn collect_until(
        mut self,
        deadline: tokio::time::Instant,
        max_events: usize,
        max_bytes: usize,
        cancel: CancellationToken,
    ) -> Result<BoundedObservationResult, ClientError> {
        let target = self.target.clone();
        let generation = self.generation.clone();
        let mut events = Vec::new();
        let mut event_bytes = 0usize;
        let end_reason = loop {
            let message = tokio::select! {
                _ = cancel.cancelled() => break ObservationEndReason::CallerCancelled,
                _ = tokio::time::sleep_until(deadline) => break ObservationEndReason::DeadlineReached,
                message = self.next_message() => message,
            };
            let message = match message {
                Ok(message) => message,
                Err(_) => break ObservationEndReason::BackendDisconnected,
            };
            let encoded_bytes = serde_json::to_vec(&message)
                .map_err(|_| ClientError::Protocol("observation event encoding failed"))?
                .len();
            if events.len() >= max_events
                || event_bytes
                    .checked_add(encoded_bytes)
                    .is_none_or(|total| total > max_bytes)
            {
                break ObservationEndReason::ResultLimitReached;
            }
            event_bytes += encoded_bytes;
            events.push(message);
            if events.len() == max_events || event_bytes == max_bytes {
                break ObservationEndReason::ResultLimitReached;
            }
        };
        let continuation_gap = observation_has_continuation_gap(&end_reason);
        Ok(BoundedObservationResult {
            target,
            generation,
            attached: true,
            events,
            end_reason,
            continuation_gap,
        })
    }
}

fn validate_observation_bounds(request: &BoundedObservationRequest) -> Result<(), ClientError> {
    if request.timeout_seconds == 0
        || request.max_events == 0
        || request.max_events > MAX_BOUNDED_EVENTS
        || request.max_bytes == 0
        || request.max_bytes > MAX_BOUNDED_BYTES
    {
        return Err(ClientError::InvalidRequest("invalid observation bound"));
    }
    Ok(())
}

const fn observation_has_continuation_gap(_end_reason: &ObservationEndReason) -> bool {
    true
}

#[cfg(test)]
mod bounded_observation_tests {
    use super::{
        BoundedObservationRequest, NativeObservation, ObservationEndReason,
        observation_has_continuation_gap, validate_observation_bounds,
    };
    use collaboration_protocol::{EndpointId, EndpointRef, SessionId, SessionRef, UuidIdentity};
    use tokio_util::sync::CancellationToken;

    fn request(
        timeout_seconds: u64,
        max_events: usize,
        max_bytes: usize,
    ) -> BoundedObservationRequest {
        BoundedObservationRequest {
            target: SessionRef {
                endpoint: EndpointRef {
                    service_id: UuidIdentity::try_from(
                        "00000000-0000-4000-8000-000000000001".to_owned(),
                    )
                    .expect("service UUID"),
                    endpoint_id: EndpointId::try_from("codex-local".to_owned())
                        .expect("endpoint ID"),
                },
                session_id: SessionId::try_from("thread-1".to_owned()).expect("session ID"),
            },
            timeout_seconds,
            max_events,
            max_bytes,
        }
    }

    #[test]
    fn every_bounded_end_state_warns_that_a_later_attach_cannot_replay_events() {
        for reason in [
            ObservationEndReason::DeadlineReached,
            ObservationEndReason::ResultLimitReached,
            ObservationEndReason::CallerCancelled,
            ObservationEndReason::BackendDisconnected,
        ] {
            assert!(observation_has_continuation_gap(&reason));
        }
    }

    #[test]
    fn declared_observation_bounds_are_enforced_by_the_sdk() {
        assert!(validate_observation_bounds(&request(1, 4096, 1_048_576)).is_ok());
        for invalid in [
            request(0, 1, 1),
            request(1, 0, 1),
            request(1, 4097, 1),
            request(1, 1, 0),
            request(1, 1, 1_048_577),
        ] {
            assert!(validate_observation_bounds(&invalid).is_err());
        }
    }

    #[tokio::test]
    async fn pre_cancelled_observation_does_not_attempt_discovery_or_attachment() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = NativeObservation::observe_bounded(
            std::path::Path::new("/path-that-must-not-be-read"),
            request(1, 1, 1),
            cancel,
        )
        .await
        .expect_err("pre-cancelled observation must stop before attachment");
        assert!(matches!(
            error,
            crate::ClientError::InvalidRequest("observation cancelled before attachment")
        ));
    }
}
