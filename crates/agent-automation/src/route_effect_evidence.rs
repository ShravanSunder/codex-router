//! Evidence identifies the client that may reconcile an uncertain side effect.
use crate::{AttemptId, CessationEvidence, NativeEffectEvidence, SubmissionEffect};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize};
use std::borrow::Cow;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RouteEffectEvidence<TTarget, TGeneration> {
    CodexAppServer(NativeEffectEvidence<TTarget, TGeneration>),
    ProviderAcp(ProviderAcpEffectEvidence<TTarget, TGeneration>),
    ClaudeCodePeer(ClaudeCodePeerEffectEvidence),
}

/// The completion evidence available for one selected run route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteSettlementState {
    Pending,
    NativeTurnConfirmed,
    ProviderOperationConfirmed,
    PeerMessageWritten,
}

impl<TTarget, TGeneration> RouteEffectEvidence<TTarget, TGeneration> {
    #[must_use]
    pub fn settlement_state(&self) -> RouteSettlementState {
        match self {
            Self::CodexAppServer(native) if native.cessation == CessationEvidence::Confirmed => {
                RouteSettlementState::NativeTurnConfirmed
            }
            Self::ProviderAcp(provider)
                if provider.settlement == ProviderSettlementEffect::Confirmed =>
            {
                RouteSettlementState::ProviderOperationConfirmed
            }
            Self::ClaudeCodePeer(peer) if peer.write == PeerWriteEffect::Written => {
                RouteSettlementState::PeerMessageWritten
            }
            _ => RouteSettlementState::Pending,
        }
    }

    #[must_use]
    pub fn is_valid(&self) -> bool {
        match self {
            Self::CodexAppServer(_) | Self::ClaudeCodePeer(_) => true,
            Self::ProviderAcp(provider) => provider.is_valid(),
        }
    }

    #[must_use]
    pub fn codex_app_server(&self) -> Option<&NativeEffectEvidence<TTarget, TGeneration>> {
        match self {
            Self::CodexAppServer(native) => Some(native),
            Self::ProviderAcp(_) | Self::ClaudeCodePeer(_) => None,
        }
    }

    #[must_use]
    pub fn codex_app_server_mut(
        &mut self,
    ) -> Option<&mut NativeEffectEvidence<TTarget, TGeneration>> {
        match self {
            Self::CodexAppServer(native) => Some(native),
            Self::ProviderAcp(_) | Self::ClaudeCodePeer(_) => None,
        }
    }
}

impl<TTarget: PartialEq, TGeneration: PartialEq> RouteEffectEvidence<TTarget, TGeneration> {
    #[must_use]
    pub fn same_client_identity(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::CodexAppServer(before), Self::CodexAppServer(after)) => {
                before.target == after.target
                    && before.generation == after.generation
                    && before.client_user_message_id == after.client_user_message_id
            }
            (Self::ProviderAcp(before), Self::ProviderAcp(after)) => {
                before.target == after.target
                    && before.generation == after.generation
                    && before.binding == after.binding
                    && before.attempt_id == after.attempt_id
            }
            (Self::ClaudeCodePeer(before), Self::ClaudeCodePeer(after)) => {
                before.session_id == after.session_id && before.process_id == after.process_id
            }
            _ => false,
        }
    }
}

impl<TTarget, TGeneration> From<NativeEffectEvidence<TTarget, TGeneration>>
    for RouteEffectEvidence<TTarget, TGeneration>
{
    fn from(native: NativeEffectEvidence<TTarget, TGeneration>) -> Self {
        Self::CodexAppServer(native)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderAcpEffectEvidence<TTarget, TGeneration> {
    pub target: TTarget,
    pub generation: TGeneration,
    pub binding: ProviderBindingReference,
    pub attempt_id: AttemptId,
    pub submission: SubmissionEffect,
    pub settlement: ProviderSettlementEffect,
}

impl<TTarget, TGeneration> ProviderAcpEffectEvidence<TTarget, TGeneration> {
    #[must_use]
    pub fn is_valid(&self) -> bool {
        match self.settlement {
            ProviderSettlementEffect::NotObserved => true,
            ProviderSettlementEffect::StopRequested | ProviderSettlementEffect::Confirmed => {
                self.submission == SubmissionEffect::Accepted
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderAcpEffectFields<TTarget, TGeneration> {
    target: TTarget,
    generation: TGeneration,
    binding: ProviderBindingReference,
    attempt_id: AttemptId,
    submission: SubmissionEffect,
    settlement: ProviderSettlementEffect,
}

impl<'de, TTarget, TGeneration> Deserialize<'de> for ProviderAcpEffectEvidence<TTarget, TGeneration>
where
    TTarget: Deserialize<'de>,
    TGeneration: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let fields = ProviderAcpEffectFields::deserialize(deserializer)?;
        let evidence = Self {
            target: fields.target,
            generation: fields.generation,
            binding: fields.binding,
            attempt_id: fields.attempt_id,
            submission: fields.submission,
            settlement: fields.settlement,
        };
        if evidence.is_valid() {
            Ok(evidence)
        } else {
            Err(serde::de::Error::custom(
                "provider settlement requires an accepted submission",
            ))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderSettlementEffect {
    NotObserved,
    StopRequested,
    Confirmed,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProviderBindingReference(String);

#[derive(Debug, thiserror::Error)]
#[error("provider binding reference requires 1 to 256 UTF-8 bytes without NUL")]
pub struct ProviderBindingReferenceError;

impl TryFrom<String> for ProviderBindingReference {
    type Error = ProviderBindingReferenceError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if (1..=256).contains(&value.len()) && !value.contains('\0') {
            Ok(Self(value))
        } else {
            Err(ProviderBindingReferenceError)
        }
    }
}

impl From<ProviderBindingReference> for String {
    fn from(value: ProviderBindingReference) -> Self {
        value.0
    }
}

impl ProviderBindingReference {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PeerWriteEffect {
    NotDispatched,
    Dispatching,
    Written,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct PeerProcessId(u32);

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PeerSessionReference(String);

impl JsonSchema for PeerSessionReference {
    fn schema_name() -> Cow<'static, str> {
        "PeerSessionReference".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"type":"string", "minLength":1, "maxLength":4096})
    }
}

#[derive(Debug, thiserror::Error)]
#[error("peer session reference requires 1 to 4096 UTF-8 bytes without NUL")]
pub struct PeerSessionReferenceError;

impl TryFrom<String> for PeerSessionReference {
    type Error = PeerSessionReferenceError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if (1..=4096).contains(&value.len()) && !value.contains('\0') {
            Ok(Self(value))
        } else {
            Err(PeerSessionReferenceError)
        }
    }
}

impl From<PeerSessionReference> for String {
    fn from(value: PeerSessionReference) -> Self {
        value.0
    }
}

impl PeerSessionReference {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
#[error("peer process ID must be positive")]
pub struct PeerProcessIdError;

impl TryFrom<u32> for PeerProcessId {
    type Error = PeerProcessIdError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(PeerProcessIdError)
        }
    }
}

impl From<PeerProcessId> for u32 {
    fn from(value: PeerProcessId) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeCodePeerEffectEvidence {
    pub session_id: PeerSessionReference,
    pub process_id: PeerProcessId,
    pub write: PeerWriteEffect,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum TaggedRouteEffectEvidence<TTarget, TGeneration> {
    CodexAppServer(NativeEffectEvidence<TTarget, TGeneration>),
    ProviderAcp(ProviderAcpEffectEvidence<TTarget, TGeneration>),
    ClaudeCodePeer(ClaudeCodePeerEffectEvidence),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum DecodedRouteEffectEvidence<TTarget, TGeneration> {
    Tagged(TaggedRouteEffectEvidence<TTarget, TGeneration>),
    LegacyNative(NativeEffectEvidence<TTarget, TGeneration>),
}

impl<'de, TTarget, TGeneration> Deserialize<'de> for RouteEffectEvidence<TTarget, TGeneration>
where
    TTarget: Deserialize<'de>,
    TGeneration: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(
            match DecodedRouteEffectEvidence::deserialize(deserializer)? {
                DecodedRouteEffectEvidence::Tagged(tagged) => match tagged {
                    TaggedRouteEffectEvidence::CodexAppServer(native) => {
                        Self::CodexAppServer(native)
                    }
                    TaggedRouteEffectEvidence::ProviderAcp(provider) => Self::ProviderAcp(provider),
                    TaggedRouteEffectEvidence::ClaudeCodePeer(peer) => Self::ClaudeCodePeer(peer),
                },
                DecodedRouteEffectEvidence::LegacyNative(native) => Self::CodexAppServer(native),
            },
        )
    }
}
