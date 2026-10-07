use codex_native_integration::RemoteControlServerName;
use serde::{Deserialize, Serialize};

use crate::{EndpointPathError, GenerationAliasPath, GenerationEvidence, GenerationId};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    try_from = "GenerationCurrentPayloadWire",
    into = "GenerationCurrentPayloadWire"
)]
pub struct GenerationCurrentPayload {
    generation: GenerationId,
    alias: GenerationAliasPath,
    evidence: GenerationEvidence,
    server_display_name: Option<RemoteControlServerName>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GenerationCurrentPayloadWire {
    generation: GenerationId,
    alias: GenerationAliasPath,
    evidence: GenerationEvidence,
    server_display_name: Option<RemoteControlServerName>,
}

impl GenerationCurrentPayload {
    pub fn new(
        generation: GenerationId,
        alias: GenerationAliasPath,
        evidence: GenerationEvidence,
        server_display_name: Option<RemoteControlServerName>,
    ) -> Result<Self, EndpointPathError> {
        alias.validate_for_generation(&generation)?;
        Ok(Self {
            generation,
            alias,
            evidence,
            server_display_name,
        })
    }

    pub fn generation(&self) -> &GenerationId {
        &self.generation
    }

    pub fn alias(&self) -> &GenerationAliasPath {
        &self.alias
    }

    pub fn evidence(&self) -> &GenerationEvidence {
        &self.evidence
    }

    pub fn server_display_name(&self) -> Option<&RemoteControlServerName> {
        self.server_display_name.as_ref()
    }
}

impl TryFrom<GenerationCurrentPayloadWire> for GenerationCurrentPayload {
    type Error = EndpointPathError;

    fn try_from(value: GenerationCurrentPayloadWire) -> Result<Self, Self::Error> {
        Self::new(
            value.generation,
            value.alias,
            value.evidence,
            value.server_display_name,
        )
    }
}

impl From<GenerationCurrentPayload> for GenerationCurrentPayloadWire {
    fn from(value: GenerationCurrentPayload) -> Self {
        Self {
            generation: value.generation,
            alias: value.alias,
            evidence: value.evidence,
            server_display_name: value.server_display_name,
        }
    }
}
