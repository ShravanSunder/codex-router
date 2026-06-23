//! Signed turn-state envelopes.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use codex_router_core::ids::AccountId;
use codex_router_core::redaction::SecretString;
use hmac::Hmac;
use hmac::Mac;
use serde::Deserialize;
use serde::Serialize;
use sha2::Sha256;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

/// Opaque signed turn-state envelope.
#[derive(Clone, Eq, PartialEq)]
pub struct TurnStateEnvelope(String);

impl TurnStateEnvelope {
    /// Returns the opaque envelope.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for TurnStateEnvelope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("TurnStateEnvelope([REDACTED])")
    }
}

/// Decoded turn-state data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedTurnState {
    account_id: AccountId,
    upstream_token: Option<SecretString>,
}

impl DecodedTurnState {
    /// Returns the pinned account.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns optional upstream token material.
    #[must_use]
    pub const fn upstream_token(&self) -> Option<&SecretString> {
        self.upstream_token.as_ref()
    }
}

/// Turn-state codec failure.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum TurnStateError {
    /// Envelope shape is invalid.
    #[error("turn-state envelope is malformed")]
    Malformed,
    /// Signature is invalid.
    #[error("turn-state envelope signature is invalid")]
    InvalidSignature,
    /// Payload could not be decoded.
    #[error("turn-state envelope payload is invalid")]
    InvalidPayload,
}

/// HMAC-based turn-state envelope codec.
#[derive(Clone, Debug)]
pub struct TurnStateEnvelopeCodec {
    signing_key: SecretString,
}

impl TurnStateEnvelopeCodec {
    /// Creates a codec with router-owned signing key material.
    #[must_use]
    pub fn new(signing_key: SecretString) -> Self {
        Self { signing_key }
    }

    /// Encodes and signs a turn-state payload.
    pub fn encode(
        &self,
        account_id: &AccountId,
        upstream_token: Option<SecretString>,
    ) -> Result<TurnStateEnvelope, TurnStateError> {
        let payload = TurnStatePayload {
            account_id: account_id.as_str().to_owned(),
            upstream_token: upstream_token.map(|token| token.expose_secret().to_owned()),
        };
        let payload_json =
            serde_json::to_vec(&payload).map_err(|_| TurnStateError::InvalidPayload)?;
        let payload_segment = URL_SAFE_NO_PAD.encode(payload_json);
        let signature_segment = self.sign_segment(&payload_segment)?;

        Ok(TurnStateEnvelope(format!(
            "{payload_segment}.{signature_segment}"
        )))
    }

    /// Decodes and verifies an envelope.
    pub fn decode(&self, envelope: &TurnStateEnvelope) -> Result<DecodedTurnState, TurnStateError> {
        self.decode_str(envelope.as_str())
    }

    /// Decodes and verifies an envelope string.
    pub fn decode_str(&self, envelope: &str) -> Result<DecodedTurnState, TurnStateError> {
        let (payload_segment, signature_segment) =
            envelope.split_once('.').ok_or(TurnStateError::Malformed)?;
        self.verify_segment(payload_segment, signature_segment)?;
        let payload_bytes = URL_SAFE_NO_PAD
            .decode(payload_segment)
            .map_err(|_| TurnStateError::InvalidPayload)?;
        let payload: TurnStatePayload =
            serde_json::from_slice(&payload_bytes).map_err(|_| TurnStateError::InvalidPayload)?;
        let account_id =
            AccountId::new(payload.account_id).map_err(|_| TurnStateError::InvalidPayload)?;
        let upstream_token = payload.upstream_token.map(SecretString::new);

        Ok(DecodedTurnState {
            account_id,
            upstream_token,
        })
    }

    fn sign_segment(&self, payload_segment: &str) -> Result<String, TurnStateError> {
        let mut mac = HmacSha256::new_from_slice(self.signing_key.expose_secret().as_bytes())
            .map_err(|_| TurnStateError::InvalidSignature)?;
        mac.update(payload_segment.as_bytes());
        let signature = mac.finalize().into_bytes();

        Ok(URL_SAFE_NO_PAD.encode(signature))
    }

    fn verify_segment(
        &self,
        payload_segment: &str,
        signature_segment: &str,
    ) -> Result<(), TurnStateError> {
        let signature = URL_SAFE_NO_PAD
            .decode(signature_segment)
            .map_err(|_| TurnStateError::Malformed)?;
        let mut mac = HmacSha256::new_from_slice(self.signing_key.expose_secret().as_bytes())
            .map_err(|_| TurnStateError::InvalidSignature)?;
        mac.update(payload_segment.as_bytes());
        mac.verify_slice(&signature)
            .map_err(|_| TurnStateError::InvalidSignature)
    }
}

#[derive(Deserialize, Serialize)]
struct TurnStatePayload {
    account_id: String,
    upstream_token: Option<String>,
}
