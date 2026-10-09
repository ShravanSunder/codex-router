use crate::RoleHandoverVersion;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(from = "PrepareModeWire", into = "PrepareModeWire")]
pub enum PrepareMode {
    Fresh,
    Replacement {
        active_degraded: Vec<(ChildComponent, ChildDegradation)>,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum PrepareModeWire {
    Fresh {},
    Replacement {
        active_degraded: Vec<(ChildComponent, ChildDegradation)>,
    },
}

impl From<PrepareModeWire> for PrepareMode {
    fn from(wire: PrepareModeWire) -> Self {
        match wire {
            PrepareModeWire::Fresh {} => Self::Fresh,
            PrepareModeWire::Replacement { active_degraded } => {
                Self::Replacement { active_degraded }
            }
        }
    }
}

impl From<PrepareMode> for PrepareModeWire {
    fn from(value: PrepareMode) -> Self {
        match value {
            PrepareMode::Fresh => Self::Fresh {},
            PrepareMode::Replacement { active_degraded } => Self::Replacement { active_degraded },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(from = "DeactivateRefusalWire", into = "DeactivateRefusalWire")]
pub enum DeactivateRefusal {
    HandoverIncompatible {
        produced: RoleHandoverVersion,
        wanted: RoleHandoverVersion,
    },
    HandoverTooLarge,
}

#[derive(Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum DeactivateRefusalWire {
    HandoverIncompatible {
        produced: RoleHandoverVersion,
        wanted: RoleHandoverVersion,
    },
    HandoverTooLarge {},
}

impl From<DeactivateRefusalWire> for DeactivateRefusal {
    fn from(wire: DeactivateRefusalWire) -> Self {
        match wire {
            DeactivateRefusalWire::HandoverIncompatible { produced, wanted } => {
                Self::HandoverIncompatible { produced, wanted }
            }
            DeactivateRefusalWire::HandoverTooLarge {} => Self::HandoverTooLarge,
        }
    }
}

impl From<DeactivateRefusal> for DeactivateRefusalWire {
    fn from(value: DeactivateRefusal) -> Self {
        match value {
            DeactivateRefusal::HandoverIncompatible { produced, wanted } => {
                Self::HandoverIncompatible { produced, wanted }
            }
            DeactivateRefusal::HandoverTooLarge => Self::HandoverTooLarge {},
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum DeactivateReason {
    Replacement,
    KeeperFullRestart,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum NoGenerationReason {
    StartupPending,
    CurrentExited,
    RecoveryExhausted,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum ChildPhase {
    Granted,
    Preparing,
    Prepared,
    Active,
    Deactivating,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(from = "PrepareFailureWire", into = "PrepareFailureWire")]
pub enum PrepareFailure {
    StoreOpenFailed,
    StoreSchemaNewerThanImage {
        store: StoreKind,
    },
    StoreMigrationHistoryInvalid {
        store: StoreKind,
        reason: MigrationHistoryDefect,
    },
    SecretStoreUnavailable,
    ListenerGrantInvalid,
    SchemaEvidenceRejected,
    FrameInvalid,
}

#[derive(Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum PrepareFailureWire {
    StoreOpenFailed {},
    StoreSchemaNewerThanImage {
        store: StoreKind,
    },
    StoreMigrationHistoryInvalid {
        store: StoreKind,
        reason: MigrationHistoryDefect,
    },
    SecretStoreUnavailable {},
    ListenerGrantInvalid {},
    SchemaEvidenceRejected {},
    FrameInvalid {},
}

impl From<PrepareFailureWire> for PrepareFailure {
    fn from(wire: PrepareFailureWire) -> Self {
        match wire {
            PrepareFailureWire::StoreOpenFailed {} => Self::StoreOpenFailed,
            PrepareFailureWire::StoreSchemaNewerThanImage { store } => {
                Self::StoreSchemaNewerThanImage { store }
            }
            PrepareFailureWire::StoreMigrationHistoryInvalid { store, reason } => {
                Self::StoreMigrationHistoryInvalid { store, reason }
            }
            PrepareFailureWire::SecretStoreUnavailable {} => Self::SecretStoreUnavailable,
            PrepareFailureWire::ListenerGrantInvalid {} => Self::ListenerGrantInvalid,
            PrepareFailureWire::SchemaEvidenceRejected {} => Self::SchemaEvidenceRejected,
            PrepareFailureWire::FrameInvalid {} => Self::FrameInvalid,
        }
    }
}

impl From<PrepareFailure> for PrepareFailureWire {
    fn from(value: PrepareFailure) -> Self {
        match value {
            PrepareFailure::StoreOpenFailed => Self::StoreOpenFailed {},
            PrepareFailure::StoreSchemaNewerThanImage { store } => {
                Self::StoreSchemaNewerThanImage { store }
            }
            PrepareFailure::StoreMigrationHistoryInvalid { store, reason } => {
                Self::StoreMigrationHistoryInvalid { store, reason }
            }
            PrepareFailure::SecretStoreUnavailable => Self::SecretStoreUnavailable {},
            PrepareFailure::ListenerGrantInvalid => Self::ListenerGrantInvalid {},
            PrepareFailure::SchemaEvidenceRejected => Self::SchemaEvidenceRejected {},
            PrepareFailure::FrameInvalid => Self::FrameInvalid {},
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum StoreKind {
    ProjectBoard,
    Automation,
    ProviderOperations,
    RouterState,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum MigrationHistoryDefect {
    DirtyMigration,
    ChecksumMismatch,
    InvalidAppliedOrder,
    UnknownAppliedMigration,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum EvidenceRejection {
    ExecutableMismatch,
    DigestMismatch,
    BundleUnreadable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum ChildComponent {
    Board,
    Delivery,
    Automation,
    Schedules,
    Mcp,
    AcpChannel,
    NativeRelay,
    Providers,
    CodexTurnAdoption,
    PooledCredentials,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(from = "ChildDegradationWire", into = "ChildDegradationWire")]
pub enum ChildDegradation {
    StoreUnavailable,
    SchemaMismatch,
    SchemaUnavailable,
    NoCurrentGeneration,
    ProviderUnavailable,
    AdoptionPending { turns: u32 },
    CredentialStoreUnavailable,
}

#[derive(Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum ChildDegradationWire {
    StoreUnavailable {},
    SchemaMismatch {},
    SchemaUnavailable {},
    NoCurrentGeneration {},
    ProviderUnavailable {},
    AdoptionPending { turns: u32 },
    CredentialStoreUnavailable {},
}

impl From<ChildDegradationWire> for ChildDegradation {
    fn from(wire: ChildDegradationWire) -> Self {
        match wire {
            ChildDegradationWire::StoreUnavailable {} => Self::StoreUnavailable,
            ChildDegradationWire::SchemaMismatch {} => Self::SchemaMismatch,
            ChildDegradationWire::SchemaUnavailable {} => Self::SchemaUnavailable,
            ChildDegradationWire::NoCurrentGeneration {} => Self::NoCurrentGeneration,
            ChildDegradationWire::ProviderUnavailable {} => Self::ProviderUnavailable,
            ChildDegradationWire::AdoptionPending { turns } => Self::AdoptionPending { turns },
            ChildDegradationWire::CredentialStoreUnavailable {} => Self::CredentialStoreUnavailable,
        }
    }
}

impl From<ChildDegradation> for ChildDegradationWire {
    fn from(value: ChildDegradation) -> Self {
        match value {
            ChildDegradation::StoreUnavailable => Self::StoreUnavailable {},
            ChildDegradation::SchemaMismatch => Self::SchemaMismatch {},
            ChildDegradation::SchemaUnavailable => Self::SchemaUnavailable {},
            ChildDegradation::NoCurrentGeneration => Self::NoCurrentGeneration {},
            ChildDegradation::ProviderUnavailable => Self::ProviderUnavailable {},
            ChildDegradation::AdoptionPending { turns } => Self::AdoptionPending { turns },
            ChildDegradation::CredentialStoreUnavailable => Self::CredentialStoreUnavailable {},
        }
    }
}
