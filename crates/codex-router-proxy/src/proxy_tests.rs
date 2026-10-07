use super::package_name;
use crate::account_selection::AccountDecisionSelector;
use crate::account_selection::AsyncAccountDecisionSelector;
use crate::account_selection::AsyncAccountSelectorRuntimeState;
use crate::account_selection::AsyncRepositoryBackedAccountSelector;
use crate::account_selection::FloorSwitchPeerAssessment;
use crate::account_selection::LiveAccountAdmissionAssessor;
use crate::account_selection::PROMPT_CACHE_ACCOUNT_AFFINITY_IDLE_TTL_SECONDS;
use crate::account_selection::QuotaAwareAccountSelector;
use crate::account_selection::QuotaAwareAccountSelectorError;
use crate::account_selection::QuotaAwareAccountState;
use crate::account_selection::RepositoryBackedAccountSelector;
use crate::account_selection::RouteBandAccountHolds;
use crate::account_selection::RouteBandQueueDegradedReason;
use crate::account_selection::RouteBandQueueHealth;
use crate::account_selection::RouteBandReservationBooks;
use crate::account_selection::RouteBandRuntimeExhaustions;
use crate::account_selection::RouteBandWeightedSelectors;
use crate::account_selection::RuntimeAccountAdmissionAssessor;
use crate::account_selection::SelectedAccountDecision;
use crate::account_selection::mark_route_band_queue_degraded;
use crate::account_selection::mark_runtime_quota_exhausted;
use crate::account_selection::release_account_reservation;
use crate::credential_runtime::ProxyCredentialResolver;
use crate::db_write_actor::DbWriteActor;
use crate::db_write_actor::SqliteDbWriteRepository;
use crate::headers::Header;
use crate::headers::HeaderCollection;
use crate::http_sse::AsyncProviderCredentialResolver;
use crate::http_sse::AuditFailureReporter;
use crate::http_sse::AuthenticatedHttpProxyService;
use crate::http_sse::HttpAffinityOwnerRecorder;
use crate::http_sse::HttpAffinitySecretProvider;
use crate::http_sse::HttpProxyError;
use crate::http_sse::HttpProxyRequest;
use crate::http_sse::HttpProxyResponse;
use crate::http_sse::HttpProxyService;
use crate::http_sse::HttpRequestHandler;
use crate::http_sse::StreamingHttpProxyResponse;
use crate::http_sse::StreamingHttpRequestHandler;
use crate::http_sse::StreamingUpstreamHttpTransport;
use crate::http_sse::UpstreamHttpRequest;
use crate::http_sse::UpstreamHttpTransport;
use crate::http_sse::append_audit_event_with_reporter;
use crate::local_auth::ProxyLocalAuthGate;
use crate::maintenance_actor::MaintenanceCompletion;
use crate::provider_error::classify_provider_error_envelope;
use crate::provider_error::record_provider_error_observation;
use crate::routes::Method;
use crate::routes::RouteClass;
use crate::routes::RouteKind;
use crate::routes::classify_route;
use crate::server::AsyncLoopbackServerRuntime;
use crate::server::HyperProtocolDispatch;
use crate::server::HyperProtocolSwitchpoint;
use crate::server::LoopbackBindAddress;
use crate::server::LoopbackHttpAdapter;
use crate::server::LoopbackHttpServer;
use crate::server::LoopbackRouterRuntime;
use crate::server::LoopbackRouterRuntimeConfig;
use crate::server::LoopbackServerRuntime;
use crate::server::ServerBindError;
use crate::upstream::HttpUpstreamTransport;
use crate::upstream::UpstreamEndpoint;
use crate::upstream::UpstreamRequestBuilder;
use crate::websocket::AsyncAuthenticatedWebSocketRouter;
use crate::websocket::AsyncWebSocketTunnel;
use crate::websocket::AuthenticatedWebSocketRouter;
use crate::websocket::BlockingWebSocketTunnel;
use crate::websocket::WebSocketCloseReason;
use crate::websocket::WebSocketFirstFrameDecision;
use crate::websocket::WebSocketFrame;
use crate::websocket::WebSocketHandshakeRequest;
use crate::websocket::WebSocketProtocolRouter;
use crate::websocket::WebSocketRevocationRegistry;
use bytes::Bytes;
use codex_router_auth::resolver::CredentialRefreshClient;
use codex_router_auth::resolver::CredentialResolverError;
use codex_router_auth::resolver::NoopCredentialRefreshClient;
use codex_router_auth::resolver::ProviderCredentialResolver;
use codex_router_auth::resolver::ResolvedProviderCredential;
use codex_router_auth::resolver::RouterCredentialResolver;
use codex_router_core::affinity::AffinityKeyHash;
use codex_router_core::affinity::PreviousResponseId;
use codex_router_core::affinity::RouterAffinityHashSecret;
use codex_router_core::affinity::hash_previous_response_id;
use codex_router_core::audit::AuditEvent;
use codex_router_core::audit::AuditEventFields;
use codex_router_core::audit::AuditFileSink;
use codex_router_core::audit::AuditOutcome;
use codex_router_core::audit::LocalAuthAuditResult;
use codex_router_core::audit::ResponseCommitState;
use codex_router_core::audit::RouteKind as AuditRouteKind;
use codex_router_core::audit::TransportKind;
use codex_router_core::ids::AccountId;
use codex_router_core::ids::RequestId;
use codex_router_core::ids::ReservationId;
use codex_router_core::ids::TokenGeneration;
use codex_router_core::local_auth::LocalAuthError;
use codex_router_core::local_auth::LocalRouterAuth;
use codex_router_core::local_auth::LocalRouterTokenRecord;
use codex_router_core::provider::Provider;
use codex_router_core::redaction::SecretString;
use codex_router_core::route_profile::RESPONSES_HTTP;
use codex_router_core::routes::RouteBand;
use codex_router_quota::snapshot::SnapshotFreshness;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::account_tokens::openai_account_credential_bundle_key;
use codex_router_secret_store::affinity_secret::load_or_create_router_affinity_hash_secret;
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore;
use codex_router_selection::burn_down::BurnDownRouteBandAssessmentInput;
use codex_router_selection::burn_down::assess_route_band;
use codex_router_selection::reservation::ReservationBook;
use codex_router_selection::reservation::ReservationHandle;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::account_routing_policy::WeeklyQuotaFloorBasisPoints;
use codex_router_state::affinity_owner::AffinitySourceTransport;
use codex_router_state::affinity_owner::PreviousResponseAffinityOwnerLookup;
use codex_router_state::affinity_owner::PreviousResponseAffinityOwnerRecord;
use codex_router_state::quota_snapshot::PersistedQuotaHistoryObservation;
use codex_router_state::quota_snapshot::PersistedQuotaSnapshot;
use codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow;
use codex_router_state::quota_snapshot::QuotaHistoryRefreshOutcome;
use codex_router_state::quota_snapshot::QuotaSnapshotSource;
use codex_router_state::quota_snapshot::SelectorQuotaWindowStatus;
use codex_router_state::repositories::AccountStateRepository;
use codex_router_state::repositories::AffinityRepository;
use codex_router_state::repositories::QuotaSnapshotRepository;
use codex_router_state::repositories::SelectorQuotaRepository;
use codex_router_state::selection_projection::project_route_band_selection_inputs_with_active_counts;
use codex_router_state::session_account_affinity::SessionAccountAffinity;
use codex_router_state::sqlite::AsyncQuotaHistoryRepository;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::AsyncWeeklyQuotaFloorMutationStore;
use codex_router_state::sqlite::SqliteStateStore;
use futures_util::SinkExt;
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::Read;
use std::io::Write;
use std::net::Shutdown;
use std::net::TcpListener;
use std::net::TcpStream;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::WebSocket;
use tokio_tungstenite::tungstenite::accept_hdr;
use tokio_tungstenite::tungstenite::accept_hdr_with_config;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::client::connect_with_config;
use tokio_tungstenite::tungstenite::connect;
use tokio_tungstenite::tungstenite::handshake::server::Request;
use tokio_tungstenite::tungstenite::handshake::server::Response;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_util::task::TaskTracker;

#[path = "proxy_tests/affinity_record_fixtures.rs"]
mod affinity_record_fixtures;
use self::affinity_record_fixtures::*;

#[path = "proxy_tests/affinity_seed_fixtures.rs"]
mod affinity_seed_fixtures;
use self::affinity_seed_fixtures::*;

#[path = "proxy_tests/credit_seed_fixtures.rs"]
mod credit_seed_fixtures;
use self::credit_seed_fixtures::*;

#[path = "proxy_tests/credential_record_fixtures.rs"]
mod credential_record_fixtures;
use self::credential_record_fixtures::*;

#[path = "proxy_tests/loopback_request_fixtures.rs"]
mod loopback_request_fixtures;
use self::loopback_request_fixtures::*;

#[path = "proxy_tests/quota_seed_fixtures.rs"]
mod quota_seed_fixtures;
use self::quota_seed_fixtures::*;

#[path = "proxy_tests/selection_record_fixtures.rs"]
mod selection_record_fixtures;
use self::selection_record_fixtures::*;

#[path = "proxy_tests/test_identity_fixtures.rs"]
mod test_identity_fixtures;
use self::test_identity_fixtures::*;

#[path = "proxy_tests/upstream_record_fixtures.rs"]
mod upstream_record_fixtures;
use self::upstream_record_fixtures::*;

#[path = "proxy_tests/audit_failure_tests.rs"]
mod audit_failure_tests;

#[path = "proxy_tests/audit_runtime_tests.rs"]
mod audit_runtime_tests;

#[path = "proxy_tests/concurrent_selection_tests.rs"]
mod concurrent_selection_tests;

#[path = "proxy_tests/credential_retry_tests.rs"]
mod credential_retry_tests;

#[path = "proxy_tests/credit_runtime_tests.rs"]
mod credit_runtime_tests;

#[path = "proxy_tests/floor_switch_tests.rs"]
mod floor_switch_tests;

#[path = "proxy_tests/http_auth_tests.rs"]
mod http_auth_tests;

#[path = "proxy_tests/http_forwarding_tests.rs"]
mod http_forwarding_tests;

#[path = "proxy_tests/http_runtime_tests.rs"]
mod http_runtime_tests;

#[path = "proxy_tests/listener_adapter_tests.rs"]
mod listener_adapter_tests;

#[path = "proxy_tests/quota_exhaustion_tests.rs"]
mod quota_exhaustion_tests;

#[path = "proxy_tests/quota_observation_tests.rs"]
mod quota_observation_tests;

#[path = "proxy_tests/quota_replay_tests.rs"]
mod quota_replay_tests;

#[path = "proxy_tests/request_contract_tests.rs"]
mod request_contract_tests;

#[path = "proxy_tests/reservation_projection_tests.rs"]
mod reservation_projection_tests;

#[path = "proxy_tests/runtime_shutdown_tests.rs"]
mod runtime_shutdown_tests;

#[path = "proxy_tests/selector_affinity_tests.rs"]
mod selector_affinity_tests;

#[path = "proxy_tests/selector_hold_tests.rs"]
mod selector_hold_tests;

#[path = "proxy_tests/selector_metadata_tests.rs"]
mod selector_metadata_tests;

#[path = "proxy_tests/selector_projection_tests.rs"]
mod selector_projection_tests;

#[path = "proxy_tests/session_affinity_tests.rs"]
mod session_affinity_tests;

#[path = "proxy_tests/session_runtime_tests.rs"]
mod session_runtime_tests;

#[path = "proxy_tests/sse_runtime_tests.rs"]
mod sse_runtime_tests;

#[path = "proxy_tests/tunnel_affinity_tests.rs"]
mod tunnel_affinity_tests;

#[path = "proxy_tests/tunnel_forwarding_tests.rs"]
mod tunnel_forwarding_tests;

#[path = "proxy_tests/websocket_affinity_tests.rs"]
mod websocket_affinity_tests;

#[path = "proxy_tests/websocket_concurrency_tests.rs"]
mod websocket_concurrency_tests;

#[path = "proxy_tests/websocket_dispatch_tests.rs"]
mod websocket_dispatch_tests;

#[path = "proxy_tests/websocket_payload_tests.rs"]
mod websocket_payload_tests;

#[path = "proxy_tests/websocket_rejection_tests.rs"]
mod websocket_rejection_tests;

#[path = "proxy_tests/websocket_routing_tests.rs"]
mod websocket_routing_tests;

#[path = "proxy_tests/websocket_upgrade_tests.rs"]
mod websocket_upgrade_tests;

#[path = "tests/credit_affinity_transport.rs"]
mod credit_affinity_transport;

#[path = "tests/credit_compact_transport.rs"]
mod credit_compact_transport;

#[path = "tests/credit_transport_proof.rs"]
mod credit_transport_proof;

#[path = "tests/credential_generation_websocket.rs"]
mod credential_generation_websocket;

#[path = "proxy_tests/credential_resolution_diagnostic_tests.rs"]
mod credential_resolution_diagnostic_tests;
