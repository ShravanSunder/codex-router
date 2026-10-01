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

    #[tokio::test]
    async fn owned_router_provisions_a_tokenless_root_for_the_claude_acp_environment() {
        let router_root = tempfile::tempdir().expect("isolated Router root");
        let secret_root = router_root.path().join("secrets");
        assert!(
            !secret_root.exists(),
            "test root starts without a token store"
        );

        provision_local_router_token(&secret_root)
            .expect("Host provisions the local Router token before serving");
        let first_record = load_current_local_router_token(&secret_root);
        provision_local_router_token(&secret_root)
            .expect("repeated Host ensure preserves the existing token");
        let second_record = load_current_local_router_token(&secret_root);

        assert_eq!(first_record.generation(), second_record.generation());
        assert!(
            first_record.token() == second_record.token(),
            "Host ensure preserves the current token"
        );

        let claude_launch = ExternalProviderStartup::Launch(
            ExternalProviderLaunchBinding::claude("claude-agent-acp".into(), Vec::new())
                .expect("Claude ACP launch binding"),
        );
        let configured =
            crate::claude_provider_launch_environment::configure_claude_provider_launches(
                vec![claude_launch],
                "127.0.0.1:18787".parse().expect("Router endpoint"),
                Some(&secret_root),
            )
            .await
            .expect("configured Host provider launches");
        let ExternalProviderStartup::Launch(binding) = &configured[0] else {
            panic!("Claude ACP is available after token provisioning")
        };
        let environment_token = binding
            .launch
            .environment
            .iter()
            .find(|(name, _)| name == "ANTHROPIC_AUTH_TOKEN")
            .map(|(_, value)| value);
        assert!(
            environment_token
                .is_some_and(|token| token.as_str() == first_record.token().expose_secret()),
            "Claude ACP receives the token provisioned by Host"
        );
        assert_eq!(binding.provider(), ProviderKind::ClaudeCode);
    }

    fn load_current_local_router_token(secret_root: &Path) -> LocalRouterTokenRecord {
        let store = FileSecretStore::open_read_only(secret_root)
            .expect("Host secret store remains available");
        let service = LocalRouterTokenService::new(store);
        service.load_current().expect("Host local Router token")
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
