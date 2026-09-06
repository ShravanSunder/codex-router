//! Content-free lifecycle facts; evidence provenance is validated before persistence.
use crate::{CodexGeneration, EndpointRef, ObservationTimestamp, SessionId, UuidIdentity};
use serde::{Deserialize, Serialize};
#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadAddress {
    pub endpoint: EndpointRef,
    pub native_thread_id: SessionId,
}
#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservationScope {
    pub endpoint: EndpointRef,
    pub generation: Option<CodexGeneration>,
    pub observer_id: UuidIdentity,
}
#[derive(schemars::JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ObservationSource {
    HostLifecycle,
    NativeNotification,
    InventoryRead,
    ObserverLifecycle,
}
#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum LifecycleSubject {
    Backend,
    Thread { address: ThreadAddress },
}
#[derive(schemars::JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackendStatus {
    Starting,
    Ready,
    Unavailable,
    Failed,
}
#[derive(schemars::JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ObservationOrdering {
    Established,
    Ambiguous,
}
#[derive(schemars::JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminalStatus {
    Completed,
    Interrupted,
    Failed,
}
#[derive(schemars::JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeActiveFlag {
    WaitingOnApproval,
    WaitingOnUserInput,
}
/// Pinned native status projection; schema admission must validate upstream compatibility.
#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum NativeThreadStatus {
    NotLoaded,
    Idle,
    SystemError,
    #[serde(rename_all = "camelCase")]
    Active {
        active_flags: Vec<NativeActiveFlag>,
    },
}
#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum LifecycleChange {
    BackendStatus {
        status: BackendStatus,
    },
    ThreadDiscovered,
    ThreadStatus {
        status: NativeThreadStatus,
        ordering: ObservationOrdering,
    },
    ThreadArchived,
    ThreadUnarchived,
    ThreadDeleted,
    ThreadClosed,
    #[serde(rename_all = "camelCase")]
    TurnTerminal {
        turn_id: SessionId,
        status: TerminalStatus,
    },
    CoverageLost,
    CoverageRestored,
}
#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LifecycleObservation {
    pub observed_at: ObservationTimestamp,
    pub source: ObservationSource,
    pub scope: ObservationScope,
    pub subject: LifecycleSubject,
    pub change: LifecycleChange,
}
impl LifecycleObservation {
    pub fn validate(&self) -> Result<(), &'static str> {
        let backend = matches!(self.subject, LifecycleSubject::Backend);
        if let LifecycleSubject::Thread { address } = &self.subject
            && address.endpoint != self.scope.endpoint
        {
            return Err("thread endpoint differs from observation scope");
        }
        if self.source == ObservationSource::NativeNotification && self.scope.generation.is_none() {
            return Err("native observation requires generation");
        }
        let valid = match &self.change {
            LifecycleChange::BackendStatus { status } => {
                backend
                    && self.source == ObservationSource::HostLifecycle
                    && (*status != BackendStatus::Ready || self.scope.generation.is_some())
            }
            LifecycleChange::CoverageLost => {
                backend && self.source == ObservationSource::ObserverLifecycle
            }
            LifecycleChange::CoverageRestored => {
                backend
                    && self.source == ObservationSource::ObserverLifecycle
                    && self.scope.generation.is_some()
            }
            LifecycleChange::ThreadDiscovered => {
                !backend
                    && matches!(
                        self.source,
                        ObservationSource::NativeNotification | ObservationSource::InventoryRead
                    )
            }
            LifecycleChange::ThreadStatus { .. } => {
                !backend
                    && self.scope.generation.is_some()
                    && matches!(
                        self.source,
                        ObservationSource::NativeNotification | ObservationSource::InventoryRead
                    )
            }
            LifecycleChange::ThreadArchived
            | LifecycleChange::ThreadUnarchived
            | LifecycleChange::ThreadDeleted
            | LifecycleChange::ThreadClosed
            | LifecycleChange::TurnTerminal { .. } => {
                !backend && self.source == ObservationSource::NativeNotification
            }
        };
        if valid {
            Ok(())
        } else {
            Err("invalid lifecycle source/subject combination")
        }
    }
}
