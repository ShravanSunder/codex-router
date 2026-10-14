//! The sole Host lifecycle retains its private peer registry across backend replacement.
use super::*;
#[path = "runtime_peer_registry_cleanup.rs"]
mod cleanup;
use cleanup::require_process_absent;
use collaboration_client::{CollaborationClient, MessageSendRequest, PublicMessageContent};
use collaboration_protocol::{
    ChannelDescription, CodexGeneration, DeliveryClientReceipt, DeliveryOutcome, EndpointId,
    EndpointRef, MessageDelivery, NativeSessionScope, NativeSessionSource, NativeSessionView,
    ProviderSessionListParams, ProviderSessionSummary, PushDeliveryState, PushRecordShowParams,
    SessionReachability,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixListener;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

type TestResult<TValue = ()> = Result<TValue, Box<dyn std::error::Error>>;
const HOST_ROOT_ENV: &str = "CODEX_HOST_PRIVATE_REGISTRY_TEST_ROOT";
const ROUTER_ADDRESS_ENV: &str = "CODEX_HOST_PRIVATE_REGISTRY_ROUTER_ADDRESS";
const PEER_SESSION: &str = "runtime-private-registry-peer";
const PEER_TOKEN: &str = "0123456789abcdef0123456789abcdef";
const HOST_TEST: &str = "runtime_peer_registry::private_registry_host_child_entrypoint";
const WAIT: Duration = Duration::from_secs(20);

#[derive(Debug)]
struct InjectedScenarioFailure;
impl std::fmt::Display for InjectedScenarioFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("controlled assertion failure after the first peer delivery")
    }
}
impl std::error::Error for InjectedScenarioFailure {}

#[test]
fn host_peer_registry_default_is_none_without_reading_a_registry() -> TestResult {
    let config = host_config(
        Path::new("/tmp/inert-registry-default"),
        "127.0.0.1:9".parse()?,
    );
    check(
        config.collaboration_peer_registry_directory().is_none(),
        "default registry must be None",
    )
}

#[tokio::test]
async fn host_private_peer_registry_survives_actual_backend_restart() -> TestResult {
    run_registry_case(false).await
}

#[tokio::test]
async fn host_private_peer_registry_failure_still_awaits_all_cleanup() -> TestResult {
    let failure = run_registry_case(true)
        .await
        .err()
        .ok_or("injected failure was not returned")?;
    check(
        failure.downcast_ref::<InjectedScenarioFailure>().is_some(),
        "scenario failure was replaced or cleanup also failed",
    )
}

#[tokio::test]
async fn private_registry_host_child_entrypoint() -> TestResult {
    let Some(root) = std::env::var_os(HOST_ROOT_ENV).map(PathBuf::from) else {
        return Ok(());
    };
    let router = std::env::var(ROUTER_ADDRESS_ENV)?.parse()?;
    let managed = root.join("managed-codex");
    let identity = codex_native_integration::executable_identity(&managed).await?;
    let command = ChildCommandSpec::new(managed.clone())
        .with_arguments([
            "--exact",
            "runtime_native_app_server_child_entrypoint",
            "--nocapture",
        ])
        .with_environment(
            "CODEX_ROUTER_HOST_RUNTIME_FIXTURE_BINARY",
            std::env::current_exe()?,
        )
        .with_environment(
            "CODEX_ROUTER_HOST_RUNTIME_NATIVE_SOCKET",
            root.join("native.sock"),
        )
        .with_environment("CODEX_ROUTER_HOST_RUNTIME_MANAGED_EXECUTABLE", &managed)
        .with_environment(
            "CODEX_ROUTER_HOST_RUNTIME_PROCESS_LOG",
            root.join("native-pids.log"),
        )
        .with_output(ChildOutput::Null);
    let config = host_config(&root, router)
        .with_collaboration_peer_registry_directory(root.join("peer-registry"))
        .with_external_provider_startup(codex_router_host::ExternalProviderStartup::unavailable(
            collaboration_protocol::ProviderKind::ClaudeCode,
            "ACP is intentionally disabled in this private peer fixture".to_owned(),
            "Use the authenticated scripted peer route for this fixture".to_owned(),
        )?);
    check(
        config.external_provider_startups().iter().all(|startup| {
            matches!(
                startup,
                codex_router_host::ExternalProviderStartup::Unavailable { .. }
            )
        }),
        "no ACP provider may be launched",
    )?;
    let exit = HostRuntime::run(
        config,
        ManagedChildLaunchPlans::new(
            None,
            AppServerLaunchPlan::new(command, identity, "1.2.3".into()),
        ),
        ManagedUpdateInputs::production(),
    )
    .await?;
    check_equal(
        exit,
        codex_router_host::HostExit::Signal,
        "owned Host must complete signal shutdown",
    )
}

fn host_config(root: &Path, router: std::net::SocketAddr) -> HostConfig {
    HostConfig::new(HostConfigInputs {
        coordination_paths: HostCoordinationPaths::new(
            root.join("operator.sock"),
            root.join("host.lock"),
        ),
        router_endpoint: router,
        mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
        app_server_socket: root.join("native.sock"),
        managed_executable: root.join("managed-codex"),
        deadlines: HostDeadlines::production(),
    })
    .with_collaboration_directory(root.join("api"), root.join("codex-home"))
}

struct PeerFrames {
    auth: Value,
    user: Value,
}
struct PeerObserver {
    stop: CancellationToken,
    task: JoinHandle<std::io::Result<()>>,
    received: mpsc::Receiver<PeerFrames>,
    count: Arc<AtomicUsize>,
}

impl PeerObserver {
    fn start(root: &Path) -> TestResult<Self> {
        let registry = root.join("peer-registry");
        std::fs::create_dir(&registry)?;
        std::fs::set_permissions(&registry, std::fs::Permissions::from_mode(0o700))?;
        let socket = registry.join("peer.sock");
        let listener = UnixListener::bind(&socket)?;
        let pid = std::process::id();
        // Same authenticated registry/frame contract as session_message_route_fixture::publish_peer.
        let record = json!({"pid":pid,"sessionId":PEER_SESSION,"status":"busy","peerProtocol":1,
            "messagingSocketPath":socket,"cwd":root,"startedAt":1000,"updatedAt":2000,
            "kind":"interactive","entrypoint":"cli"});
        write_private(
            &registry.join(format!("{pid}.json")),
            &serde_json::to_vec(&record)?,
        )?;
        let socket_text = socket.to_str().ok_or("peer socket path is not UTF-8")?;
        let digest = Sha256::digest(socket_text.as_bytes());
        write_private(
            &registry.join(format!("{pid}.{digest:x}.key")),
            &serde_json::to_vec(&json!({"peerToken":PEER_TOKEN}))?,
        )?;
        let (sender, received) = mpsc::channel(4);
        let count = Arc::new(AtomicUsize::new(0));
        let task_count = Arc::clone(&count);
        let stop = CancellationToken::new();
        let task_stop = stop.clone();
        let task = tokio::spawn(async move {
            loop {
                let connection = tokio::select! {
                    () = task_stop.cancelled() => return Ok(()),
                    accepted = listener.accept() => accepted?,
                };
                let read_and_publish = async {
                    let mut lines = BufReader::new(connection.0).lines();
                    let auth = lines
                        .next_line()
                        .await?
                        .ok_or_else(|| std::io::Error::other("missing auth frame"))?;
                    let user = lines
                        .next_line()
                        .await?
                        .ok_or_else(|| std::io::Error::other("missing user frame"))?;
                    let frames = PeerFrames {
                        auth: serde_json::from_str(&auth).map_err(std::io::Error::other)?,
                        user: serde_json::from_str(&user).map_err(std::io::Error::other)?,
                    };
                    task_count.fetch_add(1, Ordering::Relaxed);
                    sender
                        .send(frames)
                        .await
                        .map_err(|_| std::io::Error::other("peer observer receiver closed"))
                };
                tokio::select! {
                    () = task_stop.cancelled() => return Ok(()),
                    result = read_and_publish => result?,
                }
            }
        });
        Ok(Self {
            stop,
            task,
            received,
            count,
        })
    }
}

struct RegistryCase {
    root: TestDirectory,
    router: Option<PersistentRouterHealthFixture>,
    peer: Option<PeerObserver>,
    host: Option<Child>,
}

async fn run_registry_case(inject_failure: bool) -> TestResult {
    let root = TestDirectory::new("peer-registry")?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let mut owned = RegistryCase {
        root,
        router: None,
        peer: None,
        host: None,
    };
    let scenario = owned.exercise(inject_failure).await;
    let cleanup = owned.finish().await;
    match (scenario, cleanup) {
        (result, Ok(())) => result,
        (Ok(()), Err(error)) => Err(error),
        (Err(scenario), Err(cleanup)) => {
            Err(std::io::Error::other(format!("scenario: {scenario}; cleanup: {cleanup}")).into())
        }
    }
}

impl RegistryCase {
    async fn exercise(&mut self, inject_failure: bool) -> TestResult {
        let root = self.root.path().to_owned();
        std::fs::create_dir(root.join("codex-home"))?;
        std::fs::write(root.join("native-pids.log"), b"")?;
        write_managed_codex_version_fixture(&root.join("managed-codex"), "1.2.3")?;
        self.router = Some(PersistentRouterHealthFixture::start().await?);
        self.peer = Some(PeerObserver::start(&root)?);
        let address = self
            .router
            .as_ref()
            .ok_or("health fixture missing")?
            .address();
        self.host = Some(
            Command::new(std::env::current_exe()?)
                .args(["--exact", HOST_TEST, "--nocapture"])
                .env(HOST_ROOT_ENV, &root)
                .env(ROUTER_ADDRESS_ENV, address.to_string())
                .env_remove("CODEX_FIXTURE_IGNORE_TERM")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?,
        );
        let frames = send_operator_request(
            &root.join("operator.sock"),
            OperatorRequest::AwaitHostStart,
            WAIT,
        )
        .await?;
        check_equal(
            terminal_snapshot(&frames)?.hosted_readiness(),
            codex_router_host::HostedReadiness::Ready,
            "Host was not actually ready",
        )?;
        let client =
            CollaborationClient::connect(&root.join("api"), "private-peer-lifecycle", "1").await?;
        let before_generation = native_generation(&client).await?;
        let before_pid = *wait_for_process_ids(&root.join("native-pids.log"), 1)
            .await?
            .first()
            .ok_or("native PID missing")?;
        self.deliver(&client, "registry before restart").await?;
        if inject_failure {
            return Err(InjectedScenarioFailure.into());
        }
        let frames = send_operator_request(
            &root.join("operator.sock"),
            OperatorRequest::RestartAppServer,
            WAIT,
        )
        .await?;
        check_equal(
            terminal_classification(&frames)?,
            TerminalClassification::Succeeded,
            "backend restart must actually succeed",
        )?;
        check_equal(
            terminal_snapshot(&frames)?.hosted_readiness(),
            codex_router_host::HostedReadiness::Ready,
            "replacement backend must be ready",
        )?;
        let after_generation = native_generation(&client).await?;
        check(
            after_generation != before_generation,
            "backend generation did not change",
        )?;
        let pids = wait_for_process_ids(&root.join("native-pids.log"), 2).await?;
        check_equal(pids.len(), 2, "exactly one replacement backend expected")?;
        check(
            pids.last().copied() != Some(before_pid),
            "backend PID did not change",
        )?;
        require_process_absent(before_pid)?;
        self.deliver(&client, "registry after restart").await?;
        let peer = self.peer.as_ref().ok_or("peer observer missing")?;
        check_equal(
            peer.count.load(Ordering::Relaxed),
            2,
            "no peer input replay or cross-delivery",
        )
    }

    async fn deliver(&mut self, client: &CollaborationClient, body: &str) -> TestResult {
        let endpoint = EndpointRef {
            service_id: client.identity().service_id.clone(),
            endpoint_id: EndpointId::try_from("claude-local".to_owned())?,
        };
        let inventory = client
            .list_provider_sessions(ProviderSessionListParams {
                endpoint,
                view: NativeSessionView::Loaded,
                scope: NativeSessionScope::Any,
                source: NativeSessionSource::Interactive,
                query: Some(PEER_SESSION.into()),
                page_size: 10,
                cursor: None,
            })
            .await?;
        let target = inventory
            .sessions
            .iter()
            .find_map(|row| match row {
                ProviderSessionSummary::ClaudeCodeInteractive { target, .. }
                    if String::from(target.session_id.clone()) == PEER_SESSION =>
                {
                    Some(target.clone())
                }
                _ => None,
            })
            .ok_or("private registry peer missing from public discovery")?;
        let sent = client
            .send_message(MessageSendRequest {
                target: target.clone(),
                message: PublicMessageContent::HumanUser {
                    text: body.to_owned().try_into()?,
                },
                delivery: MessageDelivery::Auto,
                generation_guard: None,
            })
            .await?;
        check_equal(sent.target.clone(), target.clone(), "public target changed")?;
        check_equal(
            sent.delivery_state,
            PushDeliveryState::Delivered,
            "push not stored Delivered",
        )?;
        check_equal(
            sent.receipt.outcome,
            DeliveryOutcome::PeerMessageWritten,
            "peer route did not accept write",
        )?;
        check_equal(
            sent.receipt.reachability,
            Some(SessionReachability::ClaudeCodePeer),
            "wrong delivery route",
        )?;
        check(
            matches!(
                sent.receipt.client,
                Some(DeliveryClientReceipt::ClaudeCodePeer)
            ),
            "wrong client receipt",
        )?;
        let shown = client
            .router_show(PushRecordShowParams {
                caller: target.clone(),
                reference: sent.push_id.as_str().into(),
            })
            .await?;
        check_equal(shown.record.target, target, "store target changed")?;
        check_equal(
            shown.record.body.as_deref(),
            Some(body),
            "store body changed",
        )?;
        check_equal(
            shown.record.delivery_state,
            PushDeliveryState::Delivered,
            "stored push not settled",
        )?;
        check_equal(
            client.machine_label().as_str(),
            "shared-host-fixture",
            "fixture machine label changed",
        )?;
        let link = format!(
            "router://{}/push/{}",
            String::from(client.identity().service_id.clone()),
            sent.push_id.as_str()
        );
        check_equal(sent.link, link.clone(), "full link changed")?;
        let expected =
            format!("🧑 Owner (unverified) @shared-host-fixture → you · \"{body}\" · {link}");
        let peer = self.peer.as_mut().ok_or("peer observer missing")?;
        let received = tokio::time::timeout(WAIT, peer.received.recv())
            .await?
            .ok_or("peer observer stopped")?;
        check_equal(
            received.auth,
            json!({"type":"auth","token":PEER_TOKEN}),
            "literal auth frame changed",
        )?;
        check_equal(
            received.user,
            json!({"type":"user","message":{"role":"user","content":expected}}),
            "literal recipient frame changed",
        )
    }
}

async fn native_generation(client: &CollaborationClient) -> TestResult<CodexGeneration> {
    client
        .list_endpoints()
        .await?
        .endpoints
        .into_iter()
        .filter(|endpoint| String::from(endpoint.endpoint.endpoint_id.clone()) == "codex-local")
        .flat_map(|endpoint| endpoint.channels)
        .find_map(|channel| match channel {
            ChannelDescription::NativeCodex {
                generation: Some(generation),
                ..
            } => Some(generation),
            _ => None,
        })
        .ok_or_else(|| "ready native generation missing".into())
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}
