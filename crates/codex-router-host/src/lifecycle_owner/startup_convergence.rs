//! Ordered router and app-server startup convergence with retained-child cleanup.

use super::*;
use codex_router_secret_store::{
    file_backend::FileSecretStore,
    local_router_token::{LocalRouterTokenError, LocalRouterTokenService},
};
use std::path::Path;

pub(super) async fn start_router(
    config: &HostConfig,
    router_command: Option<&ChildCommandSpec>,
) -> Result<(RouterCondition, Option<RouterChild>), HostError> {
    let started_at = std::time::Instant::now();
    match probe_router(config.router_endpoint(), config.deadlines().router_start()).await? {
        RouterProbeResult::Compatible => {
            crate::record_debug_readiness_timing("routerReady", started_at);
            Ok((RouterCondition::ExternalReachable, None))
        }
        RouterProbeResult::AuthenticationRequired => Err(HostError::RouterAuthenticationRequired),
        RouterProbeResult::Incompatible => Err(HostError::RouterIncompatible),
        RouterProbeResult::Unavailable => {
            let command = router_command.ok_or(HostError::RouterUnavailable)?;
            let secret_root = config
                .coordination_paths()
                .router_secret_root()
                .ok_or(HostError::RouterSecretRootUnavailable)?;
            provision_local_router_token(&secret_root)?;
            crate::router_credential_migration::migrate_before_router_spawn(&secret_root).await;
            let mut command = command.command();
            let mut child = RouterChild::spawn(&mut command)?;
            let probe_result = match await_router_readiness(
                config.router_endpoint(),
                config.deadlines().router_start(),
            )
            .await
            {
                Ok(result) => result,
                Err(error) => {
                    let _shutdown_outcome = child.shutdown().await?;
                    return Err(HostError::RouterProbe(error));
                }
            };
            match probe_result {
                RouterProbeResult::Compatible => {
                    crate::record_debug_readiness_timing("routerReady", started_at);
                    Ok((RouterCondition::OwnedReachable, Some(child)))
                }
                RouterProbeResult::AuthenticationRequired => {
                    let _shutdown_outcome = child.shutdown().await?;
                    Err(HostError::RouterAuthenticationRequired)
                }
                RouterProbeResult::Incompatible => {
                    let _shutdown_outcome = child.shutdown().await?;
                    Err(HostError::RouterIncompatible)
                }
                RouterProbeResult::Unavailable => {
                    let _shutdown_outcome = child.shutdown().await?;
                    Err(HostError::RouterUnavailable)
                }
            }
        }
    }
}

fn provision_local_router_token(secret_root: &Path) -> Result<(), LocalRouterTokenError> {
    let store = FileSecretStore::open(secret_root)?;
    LocalRouterTokenService::new(store).ensure_local_token(secret_root)?;
    Ok(())
}

async fn await_router_readiness(
    endpoint: std::net::SocketAddr,
    deadline: std::time::Duration,
) -> Result<RouterProbeResult, RouterProbeError> {
    let deadline_at = tokio::time::Instant::now() + deadline;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline_at {
            return Ok(RouterProbeResult::Unavailable);
        }
        match probe_router(endpoint, deadline_at.saturating_duration_since(now)).await? {
            RouterProbeResult::Unavailable => {
                tokio::time::sleep_until(
                    deadline_at
                        .min(tokio::time::Instant::now() + std::time::Duration::from_millis(20)),
                )
                .await;
            }
            terminal => return Ok(terminal),
        }
    }
}

pub(super) async fn start_app_server(
    config: &HostConfig,
    mut launch_plan: AppServerLaunchPlan,
) -> Result<(AppServerChild, AppServerReadiness), HostError> {
    let started_at = std::time::Instant::now();
    let spawn_started_at = std::time::Instant::now();
    let mut child = launch_plan.spawn()?;
    crate::record_debug_readiness_timing("appServerSpawn", spawn_started_at);
    let readiness = child
        .await_readiness(
            config.app_server_socket(),
            config.deadlines().app_server_start(),
            config.deadlines().remote_control(),
        )
        .await;
    match readiness {
        Ok(readiness) => {
            // Schema export is optional and runs only after the native socket is accepting.
            // A failed export retains raw-native access without delaying socket startup.
            launch_plan.prepare_schema().await;
            child.set_schema_export(launch_plan.prepared_schema_export());
            crate::record_debug_readiness_timing("appServerSpawnAndReady", started_at);
            Ok((child, readiness))
        }
        Err(readiness_error) => {
            let _shutdown_outcome = child.shutdown().await?;
            Err(HostError::AppServerReadiness(readiness_error))
        }
    }
}

pub(super) async fn shutdown_owned_router_after_startup_failure(
    router: &mut Option<RouterChild>,
) -> Result<(), HostError> {
    if let Some(child) = router.as_mut() {
        let _shutdown_outcome = child.shutdown().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExternalProviderLaunchBinding, ExternalProviderStartup};
    use codex_router_core::local_auth::LocalRouterTokenRecord;
    use collaboration_protocol::ProviderKind;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener};
    use std::path::PathBuf;

    const TEST_ROUTER_SECRET_ROOT_ENV: &str = "CODEX_ROUTER_HOST_TEST_SECRET_ROOT";
    const TEST_ROUTER_ENDPOINT_ENV: &str = "CODEX_ROUTER_HOST_TEST_ENDPOINT";
    const TEST_ROUTER_TOKEN_OBSERVATION_ENV: &str = "CODEX_ROUTER_HOST_TEST_TOKEN_OBSERVATION";

    #[tokio::test]
    async fn owned_router_startup_provisions_once_before_router_and_claude_acp_launches() {
        let router_root = tempfile::tempdir().expect("isolated Router root");
        let secret_root = router_root.path().join("secrets");
        let router_token_observation_path = router_root.path().join("router-token-at-startup");
        let claude_token_observation_path = router_root.path().join("claude-token-at-launch");
        let router_endpoint = unused_loopback_endpoint();
        assert!(
            !secret_root.exists(),
            "test root starts without a token store"
        );

        let config = HostConfig::new(crate::HostConfigInputs {
            coordination_paths: crate::HostCoordinationPaths::new(
                router_root.path().join("operator.sock"),
                router_root.path().join("instance.lock"),
            ),
            router_endpoint,
            mcp_bind: "127.0.0.1:0".parse().expect("MCP endpoint"),
            app_server_socket: router_root.path().join("app-server.sock"),
            managed_executable: PathBuf::from("/unused/codex"),
            deadlines: crate::HostDeadlines::production(),
        });
        let router_command =
            ChildCommandSpec::new(std::env::current_exe().expect("Host test executable"))
                .with_arguments([
                    "--ignored",
                    "router_child_reads_host_token_and_serves_health",
                ])
                .with_environment(TEST_ROUTER_SECRET_ROOT_ENV, secret_root.as_os_str())
                .with_environment(
                    TEST_ROUTER_ENDPOINT_ENV,
                    std::ffi::OsString::from(router_endpoint.to_string()),
                )
                .with_environment(
                    TEST_ROUTER_TOKEN_OBSERVATION_ENV,
                    router_token_observation_path.as_os_str(),
                )
                .with_output(crate::ChildOutput::Null);

        let (router_condition, mut router_child) = start_router(&config, Some(&router_command))
            .await
            .expect("Host startup convergence starts its owned Router");
        let router_shutdown = match router_child.as_mut() {
            Some(router_child) => router_child.shutdown().await,
            None => panic!("tokenless root must start an owned Router child"),
        };
        router_shutdown.expect("stop the isolated Router fixture");

        let router_token = load_current_local_router_token(&secret_root);
        assert!(
            secret_root.join(".token.lock").is_file(),
            "Host token provisioning uses the shared creation lock"
        );
        let router_observed_token = std::fs::read_to_string(&router_token_observation_path)
            .expect("Router process reads the provisioned token before serving");
        let configured =
            crate::claude_provider_launch_environment::configure_claude_provider_launches(
                vec![ExternalProviderStartup::Launch(
                    ExternalProviderLaunchBinding::claude("claude-agent-acp".into(), Vec::new())
                        .expect("Claude ACP launch binding"),
                )],
                router_endpoint,
                Some(&secret_root),
            )
            .await
            .expect("configured Host provider launches");
        let ExternalProviderStartup::Launch(mut claude_binding) = configured
            .into_iter()
            .next()
            .expect("configured Claude ACP launch")
        else {
            panic!("Claude ACP remains available after Host token provisioning")
        };
        let environment_token = claude_binding
            .launch
            .environment
            .iter()
            .find(|(name, _)| name == "ANTHROPIC_AUTH_TOKEN")
            .map(|(_, value)| value.clone());
        let claude_provider = claude_binding.provider();
        let fake_acp_launch =
            crate::external_provider_runtime::acp_scripted_fixture::AcpFixtureScript::new()
                .expect_request(
                    "initialize",
                    "initialize",
                    serde_json::json!({"protocolVersion": 1}),
                )
                .respond(
                    "initialize",
                    serde_json::json!({
                        "protocolVersion": 1,
                        "agentCapabilities": {},
                        "agentInfo": {"name": "claude-token-startup-fixture", "version": "1"}
                    }),
                )
                .launch();
        let fixture_executable = fake_acp_launch.executable.to_string_lossy().into_owned();
        let fixture_arguments = fake_acp_launch.arguments;
        claude_binding.launch.executable = fake_acp_launch.executable;
        claude_binding.launch.arguments = vec![
            "-u".to_owned(),
            "-c".to_owned(),
            "import os, sys\nwith open(sys.argv[1], 'w', encoding='utf-8') as output:\n    output.write(os.environ['ANTHROPIC_AUTH_TOKEN'])\nexecutable = sys.argv[2]\nos.execvpe(executable, [executable, *sys.argv[3:]], os.environ)\n".to_owned(),
            claude_token_observation_path.to_string_lossy().into_owned(),
            fixture_executable,
        ];
        claude_binding.launch.arguments.extend(fixture_arguments);

        let runtime = crate::ExternalProviderRuntime::initialize(claude_binding.launch)
            .await
            .expect("isolated Claude ACP fixture initializes through the Host runtime");
        let claude_observed_token_result = std::fs::read_to_string(&claude_token_observation_path);
        runtime.shutdown().await;

        assert_eq!(router_condition, RouterCondition::OwnedReachable);
        assert_eq!(router_token.generation().as_u64(), 1);
        assert!(
            router_observed_token == router_token.token().expose_secret(),
            "owned Router reads the one token created by startup convergence"
        );
        assert_eq!(claude_provider, ProviderKind::ClaudeCode);
        assert!(
            environment_token
                .as_deref()
                .is_some_and(|token| token == router_token.token().expose_secret()),
            "Claude ACP launch environment uses the Host-provisioned token"
        );
        let claude_observed_token = claude_observed_token_result
            .expect("Claude ACP subprocess receives its configured token environment");
        assert!(
            claude_observed_token == router_token.token().expose_secret(),
            "Claude ACP subprocess receives the same local token observed by Router"
        );
    }

    #[test]
    #[ignore = "spawned by the Host startup-convergence acceptance test"]
    fn router_child_reads_host_token_and_serves_health() {
        let secret_root = std::env::var_os(TEST_ROUTER_SECRET_ROOT_ENV)
            .map(PathBuf::from)
            .expect("isolated Router secret root");
        let endpoint = std::env::var(TEST_ROUTER_ENDPOINT_ENV)
            .expect("isolated Router endpoint")
            .parse::<SocketAddr>()
            .expect("valid isolated Router endpoint");
        let observation_path = std::env::var_os(TEST_ROUTER_TOKEN_OBSERVATION_ENV)
            .map(PathBuf::from)
            .expect("Router token observation path");

        let store = FileSecretStore::open_read_only(&secret_root).expect("Host token store");
        let token = LocalRouterTokenService::new(store)
            .load_current()
            .expect("Host provisions the token before spawning Router");
        std::fs::write(&observation_path, token.token().expose_secret())
            .expect("record token observed at Router process start");

        let listener = TcpListener::bind(endpoint).expect("bind isolated Router endpoint");
        let body = serde_json::to_string(
            &codex_router_core::router_compatibility::RouterCompatibility::current(false),
        )
        .expect("serialize Host-compatible Router health");
        for incoming in listener.incoming() {
            let mut stream = incoming.expect("Host Router readiness probe");
            let mut health_request_buffer = [0; 1024];
            let _request_bytes = stream
                .read(&mut health_request_buffer)
                .expect("read health request");
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .expect("write Host-compatible Router health");
        }
    }

    fn load_current_local_router_token(secret_root: &Path) -> LocalRouterTokenRecord {
        let store = FileSecretStore::open_read_only(secret_root)
            .expect("Host secret store remains available");
        let service = LocalRouterTokenService::new(store);
        service.load_current().expect("Host local Router token")
    }

    fn unused_loopback_endpoint() -> SocketAddr {
        TcpListener::bind(("127.0.0.1", 0))
            .expect("reserve an available loopback port")
            .local_addr()
            .expect("read the available loopback port")
    }

    #[test]
    fn local_router_token_host_error_names_provisioning_and_preserves_the_cause() {
        let source = LocalRouterTokenError::Random(std::io::Error::other("fixture failure"));
        let error = HostError::from(source);

        assert!(
            error
                .to_string()
                .contains("local Router token could not be ensured")
        );
        assert!(
            error
                .to_string()
                .contains("failed to generate local router token")
        );
        assert!(error.to_string().contains("fixture failure"));
    }
}
