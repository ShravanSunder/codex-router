//! The single private wire contract shared by the caller and source Sessions process.
use super::router_connection_registry::{
    AbsoluteRemoteCwd, AbsoluteSourcePath, RouterConnectionProfile, SourceClientProfile,
};
use codex_native_integration::ResumeModelChoice;
use collaboration_client::protocol::{
    CodexGeneration, EndpointInventory, EndpointRef, NativeSessionListParams,
    NativeSessionListResult, ProviderSessionListParams, ProviderSessionListResult, SchemaDigest,
    SessionRef, UuidIdentity,
};
use serde::{Deserialize, Serialize};

pub(crate) const SOURCE_REQUEST_SWITCH: &str = "--source-request-v1";
pub(crate) const MAX_SOURCE_REQUEST_BYTES: usize = 65536;
pub(crate) const MAX_SOURCE_REPLY_BYTES: usize =
    4 * collaboration_client::protocol::MAX_CONTROL_FRAME_BYTES;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceRequest {
    pub(crate) service_directory: AbsoluteSourcePath,
    pub(crate) expected_service: UuidIdentity,
    pub(crate) operation: SourceOperation,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum SourceOperation {
    Endpoints,
    NativeSessions {
        params: NativeSessionListParams,
    },
    ProviderSessions {
        params: ProviderSessionListParams,
    },
    Prepare {
        action: SourceSessionAction,
        client_profile: SourceClientProfile,
    },
    Launch {
        prepared: Box<PreparedSourceLaunch>,
        options: SourceNativeOptions,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum SourceReply {
    Endpoints { inventory: EndpointInventory },
    NativeSessions { page: NativeSessionListResult },
    ProviderSessions { page: ProviderSessionListResult },
    Prepared { launch: Box<PreparedSourceLaunch> },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum SourceResponse {
    Ready { reply: SourceReply },
    Rejected { reason: SourceRejection },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum SourceSessionAction {
    New {
        directory: AbsoluteRemoteCwd,
    },
    Resume {
        target: SessionRef,
        stored_model: SourceStoredModel,
    },
    Fork {
        target: SessionRef,
        stored_model: SourceStoredModel,
    },
}

impl SourceSessionAction {
    pub(crate) fn target(&self) -> Option<&SessionRef> {
        match self {
            Self::New { .. } => None,
            Self::Resume { target, .. } | Self::Fork { target, .. } => Some(target),
        }
    }

    pub(crate) fn validate(&self, service: &UuidIdentity) -> Result<(), SourceRejection> {
        match self {
            Self::New { .. } => Ok(()),
            Self::Resume {
                target,
                stored_model,
            }
            | Self::Fork {
                target,
                stored_model,
            } => {
                if target.endpoint.service_id != *service {
                    return Err(SourceRejection::WrongService);
                }
                stored_model.validate()
            }
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceStoredModel {
    pub(crate) model: Option<String>,
    pub(crate) reasoning_effort: Option<String>,
}

impl SourceStoredModel {
    pub(crate) fn from_choice(choice: &ResumeModelChoice) -> Self {
        Self {
            model: choice.stored_model().map(str::to_owned),
            reasoning_effort: choice.stored_reasoning_effort().map(str::to_owned),
        }
    }

    pub(crate) fn validate(&self) -> Result<(), SourceRejection> {
        validate_model_values(self.model.as_deref(), self.reasoning_effort.as_deref())
    }

    pub(crate) fn native_choice(&self) -> ResumeModelChoice {
        ResumeModelChoice::from_stored_values(
            self.model.as_deref(),
            self.reasoning_effort.as_deref(),
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PreparedSourceLaunch {
    pub(crate) action: SourceSessionAction,
    pub(crate) endpoint: EndpointRef,
    pub(crate) generation: CodexGeneration,
    pub(crate) schema_digest: SchemaDigest,
    pub(crate) working_directory: AbsoluteRemoteCwd,
    pub(crate) client_profile: SourceClientProfile,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceNativeOptions {
    pub(crate) model: Option<String>,
    pub(crate) reasoning_effort: Option<String>,
    pub(crate) no_alt_screen: bool,
}

impl SourceNativeOptions {
    pub(crate) fn parse(arguments: &[std::ffi::OsString]) -> Result<Self, SourceRejection> {
        super::source_native_options::parse(arguments)
    }

    pub(crate) fn validate(&self) -> Result<(), SourceRejection> {
        validate_model_values(self.model.as_deref(), self.reasoning_effort.as_deref())
    }
}

fn validate_model_values(model: Option<&str>, effort: Option<&str>) -> Result<(), SourceRejection> {
    if [model, effort]
        .into_iter()
        .flatten()
        .any(|value| value.trim().is_empty() || value.len() > 1024)
        || ResumeModelChoice::rejects_stored_value(model, effort)
    {
        return Err(SourceRejection::ConflictingArguments);
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparedSourceSession {
    pub(crate) profile: RouterConnectionProfile,
    pub(crate) launch: PreparedSourceLaunch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SourceRejection {
    #[error("source request is invalid")]
    InvalidRequest,
    #[error("source connection is unsupported")]
    UnsupportedConnection,
    #[error("selected Router service does not match")]
    WrongService,
    #[error("selected native endpoint is unavailable")]
    UnavailableEndpoint,
    #[error("selected native connection changed; choose again")]
    StaleBinding,
    #[error("source native channel is invalid")]
    InvalidNativeChannel,
    #[error("source session is unavailable")]
    SessionUnavailable,
    #[error("source working directory is unavailable")]
    DirectoryUnavailable,
    #[error("source profile contains a permission override")]
    ProfilePermissionConflict,
    #[error("configured launch accepts only model, effort and terminal display options")]
    ConflictingArguments,
    #[error("source Router is unavailable")]
    SourceUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum SourceReadError {
    #[error("{0}")]
    Rejected(SourceRejection),
    #[error("trusted source SSH read is unavailable")]
    ReadUnavailable,
    #[error("source reply is invalid")]
    InvalidReply,
}
