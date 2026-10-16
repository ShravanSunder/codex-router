//! Request-local classification and projection of existing recorded authority.
use codex_acp_adapter::{ApprovalBroker, ApprovalRoute, RecordedSessionResume, SessionSetupError};
use collaboration_protocol::SessionRef;
use std::sync::Weak;

#[derive(Clone)]
pub(crate) enum RecordedNativeRouteClassification {
    GenericRoute,
    RecordedRoute {
        recorded_route: Box<ApprovalRoute>,
        policy_owner: Weak<dyn ApprovalBroker>,
    },
    UnresolvedAuthority,
}

impl RecordedNativeRouteClassification {
    pub(crate) fn may_resume(&self) -> bool {
        !matches!(self, Self::GenericRoute)
    }
}

pub(crate) async fn classify_recorded_native_route(
    broker: Option<&Weak<dyn ApprovalBroker>>,
    target: &SessionRef,
) -> RecordedNativeRouteClassification {
    let Some(policy_owner) = broker else {
        return RecordedNativeRouteClassification::GenericRoute;
    };
    let Some(broker) = policy_owner.upgrade() else {
        return RecordedNativeRouteClassification::UnresolvedAuthority;
    };
    match broker.route(&String::from(target.session_id.clone())).await {
        Ok(Some(recorded_route)) => RecordedNativeRouteClassification::RecordedRoute {
            recorded_route: Box::new(recorded_route),
            policy_owner: policy_owner.clone(),
        },
        Ok(None) => RecordedNativeRouteClassification::GenericRoute,
        Err(_) => RecordedNativeRouteClassification::UnresolvedAuthority,
    }
}

pub(crate) async fn prepare_classified_native_policy(
    classification: &RecordedNativeRouteClassification,
    target: &SessionRef,
    working_directory: Option<&str>,
) -> Result<Option<RecordedSessionResume>, SessionSetupError> {
    let (recorded_route, policy_owner) = match classification {
        RecordedNativeRouteClassification::GenericRoute => return Ok(None),
        RecordedNativeRouteClassification::UnresolvedAuthority => {
            return Err(SessionSetupError::Unavailable);
        }
        RecordedNativeRouteClassification::RecordedRoute {
            recorded_route,
            policy_owner,
        } => (recorded_route, policy_owner),
    };
    let _owner = policy_owner
        .upgrade()
        .ok_or(SessionSetupError::Unavailable)?;
    if recorded_route.created_by.endpoint.service_id != target.endpoint.service_id
        || recorded_route.approver.endpoint.service_id != target.endpoint.service_id
    {
        return Err(SessionSetupError::ConfigurationMismatch);
    }
    let thread_id = String::from(target.session_id.clone());
    let directory = working_directory.ok_or(SessionSetupError::InvalidParameters)?;
    RecordedSessionResume::prepare(recorded_route, &thread_id, directory)
        .await
        .map(Some)
}

pub(crate) async fn prepare_recorded_native_policy(
    broker: Option<&Weak<dyn ApprovalBroker>>,
    target: &SessionRef,
    working_directory: Option<&str>,
) -> Result<Option<RecordedSessionResume>, SessionSetupError> {
    let classification = classify_recorded_native_route(broker, target).await;
    prepare_classified_native_policy(&classification, target, working_directory).await
}
