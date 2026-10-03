//! Select the provider hub or native attachment from endpoint capabilities.

use std::{path::Path, time::Duration};

use collaboration_protocol::{
    BoundedObservationRequest, BoundedObservationResult, ChannelDescription, CodexGeneration,
    EndpointId, EndpointRef, ProviderSessionListenReady, ProviderSessionListenRequest, SessionId,
    SessionRef,
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{ClientError, ControlClient, NativeObservation, OperationError};

pub enum SessionObservation {
    Native(NativeObservation),
    Provider {
        control: ControlClient,
        ready: ProviderSessionListenReady,
    },
}

impl SessionObservation {
    pub async fn attach_by_ids_with_context(
        directory: &Path,
        endpoint_id: EndpointId,
        session_id: SessionId,
    ) -> Result<Self, OperationError> {
        let mut control = ControlClient::connect(
            directory,
            "agent-collaboration-observer",
            env!("CARGO_PKG_VERSION"),
        )
        .await
        .map_err(|error| OperationError::before_dispatch("observation-connect", None, error))?;
        let target = SessionRef {
            endpoint: EndpointRef {
                service_id: control.identity().service_id.clone(),
                endpoint_id,
            },
            session_id,
        };
        if endpoint_is_provider(&mut control, &target).await? {
            let ready = control
                .listen_provider_session(ProviderSessionListenRequest {
                    target: target.clone(),
                })
                .await
                .map_err(|error| {
                    OperationError::before_dispatch("observation-attach", Some(target), error)
                })?;
            Ok(Self::Provider { control, ready })
        } else {
            NativeObservation::attach_with_control_context(directory, control, target)
                .await
                .map(Self::Native)
        }
    }

    pub async fn observe_bounded(
        directory: &Path,
        request: BoundedObservationRequest,
        cancel: CancellationToken,
    ) -> Result<BoundedObservationResult, OperationError> {
        let target = request.target.clone();
        crate::observation_session::validate_observation_bounds(&request).map_err(|error| {
            OperationError::before_dispatch("observation-validation", Some(target.clone()), error)
        })?;
        let mut control = ControlClient::connect(
            directory,
            "agent-collaboration-observer",
            env!("CARGO_PKG_VERSION"),
        )
        .await
        .map_err(|error| {
            OperationError::before_dispatch("observation-connect", Some(target.clone()), error)
        })?;
        if endpoint_is_provider(&mut control, &target).await? {
            tokio::select! {
                () = cancel.cancelled() => Err(OperationError::before_dispatch(
                    "observation-collect", Some(target), ClientError::InvalidRequest("observation cancelled"))),
                result = control.observe_provider_session(request) => result.map_err(|error|
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
                NativeObservation::attach_with_control_context(directory, control, target.clone())
                    .await?;
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
            Self::Provider { ready, .. } => &ready.target,
        }
    }

    #[must_use]
    pub fn generation(&self) -> &CodexGeneration {
        match self {
            Self::Native(native) => native.generation(),
            Self::Provider { ready, .. } => &ready.generation,
        }
    }

    pub async fn next_message(&mut self) -> Result<Value, ClientError> {
        match self {
            Self::Native(native) => native.next_message().await,
            Self::Provider { control, .. } => control.next_provider_session_notification().await,
        }
    }

    #[must_use]
    pub const fn is_provider(&self) -> bool {
        matches!(self, Self::Provider { .. })
    }
}

async fn endpoint_is_provider(
    control: &mut ControlClient,
    target: &SessionRef,
) -> Result<bool, OperationError> {
    let inventory = control.list_endpoints().await.map_err(|error| {
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

impl ControlClient {
    pub async fn observe_provider_session(
        &mut self,
        request: BoundedObservationRequest,
    ) -> Result<BoundedObservationResult, ClientError> {
        let timeout = Duration::from_secs(request.timeout_seconds.saturating_add(5));
        let params = serde_json::to_value(request)
            .map_err(|_| ClientError::InvalidRequest("invalid provider observation request"))?;
        let response = self
            .connection
            .call_with_timeout("provider/sessionObserve", params, timeout)
            .await?;
        serde_json::from_value(response)
            .map_err(|_| ClientError::Protocol("invalid provider observation response"))
    }

    pub async fn listen_provider_session(
        &mut self,
        request: ProviderSessionListenRequest,
    ) -> Result<ProviderSessionListenReady, ClientError> {
        let params = serde_json::to_value(request)
            .map_err(|_| ClientError::InvalidRequest("invalid provider listen request"))?;
        let response = self
            .connection
            .call("provider/sessionListen", params)
            .await?;
        let ready: ProviderSessionListenReady = serde_json::from_value(response)
            .map_err(|_| ClientError::Protocol("invalid provider listen response"))?;
        // The server sends the snapshot as ordered notifications after this reply.
        Ok(ready)
    }
}
