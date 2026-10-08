use crate::{
    ChildComponent, ChildDegradation, ChildPhase, ComponentFingerprint, GenerationId, ListenerKind,
};
use serde::{Deserialize, Serialize, ser::Error as _};

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ChildSnapshotError {
    #[error("pending listener request is only valid while granted or preparing")]
    PendingListenerRequestOutsidePrepare,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(try_from = "ChildSnapshotWire")]
pub struct ChildSnapshot {
    pub phase: ChildPhase,
    pub fingerprint: ComponentFingerprint,
    pub committed_generation: Option<GenerationId>,
    pub pending_listener_request: Option<ListenerKind>,
    pub degraded: Vec<(ChildComponent, ChildDegradation)>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChildSnapshotWire {
    phase: ChildPhase,
    fingerprint: ComponentFingerprint,
    committed_generation: Option<GenerationId>,
    pending_listener_request: Option<ListenerKind>,
    degraded: Vec<(ChildComponent, ChildDegradation)>,
}

impl ChildSnapshot {
    pub fn new(
        phase: ChildPhase,
        fingerprint: ComponentFingerprint,
        committed_generation: Option<GenerationId>,
        pending_listener_request: Option<ListenerKind>,
        degraded: Vec<(ChildComponent, ChildDegradation)>,
    ) -> Result<Self, ChildSnapshotError> {
        let snapshot = Self {
            phase,
            fingerprint,
            committed_generation,
            pending_listener_request,
            degraded,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    fn validate(&self) -> Result<(), ChildSnapshotError> {
        if self.pending_listener_request.is_some()
            && !matches!(self.phase, ChildPhase::Granted | ChildPhase::Preparing)
        {
            return Err(ChildSnapshotError::PendingListenerRequestOutsidePrepare);
        }
        Ok(())
    }
}

impl TryFrom<ChildSnapshotWire> for ChildSnapshot {
    type Error = ChildSnapshotError;

    fn try_from(wire: ChildSnapshotWire) -> Result<Self, Self::Error> {
        Self::new(
            wire.phase,
            wire.fingerprint,
            wire.committed_generation,
            wire.pending_listener_request,
            wire.degraded,
        )
    }
}

impl From<ChildSnapshot> for ChildSnapshotWire {
    fn from(snapshot: ChildSnapshot) -> Self {
        Self {
            phase: snapshot.phase,
            fingerprint: snapshot.fingerprint,
            committed_generation: snapshot.committed_generation,
            pending_listener_request: snapshot.pending_listener_request,
            degraded: snapshot.degraded,
        }
    }
}

impl Serialize for ChildSnapshot {
    fn serialize<TSerializer>(
        &self,
        serializer: TSerializer,
    ) -> Result<TSerializer::Ok, TSerializer::Error>
    where
        TSerializer: serde::Serializer,
    {
        self.validate().map_err(TSerializer::Error::custom)?;
        ChildSnapshotWire::from(self.clone()).serialize(serializer)
    }
}
