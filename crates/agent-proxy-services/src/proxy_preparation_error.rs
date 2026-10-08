use codex_router_keeper_protocol::{MigrationHistoryDefect, PrepareFailure, StoreKind};
use codex_router_state::schema_preparation::StateSchemaPreparationError;
#[derive(Debug, thiserror::Error)]
pub enum ProxyPreparationError {
    #[error(transparent)]
    Secret(#[from] codex_router_secret_store::model::SecretStoreError),
    #[error(transparent)]
    Token(#[from] codex_router_secret_store::local_router_token::LocalRouterTokenError),
    #[error(transparent)]
    Schema(#[from] StateSchemaPreparationError),
    #[error(transparent)]
    Core(#[from] codex_router_proxy::server::LoopbackRouterRuntimeError),
    #[error("existing credentials do not meet the active proxy's degradation allowance")]
    CredentialsUnavailable,
    #[error("state preparation path could not be inspected")]
    StateInspection(#[source] std::io::Error),
    #[error("prepare parameters do not belong to the proxy role")]
    InvalidParameters,
    #[error("proxy preparation exceeded its deadline")]
    Deadline,
    #[error("proxy credential preparation task failed")]
    CredentialTask(#[source] tokio::task::JoinError),
}
impl ProxyPreparationError {
    #[must_use]
    pub fn prepare_failure(&self) -> PrepareFailure {
        match self {
            Self::Secret(_)
            | Self::Token(_)
            | Self::CredentialsUnavailable
            | Self::CredentialTask(_)
            | Self::Deadline => PrepareFailure::SecretStoreUnavailable,
            Self::StateInspection(_) => PrepareFailure::StoreOpenFailed,
            Self::InvalidParameters => PrepareFailure::FrameInvalid,
            Self::Core(_) => PrepareFailure::ListenerGrantInvalid,
            Self::Schema(StateSchemaPreparationError::SchemaNewerThanImage) => {
                PrepareFailure::StoreSchemaNewerThanImage {
                    store: StoreKind::RouterState,
                }
            }
            Self::Schema(error) => match error {
                StateSchemaPreparationError::DirtyMigration => {
                    PrepareFailure::StoreMigrationHistoryInvalid {
                        store: StoreKind::RouterState,
                        reason: MigrationHistoryDefect::DirtyMigration,
                    }
                }
                StateSchemaPreparationError::ChecksumMismatch => {
                    PrepareFailure::StoreMigrationHistoryInvalid {
                        store: StoreKind::RouterState,
                        reason: MigrationHistoryDefect::ChecksumMismatch,
                    }
                }
                StateSchemaPreparationError::InvalidAppliedOrder => {
                    PrepareFailure::StoreMigrationHistoryInvalid {
                        store: StoreKind::RouterState,
                        reason: MigrationHistoryDefect::InvalidAppliedOrder,
                    }
                }
                StateSchemaPreparationError::UnknownAppliedMigration => {
                    PrepareFailure::StoreMigrationHistoryInvalid {
                        store: StoreKind::RouterState,
                        reason: MigrationHistoryDefect::UnknownAppliedMigration,
                    }
                }
                _ => PrepareFailure::StoreOpenFailed,
            },
        }
    }
}
