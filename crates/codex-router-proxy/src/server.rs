//! Loopback-only server runtime primitives.

use std::collections::VecDeque;
use std::convert::Infallible;
#[cfg(test)]
use std::io::Read;
#[cfg(test)]
use std::io::Write;
use std::net::AddrParseError;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
#[cfg(test)]
use std::net::TcpListener;
#[cfg(test)]
use std::net::TcpStream;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use bytes::Bytes;
use codex_router_auth::resolver::CredentialRefreshTaskSupervisor;
use futures_util::future::BoxFuture;
use futures_util::stream;
use http::HeaderMap;
use http::Method as HttpMethod;
use http::Request as HttpRequest;
use http::Response as HttpResponse;
use http::StatusCode;
use http::Uri;
use http_body_util::BodyExt;
use http_body_util::Empty;
use http_body_util::Full;
use http_body_util::StreamBody;
use http_body_util::combinators::BoxBody;
use hyper::body::Body as HyperBody;
use hyper::body::Frame;
use hyper::body::Incoming;
use hyper::body::SizeHint;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener as TokioTcpListener;
use tokio::task::JoinError;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::instrument::WithSubscriber;

use codex_router_core::affinity::hash_previous_response_id;
use codex_router_core::audit::AuditFileSink;
use codex_router_core::audit::RouteKind as AuditRouteKind;
use codex_router_core::audit::TransportKind;
use codex_router_core::ids::AccountId;
use codex_router_core::local_auth::LocalAuthError;
use codex_router_core::local_auth::LocalRouterAuth;
use codex_router_core::local_auth::LocalRouterTokenRecord;
use codex_router_core::redaction::safe_account_label;
use codex_router_core::route_profile::ClaudeFiveHourReservePercent;
use codex_router_core::route_profile::DEFAULT_CLAUDE_FIVE_HOUR_RESERVE_PERCENT;
use codex_router_core::route_profile::RouteProfile;
use codex_router_core::router_compatibility::RouterCompatibility;
use codex_router_core::routes::RouteBand;
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore;
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStoreStatus;
use codex_router_selection::selection_outcome::CredentialStoreAvailability;
use codex_router_selection::selection_outcome::HeadroomTimestamp;
use codex_router_selection::selection_outcome::SelectionHoldReason;
use codex_router_selection::selection_outcome::UnavailableReason;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::affinity_owner::AffinitySourceTransport;
use codex_router_state::affinity_owner::PreviousResponseAffinityOwnerRecord;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::StateStoreError;

use crate::account_selection::AsyncAccountSelectorRuntimeState;
use crate::account_selection::AsyncRepositoryBackedAccountSelector;
use crate::account_selection::DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS;
use crate::account_selection::RouteBandAccountHolds;
use crate::account_selection::RouteBandPostExhaustionOutcomeInput;
use crate::account_selection::RouteBandQueueHealth;
use crate::account_selection::RouteBandReservationBooks;
use crate::account_selection::RouteBandRuntimeExhaustions;
use crate::account_selection::RouteBandWeightedSelectors;
use crate::account_selection::RuntimeAccountAdmissionAssessor;
use crate::account_selection::SelectionReservationLock;
use crate::account_selection::SqliteActiveClientLeaseReporter;
use crate::account_selection::mark_runtime_quota_exhausted;
use crate::account_selection::route_band_post_exhaustion_outcome;
use crate::claude_edge::server_pipeline::ClaudeServerRuntime;
use crate::credential_runtime::AsyncProxyCredentialResolverFactory;
use crate::credential_runtime::ProxyRuntimeCredentialResources;
use crate::credential_runtime::ProxyRuntimeCredentialResourcesOpenError;
use crate::credential_runtime::RuntimeAffinitySecretProvider;
use crate::db_write_actor::DbWriteActor;
use crate::db_write_actor::DbWriteCommand;
use crate::db_write_actor::DbWriteEnqueueResult;
use crate::db_write_actor::PROVIDER_EXHAUSTION_QUEUE_CAPACITY;
use crate::db_write_actor::SqliteDbWriteRepository;
use crate::headers::Header;
use crate::headers::HeaderCollection;
use crate::http_sse::AsyncHttpAffinityOwnerRecorder;
use crate::http_sse::AsyncHttpBodyError;
use crate::http_sse::AsyncStreamingHttpProxyResponse;
use crate::http_sse::AsyncStreamingUpstreamHttpTransport;
use crate::http_sse::AuthenticatedHttpProxyService;
use crate::http_sse::HttpProxyError;
use crate::http_sse::HttpProxyRequest;
#[cfg(test)]
use crate::http_sse::HttpProxyResponse;
#[cfg(test)]
use crate::http_sse::HttpRequestHandler;
use crate::http_sse::PreparedAsyncStreamingHttpProxyRequest;
use crate::http_sse::StderrAuditFailureReporter;
use crate::http_sse::StreamingHttpProxyCompletion;
#[cfg(test)]
use crate::http_sse::StreamingHttpProxyResponse;
#[cfg(test)]
use crate::http_sse::StreamingHttpRequestHandler;
use crate::http_sse::append_audit_event_with_reporter;
use crate::http_sse::extract_response_id_from_body;
use crate::http_sse::local_auth_rejection_audit_event;
use crate::local_auth::extract_presented_local_token_from_request;
use crate::maintenance_actor::MAINTENANCE_QUEUE_CAPACITY;
use crate::maintenance_actor::MaintenanceActor;
use crate::maintenance_actor::MaintenanceHint;
use crate::provider_error::AsyncProviderErrorObserver;
use crate::provider_error::ProviderErrorClassification;
use crate::provider_error::ProviderErrorObservationError;
use crate::provider_error::classify_provider_error_envelope;
use crate::provider_error::record_provider_error_observation;
use crate::routes::Method;
use crate::routes::RouteClass;
use crate::routes::classify_route;
use crate::routes::is_claude_edge_path;
use crate::session_account_affinity_cache::DEFAULT_SESSION_PIN_IDLE_TTL;
use crate::session_account_affinity_cache::SessionAccountAffinityCache;
use crate::session_account_affinity_cache::SharedSessionAccountAffinityCache;
#[cfg(debug_assertions)]
use crate::upstream::ClaudeUpstreamEndpoint;
use crate::upstream::HyperHttpUpstreamTransport;
use crate::upstream::UpstreamEndpoint;
use crate::websocket::AsyncWebSocketTunnel;
use crate::websocket::WebSocketHandshakeRequest;
use crate::websocket::WebSocketProtocolRouter;
use crate::websocket::WebSocketQuotaFloorNotifier;
use crate::websocket::WebSocketRegistrySnapshot;
use crate::websocket::WebSocketRevocationRegistry;
use crate::websocket::router_websocket_config;

#[cfg(test)]
const MAX_HTTP_HEADER_BYTES: usize = 64 * 1024;
const ACTIVE_SESSION_EVENT_RETENTION_SECONDS: u64 = 7 * 86_400;

#[path = "server/auth_reloader.rs"]
mod auth_reloader;
#[path = "server/connection_diagnostics.rs"]
mod connection_diagnostics;
#[path = "server/db_affinity.rs"]
mod db_affinity;
#[path = "server/errors_and_adapters.rs"]
mod errors_and_adapters;
#[path = "server/hyper_request.rs"]
mod hyper_request;
#[path = "server/loopback_bind.rs"]
mod loopback_bind;
#[path = "server/protocol_handler.rs"]
mod protocol_handler;
#[path = "server/response_body.rs"]
mod response_body;
#[path = "server/runtime_cleanup.rs"]
mod runtime_cleanup;
#[path = "server/runtime_config.rs"]
mod runtime_config;
#[path = "server/runtime_lifecycle.rs"]
mod runtime_lifecycle;
#[path = "server/runtime_maintenance.rs"]
mod runtime_maintenance;
#[path = "server/runtime_preparation.rs"]
mod runtime_preparation;
#[path = "server/runtime_serving.rs"]
mod runtime_serving;
#[path = "server/runtime_startup.rs"]
mod runtime_startup;
pub use runtime_preparation::PreparedLoopbackRouterRuntime;
#[path = "server/runtime_activation.rs"]
mod runtime_activation;
#[cfg(test)]
#[path = "server/runtime_fixture.rs"]
mod runtime_fixture;
#[path = "server/runtime_state.rs"]
mod runtime_state;

pub use auth_reloader::*;
use connection_diagnostics::*;
use db_affinity::*;
pub use errors_and_adapters::*;
pub(crate) use hyper_request::*;
pub use loopback_bind::*;
use protocol_handler::*;
pub(crate) use response_body::*;
pub use runtime_config::*;
use runtime_lifecycle::*;
pub use runtime_serving::*;
#[cfg(test)]
use runtime_startup::open_runtime_writable_state_stores;
pub use runtime_state::*;

#[cfg(test)]
#[path = "server/tests.rs"]
mod tests;
