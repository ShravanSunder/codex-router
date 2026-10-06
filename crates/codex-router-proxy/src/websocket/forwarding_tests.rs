#[path = "forwarding_tests/affinity_activity_tests.rs"]
mod affinity_activity_tests;
#[path = "forwarding_tests/alternative_selection_tests.rs"]
mod alternative_selection_tests;
#[path = "forwarding_tests/credit-included-peer-transition-tests.rs"]
mod credit_included_peer_transition_tests;
#[path = "forwarding_tests/credit-source-assessment-tests.rs"]
mod credit_source_assessment_tests;
#[path = "forwarding_tests/credit-turn-admission-tests.rs"]
mod credit_turn_admission_tests;
#[path = "forwarding_tests/credit-turn-depletion-tests.rs"]
mod credit_turn_depletion_tests;
#[path = "forwarding_tests/credit-turn-source-transition-tests.rs"]
mod credit_turn_source_transition_tests;
#[path = "forwarding_tests/credit-turn-test-support.rs"]
mod credit_turn_test_support;
#[path = "forwarding_tests/credit-turn-unknown-transition-tests.rs"]
mod credit_turn_unknown_transition_tests;
#[path = "forwarding_tests/floor-switch-supervisor-tests.rs"]
mod floor_switch_supervisor_tests;
#[path = "forwarding_tests/floor-switch-terminal-tests.rs"]
mod floor_switch_terminal_tests;
#[path = "forwarding_tests/floor-switch-tests.rs"]
mod floor_switch_tests;
#[path = "forwarding_tests/floor-switch-write-readiness-tests.rs"]
mod floor_switch_write_readiness_tests;
#[path = "forwarding_tests/handshake-cancellation-tests.rs"]
mod handshake_cancellation_tests;
#[path = "forwarding_tests/handshake_outcome_tests.rs"]
mod handshake_outcome_tests;
#[path = "forwarding_tests/handshake_record_fixtures.rs"]
mod handshake_record_fixtures;
#[path = "forwarding_tests/idle_close_tests.rs"]
mod idle_close_tests;
#[path = "forwarding_tests/metadata_boundary_tests.rs"]
mod metadata_boundary_tests;
#[path = "forwarding_tests/provider_observer_fixtures.rs"]
mod provider_observer_fixtures;
#[path = "forwarding_tests/quota_frame_tests.rs"]
mod quota_frame_tests;
#[path = "forwarding_tests/quota_persistence_tests.rs"]
mod quota_persistence_tests;
#[path = "forwarding_tests/shutdown-admission-readiness-tests.rs"]
mod shutdown_admission_readiness_tests;
#[path = "forwarding_tests/shutdown_floor_tests.rs"]
mod shutdown_floor_tests;
#[path = "forwarding_tests/turn_reservation_tests.rs"]
mod turn_reservation_tests;
#[path = "forwarding_tests/unavailable_authority_tests.rs"]
mod unavailable_authority_tests;

use super::ActiveTurnReservationState;
use super::AsyncWebSocketTunnel;
use super::PostExhaustionRouteBandOutcome;
use super::TokenGeneration;
use super::UpstreamToLocalPumpContext;
use super::WebSocketAffinityOwnerContext;
use super::WebSocketFrame;
use super::WebSocketHandshakeRequest;
use super::WebSocketProtocolRouter;
use super::WebSocketTunnelError;
use super::account_turn_admission::AccountTurnAdmission;
use super::account_turn_admission::FloorSwitchIntent;
use super::current_unix_seconds;
use super::duplex_forwarding::LocalToUpstreamPumpContext;
use super::duplex_forwarding::WebSocketForwardingContext;
use super::duplex_forwarding::forward_duplex_until_complete;
use super::duplex_forwarding::pump_local_to_upstream;
use super::duplex_forwarding::supervise_websocket_pumps;
use super::provider_signals::CODEX_WEBSOCKET_RECONNECT_SIGNAL;
use super::provider_signals::{
    ROUTER_ALL_ACCOUNTS_EXHAUSTED_SIGNAL, ROUTER_QUOTA_STATE_UNAVAILABLE_SIGNAL,
};
use super::response_metadata::is_response_completed;
use super::response_metadata::is_response_create;
use super::response_metadata::provider_error_classification_from_message;
use super::response_metadata::record_forwarded_websocket_metadata;
use super::response_metadata::websocket_affinity_owner_record;
use super::session_registry::WebSocketQuotaFloorNotifier;
use super::session_registry::WebSocketRevocationRegistry;
use crate::account_selection::FloorSwitchPeerAssessment;
use crate::account_selection::LiveAccountAdmissionAssessor;
use bytes::Bytes;
use codex_router_auth::resolver::CredentialResolverError;
use codex_router_auth::resolver::ResolvedProviderCredential;
use codex_router_core::affinity::RouterAffinityHashSecret;
use codex_router_core::ids::AccountId;
use codex_router_core::ids::TokenGeneration as LocalTokenGeneration;
use codex_router_core::provider::Provider;
use codex_router_core::redaction::SecretString;
use codex_router_core::route_profile::RESPONSES_WEBSOCKET;
use futures_util::SinkExt;
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::io::duplex;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::sync::watch;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::account_selection::ActiveReservationGuard;
use crate::account_selection::AsyncAccountDecisionSelector;
use crate::account_selection::QuotaAwareAccountSelectorError;
use crate::account_selection::RouteBandReservationBooks;
use crate::account_selection::SelectedAccountDecision;
use crate::db_write_actor::DbWriteEnqueueResult;
use crate::http_sse::AsyncHttpAffinityOwnerRecorder;
use crate::http_sse::AsyncProviderCredentialResolver;
use crate::http_sse::HttpAffinitySecretProvider;
use crate::http_sse::HttpProxyError;
use crate::http_sse::HttpProxyRequest;
use crate::local_auth::ProxyLocalAuthGate;
use crate::provider_error::AsyncProviderErrorObserver;
use crate::provider_error::ProviderErrorClassification;
use crate::provider_error::ProviderErrorObservationError;
use crate::session_account_affinity_cache::SessionAccountAffinityCache;
use crate::session_account_affinity_cache::lookup_session_account_affinity;
use crate::session_account_affinity_cache::publish_session_account_affinity;
use codex_router_selection::reservation::ReservationBook;
use codex_router_state::affinity_owner::PreviousResponseAffinityOwnerRecord;

use self::handshake_record_fixtures::*;
use self::provider_observer_fixtures::*;
