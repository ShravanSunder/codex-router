//! Real installed-Codex client proof for the fixture-owned two-stage floor.

mod upstream;

use upstream::FloorUpstream;
use upstream::FloorUpstreamObservation;

use std::fs;
use std::path::Path;
use std::process::Output;
use std::thread;
use std::time::Duration;

use codex_native_integration::CodexRouterProfile;
use codex_router_cli::profile::CodexRouterProfileWriter;
use codex_router_proxy::server::LoopbackBindAddress;
use codex_router_proxy::server::LoopbackRouterRuntime;
use codex_router_proxy::server::LoopbackRouterRuntimeConfig;
use codex_router_proxy::upstream::UpstreamEndpoint;
use codex_router_proxy::websocket::WebSocketQuotaFloorNotifier;
use codex_router_state::account_routing_policy::WeeklyQuotaFloorBasisPoints;
use codex_router_state::quota_snapshot::SelectorQuotaWindowStatus;
use codex_router_state::sqlite::AsyncWeeklyQuotaFloorMutationStore;
use codex_router_state::sqlite::SqliteStateStore;
use tokio_util::sync::CancellationToken;

use super::CODEX_COMMAND_TIMEOUT;
use super::CodexChildEnvironment;
use super::CodexTransportMode;
use super::QUOTA_RECONNECT_FALLBACK;
use super::QUOTA_RECONNECT_PRIMARY;
use super::QUOTA_RECONNECT_PRIMARY_FOR_INITIAL_ADMISSION;
use super::SmokeAccountFixture;
use super::SmokeTempRoot;
use super::account_id;
use super::assert_codex_visible_output;
use super::process_output_markers;
use super::reset_fixture_route_band_state;
use super::run_codex_exec_with_timeout;
use super::seed_smoke_account;

const FIXTURE_WAIT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug)]
enum FloorJourney {
    HealthyPeer,
    NoPeer,
    HardFloor,
}

impl FloorJourney {
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::HealthyPeer => "healthy-peer",
            Self::NoPeer => "no-peer",
            Self::HardFloor => "hard-floor",
        }
    }

    pub(super) const fn new_remaining(self) -> u32 {
        match self {
            Self::HealthyPeer | Self::NoPeer => 8,
            Self::HardFloor => 5,
        }
    }
}

struct FloorRouter {
    pub(super) notifier: WebSocketQuotaFloorNotifier,
    pub(super) shutdown: CancellationToken,
    pub(super) serve_thread: Option<thread::JoinHandle<Result<usize, String>>>,
    pub(super) port: u16,
}

impl FloorRouter {
    pub(super) fn start(
        state_path: &Path,
        secret_root: &Path,
        upstream_address: &str,
    ) -> Result<Self, String> {
        let bind_address = LoopbackBindAddress::new("127.0.0.1", 0)
            .map_err(|error| format!("invalid floor fixture bind address: {error}"))?;
        let endpoint = UpstreamEndpoint::new(format!("http://{upstream_address}/v1"))
            .map_err(|error| format!("invalid floor fixture upstream: {error}"))?;
        let credential_store =
            codex_router_secret_store::test_support::open_encrypted_credential_store(secret_root)
                .map_err(|error| format!("floor fixture credential store failed to open: {error}"))?;
        let runtime = LoopbackRouterRuntime::start(
            LoopbackRouterRuntimeConfig::new_tokenless(
                bind_address,
                endpoint,
                state_path.to_path_buf(),
                secret_root.to_path_buf(),
            ),
            credential_store.into(),
        )
        .map_err(|error| format!("floor fixture router failed to start: {error}"))?;
        let port = runtime.local_addr().port();
        let notifier = runtime.websocket_quota_floor_notifier();
        let shutdown = CancellationToken::new();
        let serve_shutdown = shutdown.clone();
        let serve_thread = thread::Builder::new()
            .name("codex-router-installed-floor-fixture".to_owned())
            .spawn(move || {
                runtime
                    .serve_protocol_connections_until_cancelled(usize::MAX, serve_shutdown)
                    .map_err(|error| format!("floor fixture router serve failed: {error}"))
            })
            .map_err(|error| format!("floor fixture router thread failed: {error}"))?;
        Ok(Self {
            notifier,
            shutdown,
            serve_thread: Some(serve_thread),
            port,
        })
    }

    pub(super) fn stop(mut self) -> Result<(), String> {
        self.shutdown.cancel();
        let handle = self
            .serve_thread
            .take()
            .ok_or_else(|| "floor fixture router already stopped".to_owned())?;
        handle
            .join()
            .map_err(|_| "floor fixture router thread panicked".to_owned())??;
        Ok(())
    }
}

impl Drop for FloorRouter {
    fn drop(&mut self) {
        self.shutdown.cancel();
        if let Some(handle) = self.serve_thread.take() {
            let _ = handle.join();
        }
    }
}

fn seed_floor_fixture(state_path: &Path, secret_root: &Path) -> Result<(), String> {
    let state = SqliteStateStore::open(state_path)
        .map_err(|error| format!("floor fixture state open failed: {error}"))?;
    let secrets =
        codex_router_secret_store::test_support::open_encrypted_credential_store(secret_root)
            .map_err(|error| format!("floor fixture secret store open failed: {error}"))?;
    reset_fixture_route_band_state(
        state_path,
        &[
            QUOTA_RECONNECT_PRIMARY_FOR_INITIAL_ADMISSION,
            QUOTA_RECONNECT_FALLBACK,
        ],
        "installed floor",
    )?;
    seed_smoke_account(
        &state,
        &secrets,
        QUOTA_RECONNECT_PRIMARY_FOR_INITIAL_ADMISSION,
    )?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("floor policy runtime failed: {error}"))?;
    let mutation = runtime
        .block_on(AsyncWeeklyQuotaFloorMutationStore::open(state_path))
        .map_err(|error| format!("floor policy store failed: {error}"))?;
    let floor = WeeklyQuotaFloorBasisPoints::new(500)
        .map_err(|error| format!("floor policy invalid: {error}"))?;
    runtime
        .block_on(
            mutation.set_weekly_quota_floor_by_label(QUOTA_RECONNECT_PRIMARY.label, Some(floor)),
        )
        .map_err(|error| format!("floor policy write failed: {error}"))?;
    Ok(())
}

fn save_floor_observation(
    state_path: &Path,
    secret_root: &Path,
    journey: FloorJourney,
) -> Result<(), String> {
    let state = SqliteStateStore::open(state_path)
        .map_err(|error| format!("floor observation state open failed: {error}"))?;
    let secrets =
        codex_router_secret_store::test_support::open_encrypted_credential_store(secret_root)
            .map_err(|error| format!("floor observation secret store open failed: {error}"))?;
    let primary = SmokeAccountFixture {
        weekly_remaining: journey.new_remaining(),
        ..QUOTA_RECONNECT_PRIMARY_FOR_INITIAL_ADMISSION
    };
    seed_smoke_account(&state, &secrets, primary)?;
    let fallback = if matches!(journey, FloorJourney::NoPeer) {
        SmokeAccountFixture {
            weekly_remaining: 0,
            weekly_status: SelectorQuotaWindowStatus::Ineligible,
            ..QUOTA_RECONNECT_FALLBACK
        }
    } else {
        QUOTA_RECONNECT_FALLBACK
    };
    seed_smoke_account(&state, &secrets, fallback)?;
    Ok(())
}

fn run_floor_journey(journey: FloorJourney) -> Result<(Output, FloorUpstreamObservation), String> {
    let smoke_root = SmokeTempRoot::new(&format!("installed-codex-floor-{}", journey.name()))?;
    let codex_home = smoke_root.path().join("codex-home");
    let process_home = smoke_root.path().join("home");
    let workdir = smoke_root.path().join("workdir");
    let xdg_config = smoke_root.path().join("xdg-config");
    let xdg_state = smoke_root.path().join("xdg-state");
    let xdg_cache = smoke_root.path().join("xdg-cache");
    let state_path = smoke_root.path().join("router/state.sqlite");
    let secret_root = smoke_root.path().join("router/secrets");
    for path in [
        &codex_home,
        &process_home,
        &workdir,
        &xdg_config,
        &xdg_state,
        &xdg_cache,
        &secret_root,
    ] {
        fs::create_dir_all(path)
            .map_err(|error| format!("floor fixture directory creation failed: {error}"))?;
    }
    seed_floor_fixture(&state_path, &secret_root)?;
    let upstream = FloorUpstream::start(journey)?;
    let router = FloorRouter::start(&state_path, &secret_root, &upstream.address)?;
    let profile = CodexRouterProfile::new(router.port);
    CodexRouterProfileWriter::new(&codex_home)
        .write(&profile, true)
        .map_err(|error| format!("floor Codex profile write failed: {error}"))?;
    let last_message_path = smoke_root.path().join("last-message.txt");
    let child_home = codex_home;
    let child_workdir = workdir;
    let child_last_message = last_message_path.clone();
    let codex_child = thread::spawn(move || {
        run_codex_exec_with_timeout(
            CodexTransportMode::WebSocket,
            &child_home,
            &child_workdir,
            &child_last_message,
            CodexChildEnvironment::new(&process_home, &xdg_config, &xdg_state, &xdg_cache),
            CODEX_COMMAND_TIMEOUT,
        )
    });
    let controller_result: Result<(), String> = (|| {
        upstream
            .first_turn_started
            .recv_timeout(FIXTURE_WAIT)
            .map_err(|_| "installed Codex did not begin the fixture turn".to_owned())?;
        save_floor_observation(&state_path, &secret_root, journey)?;
        let primary_id = account_id(QUOTA_RECONNECT_PRIMARY.account_id)?;
        match journey {
            FloorJourney::HealthyPeer | FloorJourney::NoPeer => router
                .notifier
                .request_weekly_quota_floor_switch(&primary_id),
            FloorJourney::HardFloor => router
                .notifier
                .signal_weekly_quota_floor_reached(&primary_id),
        }
        upstream
            .release_first_turn
            .send(())
            .map_err(|_| "floor upstream ended before first turn release".to_owned())?;
        Ok(())
    })();
    let child_output = codex_child
        .join()
        .map_err(|_| "installed Codex floor child thread panicked".to_owned())?;
    let upstream_observation = upstream.finish();
    let router_stop = router.stop();
    if let Err(controller_error) = controller_result {
        let child_summary = match &child_output {
            Ok(output) => format!(
                "status={},stdout_bytes={},stderr_bytes={},stderr_markers={}",
                output.status,
                output.stdout.len(),
                output.stderr.len(),
                process_output_markers(&String::from_utf8_lossy(&output.stderr)),
            ),
            Err(error) => error.clone(),
        };
        let upstream_summary = upstream_observation
            .as_ref()
            .map(|_| "completed".to_owned())
            .unwrap_or_else(Clone::clone);
        let router_summary = router_stop
            .as_ref()
            .map(|_| "completed".to_owned())
            .unwrap_or_else(Clone::clone);
        return Err(format!(
            "{controller_error}; codex={child_summary}; upstream={upstream_summary}; router={router_summary}",
        ));
    }
    let child_output = child_output.map_err(|error| {
        format!(
            "{error}; upstream={}; router={}",
            upstream_observation
                .as_ref()
                .err()
                .map_or("completed", String::as_str),
            router_stop
                .as_ref()
                .err()
                .map_or("completed", String::as_str),
        )
    })?;
    router_stop?;
    let observation = upstream_observation?;
    assert_codex_visible_output("installed Codex floor", &child_output, &last_message_path)?;
    Ok((child_output, observation))
}

#[test]
#[ignore = "real installed Codex floor fixture; run the named ignored test deliberately"]
fn installed_codex_websocket_floor_switch_e2e_healthy_peer() {
    assert_installed_floor_journey(FloorJourney::HealthyPeer);
}

#[test]
#[ignore = "real installed Codex floor fixture; run the named ignored test deliberately"]
fn installed_codex_websocket_floor_switch_e2e_no_peer() {
    assert_installed_floor_journey(FloorJourney::NoPeer);
}

#[test]
#[ignore = "real installed Codex floor fixture; run the named ignored test deliberately"]
fn installed_codex_websocket_floor_switch_e2e_hard_floor() {
    assert_installed_floor_journey(FloorJourney::HardFloor);
}

fn assert_installed_floor_journey(journey: FloorJourney) {
    let (_output, observation) =
        run_floor_journey(journey).unwrap_or_else(|error| panic!("{}: {error}", journey.name()));
    assert_eq!(observation.first_account, "primary", "{}", journey.name());
    let expected_peer = if matches!(journey, FloorJourney::NoPeer) {
        "primary"
    } else {
        "fallback"
    };
    assert_eq!(
        observation.followup_account,
        expected_peer,
        "{}",
        journey.name()
    );
    assert_eq!(
        observation.followed_on_new_connection,
        !matches!(journey, FloorJourney::NoPeer),
        "{}",
        journey.name()
    );
    if !matches!(journey, FloorJourney::NoPeer) {
        assert!(
            !observation.forwarded_stale_response_id,
            "{} forwarded the stale primary response id",
            journey.name()
        );
    }
    if !matches!(journey, FloorJourney::HardFloor) {
        assert!(
            observation.followup_contains_first_tool_output,
            "{} did not complete the first tool turn before follow-up",
            journey.name()
        );
    }
    eprintln!(
        "installed_codex_floor_journey={} first={} followup={} reconnect={} tool_followup={} stale_id_forwarded={}",
        journey.name(),
        observation.first_account,
        observation.followup_account,
        observation.followed_on_new_connection,
        observation.followup_contains_first_tool_output,
        observation.forwarded_stale_response_id,
    );
}
