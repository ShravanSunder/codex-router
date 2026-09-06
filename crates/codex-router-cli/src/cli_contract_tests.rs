//! Preserved CLI behavior and provider-isolation contracts.
use super::*;

use std::cell::RefCell;
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::io::Write;
use std::net::Shutdown;
use std::net::TcpListener;
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use tungstenite::Message;
use tungstenite::accept_hdr;
use tungstenite::client::IntoClientRequest;

use tungstenite::connect;
use tungstenite::handshake::server::Request;
use tungstenite::handshake::server::Response;
use tungstenite::http::HeaderValue;

use codex_router_auth::resolver::CredentialRefreshClient;
use codex_router_auth::resolver::CredentialResolverError;
use codex_router_auth::resolver::NoopCredentialRefreshClient;
use codex_router_auth::resolver::ProviderCredentialResolver;
use codex_router_auth::resolver::ResolvedProviderCredential;
use codex_router_auth::resolver::RouterCredentialResolver;
use codex_router_core::ids::AccountId;
use codex_router_core::ids::ReservationId;
use codex_router_core::redaction::SecretString;
use codex_router_proxy::server::LoopbackBindAddress;
use codex_router_proxy::server::LoopbackRouterRuntime;
use codex_router_proxy::server::LoopbackRouterRuntimeConfig;
use codex_router_proxy::upstream::UpstreamEndpoint;
use codex_router_proxy::websocket::WebSocketRegistrySnapshot;
use codex_router_proxy::websocket::WebSocketSessionPeerAddr;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::account_tokens::account_credential_bundle_key;
use codex_router_secret_store::account_tokens::upstream_access_token_key;
use codex_router_secret_store::file_backend::FileSecretStore;
use codex_router_secret_store::model::SecretKey;
use codex_router_secret_store::model::SecretStoreError;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::account_routing_policy::WeeklyQuotaFloorBasisPoints;
use codex_router_state::quota_snapshot::PersistedQuotaHistoryObservation;
use codex_router_state::quota_snapshot::PersistedQuotaSnapshot;
use codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow;
use codex_router_state::quota_snapshot::QuotaSnapshotSource;
use codex_router_state::quota_snapshot::SelectorQuotaWindowStatus;
use codex_router_state::repositories::AccountStateRepository;
use codex_router_state::repositories::QuotaSnapshotRepository;
use codex_router_state::repositories::SelectorQuotaRepository;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::AsyncWeeklyQuotaFloorMutationStore;
use codex_router_state::sqlite::SqliteStateStore;

use crate::account::AccountCommand;
use crate::account::AccountCommandError;
use crate::account::AccountImportRequest;
use crate::account::import_codex_auth_from_request;
use crate::credential_runtime::AsyncProviderCredentialResolver;
use crate::credential_runtime::CliCredentialResolver;
use crate::doctor::DoctorAccountState;
use crate::doctor::DoctorReport;
use crate::doctor::QuotaDoctorState;
use crate::profile::CodexRouterProfile;
use crate::profile::CodexRouterProfileWriter;
use crate::profile::ProfileWriteError;
use crate::quota::BackgroundQuotaRefreshRuntime;
use crate::quota::HttpQuotaRefreshProvider;
use crate::quota::QuotaCommand;
use crate::quota::QuotaRefreshObservationContext;
use crate::quota::QuotaRefreshProvider;
use crate::quota::QuotaRefreshProviderRequest;
use crate::quota::QuotaRefreshProviderResponse;
use crate::quota::QuotaRefreshProviderWindow;
use crate::quota::WeeklyQuotaFloorReachedObserver;
use crate::quota::refresh_quota_store_paths_with_dependencies as refresh_quota_store_paths_with_dependencies_async;
use crate::quota::refresh_quota_store_paths_with_dependencies_and_floor_notifier as refresh_quota_store_paths_with_dependencies_and_floor_notifier_async;
use crate::quota::refresh_quota_with_dependencies as refresh_quota_with_dependencies_async;
use crate::quota::start_background_quota_refresh_worker_with_clock;
use crate::quota::start_background_quota_refresh_worker_with_dependencies;
use crate::quota::start_background_quota_refresh_worker_with_reporter;
use crate::token::LocalRouterTokenService;
use crate::token::Shell;
use crate::token::export_token_assignment;

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[path = "cli_contract_tests/cli_test_support.rs"]
mod cli_test_support;
use cli_test_support::*;

#[path = "cli_contract_tests/quota_test_support.rs"]
mod quota_test_support;
use quota_test_support::*;

#[path = "cli_contract_tests/transport_test_support.rs"]
mod transport_test_support;
use transport_test_support::*;

#[path = "cli_contract_tests/cli_entrypoint_tests.rs"]
mod cli_entrypoint_tests;

#[path = "cli_contract_tests/token_command_tests.rs"]
mod token_command_tests;

#[path = "cli_contract_tests/profile_command_tests.rs"]
mod profile_command_tests;

#[path = "cli_contract_tests/account_import_tests.rs"]
mod account_import_tests;

#[path = "cli_contract_tests/quota_rendering_tests.rs"]
mod quota_rendering_tests;

#[path = "cli_contract_tests/quota_observation_tests.rs"]
mod quota_observation_tests;

#[path = "cli_contract_tests/quota_auth_tests.rs"]
mod quota_auth_tests;

#[path = "cli_contract_tests/quota_worker_tests.rs"]
mod quota_worker_tests;

#[path = "cli_contract_tests/quota_snapshot_tests.rs"]
mod quota_snapshot_tests;

#[path = "cli_contract_tests/quota_http_tests.rs"]
mod quota_http_tests;

#[path = "cli_contract_tests/live_quota_tests.rs"]
mod live_quota_tests;

#[path = "cli_contract_tests/account_policy_tests.rs"]
mod account_policy_tests;

#[path = "cli_contract_tests/router_startup_tests.rs"]
mod router_startup_tests;

#[path = "cli_contract_tests/quota_concurrency_tests.rs"]
mod quota_concurrency_tests;

#[path = "cli_contract_tests/token_rotation_tests.rs"]
mod token_rotation_tests;
