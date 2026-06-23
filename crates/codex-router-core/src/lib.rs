//! Shared primitives for codex-router.

pub mod affinity;
pub mod audit;
pub mod config;
pub mod error;
pub mod ids;
pub mod local_auth;
pub mod redaction;
pub mod routes;

/// Returns this crate's package name.
#[must_use]
pub const fn package_name() -> &'static str {
    "codex-router-core"
}

#[cfg(test)]
mod tests {
    use super::package_name;
    use crate::audit::AuditEvent;
    use crate::audit::AuditOutcome;
    use crate::audit::RouteKind;
    use crate::config::RouterConfig;
    use crate::ids::AccountId;
    use crate::ids::RequestId;
    use crate::ids::TokenGeneration;
    use crate::local_auth::LocalAuthError;
    use crate::local_auth::LocalRouterAuth;
    use crate::local_auth::LocalRouterTokenRecord;
    use crate::redaction::SecretString;

    #[test]
    fn reports_package_name() {
        assert_eq!(package_name(), "codex-router-core");
    }

    #[test]
    fn config_accepts_loopback_listener_and_private_audit_file() {
        let config_result = toml::from_str::<RouterConfig>(
            r#"
            [server]
            listen_host = "127.0.0.1"
            port = 8787
            router_root = "/tmp/codex-router-test"

            [auth]
            local_token_env = "CODEX_ROUTER_TOKEN"

            [audit]
            sink = "file"
            "#,
        );
        let config = match config_result {
            Ok(config) => config,
            Err(error) => panic!("valid config should parse: {error}"),
        };

        if let Err(error) = config.validate() {
            panic!("loopback listener should be accepted: {error}");
        }
        assert_eq!(config.server.port, 8787);
        assert_eq!(config.auth.local_token_env, "CODEX_ROUTER_TOKEN");
    }

    #[test]
    fn config_denies_unknown_and_forbidden_fields() {
        let error = toml::from_str::<RouterConfig>(
            r#"
            [server]
            listen_host = "127.0.0.1"
            port = 8787
            router_root = "/tmp/codex-router-test"
            provider_timeout_ms = 1000

            [auth]
            local_token_env = "CODEX_ROUTER_TOKEN"

            [audit]
            sink = "file"
            "#,
        )
        .err();
        let error = match error {
            Some(error) => error,
            None => panic!("unknown provider policy fields should be rejected"),
        };

        assert!(error.to_string().contains("provider_timeout_ms"));
    }

    #[test]
    fn config_rejects_non_loopback_listener_values() {
        for listen_host in ["0.0.0.0", "::", "192.168.1.10"] {
            let config = RouterConfig::for_test(listen_host);
            let error = match config.validate() {
                Ok(()) => panic!("non-loopback listener should be rejected"),
                Err(error) => error,
            };

            assert!(error.to_string().contains("non-loopback"));
        }
    }

    #[test]
    fn secret_string_redacts_debug_display_and_json() {
        let secret = SecretString::new("token-canary-value");

        assert_eq!(format!("{secret}"), "[REDACTED]");
        assert_eq!(format!("{secret:?}"), "SecretString([REDACTED])");

        let event = AuditEvent::local_auth_rejected(
            RequestId::new("req_123"),
            RouteKind::Responses,
            AuditOutcome::Rejected,
            secret,
        );
        let json = match serde_json::to_string(&event) {
            Ok(json) => json,
            Err(error) => panic!("audit event should serialize: {error}"),
        };

        assert!(json.contains("local_auth_rejected"));
        assert!(!json.contains("token-canary-value"));
        assert!(!json.contains("prompt"));
        assert!(!json.contains("body"));
    }

    #[test]
    fn ids_reject_empty_values_and_keep_readable_debug() {
        assert!(AccountId::new("").is_err());
        let account_id = match AccountId::new("acct_primary") {
            Ok(account_id) => account_id,
            Err(error) => panic!("valid account id should parse: {error}"),
        };
        assert_eq!(format!("{account_id:?}"), "AccountId(\"acct_primary\")");
    }

    #[test]
    fn local_auth_rejects_missing_empty_wrong_and_old_tokens() {
        let current = LocalRouterTokenRecord::new(
            SecretString::new("current-token"),
            TokenGeneration::new(3),
        );
        let old =
            LocalRouterTokenRecord::new(SecretString::new("old-token"), TokenGeneration::new(2));
        let auth = LocalRouterAuth::new(current, vec![old]);

        assert_eq!(auth.validate(None), Err(LocalAuthError::Missing));
        assert_eq!(auth.validate(Some("")), Err(LocalAuthError::Empty));
        assert_eq!(
            auth.validate(Some("wrong-token")),
            Err(LocalAuthError::Wrong)
        );
        assert_eq!(auth.validate(Some("old-token")), Err(LocalAuthError::Old));
        assert_eq!(
            auth.validate(Some("current-token")),
            Ok(TokenGeneration::new(3))
        );
    }

    #[test]
    fn local_token_record_redacts_token_but_exposes_generation() {
        let record =
            LocalRouterTokenRecord::new(SecretString::new("token-canary"), TokenGeneration::new(7));

        assert_eq!(record.generation(), TokenGeneration::new(7));
        assert_eq!(
            format!("{record:?}"),
            "LocalRouterTokenRecord { token: SecretString([REDACTED]), generation: TokenGeneration(7) }"
        );
    }
}
