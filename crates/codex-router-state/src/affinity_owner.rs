//! Previous-response affinity owner records.

use codex_router_core::affinity::AffinityKeyHash;
use codex_router_core::ids::AccountId;
use codex_router_core::routes::RouteBand;

/// Transport that produced an affinity owner record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AffinitySourceTransport {
    /// HTTP/SSE `/v1/responses` response.
    HttpSse,
    /// WebSocket `/v1/responses` response frame.
    WebSocket,
}

impl AffinitySourceTransport {
    /// Serializes transport to SQLite.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HttpSse => "http_sse",
            Self::WebSocket => "websocket",
        }
    }

    /// Parses transport from SQLite.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "http_sse" => Some(Self::HttpSse),
            "websocket" => Some(Self::WebSocket),
            _ => None,
        }
    }
}

/// Durable owner metadata for a hashed previous-response id.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviousResponseAffinityOwnerRecord {
    affinity_key_hash: AffinityKeyHash,
    account_id: AccountId,
    credential_generation: u64,
    route_band: RouteBand,
    source_transport: AffinitySourceTransport,
    created_unix_seconds: u64,
}

impl PreviousResponseAffinityOwnerRecord {
    /// Creates a previous-response affinity owner record.
    #[must_use]
    pub const fn new(
        affinity_key_hash: AffinityKeyHash,
        account_id: AccountId,
        credential_generation: u64,
        route_band: RouteBand,
        source_transport: AffinitySourceTransport,
        created_unix_seconds: u64,
    ) -> Self {
        Self {
            affinity_key_hash,
            account_id,
            credential_generation,
            route_band,
            source_transport,
            created_unix_seconds,
        }
    }

    /// Returns affinity key hash.
    #[must_use]
    pub const fn affinity_key_hash(&self) -> &AffinityKeyHash {
        &self.affinity_key_hash
    }

    /// Returns account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns credential generation.
    #[must_use]
    pub const fn credential_generation(&self) -> u64 {
        self.credential_generation
    }

    /// Returns route band.
    #[must_use]
    pub const fn route_band(&self) -> RouteBand {
        self.route_band
    }

    /// Returns source transport.
    #[must_use]
    pub const fn source_transport(&self) -> AffinitySourceTransport {
        self.source_transport
    }

    /// Returns creation time.
    #[must_use]
    pub const fn created_unix_seconds(&self) -> u64 {
        self.created_unix_seconds
    }
}

/// Owner lookup result for a hashed previous-response id.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PreviousResponseAffinityOwnerLookup {
    /// No matching owner row exists.
    Missing,
    /// Exactly one matching owner row exists.
    Found(PreviousResponseAffinityOwnerRecord),
    /// More than one matching owner exists; callers must fail closed.
    Ambiguous,
}
