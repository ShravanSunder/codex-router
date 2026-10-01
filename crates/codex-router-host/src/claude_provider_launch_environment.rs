//! Router-owned environment for Claude ACP launches.

use crate::ExternalProviderStartup;
use codex_router_core::redaction::SecretString;
use codex_router_secret_store::{SecretStore, file_backend::FileSecretStore, model::SecretKey};
use collaboration_protocol::ProviderKind;
use std::{io, net::SocketAddr, path::Path};

const ANTHROPIC_BASE_URL_ENV: &str = "ANTHROPIC_BASE_URL";
const ANTHROPIC_AUTH_TOKEN_ENV: &str = "ANTHROPIC_AUTH_TOKEN";
const LOCAL_ROUTER_TOKEN_KEY: &str = "local_router_token";

pub(crate) async fn configure_claude_provider_launches(
    startups: Vec<ExternalProviderStartup>,
    router_endpoint: SocketAddr,
    secret_root: Option<&Path>,
) -> io::Result<Vec<ExternalProviderStartup>> {
    let has_claude_launch = startups.iter().any(|startup| {
        matches!(startup, ExternalProviderStartup::Launch(binding) if binding.provider() == ProviderKind::ClaudeCode)
    });
    if !has_claude_launch {
        return Ok(startups);
    }

    let local_token = match secret_root {
        Some(secret_root) => {
            let secret_root = secret_root.to_owned();
            match tokio::task::spawn_blocking(move || read_local_router_token(&secret_root)).await {
                Ok(token) => token,
                Err(error) => Err(format!("failed reading local Router token: {error}")),
            }
        }
        None => Err("Router secret root is unavailable".to_owned()),
    };
    let anthropic_base_url = format!("http://{router_endpoint}/anthropic");

    startups
        .into_iter()
        .map(|startup| match startup {
            ExternalProviderStartup::Launch(mut binding)
                if binding.provider() == ProviderKind::ClaudeCode =>
            {
                match &local_token {
                    Ok(token) => {
                        set_environment_value(
                            &mut binding.launch.environment,
                            ANTHROPIC_BASE_URL_ENV,
                            anthropic_base_url.clone(),
                        );
                        set_environment_value(
                            &mut binding.launch.environment,
                            ANTHROPIC_AUTH_TOKEN_ENV,
                            token.expose_secret().to_owned(),
                        );
                        Ok(ExternalProviderStartup::Launch(binding))
                    }
                    Err(error) => ExternalProviderStartup::unavailable(
                        ProviderKind::ClaudeCode,
                        format!("local Router token required for Claude ACP launch: {error}"),
                        "start Router with Claude routing enabled to provision its local token, then restart Host".to_owned(),
                    )
                    .map_err(io::Error::other),
                }
            }
            unchanged => Ok(unchanged),
        })
        .collect()
}

fn read_local_router_token(secret_root: &Path) -> Result<SecretString, String> {
    let store = FileSecretStore::open_read_only(secret_root).map_err(|error| error.to_string())?;
    let key = SecretKey::new(LOCAL_ROUTER_TOKEN_KEY).map_err(|error| error.to_string())?;
    let token = store.read_secret(&key).map_err(|error| error.to_string())?;
    if token.expose_secret().is_empty() {
        return Err("local Router token file is empty".to_owned());
    }
    Ok(token)
}

fn set_environment_value(environment: &mut Vec<(String, String)>, name: &str, value: String) {
    environment.retain(|(existing_name, _)| existing_name != name);
    environment.push((name.to_owned(), value));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExternalProviderLaunchBinding;
    use tempfile::tempdir;

    fn launch_startups() -> Vec<ExternalProviderStartup> {
        vec![
            ExternalProviderStartup::Launch(
                ExternalProviderLaunchBinding::claude("claude-agent-acp".into(), Vec::new())
                    .expect("Claude ACP launch binding"),
            ),
            ExternalProviderStartup::Launch(
                ExternalProviderLaunchBinding::cursor("agent".into(), vec!["acp".to_owned()])
                    .expect("Cursor launch binding"),
            ),
        ]
    }

    #[tokio::test]
    async fn claude_acp_receives_router_url_and_local_token_without_changing_other_launches() {
        let secrets = tempdir().expect("secret root");
        let store = FileSecretStore::open(secrets.path()).expect("file secret store");
        let token_key = SecretKey::new(LOCAL_ROUTER_TOKEN_KEY).expect("local token key");
        store
            .write_secret(&token_key, &SecretString::new("router-token-canary"))
            .expect("write local token");
        let mut startups = launch_startups();
        let ExternalProviderStartup::Launch(claude) = &mut startups[0] else {
            panic!("Claude launch startup")
        };
        claude.launch.environment = vec![
            ("PATH".to_owned(), "/fixture/bin".to_owned()),
            (
                ANTHROPIC_BASE_URL_ENV.to_owned(),
                "https://api.anthropic.com".to_owned(),
            ),
            (
                ANTHROPIC_AUTH_TOKEN_ENV.to_owned(),
                "direct-login-token".to_owned(),
            ),
        ];

        let configured = configure_claude_provider_launches(
            startups,
            "127.0.0.1:18787".parse().expect("Router endpoint"),
            Some(secrets.path()),
        )
        .await
        .expect("configured provider startups");

        let ExternalProviderStartup::Launch(claude) = &configured[0] else {
            panic!("Claude startup remains available with a local token")
        };
        assert_eq!(
            claude.launch.environment,
            vec![
                ("PATH".to_owned(), "/fixture/bin".to_owned()),
                (
                    ANTHROPIC_BASE_URL_ENV.to_owned(),
                    "http://127.0.0.1:18787/anthropic".to_owned(),
                ),
                (
                    ANTHROPIC_AUTH_TOKEN_ENV.to_owned(),
                    "router-token-canary".to_owned(),
                ),
            ]
        );
        let ExternalProviderStartup::Launch(cursor) = &configured[1] else {
            panic!("Cursor startup remains unchanged")
        };
        assert_eq!(cursor.launch.environment, Vec::<(String, String)>::new());
    }

    #[tokio::test]
    async fn claude_acp_is_unavailable_when_the_local_token_file_is_missing() {
        let secrets = tempdir().expect("secret root");
        let configured = configure_claude_provider_launches(
            launch_startups(),
            "127.0.0.1:8787".parse().expect("Router endpoint"),
            Some(secrets.path()),
        )
        .await
        .expect("missing token is a provider availability result");

        let ExternalProviderStartup::Unavailable { reason, fix, .. } = &configured[0] else {
            panic!("Claude must not launch without the required local token")
        };
        assert!(String::from((*reason).clone()).contains("local Router token required"));
        assert!(String::from((*fix).clone()).contains("restart Host"));
        let ExternalProviderStartup::Launch(cursor) = &configured[1] else {
            panic!("Cursor remains independently available")
        };
        assert_eq!(cursor.launch.environment, Vec::<(String, String)>::new());
    }

    #[tokio::test]
    async fn claude_acp_is_unavailable_when_the_local_token_file_is_empty() {
        let secrets = tempdir().expect("secret root");
        let store = FileSecretStore::open(secrets.path()).expect("file secret store");
        let token_key = SecretKey::new(LOCAL_ROUTER_TOKEN_KEY).expect("local token key");
        store
            .write_secret(&token_key, &SecretString::new(String::new()))
            .expect("write empty local token");

        let configured = configure_claude_provider_launches(
            launch_startups(),
            "127.0.0.1:8787".parse().expect("Router endpoint"),
            Some(secrets.path()),
        )
        .await
        .expect("empty token is a provider availability result");

        let ExternalProviderStartup::Unavailable { reason, .. } = &configured[0] else {
            panic!("Claude must not launch with an empty local token")
        };
        assert!(String::from((*reason).clone()).contains("local Router token file is empty"));
    }

    #[tokio::test]
    async fn cursor_only_startup_does_not_read_a_missing_router_token() {
        let secrets = tempdir().expect("secret root");
        let cursor = launch_startups().pop().expect("Cursor startup");

        let configured = configure_claude_provider_launches(
            vec![cursor.clone()],
            "127.0.0.1:8787".parse().expect("Router endpoint"),
            Some(secrets.path()),
        )
        .await
        .expect("Cursor launch needs no Claude token");

        assert_eq!(configured, vec![cursor]);
    }
}
