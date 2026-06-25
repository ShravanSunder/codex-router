use std::fs;
use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::thread;

use codex_router_core::ids::AccountId;
use codex_router_core::ids::TokenGeneration;
use codex_router_core::local_auth::LocalRouterTokenRecord;
use codex_router_core::redaction::SecretString;
use codex_router_proxy::server::LoopbackBindAddress;
use codex_router_proxy::server::LoopbackRouterRuntime;
use codex_router_proxy::server::LoopbackRouterRuntimeConfig;
use codex_router_proxy::upstream::UpstreamEndpoint;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::account_tokens::account_credential_bundle_key;
use codex_router_secret_store::file_backend::FileSecretStore;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::quota_snapshot::PersistedQuotaSnapshot;
use codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow;
use codex_router_state::quota_snapshot::QuotaSnapshotSource;
use codex_router_state::quota_snapshot::SelectorQuotaWindowStatus;
use codex_router_state::repositories::AccountStateRepository;
use codex_router_state::repositories::QuotaSnapshotRepository;
use codex_router_state::repositories::SelectorQuotaRepository;
use codex_router_state::sqlite::SqliteStateStore;

pub(super) const LOCAL_TOKEN: &str = "route-native-local-token";

pub(super) const ROUTE_NATIVE_ACCOUNTS: &[RouteNativeAccountFixture] = &[
    RouteNativeAccountFixture {
        account_id: "acct_route_native_responses",
        label: "route-responses",
        route_band: "responses",
        upstream_token: "route-native-responses-token",
    },
    RouteNativeAccountFixture {
        account_id: "acct_route_native_models",
        label: "route-models",
        route_band: "models",
        upstream_token: "route-native-models-token",
    },
    RouteNativeAccountFixture {
        account_id: "acct_route_native_memories",
        label: "route-memories",
        route_band: "memories_trace_summarize",
        upstream_token: "route-native-memories-token",
    },
    RouteNativeAccountFixture {
        account_id: "acct_route_native_compact",
        label: "route-compact",
        route_band: "responses_compact",
        upstream_token: "route-native-compact-token",
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RouteNativeAccountFixture {
    pub(super) account_id: &'static str,
    pub(super) label: &'static str,
    pub(super) route_band: &'static str,
    pub(super) upstream_token: &'static str,
}

pub(super) struct RouteNativeTempRoot {
    path: PathBuf,
}

impl RouteNativeTempRoot {
    pub(super) fn new(name: &str) -> Result<Self, String> {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "codex-router-{name}-{}-{counter}",
            std::process::id()
        ));
        fs::create_dir_all(&path)
            .map_err(|error| format!("failed to create temp root {}: {error}", path.display()))?;
        Ok(Self { path })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RouteNativeTempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub(super) struct StartedRouteNativeRouter {
    pub(super) address: SocketAddr,
    handle: thread::JoinHandle<Result<(), String>>,
}

impl StartedRouteNativeRouter {
    pub(super) fn join(self) -> Result<(), String> {
        join_result(self.handle, "route-native router")
    }
}

pub(super) fn start_route_native_router(
    state_path: &Path,
    secret_root: &Path,
    upstream_base_url: String,
    max_connections: usize,
) -> Result<StartedRouteNativeRouter, String> {
    let bind_address = LoopbackBindAddress::new("127.0.0.1", 0)
        .map_err(|error| format!("failed to build route-native bind address: {error}"))?;
    let upstream_endpoint = UpstreamEndpoint::new(upstream_base_url)
        .map_err(|error| format!("failed to build route-native upstream endpoint: {error}"))?;
    let local_token = LocalRouterTokenRecord::new(
        SecretString::new(LOCAL_TOKEN.to_owned()),
        TokenGeneration::new(1),
    );
    let runtime = LoopbackRouterRuntime::start(
        LoopbackRouterRuntimeConfig::new(
            bind_address,
            upstream_endpoint,
            state_path.to_path_buf(),
            secret_root.to_path_buf(),
            local_token,
        )
        .with_quota_clock(1_030, 60),
    )
    .map_err(|error| format!("failed to start route-native router: {error}"))?;
    let address = runtime.local_addr();
    let handle = thread::Builder::new()
        .name("codex-router-route-native-router".to_owned())
        .spawn(move || {
            runtime
                .serve_protocol_connections(max_connections)
                .map(|_| ())
                .map_err(|error| format!("route-native router failed: {error}"))
        })
        .map_err(|error| format!("failed to spawn route-native router: {error}"))?;

    Ok(StartedRouteNativeRouter { address, handle })
}

pub(super) fn seed_route_native_state(state_path: &Path, secret_root: &Path) -> Result<(), String> {
    let state = SqliteStateStore::open(state_path)
        .map_err(|error| format!("failed to open route-native state: {error}"))?;
    let secrets = FileSecretStore::open(secret_root)
        .map_err(|error| format!("failed to open route-native secrets: {error}"))?;
    for fixture in ROUTE_NATIVE_ACCOUNTS {
        seed_route_native_account(&state, &secrets, *fixture)?;
    }

    Ok(())
}

pub(super) fn selected_account_label_from_authorization(value: &str) -> Option<&'static str> {
    let token = value.strip_prefix("Bearer ")?;
    ROUTE_NATIVE_ACCOUNTS
        .iter()
        .find(|fixture| fixture.upstream_token == token)
        .map(|fixture| fixture.label)
}

pub(super) fn contains_route_native_upstream_token(text: &str) -> bool {
    ROUTE_NATIVE_ACCOUNTS
        .iter()
        .any(|fixture| text.contains(fixture.upstream_token))
}

pub(super) fn join_result<T>(
    handle: thread::JoinHandle<Result<T, String>>,
    label: &str,
) -> Result<T, String> {
    match handle.join() {
        Ok(result) => result,
        Err(error) => Err(format!("{label} thread panicked: {error:?}")),
    }
}

fn seed_route_native_account(
    state: &SqliteStateStore,
    secrets: &FileSecretStore,
    fixture: RouteNativeAccountFixture,
) -> Result<(), String> {
    let account_id = account_id(fixture.account_id)?;
    let account = AccountRecord::new(account_id.clone(), fixture.label, AccountStatus::Enabled)
        .with_active_credential_generation(1);
    AccountStateRepository::upsert_account(state, &account).map_err(|error| {
        format!(
            "failed to seed route-native account {}: {error}",
            fixture.label
        )
    })?;
    let route_band = fixture.route_band;
    let snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(1_000)
            .with_route_band(route_band, 88)
            .with_reset_unix_seconds(18_000);
    QuotaSnapshotRepository::upsert_snapshot(state, &snapshot).map_err(|error| {
        format!("failed to seed route-native quota snapshot for {route_band}: {error}")
    })?;
    let short_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        route_band,
        18_000,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(88)
    .with_reset_unix_seconds(18_000)
    .with_effective(true)
    .with_observed_unix_seconds(1_000);
    SelectorQuotaRepository::upsert_selector_window(state, &short_window).map_err(|error| {
        format!("failed to seed route-native short window for {route_band}: {error}")
    })?;
    let weekly_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        route_band,
        604_800,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(72)
    .with_reset_unix_seconds(604_800)
    .with_observed_unix_seconds(1_000);
    SelectorQuotaRepository::upsert_selector_window(state, &weekly_window).map_err(|error| {
        format!("failed to seed route-native weekly window for {route_band}: {error}")
    })?;

    let credential_key = account_credential_bundle_key(&account_id, 1)
        .map_err(|error| format!("failed to build route-native credential key: {error}"))?;
    let credential_bundle = AccountCredentialBundle::imported_codex_auth(
        fixture.upstream_token,
        Some(format!("{}-refresh", fixture.upstream_token)),
    )
    .to_secret_string()
    .map_err(|error| format!("failed to serialize route-native credential bundle: {error}"))?;
    secrets
        .write_secret(&credential_key, &credential_bundle)
        .map_err(|error| format!("failed to write route-native credential bundle: {error}"))?;

    Ok(())
}

fn account_id(value: &str) -> Result<AccountId, String> {
    AccountId::new(value).map_err(|error| format!("invalid account id {value}: {error}"))
}
