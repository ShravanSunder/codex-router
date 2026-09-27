//! Boundary identity for conversation creation across native and provider clients.

use crate::conversation_client::{ConversationClientError, ConversationCreateInput, unsupported};
use collaboration_protocol::{ProviderIdentity, SessionRef};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Accepts the established SessionRef shape and typed Human or Session identities.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, JsonSchema, Serialize)]
#[serde(untagged)]
pub enum ConversationCreateActor {
    Session(SessionRef),
    Typed(message_board::Identity),
}

impl From<SessionRef> for ConversationCreateActor {
    fn from(session: SessionRef) -> Self {
        Self::Session(session)
    }
}

impl ConversationCreateActor {
    pub(crate) fn session(&self) -> Result<Option<SessionRef>, ConversationClientError> {
        Ok(self.provider_identity()?.session().cloned())
    }

    pub(crate) fn provider_identity(&self) -> Result<ProviderIdentity, ConversationClientError> {
        match self {
            Self::Session(session) => Ok(session.clone().into()),
            Self::Typed(identity) => ProviderIdentity::from_board_identity(identity)
                .map_err(ConversationClientError::InvalidInput),
        }
    }
}

pub(crate) fn codex_create_actors(
    input: &ConversationCreateInput,
) -> Result<(SessionRef, Option<SessionRef>), ConversationClientError> {
    let provider_only = "Human creators and Approvers are supported for provider endpoints only";
    let created_by = input
        .created_by
        .session()?
        .ok_or_else(|| unsupported(&input.endpoint, "createdBy", provider_only))?;
    let approver = input
        .approver
        .as_ref()
        .map(|actor| {
            actor
                .session()?
                .ok_or_else(|| unsupported(&input.endpoint, "approver", provider_only))
        })
        .transpose()?;
    Ok((created_by, approver))
}
