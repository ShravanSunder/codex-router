//! Loopback proxy boundary for codex-router.

pub mod account_selection;
mod credential_runtime;
pub mod headers;
pub mod http_sse;
pub mod local_auth;
pub mod routes;
mod secret_store_factory;
pub mod server;
pub mod upstream;
pub mod websocket;

/// Returns this crate's package name.
#[must_use]
pub const fn package_name() -> &'static str {
    "codex-router-proxy"
}

#[cfg(test)]
mod tests {
    use super::package_name;
    use crate::account_selection::AccountDecisionSelector;
    use crate::account_selection::QuotaAwareAccountSelector;
    use crate::account_selection::QuotaAwareAccountSelectorError;
    use crate::account_selection::QuotaAwareAccountState;
    use crate::account_selection::RepositoryBackedAccountSelector;
    use crate::account_selection::RouteBandAccountHolds;
    use crate::account_selection::RouteBandWeightedSelectors;
    use crate::account_selection::SelectedAccountDecision;
    use crate::credential_runtime::ProxyCredentialResolver;
    use crate::headers::Header;
    use crate::headers::HeaderCollection;
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
    use crate::routes::Method;
    use crate::routes::RouteClass;
    use crate::routes::RouteKind;
    use crate::routes::classify_route;
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
    use crate::websocket::AuthenticatedWebSocketRouter;
    use crate::websocket::BlockingWebSocketTunnel;
    use crate::websocket::FirstFramePolicy;
    use crate::websocket::WebSocketCloseReason;
    use crate::websocket::WebSocketFirstFrameDecision;
    use crate::websocket::WebSocketFrame;
    use crate::websocket::WebSocketHandshakeRequest;
    use crate::websocket::WebSocketProtocolRouter;
    use codex_router_auth::resolver::CredentialRefreshClient;
    use codex_router_auth::resolver::CredentialResolverError;
    use codex_router_auth::resolver::NoopCredentialRefreshClient;
    use codex_router_auth::resolver::ProviderCredentialResolver;
    use codex_router_auth::resolver::ResolvedProviderCredential;
    use codex_router_auth::resolver::RouterCredentialResolver;
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
    use codex_router_core::ids::RequestId;
    use codex_router_core::ids::TokenGeneration;
    use codex_router_core::local_auth::LocalAuthError;
    use codex_router_core::local_auth::LocalRouterAuth;
    use codex_router_core::local_auth::LocalRouterTokenRecord;
    use codex_router_core::redaction::SecretString;
    use codex_router_quota::snapshot::SnapshotFreshness;
    use codex_router_secret_store::SecretStore;
    use codex_router_secret_store::account_tokens::AccountCredentialBundle;
    use codex_router_secret_store::account_tokens::account_credential_bundle_key;
    use codex_router_secret_store::affinity_secret::load_or_create_router_affinity_hash_secret;
    use codex_router_secret_store::file_backend::FileSecretStore;
    use codex_router_state::account::AccountRecord;
    use codex_router_state::account::AccountStatus;
    use codex_router_state::affinity_owner::AffinitySourceTransport;
    use codex_router_state::affinity_owner::PreviousResponseAffinityOwnerLookup;
    use codex_router_state::affinity_owner::PreviousResponseAffinityOwnerRecord;
    use codex_router_state::quota_snapshot::PersistedQuotaSnapshot;
    use codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow;
    use codex_router_state::quota_snapshot::QuotaSnapshotSource;
    use codex_router_state::quota_snapshot::SelectorQuotaWindowStatus;
    use codex_router_state::repositories::AccountStateRepository;
    use codex_router_state::repositories::AffinityRepository;
    use codex_router_state::repositories::QuotaSnapshotRepository;
    use codex_router_state::repositories::SelectorQuotaRepository;
    use codex_router_state::sqlite::SqliteStateStore;
    use std::cell::RefCell;
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
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;
    use std::time::Instant;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;
    use tungstenite::Message;
    use tungstenite::accept_hdr;
    use tungstenite::client::IntoClientRequest;
    use tungstenite::connect;
    use tungstenite::handshake::server::Request;
    use tungstenite::handshake::server::Response;
    use tungstenite::http::HeaderValue;

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);
    static TEST_AFFINITY_SECRET_PROVIDER: TestAffinitySecretProvider = TestAffinitySecretProvider;

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    struct TestAffinitySecretProvider;

    impl HttpAffinitySecretProvider for TestAffinitySecretProvider {
        fn load_or_create_affinity_secret(
            &self,
        ) -> Result<RouterAffinityHashSecret, HttpProxyError> {
            Ok(test_affinity_secret())
        }
    }

    #[derive(Clone, Debug, Default)]
    struct RecordingAffinityOwnerRecorder {
        records: Arc<Mutex<Vec<PreviousResponseAffinityOwnerRecord>>>,
    }

    impl RecordingAffinityOwnerRecorder {
        fn take_records(&self) -> Vec<PreviousResponseAffinityOwnerRecord> {
            self.records
                .lock()
                .expect("test recorder lock should be available")
                .drain(..)
                .collect()
        }
    }

    impl HttpAffinityOwnerRecorder for RecordingAffinityOwnerRecorder {
        fn record_affinity_owner(
            &self,
            owner: &PreviousResponseAffinityOwnerRecord,
        ) -> Result<(), HttpProxyError> {
            self.records
                .lock()
                .expect("test recorder lock should be available")
                .push(owner.clone());
            Ok(())
        }
    }

    #[test]
    fn reports_package_name() {
        assert_eq!(package_name(), "codex-router-proxy");
    }

    #[test]
    fn proxy_auth_gate_rejects_before_selection() {
        let current = LocalRouterTokenRecord::new(
            SecretString::new("current-token"),
            TokenGeneration::new(1),
        );
        let auth = LocalRouterAuth::new(current, Vec::new());
        let gate = ProxyLocalAuthGate::new(auth);

        assert_eq!(gate.authorize(None), Err(LocalAuthError::Missing));
        assert_eq!(
            gate.authorize(Some("current-token")),
            Ok(TokenGeneration::new(1))
        );
    }

    #[test]
    fn route_classifier_supports_required_codex_routes_and_rejects_realtime() {
        assert_eq!(
            classify_route(Method::Post, "/v1/responses", false),
            RouteClass::Supported(RouteKind::Responses)
        );
        assert_eq!(
            classify_route(Method::Post, "/v1/responses", true),
            RouteClass::Supported(RouteKind::ResponsesWebSocket)
        );
        assert_eq!(
            classify_route(Method::Get, "/v1/models", false),
            RouteClass::Supported(RouteKind::Models)
        );
        assert_eq!(
            classify_route(Method::Post, "/v1/memories/trace_summarize", false),
            RouteClass::Supported(RouteKind::MemoriesTraceSummarize)
        );
        assert_eq!(
            classify_route(Method::Post, "/v1/responses/compact", false),
            RouteClass::Supported(RouteKind::ResponsesCompact)
        );
        assert_eq!(
            classify_route(Method::Get, "/v1/realtime", true),
            RouteClass::Rejected {
                reason: "unsupported_path"
            }
        );
    }

    #[test]
    fn upstream_request_strips_local_and_hop_headers_and_injects_auth_once() {
        let request = UpstreamRequestBuilder::new(RouteKind::Responses)
            .with_header(Header::new("X-Codex-Router-Token", "local-token-canary"))
            .with_header(Header::new("Host", "127.0.0.1:8787"))
            .with_header(Header::new("Content-Length", "42"))
            .with_header(Header::new("Connection", "upgrade"))
            .with_header(Header::new("Upgrade", "websocket"))
            .with_header(Header::new("Authorization", "Bearer user-supplied"))
            .with_header(Header::new("ChatGPT-Account-Id", "hostile-account-id"))
            .with_header(Header::new("Cookie", "session=user-cookie"))
            .with_header(Header::new("OpenAI-Beta", "responses=v1"))
            .with_body(br#"{"model":"gpt-5","unknown_codex_field":{"kept":true}}"#.to_vec())
            .build_with_chatgpt_account_id(
                SecretString::new("upstream-account-token"),
                Some("chatgpt-account-id-canary"),
            );

        assert_eq!(request.route_kind(), RouteKind::Responses);
        assert_eq!(
            request.body(),
            br#"{"model":"gpt-5","unknown_codex_field":{"kept":true}}"#
        );
        assert_eq!(request.headers().value("openai-beta"), Some("responses=v1"));
        assert_eq!(
            request.headers().values("authorization"),
            vec!["Bearer upstream-account-token"]
        );
        assert_eq!(
            request.headers().values("chatgpt-account-id"),
            vec!["chatgpt-account-id-canary"]
        );
        assert_eq!(request.headers().value("x-codex-router-token"), None);
        assert_eq!(request.headers().value("host"), None);
        assert_eq!(request.headers().value("content-length"), None);
        assert_eq!(request.headers().value("connection"), None);
        assert_eq!(request.headers().value("upgrade"), None);
        assert_eq!(request.headers().value("cookie"), None);
    }

    #[test]
    fn upstream_endpoint_joins_base_url_with_codex_path_without_losing_query() {
        let endpoint = match UpstreamEndpoint::new("https://api.openai.com/v1") {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("upstream endpoint should validate: {error}"),
        };

        assert_eq!(
            endpoint.url_for_path("/v1/responses?stream=true&cursor=abc"),
            "https://api.openai.com/v1/responses?stream=true&cursor=abc"
        );
        assert_eq!(
            endpoint.url_for_path("v1/models"),
            "https://api.openai.com/v1/models"
        );
    }

    #[test]
    fn upstream_endpoint_maps_chatgpt_backend_api_to_codex_runtime_paths() {
        let endpoint = match UpstreamEndpoint::new("https://chatgpt.com/backend-api") {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("upstream endpoint should validate: {error}"),
        };

        assert_eq!(
            endpoint.url_for_path("/v1/responses?stream=true&cursor=abc"),
            "https://chatgpt.com/backend-api/codex/responses?stream=true&cursor=abc"
        );
        assert_eq!(
            endpoint.url_for_path("/v1/responses/compact"),
            "https://chatgpt.com/backend-api/codex/responses/compact"
        );
        assert_eq!(
            endpoint.websocket_url_for_path("/v1/responses"),
            "wss://chatgpt.com/backend-api/codex/responses"
        );
    }

    #[test]
    fn http_upstream_transport_forwards_real_request_to_mock_server() {
        let listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(error) => panic!("mock upstream should bind: {error}"),
        };
        let server_address = match listener.local_addr() {
            Ok(address) => address,
            Err(error) => panic!("mock upstream address should be readable: {error}"),
        };
        let (request_sender, request_receiver) = mpsc::channel();
        let server_thread = thread::spawn(move || {
            let (mut stream, _peer_address) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("mock upstream should accept one connection: {error}"),
            };
            let mut request = String::new();
            if let Err(error) = stream.read_to_string(&mut request) {
                panic!("mock upstream should read request: {error}");
            }
            if let Err(error) = request_sender.send(request) {
                panic!("mock upstream request should record: {error}");
            }
            if let Err(error) = stream.write_all(
                b"HTTP/1.1 201 Created\r\nETag: upstream-etag\r\nContent-Length: 16\r\n\r\n{\"ok\":true}\nrest",
            ) {
                panic!("mock upstream should write response: {error}");
            }
        });
        let endpoint = match UpstreamEndpoint::new(format!("http://{server_address}/v1")) {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("mock endpoint should validate: {error}"),
        };
        let upstream = HttpUpstreamTransport::new(endpoint);
        let service = HttpProxyService::new(&upstream);

        let response = match service.handle(
            HttpProxyRequest::new(Method::Post, "/v1/responses?stream=true")
                .with_header(Header::new("X-Codex-Router-Token", "local-token"))
                .with_header(Header::new("Authorization", "Bearer wrong"))
                .with_header(Header::new("OpenAI-Beta", "responses=v1"))
                .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
            SecretString::new("selected-upstream-token"),
            None,
        ) {
            Ok(response) => response,
            Err(error) => panic!("HTTP upstream transport should forward request: {error}"),
        };

        assert_eq!(response.status(), 201);
        assert_eq!(response.headers().value("etag"), Some("upstream-etag"));
        assert_eq!(response.body(), b"{\"ok\":true}\nrest");
        let recorded_request = match request_receiver.recv() {
            Ok(request) => request,
            Err(error) => panic!("mock upstream request should be recorded: {error}"),
        };
        assert!(recorded_request.starts_with("POST /v1/responses?stream=true HTTP/1.1\r\n"));
        assert!(recorded_request.contains("authorization: Bearer selected-upstream-token\r\n"));
        assert!(recorded_request.contains("openai-beta: responses=v1\r\n"));
        assert!(!recorded_request.contains("X-Codex-Router-Token"));
        assert!(!recorded_request.contains("Bearer wrong"));

        match server_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("mock upstream thread panicked: {error:?}"),
        }
    }

    #[test]
    fn http_upstream_transport_accepts_https_endpoints_at_send_time() {
        let endpoint = match UpstreamEndpoint::new("https://127.0.0.1:1/v1") {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("https endpoint should validate: {error}"),
        };
        let upstream = HttpUpstreamTransport::new(endpoint);
        let service = HttpProxyService::new(&upstream);

        let error = match service.handle(
            HttpProxyRequest::new(Method::Get, "/v1/models"),
            SecretString::new("selected-upstream-token"),
            None,
        ) {
            Ok(_response) => panic!("closed local port should not produce a response"),
            Err(error) => error,
        };

        match error {
            HttpProxyError::Upstream { message } => {
                assert_ne!(message, "http upstream transport requires http endpoint");
            }
            other => panic!("expected upstream error, got {other:?}"),
        }
    }

    #[test]
    fn http_proxy_forwards_supported_routes_and_preserves_models_etag() {
        let upstream = RecordingUpstream::new(HttpProxyResponse::new(
            200,
            HeaderCollection::new(vec![Header::new("ETag", "models-etag")]),
            br#"{"object":"list"}"#.to_vec(),
        ));
        let service = HttpProxyService::new(&upstream);
        let response = match service.handle(
            HttpProxyRequest::new(Method::Get, "/v1/models")
                .with_header(Header::new("X-Codex-Router-Token", "local-token"))
                .with_header(Header::new("Authorization", "Bearer wrong"))
                .with_body(Vec::new()),
            SecretString::new("selected-upstream-token"),
            None,
        ) {
            Ok(response) => response,
            Err(error) => panic!("models request should forward: {error}"),
        };

        assert_eq!(response.status(), 200);
        assert_eq!(response.headers().value("etag"), Some("models-etag"));
        assert_eq!(response.body(), br#"{"object":"list"}"#);

        let recorded = upstream.take_recorded();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].method(), Method::Get);
        assert_eq!(recorded[0].path(), "/v1/models");
        assert_eq!(
            recorded[0].headers().values("authorization"),
            vec!["Bearer selected-upstream-token"]
        );
        assert_eq!(recorded[0].headers().value("x-codex-router-token"), None);
    }

    #[test]
    fn http_proxy_preserves_responses_body_bytes_without_interpreting_unknown_fields() {
        let upstream = RecordingUpstream::new(HttpProxyResponse::new(
            200,
            HeaderCollection::default(),
            b"data: kept\n\n".to_vec(),
        ));
        let service = HttpProxyService::new(&upstream);
        let body = br#"{"unknown_codex_field":{"kept":true}}"#.to_vec();
        let response = match service.handle(
            HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_header(Header::new("Accept", "text/event-stream"))
                .with_body(body.clone()),
            SecretString::new("selected-upstream-token"),
            None,
        ) {
            Ok(response) => response,
            Err(error) => panic!("responses request should forward: {error}"),
        };

        assert_eq!(response.body(), b"data: kept\n\n");
        let recorded = upstream.take_recorded();
        assert_eq!(recorded[0].body(), body.as_slice());
        assert_eq!(
            recorded[0].headers().value("accept"),
            Some("text/event-stream")
        );
    }

    #[test]
    fn http_proxy_resolver_refreshes_expired_access_token_before_upstream_egress() {
        let upstream = RecordingUpstream::new(HttpProxyResponse::new(
            200,
            HeaderCollection::default(),
            b"ok".to_vec(),
        ));
        let selector = RecordingSelector::new();
        let resolver = RecordingProviderCredentialResolver::new("resolved-upstream-token");
        let auth_gate = local_auth_gate();
        let service =
            AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

        let response = must_ok(
            service.handle_request(
                HttpProxyRequest::new(Method::Post, "/v1/responses")
                    .with_header(Header::new("X-Codex-Router-Token", "current-token"))
                    .with_header(Header::new("Authorization", "Bearer stale-token-canary"))
                    .with_body(br#"{"input":"hi"}"#.to_vec()),
            ),
        );

        assert_eq!(response.status(), 200);
        assert_eq!(
            selector.take_recorded(),
            vec![("/v1/responses".to_owned(), TokenGeneration::new(1))]
        );
        assert_eq!(resolver.take_recorded(), vec!["acct_selected".to_owned()]);
        let recorded = upstream.take_recorded();
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            recorded[0].headers().values("authorization"),
            vec!["Bearer resolved-upstream-token"]
        );
        assert_ne!(
            recorded[0].headers().values("authorization"),
            vec!["Bearer stale-token-canary"]
        );
    }

    #[test]
    fn http_proxy_preserves_query_string_after_route_classification() {
        let upstream = RecordingUpstream::new(HttpProxyResponse::new(
            200,
            HeaderCollection::default(),
            b"data: kept\n\n".to_vec(),
        ));
        let service = HttpProxyService::new(&upstream);
        let response = match service.handle(
            HttpProxyRequest::new(Method::Post, "/v1/responses?stream=true&cursor=abc")
                .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
            SecretString::new("selected-upstream-token"),
            None,
        ) {
            Ok(response) => response,
            Err(error) => panic!("responses request with query should forward: {error}"),
        };

        assert_eq!(response.body(), b"data: kept\n\n");
        let recorded = upstream.take_recorded();
        assert_eq!(recorded[0].path(), "/v1/responses?stream=true&cursor=abc");
        assert_eq!(recorded[0].route_kind(), RouteKind::Responses);
    }

    #[test]
    fn http_proxy_rejects_unsupported_paths_before_upstream() {
        let upstream = RecordingUpstream::new(HttpProxyResponse::new(
            200,
            HeaderCollection::default(),
            Vec::new(),
        ));
        let service = HttpProxyService::new(&upstream);
        let error = match service.handle(
            HttpProxyRequest::new(Method::Post, "/v1/realtime").with_body(Vec::new()),
            SecretString::new("selected-upstream-token"),
            None,
        ) {
            Ok(response) => panic!("unsupported path should fail closed: {response:?}"),
            Err(error) => error,
        };

        assert_eq!(
            error,
            HttpProxyError::Rejected {
                reason: "unsupported_path"
            }
        );
        assert!(upstream.take_recorded().is_empty());
    }

    #[test]
    fn authenticated_http_proxy_rejects_missing_token_before_selection_or_upstream() {
        let upstream = RecordingUpstream::new(HttpProxyResponse::new(
            200,
            HeaderCollection::default(),
            Vec::new(),
        ));
        let selector = RecordingSelector::new();
        let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
        let auth_gate = local_auth_gate();
        let service =
            AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

        let error = match service.handle_request(
            HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
        ) {
            Ok(response) => panic!("missing token should reject locally: {response:?}"),
            Err(error) => error,
        };

        assert_eq!(
            error,
            HttpProxyError::LocalAuth {
                reason: LocalAuthError::Missing
            }
        );
        assert!(selector.take_recorded().is_empty());
        assert!(upstream.take_recorded().is_empty());
    }

    #[test]
    fn authenticated_http_proxy_requires_affinity_secret_before_response_selection() {
        let upstream = RecordingUpstream::new(HttpProxyResponse::new(
            200,
            HeaderCollection::default(),
            b"should-not-send".to_vec(),
        ));
        let selector = RecordingSelector::new();
        let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
        let auth_gate = local_auth_gate();
        let service =
            AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream);

        let error = match service.handle_request(
            HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_header(Header::new("X-Codex-Router-Token", "current-token"))
                .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
        ) {
            Ok(response) => panic!("missing affinity secret provider should reject: {response:?}"),
            Err(error) => error,
        };

        assert_eq!(
            error,
            HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::SecretUnavailable
            }
        );
        assert!(selector.take_recorded().is_empty());
        assert!(resolver.take_recorded().is_empty());
        assert!(upstream.take_recorded().is_empty());
    }

    #[test]
    fn authenticated_http_proxy_selects_after_auth_and_forwards_selected_token() {
        let upstream = RecordingUpstream::new(HttpProxyResponse::new(
            200,
            HeaderCollection::default(),
            b"data: kept\n\n".to_vec(),
        ));
        let selector = RecordingSelector::new();
        let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
        let auth_gate = local_auth_gate();
        let service =
            AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

        let response = match service.handle_request(
            HttpProxyRequest::new(Method::Post, "/v1/responses?stream=true")
                .with_header(Header::new("X-Codex-Router-Token", "current-token"))
                .with_header(Header::new("Authorization", "Bearer wrong"))
                .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
        ) {
            Ok(response) => response,
            Err(error) => panic!("authorized request should forward: {error}"),
        };

        assert_eq!(response.body(), b"data: kept\n\n");
        let selected = selector.take_recorded();
        assert_eq!(
            selected,
            vec![(
                "/v1/responses?stream=true".to_owned(),
                TokenGeneration::new(1)
            )]
        );
        let recorded = upstream.take_recorded();
        assert_eq!(recorded[0].path(), "/v1/responses?stream=true");
        assert_eq!(
            recorded[0].headers().values("authorization"),
            vec!["Bearer selected-upstream-token"]
        );
        assert_eq!(recorded[0].headers().value("x-codex-router-token"), None);
    }

    #[test]
    fn authenticated_http_proxy_records_top_level_response_id_owner() {
        let upstream = RecordingUpstream::new(HttpProxyResponse::new(
            200,
            HeaderCollection::default(),
            br#"{"id":"resp_top_level","output":[{"id":"resp_nested"}]}"#.to_vec(),
        ));
        let selector = RecordingSelector::new();
        let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
        let auth_gate = local_auth_gate();
        let recorder = RecordingAffinityOwnerRecorder::default();
        let service =
            AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER)
                .with_affinity_owner_recorder(Arc::new(recorder.clone()));

        let response = must_ok(
            service.handle_request(
                HttpProxyRequest::new(Method::Post, "/v1/responses")
                    .with_header(Header::new("X-Codex-Router-Token", "current-token"))
                    .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
            ),
        );

        assert_eq!(response.status(), 200);
        let records = recorder.take_records();
        assert_eq!(records.len(), 1);
        let owner = &records[0];
        assert_eq!(owner.account_id().as_str(), "acct_selected");
        assert_eq!(owner.credential_generation(), 1);
        assert_eq!(
            owner.route_band(),
            codex_router_core::routes::RouteBand::Responses
        );
        assert_eq!(owner.source_transport(), AffinitySourceTransport::HttpSse);
        let expected_hash = must_ok(hash_previous_response_id(
            &test_affinity_secret(),
            &must_ok(PreviousResponseId::new("resp_top_level")),
        ));
        assert_eq!(owner.affinity_key_hash(), &expected_hash);
        assert_ne!(owner.affinity_key_hash().as_str(), "resp_top_level");
    }

    #[test]
    fn authenticated_http_proxy_ignores_nested_response_ids() {
        let upstream = RecordingUpstream::new(HttpProxyResponse::new(
            200,
            HeaderCollection::default(),
            br#"{"output":[{"id":"resp_nested"}]}"#.to_vec(),
        ));
        let selector = RecordingSelector::new();
        let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
        let auth_gate = local_auth_gate();
        let recorder = RecordingAffinityOwnerRecorder::default();
        let service =
            AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER)
                .with_affinity_owner_recorder(Arc::new(recorder.clone()));

        let response = must_ok(
            service.handle_request(
                HttpProxyRequest::new(Method::Post, "/v1/responses")
                    .with_header(Header::new("X-Codex-Router-Token", "current-token"))
                    .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
            ),
        );

        assert_eq!(response.status(), 200);
        assert!(recorder.take_records().is_empty());
    }

    #[test]
    fn authenticated_http_proxy_records_streaming_sse_response_id_after_body_read() {
        let upstream = RecordingUpstream::new(HttpProxyResponse::new(
            200,
            HeaderCollection::new(vec![Header::new("Content-Type", "text/event-stream")]),
            b"data: {\"id\":\"resp_stream\"}\n\ndata: [DONE]\n\n".to_vec(),
        ));
        let selector = RecordingSelector::new();
        let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
        let auth_gate = local_auth_gate();
        let recorder = RecordingAffinityOwnerRecorder::default();
        let service =
            AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER)
                .with_affinity_owner_recorder(Arc::new(recorder.clone()));

        let mut response = must_ok(
            service.handle_streaming_request(
                HttpProxyRequest::new(Method::Post, "/v1/responses")
                    .with_header(Header::new("X-Codex-Router-Token", "current-token"))
                    .with_body(br#"{"model":"gpt-5","stream":true}"#.to_vec()),
            ),
        );
        assert!(recorder.take_records().is_empty());
        let mut body = Vec::new();
        must_ok(response.body_mut().read_to_end(&mut body));

        assert_eq!(body, b"data: {\"id\":\"resp_stream\"}\n\ndata: [DONE]\n\n");
        let records = recorder.take_records();
        assert_eq!(records.len(), 1);
        let expected_hash = must_ok(hash_previous_response_id(
            &test_affinity_secret(),
            &must_ok(PreviousResponseId::new("resp_stream")),
        ));
        assert_eq!(records[0].affinity_key_hash(), &expected_hash);
    }

    #[test]
    fn authenticated_http_proxy_accepts_codex_env_key_authorization_bearer() {
        let upstream = RecordingUpstream::new(HttpProxyResponse::new(
            200,
            HeaderCollection::default(),
            b"data: kept\n\n".to_vec(),
        ));
        let selector = RecordingSelector::new();
        let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
        let auth_gate = local_auth_gate();
        let service =
            AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

        let response = match service.handle_request(
            HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_header(Header::new("Authorization", "Bearer current-token"))
                .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
        ) {
            Ok(response) => response,
            Err(error) => panic!("authorization bearer should satisfy local auth: {error}"),
        };

        assert_eq!(response.body(), b"data: kept\n\n");
        assert_eq!(
            selector.take_recorded(),
            vec![("/v1/responses".to_owned(), TokenGeneration::new(1))]
        );
        let recorded = upstream.take_recorded();
        assert_eq!(
            recorded[0].headers().values("authorization"),
            vec!["Bearer selected-upstream-token"]
        );
        assert!(
            !recorded[0]
                .headers()
                .values("authorization")
                .contains(&"Bearer current-token")
        );
    }

    #[test]
    fn http_proxy_missing_refresh_token_fails_closed_before_upstream_egress() {
        let upstream = RecordingUpstream::new(HttpProxyResponse::new(
            200,
            HeaderCollection::default(),
            b"should-not-send".to_vec(),
        ));
        let selector = RecordingSelector::new();
        let resolver =
            RejectingProviderCredentialResolver::new(CredentialResolverError::RefreshUnavailable);
        let auth_gate = local_auth_gate();
        let service =
            AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

        let error = match service.handle_request(
            HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_header(Header::new("X-Codex-Router-Token", "current-token"))
                .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
        ) {
            Ok(response) => panic!("missing refresh token should fail closed: {response:?}"),
            Err(error) => error,
        };

        assert_eq!(
            error,
            HttpProxyError::ProviderCredential {
                reason: CredentialResolverError::RefreshUnavailable
            }
        );
        assert_eq!(resolver.take_recorded(), vec!["acct_selected".to_owned()]);
        assert!(upstream.take_recorded().is_empty());
    }

    #[test]
    fn authenticated_http_proxy_audits_selection_rejection_after_local_auth() {
        let temp_dir = ProxyTestTempDir::new("http_selection_rejection_audit");
        let audit_path = temp_dir.path().join("audit").join("events.jsonl");
        let audit_sink = AuditFileSink::new(audit_path.clone());
        let upstream = RecordingUpstream::new(HttpProxyResponse::new(
            200,
            HeaderCollection::default(),
            b"should-not-send".to_vec(),
        ));
        let selector = RejectingSelector::new(QuotaAwareAccountSelectorError::NoEligibleAccounts);
        let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
        let auth_gate = local_auth_gate();
        let service =
            AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER)
                .with_audit_sink(&audit_sink);

        let error = match service.handle_request(
            HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_header(Header::new("X-Codex-Router-Token", "current-token"))
                .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
        ) {
            Ok(response) => panic!("selection rejection should fail closed: {response:?}"),
            Err(error) => error,
        };

        assert_eq!(
            error,
            HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::NoEligibleAccounts
            }
        );
        assert!(resolver.take_recorded().is_empty());
        assert!(upstream.take_recorded().is_empty());
        let audit_contents = must_ok(fs::read_to_string(&audit_path));
        assert!(audit_contents.contains("\"transport_kind\":\"http\""));
        assert!(audit_contents.contains("\"decision_reason\":\"selection_rejected\""));
        assert!(audit_contents.contains("\"error_class\":\"selection\""));
        assert!(audit_contents.contains("\"response_commit_state\":\"not_committed\""));
        assert!(!audit_contents.contains("current-token"));
    }

    #[test]
    fn authenticated_http_proxy_audits_provider_credential_rejection_after_selection() {
        let temp_dir = ProxyTestTempDir::new("http_credential_rejection_audit");
        let audit_path = temp_dir.path().join("audit").join("events.jsonl");
        let audit_sink = AuditFileSink::new(audit_path.clone());
        let upstream = RecordingUpstream::new(HttpProxyResponse::new(
            200,
            HeaderCollection::default(),
            b"should-not-send".to_vec(),
        ));
        let selector = RecordingSelector::new();
        let resolver =
            RejectingProviderCredentialResolver::new(CredentialResolverError::RefreshUnavailable);
        let auth_gate = local_auth_gate();
        let service =
            AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER)
                .with_audit_sink(&audit_sink);

        let error = match service.handle_request(
            HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_header(Header::new("X-Codex-Router-Token", "current-token"))
                .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
        ) {
            Ok(response) => panic!("credential rejection should fail closed: {response:?}"),
            Err(error) => error,
        };

        assert_eq!(
            error,
            HttpProxyError::ProviderCredential {
                reason: CredentialResolverError::RefreshUnavailable
            }
        );
        assert_eq!(resolver.take_recorded(), vec!["acct_selected".to_owned()]);
        assert!(upstream.take_recorded().is_empty());
        let audit_contents = must_ok(fs::read_to_string(&audit_path));
        assert!(audit_contents.contains("\"transport_kind\":\"http\""));
        assert!(audit_contents.contains("\"decision_reason\":\"credential_rejected\""));
        assert!(audit_contents.contains("\"error_class\":\"provider_credential\""));
        assert!(audit_contents.contains("\"account_hash\""));
        assert!(audit_contents.contains("\"response_commit_state\":\"not_committed\""));
        assert!(!audit_contents.contains("current-token"));
        assert!(!audit_contents.contains("acct_selected"));
    }

    #[test]
    fn quota_aware_selector_prefers_fresh_headroom_over_penalized_stale_account() {
        let fresh_account = quota_account(
            "acct_fresh",
            50,
            SnapshotFreshness::Fresh { age_seconds: 10 },
        );
        let stale_account = quota_account(
            "acct_stale",
            100,
            SnapshotFreshness::StaleWithPenalty { age_seconds: 600 },
        );
        let selector = QuotaAwareAccountSelector::new(vec![stale_account, fresh_account]);

        let selected = match selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        ) {
            Ok(selected) => selected,
            Err(error) => panic!("selector should choose eligible account: {error}"),
        };

        assert_eq!(selected.account_id().as_str(), "acct_fresh");
        assert_eq!(selected.selection_reason(), "preferred_next");
    }

    #[test]
    fn quota_aware_selector_fails_closed_when_no_account_has_headroom() {
        let selector = QuotaAwareAccountSelector::new(vec![quota_account(
            "acct_empty",
            0,
            SnapshotFreshness::Fresh { age_seconds: 10 },
        )]);

        assert_eq!(
            selector.select_upstream_account(
                &HttpProxyRequest::new(Method::Post, "/v1/responses"),
                TokenGeneration::new(1),
                None,
            ),
            Err(HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::NoEligibleAccounts
            })
        );
    }

    #[test]
    fn repository_backed_selector_hydrates_enabled_accounts_from_state_quota_and_secret_store() {
        let temp_dir = ProxyTestTempDir::new("repository_selector");
        let database_path = temp_dir.path().join("state.sqlite");
        let secret_path = temp_dir.path().join("secrets");
        let state = match SqliteStateStore::open(&database_path) {
            Ok(state) => state,
            Err(error) => panic!("state store should open: {error}"),
        };
        let secrets = match FileSecretStore::open(&secret_path) {
            Ok(secrets) => secrets,
            Err(error) => panic!("secret store should open: {error}"),
        };
        let alpha = AccountRecord::new(account_id("acct_alpha"), "alpha", AccountStatus::Enabled);
        let beta = AccountRecord::new(account_id("acct_beta"), "beta", AccountStatus::Enabled);
        let disabled = AccountRecord::new(
            account_id("acct_disabled"),
            "disabled",
            AccountStatus::Disabled,
        );

        persist_account_with_snapshot_and_token(&state, &secrets, &beta, 40, "beta-token");
        persist_account_with_snapshot_and_token(&state, &secrets, &alpha, 80, "alpha-token");
        persist_account_with_snapshot_and_token(&state, &secrets, &disabled, 500, "disabled-token");

        let selector = RepositoryBackedAccountSelector::new(&state);
        let selected = match selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        ) {
            Ok(selected) => selected,
            Err(error) => panic!("repository-backed selector should select account: {error}"),
        };

        assert_eq!(selected.account_id().as_str(), "acct_alpha");
        assert_eq!(selected.selection_reason(), "preferred_next");
    }

    #[test]
    fn repository_backed_selector_uses_route_specific_quota_snapshots() {
        let temp_dir = ProxyTestTempDir::new("repository_selector_route_band");
        let database_path = temp_dir.path().join("state.sqlite");
        let secret_path = temp_dir.path().join("secrets");
        let state = match SqliteStateStore::open(&database_path) {
            Ok(state) => state,
            Err(error) => panic!("state store should open: {error}"),
        };
        let secrets = match FileSecretStore::open(&secret_path) {
            Ok(secrets) => secrets,
            Err(error) => panic!("secret store should open: {error}"),
        };
        let account = AccountRecord::new(
            account_id("acct_route_specific"),
            "route-specific",
            AccountStatus::Enabled,
        );
        if let Err(error) = AccountStateRepository::upsert_account(
            &state,
            &account.clone().with_active_credential_generation(1),
        ) {
            panic!("account should persist: {error}");
        }
        let responses_snapshot = PersistedQuotaSnapshot::new(
            account.account_id().clone(),
            QuotaSnapshotSource::MockEndpoint,
        )
        .with_observed_unix_seconds(1_000)
        .with_route_band("responses", 0)
        .with_stale_penalty(false);
        let models_snapshot = PersistedQuotaSnapshot::new(
            account.account_id().clone(),
            QuotaSnapshotSource::MockEndpoint,
        )
        .with_observed_unix_seconds(1_000)
        .with_route_band("models", 10)
        .with_stale_penalty(false);
        if let Err(error) = QuotaSnapshotRepository::upsert_snapshot(&state, &responses_snapshot) {
            panic!("responses quota should persist: {error}");
        }
        if let Err(error) = QuotaSnapshotRepository::upsert_snapshot(&state, &models_snapshot) {
            panic!("models quota should persist: {error}");
        }
        let responses_window = PersistedSelectorQuotaWindow::new(
            account.account_id().clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Ineligible,
        )
        .with_remaining_headroom(0)
        .with_effective(true)
        .with_observed_unix_seconds(test_unix_seconds())
        .with_reset_unix_seconds(selector_reset_seconds(18_000));
        let responses_weekly_window = PersistedSelectorQuotaWindow::new(
            account.account_id().clone(),
            "responses",
            604_800,
            SelectorQuotaWindowStatus::Ineligible,
        )
        .with_remaining_headroom(0)
        .with_effective(false)
        .with_observed_unix_seconds(test_unix_seconds())
        .with_reset_unix_seconds(selector_reset_seconds(604_800));
        let models_window = PersistedSelectorQuotaWindow::new(
            account.account_id().clone(),
            "models",
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(10)
        .with_effective(true)
        .with_observed_unix_seconds(test_unix_seconds())
        .with_reset_unix_seconds(selector_reset_seconds(18_000));
        let models_weekly_window = PersistedSelectorQuotaWindow::new(
            account.account_id().clone(),
            "models",
            604_800,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(10)
        .with_effective(false)
        .with_observed_unix_seconds(test_unix_seconds())
        .with_reset_unix_seconds(selector_reset_seconds(604_800));
        if let Err(error) =
            SelectorQuotaRepository::upsert_selector_window(&state, &responses_window)
        {
            panic!("responses selector window should persist: {error}");
        }
        if let Err(error) =
            SelectorQuotaRepository::upsert_selector_window(&state, &responses_weekly_window)
        {
            panic!("responses weekly selector window should persist: {error}");
        }
        if let Err(error) = SelectorQuotaRepository::upsert_selector_window(&state, &models_window)
        {
            panic!("models selector window should persist: {error}");
        }
        if let Err(error) =
            SelectorQuotaRepository::upsert_selector_window(&state, &models_weekly_window)
        {
            panic!("models weekly selector window should persist: {error}");
        }
        let token_key = match account_credential_bundle_key(account.account_id(), 1) {
            Ok(token_key) => token_key,
            Err(error) => panic!("token key should build: {error}"),
        };
        let bundle = match AccountCredentialBundle::imported_codex_auth(
            "route-specific-token",
            Some("route-specific-refresh-token".to_owned()),
        )
        .to_secret_string()
        {
            Ok(bundle) => bundle,
            Err(error) => panic!("credential bundle should serialize: {error}"),
        };
        if let Err(error) = secrets.write_secret(&token_key, &bundle) {
            panic!("upstream token should persist: {error}");
        }

        let selector = RepositoryBackedAccountSelector::new(&state);
        let selected = match selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Get, "/v1/models"),
            TokenGeneration::new(1),
            None,
        ) {
            Ok(selected) => selected,
            Err(error) => panic!("models request should select from models quota: {error}"),
        };
        assert_eq!(selected.account_id(), account.account_id());

        assert_eq!(
            selector.select_upstream_account(
                &HttpProxyRequest::new(Method::Post, "/v1/responses"),
                TokenGeneration::new(1),
                None,
            ),
            Err(HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::NoEligibleAccounts
            })
        );
    }

    #[test]
    fn repository_backed_selector_weights_weekly_pressure_over_short_window_headroom() {
        let temp_dir = ProxyTestTempDir::new("repository_selector_weekly_pressure");
        let database_path = temp_dir.path().join("state.sqlite");
        let state = match SqliteStateStore::open(&database_path) {
            Ok(state) => state,
            Err(error) => panic!("state store should open: {error}"),
        };
        let short_rich_weekly_poor = AccountRecord::new(
            account_id("acct_short_rich_weekly_poor"),
            "short-rich-weekly-poor",
            AccountStatus::Enabled,
        );
        let weekly_healthy = AccountRecord::new(
            account_id("acct_weekly_healthy"),
            "weekly-healthy",
            AccountStatus::Enabled,
        );
        persist_account_with_selector_window_specs(
            &state,
            &short_rich_weekly_poor,
            "responses",
            &[(18_000, 90, true), (604_800, 5, false)],
        );
        persist_account_with_selector_window_specs(
            &state,
            &weekly_healthy,
            "responses",
            &[(18_000, 50, true), (604_800, 50, false)],
        );

        let selector = RepositoryBackedAccountSelector::new(&state);
        let selected = match selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        ) {
            Ok(selected) => selected,
            Err(error) => panic!("repository-backed selector should select account: {error}"),
        };

        assert_eq!(selected.account_id(), weekly_healthy.account_id());
        assert_eq!(selected.selection_reason(), "preferred_next");
    }

    #[test]
    fn repository_backed_selector_skips_ineligible_account_for_next_normal_request() {
        let temp_dir = ProxyTestTempDir::new("repository_selector_next_normal");
        let database_path = temp_dir.path().join("state.sqlite");
        let state = match SqliteStateStore::open(&database_path) {
            Ok(state) => state,
            Err(error) => panic!("state store should open: {error}"),
        };
        let exhausted = AccountRecord::new(
            account_id("acct_exhausted"),
            "exhausted",
            AccountStatus::Enabled,
        );
        let eligible = AccountRecord::new(
            account_id("acct_eligible"),
            "eligible",
            AccountStatus::Enabled,
        );
        persist_account_with_selector_window_specs(
            &state,
            &exhausted,
            "responses",
            &[(18_000, 0, true), (604_800, 0, false)],
        );
        persist_account_with_selector_window_specs(
            &state,
            &eligible,
            "responses",
            &[(18_000, 42, true), (604_800, 42, false)],
        );

        let selector = RepositoryBackedAccountSelector::new(&state);
        let selected = match selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        ) {
            Ok(selected) => selected,
            Err(error) => panic!("next normal request should select eligible account: {error}"),
        };

        assert_eq!(selected.account_id(), eligible.account_id());
        assert_eq!(selected.selection_reason(), "preferred_next");
    }

    #[test]
    fn repository_backed_selector_uses_unknown_fallback_when_all_accounts_need_probe() {
        let temp_dir = ProxyTestTempDir::new("repository_selector_all_unknown_fallback");
        let database_path = temp_dir.path().join("state.sqlite");
        let state = match SqliteStateStore::open(&database_path) {
            Ok(state) => state,
            Err(error) => panic!("state store should open: {error}"),
        };
        let unknown = AccountRecord::new(
            account_id("acct_unknown"),
            "unknown",
            AccountStatus::Enabled,
        );
        persist_account_with_selector_window_status_specs(
            &state,
            &unknown,
            "responses",
            &[
                (18_000, 100, true, SelectorQuotaWindowStatus::Unknown),
                (604_800, 100, false, SelectorQuotaWindowStatus::Unknown),
            ],
        );

        let selector = RepositoryBackedAccountSelector::new(&state);

        let selected = match selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        ) {
            Ok(selected) => selected,
            Err(error) => panic!("unknown fallback account should be selected: {error}"),
        };

        assert_eq!(selected.account_id(), unknown.account_id());
        assert_eq!(selected.selection_reason(), "fallback");
    }

    #[test]
    fn repository_backed_selector_affinity_owner_bypasses_hold_and_weighted_choice() {
        let temp_dir = ProxyTestTempDir::new("repository_selector_affinity_owner_hit");
        let database_path = temp_dir.path().join("state.sqlite");
        let state = match SqliteStateStore::open(&database_path) {
            Ok(state) => state,
            Err(error) => panic!("state store should open: {error}"),
        };
        let alpha = AccountRecord::new(account_id("acct_alpha"), "alpha", AccountStatus::Enabled);
        let beta = AccountRecord::new(account_id("acct_beta"), "beta", AccountStatus::Enabled);
        persist_account_with_selector_window_specs(
            &state,
            &alpha,
            "responses",
            &[(18_000, 100, true), (604_800, 100, false)],
        );
        persist_account_with_selector_window_specs(
            &state,
            &beta,
            "responses",
            &[(18_000, 100, true), (604_800, 100, false)],
        );
        let affinity_secret = test_affinity_secret();
        if let Err(error) = persist_previous_response_owner(
            &state,
            "resp_beta",
            &affinity_secret,
            beta.account_id(),
        ) {
            panic!("affinity owner should persist: {error}");
        }

        let selector = RepositoryBackedAccountSelector::new(&state);
        let selected = match selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_body(br#"{"previous_response_id":"resp_beta"}"#.to_vec()),
            TokenGeneration::new(1),
            Some(&affinity_secret),
        ) {
            Ok(selected) => selected,
            Err(error) => panic!("affinity owner should select: {error}"),
        };

        assert_eq!(selected.account_id(), beta.account_id());
        assert_eq!(selected.selection_reason(), "previous_response_affinity");
    }

    #[test]
    fn repository_backed_selector_affinity_missing_owner_fails_closed() {
        let temp_dir = ProxyTestTempDir::new("repository_selector_affinity_missing_owner");
        let database_path = temp_dir.path().join("state.sqlite");
        let state = match SqliteStateStore::open(&database_path) {
            Ok(state) => state,
            Err(error) => panic!("state store should open: {error}"),
        };
        let alpha = AccountRecord::new(account_id("acct_alpha"), "alpha", AccountStatus::Enabled);
        persist_account_with_selector_window_specs(
            &state,
            &alpha,
            "responses",
            &[(18_000, 100, true), (604_800, 100, false)],
        );

        let selector = RepositoryBackedAccountSelector::new(&state);

        assert_eq!(
            selector.select_upstream_account(
                &HttpProxyRequest::new(Method::Post, "/v1/responses")
                    .with_body(br#"{"previous_response_id":"resp_missing"}"#.to_vec()),
                TokenGeneration::new(1),
                Some(&test_affinity_secret()),
            ),
            Err(HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::AffinityOwnerMissing
            })
        );
    }

    #[test]
    fn repository_backed_selector_affinity_ineligible_owner_fails_closed() {
        let temp_dir = ProxyTestTempDir::new("repository_selector_affinity_owner_ineligible");
        let database_path = temp_dir.path().join("state.sqlite");
        let state = match SqliteStateStore::open(&database_path) {
            Ok(state) => state,
            Err(error) => panic!("state store should open: {error}"),
        };
        let exhausted = AccountRecord::new(
            account_id("acct_exhausted"),
            "exhausted",
            AccountStatus::Enabled,
        );
        let eligible = AccountRecord::new(
            account_id("acct_eligible"),
            "eligible",
            AccountStatus::Enabled,
        );
        persist_account_with_selector_window_specs(
            &state,
            &exhausted,
            "responses",
            &[(18_000, 0, true), (604_800, 0, false)],
        );
        persist_account_with_selector_window_specs(
            &state,
            &eligible,
            "responses",
            &[(18_000, 100, true), (604_800, 100, false)],
        );
        let affinity_secret = test_affinity_secret();
        if let Err(error) = persist_previous_response_owner(
            &state,
            "resp_exhausted",
            &affinity_secret,
            exhausted.account_id(),
        ) {
            panic!("affinity owner should persist: {error}");
        }

        let selector = RepositoryBackedAccountSelector::new(&state);

        assert_eq!(
            selector.select_upstream_account(
                &HttpProxyRequest::new(Method::Post, "/v1/responses")
                    .with_body(br#"{"previous_response_id":"resp_exhausted"}"#.to_vec()),
                TokenGeneration::new(1),
                Some(&affinity_secret),
            ),
            Err(HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::AffinityOwnerUnavailable
            })
        );
    }

    #[test]
    fn repository_backed_selector_malformed_affinity_key_fails_closed() {
        let temp_dir = ProxyTestTempDir::new("repository_selector_affinity_malformed");
        let database_path = temp_dir.path().join("state.sqlite");
        let state = match SqliteStateStore::open(&database_path) {
            Ok(state) => state,
            Err(error) => panic!("state store should open: {error}"),
        };
        let alpha = AccountRecord::new(account_id("acct_alpha"), "alpha", AccountStatus::Enabled);
        persist_account_with_selector_window_specs(
            &state,
            &alpha,
            "responses",
            &[(18_000, 100, true), (604_800, 100, false)],
        );

        let selector = RepositoryBackedAccountSelector::new(&state);

        assert_eq!(
            selector.select_upstream_account(
                &HttpProxyRequest::new(Method::Post, "/v1/responses")
                    .with_body(br#"{"previous_response_id":42}"#.to_vec()),
                TokenGeneration::new(1),
                None,
            ),
            Err(HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::MalformedAffinityKey
            })
        );
    }

    #[test]
    fn repository_backed_selector_skips_account_with_ineligible_secondary_window() {
        let temp_dir = ProxyTestTempDir::new("repository_selector_secondary_ineligible");
        let database_path = temp_dir.path().join("state.sqlite");
        let state = match SqliteStateStore::open(&database_path) {
            Ok(state) => state,
            Err(error) => panic!("state store should open: {error}"),
        };
        let weekly_exhausted = AccountRecord::new(
            account_id("acct_weekly_exhausted"),
            "weekly-exhausted",
            AccountStatus::Enabled,
        );
        let eligible = AccountRecord::new(
            account_id("acct_weekly_eligible"),
            "weekly-eligible",
            AccountStatus::Enabled,
        );
        persist_account_with_selector_window_status_specs(
            &state,
            &weekly_exhausted,
            "responses",
            &[
                (18_000, 80, true, SelectorQuotaWindowStatus::Eligible),
                (604_800, 0, false, SelectorQuotaWindowStatus::Ineligible),
            ],
        );
        persist_account_with_selector_window_status_specs(
            &state,
            &eligible,
            "responses",
            &[
                (18_000, 42, true, SelectorQuotaWindowStatus::Eligible),
                (604_800, 42, false, SelectorQuotaWindowStatus::Eligible),
            ],
        );

        let selector = RepositoryBackedAccountSelector::new(&state);
        let selected = match selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        ) {
            Ok(selected) => selected,
            Err(error) => panic!("next normal request should select eligible account: {error}"),
        };

        assert_eq!(selected.account_id(), eligible.account_id());
        assert_eq!(selected.selection_reason(), "preferred_next");
    }

    #[test]
    fn repository_backed_selector_reuses_held_account_inside_cooldown() {
        let temp_dir = ProxyTestTempDir::new("repository_selector_hold_cooldown");
        let database_path = temp_dir.path().join("state.sqlite");
        let state = match SqliteStateStore::open(&database_path) {
            Ok(state) => state,
            Err(error) => panic!("state store should open: {error}"),
        };
        let alpha = AccountRecord::new(account_id("acct_alpha"), "alpha", AccountStatus::Enabled);
        let beta = AccountRecord::new(account_id("acct_beta"), "beta", AccountStatus::Enabled);
        persist_account_with_selector_window_specs(
            &state,
            &alpha,
            "responses",
            &[(18_000, 100, true), (604_800, 100, false)],
        );
        persist_account_with_selector_window_specs(
            &state,
            &beta,
            "responses",
            &[(18_000, 100, true), (604_800, 100, false)],
        );

        let now = Arc::new(Mutex::new(test_unix_seconds()));
        let clock_now = Arc::clone(&now);
        let selector = RepositoryBackedAccountSelector::new_with_runtime(
            &state,
            RouteBandWeightedSelectors::default(),
            RouteBandAccountHolds::default(),
            120,
            Arc::new(move || {
                *clock_now
                    .lock()
                    .expect("test clock lock should be available")
            }),
        );

        let first = match selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        ) {
            Ok(selected) => selected,
            Err(error) => panic!("first request should select account: {error}"),
        };
        let second = match selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        ) {
            Ok(selected) => selected,
            Err(error) => panic!("second request should reuse held account: {error}"),
        };

        assert_eq!(first.account_id(), alpha.account_id());
        assert_eq!(first.selection_reason(), "preferred_next");
        assert_eq!(second.account_id(), alpha.account_id());
        assert_eq!(second.selection_reason(), "account_hold_cooldown");
    }

    #[test]
    fn repository_backed_selector_rebalances_after_cooldown_expires() {
        let temp_dir = ProxyTestTempDir::new("repository_selector_hold_expired");
        let database_path = temp_dir.path().join("state.sqlite");
        let state = match SqliteStateStore::open(&database_path) {
            Ok(state) => state,
            Err(error) => panic!("state store should open: {error}"),
        };
        let alpha = AccountRecord::new(account_id("acct_alpha"), "alpha", AccountStatus::Enabled);
        let beta = AccountRecord::new(account_id("acct_beta"), "beta", AccountStatus::Enabled);
        persist_account_with_selector_window_specs(
            &state,
            &alpha,
            "responses",
            &[(18_000, 100, true), (604_800, 100, false)],
        );
        persist_account_with_selector_window_specs(
            &state,
            &beta,
            "responses",
            &[(18_000, 100, true), (604_800, 100, false)],
        );

        let now = Arc::new(Mutex::new(test_unix_seconds()));
        let clock_now = Arc::clone(&now);
        let selector = RepositoryBackedAccountSelector::new_with_runtime(
            &state,
            RouteBandWeightedSelectors::default(),
            RouteBandAccountHolds::default(),
            120,
            Arc::new(move || {
                *clock_now
                    .lock()
                    .expect("test clock lock should be available")
            }),
        );

        let first = match selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        ) {
            Ok(selected) => selected,
            Err(error) => panic!("first request should select account: {error}"),
        };
        {
            let mut now = now.lock().expect("test clock lock should be available");
            *now = now.saturating_add(121);
        }
        let second = match selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        ) {
            Ok(selected) => selected,
            Err(error) => panic!("second request should rebalance after cooldown: {error}"),
        };

        assert_eq!(first.account_id(), alpha.account_id());
        assert_eq!(second.account_id(), beta.account_id());
        assert_eq!(second.selection_reason(), "available");
    }

    #[test]
    fn repository_backed_selector_breaks_hold_when_account_needs_probe() {
        let temp_dir = ProxyTestTempDir::new("repository_selector_hold_probe_required");
        let database_path = temp_dir.path().join("state.sqlite");
        let state = match SqliteStateStore::open(&database_path) {
            Ok(state) => state,
            Err(error) => panic!("state store should open: {error}"),
        };
        let alpha = AccountRecord::new(account_id("acct_alpha"), "alpha", AccountStatus::Enabled);
        let beta = AccountRecord::new(account_id("acct_beta"), "beta", AccountStatus::Enabled);
        persist_account_with_selector_window_specs(
            &state,
            &alpha,
            "responses",
            &[(18_000, 100, true), (604_800, 100, false)],
        );
        persist_account_with_selector_window_specs(
            &state,
            &beta,
            "responses",
            &[(18_000, 100, true), (604_800, 100, false)],
        );

        let selector = RepositoryBackedAccountSelector::new_with_runtime(
            &state,
            RouteBandWeightedSelectors::default(),
            RouteBandAccountHolds::default(),
            120,
            Arc::new(test_unix_seconds),
        );
        let first = match selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        ) {
            Ok(selected) => selected,
            Err(error) => panic!("first request should select account: {error}"),
        };
        persist_account_with_selector_window_status_specs(
            &state,
            &alpha,
            "responses",
            &[
                (18_000, 100, true, SelectorQuotaWindowStatus::Unknown),
                (604_800, 100, false, SelectorQuotaWindowStatus::Unknown),
            ],
        );
        let second = match selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        ) {
            Ok(selected) => selected,
            Err(error) => panic!("second request should skip probe-required held account: {error}"),
        };

        assert_eq!(first.account_id(), alpha.account_id());
        assert_eq!(second.account_id(), beta.account_id());
        assert_eq!(second.selection_reason(), "preferred_next");
    }

    #[test]
    fn repository_backed_selector_partitions_weighted_state_by_route_band() {
        let temp_dir = ProxyTestTempDir::new("repository_selector_weighted_route_band");
        let database_path = temp_dir.path().join("state.sqlite");
        let state = match SqliteStateStore::open(&database_path) {
            Ok(state) => state,
            Err(error) => panic!("state store should open: {error}"),
        };
        let alpha = AccountRecord::new(account_id("acct_alpha"), "alpha", AccountStatus::Enabled);
        let beta = AccountRecord::new(account_id("acct_beta"), "beta", AccountStatus::Enabled);
        persist_account_with_selector_windows(&state, &alpha, &["models", "responses"], 10);
        persist_account_with_selector_windows(&state, &beta, &["models", "responses"], 10);

        let selector = RepositoryBackedAccountSelector::new(&state);
        let selected_models = match selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Get, "/v1/models"),
            TokenGeneration::new(1),
            None,
        ) {
            Ok(selected) => selected,
            Err(error) => panic!("models request should select account: {error}"),
        };
        let selected_responses = match selector.select_upstream_account(
            &HttpProxyRequest::new(Method::Post, "/v1/responses"),
            TokenGeneration::new(1),
            None,
        ) {
            Ok(selected) => selected,
            Err(error) => panic!("responses request should select account: {error}"),
        };

        assert_eq!(selected_models.account_id().as_str(), "acct_alpha");
        assert_eq!(selected_responses.account_id().as_str(), "acct_alpha");
    }

    #[test]
    fn loopback_server_binds_ephemeral_tcp_listener_on_loopback() {
        let address = match LoopbackBindAddress::new("127.0.0.1", 0) {
            Ok(address) => address,
            Err(error) => panic!("loopback address should validate: {error}"),
        };
        let runtime = match LoopbackServerRuntime::bind(address) {
            Ok(runtime) => runtime,
            Err(error) => panic!("loopback bind should succeed: {error}"),
        };

        assert!(runtime.local_addr().ip().is_loopback());
        assert_ne!(runtime.local_addr().port(), 0);
    }

    #[test]
    fn loopback_server_accepts_localhost_and_ipv6_loopback() {
        let localhost = match LoopbackBindAddress::new("localhost", 0) {
            Ok(address) => address,
            Err(error) => panic!("localhost should validate: {error}"),
        };
        let ipv6_loopback = match LoopbackBindAddress::new("::1", 0) {
            Ok(address) => address,
            Err(error) => panic!("IPv6 loopback should validate: {error}"),
        };

        assert!(localhost.socket_addr().ip().is_loopback());
        assert!(ipv6_loopback.socket_addr().ip().is_loopback());
    }

    #[test]
    fn loopback_server_rejects_non_loopback_before_binding() {
        assert_eq!(
            LoopbackBindAddress::new("0.0.0.0", 8787),
            Err(ServerBindError::NonLoopback {
                host: "0.0.0.0".to_owned()
            })
        );
        assert_eq!(
            LoopbackBindAddress::new("::", 8787),
            Err(ServerBindError::NonLoopback {
                host: "::".to_owned()
            })
        );
        assert_eq!(
            LoopbackBindAddress::new("192.168.1.10", 8787),
            Err(ServerBindError::NonLoopback {
                host: "192.168.1.10".to_owned()
            })
        );
    }

    #[test]
    fn loopback_http_adapter_forwards_real_tcp_request_and_serializes_response() {
        let address = match LoopbackBindAddress::new("127.0.0.1", 0) {
            Ok(address) => address,
            Err(error) => panic!("loopback address should validate: {error}"),
        };
        let runtime = match LoopbackServerRuntime::bind(address) {
            Ok(runtime) => runtime,
            Err(error) => panic!("loopback bind should succeed: {error}"),
        };
        let listener = match runtime.listener().try_clone() {
            Ok(listener) => listener,
            Err(error) => panic!("listener clone should succeed: {error}"),
        };
        let (request_sender, request_receiver) = mpsc::channel();
        let server_thread = thread::spawn(move || {
            let (stream, _peer_address) = match listener.accept() {
                Ok(accepted) => accepted,
                Err(error) => panic!("server should accept one client: {error}"),
            };
            let upstream = ChannelUpstream::new(
                request_sender,
                HttpProxyResponse::new(
                    200,
                    HeaderCollection::new(vec![Header::new("Content-Type", "text/event-stream")]),
                    b"data: ok\n\n".to_vec(),
                ),
            );
            let selector = RecordingSelector::new();
            let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
            let auth_gate = local_auth_gate();
            let service =
                AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                    .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

            match LoopbackHttpAdapter::handle_connection(stream, &service) {
                Ok(()) => {}
                Err(error) => panic!("connection should be handled: {error}"),
            }
        });

        let mut client = match TcpStream::connect(runtime.local_addr()) {
            Ok(client) => client,
            Err(error) => panic!("client should connect to loopback listener: {error}"),
        };
        let request = concat!(
            "POST /v1/responses?stream=true HTTP/1.1\r\n",
            "Host: 127.0.0.1\r\n",
            "X-Codex-Router-Token: current-token\r\n",
            "Authorization: Bearer wrong\r\n",
            "Accept: text/event-stream\r\n",
            "Content-Length: 17\r\n",
            "\r\n",
            "{\"model\":\"gpt-5\"}"
        );
        if let Err(error) = client.write_all(request.as_bytes()) {
            panic!("client request write should succeed: {error}");
        }
        if let Err(error) = client.shutdown(Shutdown::Write) {
            panic!("client write shutdown should succeed: {error}");
        }
        let mut response = String::new();
        if let Err(error) = client.read_to_string(&mut response) {
            panic!("client response read should succeed: {error}");
        }

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("content-type: text/event-stream\r\n"));
        assert!(response.ends_with("\r\ndata: ok\n\n"));

        let recorded = match request_receiver.recv() {
            Ok(request) => request,
            Err(error) => panic!("upstream request should be recorded: {error}"),
        };
        assert_eq!(recorded.method(), Method::Post);
        assert_eq!(recorded.path(), "/v1/responses?stream=true");
        assert_eq!(recorded.body(), br#"{"model":"gpt-5"}"#);
        assert_eq!(
            recorded.headers().values("authorization"),
            vec!["Bearer selected-upstream-token"]
        );
        assert_eq!(recorded.headers().value("x-codex-router-token"), None);
        assert_eq!(
            recorded.headers().value("accept"),
            Some("text/event-stream")
        );

        match server_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("server thread panicked: {error:?}"),
        }
    }

    #[test]
    fn loopback_http_adapter_returns_status_for_post_auth_proxy_rejections() {
        let selection_response = http_response_from_one_connection(|stream| {
            let upstream = RecordingUpstream::new(HttpProxyResponse::new(
                200,
                HeaderCollection::default(),
                b"should-not-send".to_vec(),
            ));
            let selector =
                RejectingSelector::new(QuotaAwareAccountSelectorError::NoEligibleAccounts);
            let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
            let auth_gate = local_auth_gate();
            let service =
                AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                    .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);
            must_ok(LoopbackHttpAdapter::handle_connection(stream, &service));
        });
        assert!(selection_response.starts_with("HTTP/1.1 503 Service Unavailable\r\n"));

        let credential_response = http_response_from_one_connection(|stream| {
            let upstream = RecordingUpstream::new(HttpProxyResponse::new(
                200,
                HeaderCollection::default(),
                b"should-not-send".to_vec(),
            ));
            let selector = RecordingSelector::new();
            let resolver = RejectingProviderCredentialResolver::new(
                CredentialResolverError::RefreshUnavailable,
            );
            let auth_gate = local_auth_gate();
            let service =
                AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                    .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);
            must_ok(LoopbackHttpAdapter::handle_connection(stream, &service));
        });
        assert!(credential_response.starts_with("HTTP/1.1 502 Bad Gateway\r\n"));
    }

    #[test]
    fn loopback_http_streaming_adapter_returns_status_for_post_auth_proxy_rejections() {
        let selection_response = http_response_from_one_connection(|stream| {
            let upstream = RecordingUpstream::new(HttpProxyResponse::new(
                200,
                HeaderCollection::default(),
                b"should-not-send".to_vec(),
            ));
            let selector =
                RejectingSelector::new(QuotaAwareAccountSelectorError::NoEligibleAccounts);
            let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
            let auth_gate = local_auth_gate();
            let service =
                AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                    .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);
            must_ok(LoopbackHttpAdapter::handle_streaming_connection(
                stream, &service,
            ));
        });
        assert!(selection_response.starts_with("HTTP/1.1 503 Service Unavailable\r\n"));

        let credential_response = http_response_from_one_connection(|stream| {
            let upstream = RecordingUpstream::new(HttpProxyResponse::new(
                200,
                HeaderCollection::default(),
                b"should-not-send".to_vec(),
            ));
            let selector = RecordingSelector::new();
            let resolver = RejectingProviderCredentialResolver::new(
                CredentialResolverError::RefreshUnavailable,
            );
            let auth_gate = local_auth_gate();
            let service =
                AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                    .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);
            must_ok(LoopbackHttpAdapter::handle_streaming_connection(
                stream, &service,
            ));
        });
        assert!(credential_response.starts_with("HTTP/1.1 502 Bad Gateway\r\n"));
    }

    #[test]
    fn loopback_http_adapter_responds_without_client_write_shutdown() {
        let address = match LoopbackBindAddress::new("127.0.0.1", 0) {
            Ok(address) => address,
            Err(error) => panic!("loopback address should validate: {error}"),
        };
        let runtime = match LoopbackServerRuntime::bind(address) {
            Ok(runtime) => runtime,
            Err(error) => panic!("loopback bind should succeed: {error}"),
        };
        let listener = match runtime.listener().try_clone() {
            Ok(listener) => listener,
            Err(error) => panic!("listener clone should succeed: {error}"),
        };
        let server_thread = thread::spawn(move || {
            let (stream, _peer_address) = match listener.accept() {
                Ok(accepted) => accepted,
                Err(error) => panic!("server should accept one client: {error}"),
            };
            let (request_sender, _request_receiver) = mpsc::channel();
            let upstream = ChannelUpstream::new(
                request_sender,
                HttpProxyResponse::new(200, HeaderCollection::default(), b"ok".to_vec()),
            );
            let selector = RecordingSelector::new();
            let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
            let auth_gate = local_auth_gate();
            let service =
                AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                    .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

            match LoopbackHttpAdapter::handle_connection(stream, &service) {
                Ok(()) => {}
                Err(error) => panic!("connection should be handled: {error}"),
            }
        });

        let mut client = match TcpStream::connect(runtime.local_addr()) {
            Ok(client) => client,
            Err(error) => panic!("client should connect to loopback listener: {error}"),
        };
        if let Err(error) = client.set_read_timeout(Some(Duration::from_millis(250))) {
            panic!("client read timeout should be set: {error}");
        }
        let request = concat!(
            "POST /v1/responses HTTP/1.1\r\n",
            "Host: 127.0.0.1\r\n",
            "X-Codex-Router-Token: current-token\r\n",
            "Content-Length: 17\r\n",
            "\r\n",
            "{\"model\":\"gpt-5\"}"
        );
        if let Err(error) = client.write_all(request.as_bytes()) {
            panic!("client request write should succeed: {error}");
        }
        let mut response = String::new();
        let read_result = client.read_to_string(&mut response);
        drop(client);

        match server_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("server thread panicked: {error:?}"),
        }
        match read_result {
            Ok(_) => {}
            Err(error) => panic!("client should receive response without write shutdown: {error}"),
        }
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.ends_with("\r\nok"));
    }

    #[test]
    fn loopback_http_server_accepts_multiple_connections_until_bound_is_reached() {
        let address = match LoopbackBindAddress::new("127.0.0.1", 0) {
            Ok(address) => address,
            Err(error) => panic!("loopback address should validate: {error}"),
        };
        let runtime = match LoopbackServerRuntime::bind(address) {
            Ok(runtime) => runtime,
            Err(error) => panic!("loopback bind should succeed: {error}"),
        };
        let listener = match runtime.listener().try_clone() {
            Ok(listener) => listener,
            Err(error) => panic!("listener clone should succeed: {error}"),
        };
        let server_address = runtime.local_addr();
        let (request_sender, request_receiver) = mpsc::channel();
        let server_thread = thread::spawn(move || {
            let upstream = ChannelUpstream::new(
                request_sender,
                HttpProxyResponse::new(200, HeaderCollection::default(), b"ok".to_vec()),
            );
            let selector = RecordingSelector::new();
            let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
            let auth_gate = local_auth_gate();
            let service =
                AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                    .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

            match LoopbackHttpServer::serve_connections(listener, &service, 2) {
                Ok(handled) => handled,
                Err(error) => panic!("server should handle bounded connections: {error}"),
            }
        });

        let first_response = send_loopback_request(
            server_address,
            "POST /v1/responses?turn=1 HTTP/1.1\r\n",
            br#"{"model":"gpt-5","turn":1}"#,
        );
        let second_response = send_loopback_request(
            server_address,
            "POST /v1/responses?turn=2 HTTP/1.1\r\n",
            br#"{"model":"gpt-5","turn":2}"#,
        );

        assert!(first_response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(second_response.starts_with("HTTP/1.1 200 OK\r\n"));

        let first_recorded = match request_receiver.recv() {
            Ok(request) => request,
            Err(error) => panic!("first upstream request should be recorded: {error}"),
        };
        let second_recorded = match request_receiver.recv() {
            Ok(request) => request,
            Err(error) => panic!("second upstream request should be recorded: {error}"),
        };
        assert_eq!(first_recorded.path(), "/v1/responses?turn=1");
        assert_eq!(second_recorded.path(), "/v1/responses?turn=2");

        match server_thread.join() {
            Ok(handled) => assert_eq!(handled, 2),
            Err(error) => panic!("server thread panicked: {error:?}"),
        }
    }

    #[test]
    fn assembled_loopback_router_runtime_forwards_with_repository_state_and_secrets() {
        let temp_dir = ProxyTestTempDir::new("assembled_runtime");
        let database_path = temp_dir.path().join("state.sqlite");
        let secret_path = temp_dir.path().join("secrets");
        let state = match SqliteStateStore::open(&database_path) {
            Ok(state) => state,
            Err(error) => panic!("state store should open: {error}"),
        };
        let secrets = match FileSecretStore::open(&secret_path) {
            Ok(secrets) => secrets,
            Err(error) => panic!("secret store should open: {error}"),
        };
        let account = AccountRecord::new(
            account_id("acct_runtime"),
            "runtime",
            AccountStatus::Enabled,
        );
        persist_account_with_snapshot_and_token(
            &state,
            &secrets,
            &account,
            70,
            "runtime-upstream-token",
        );
        let affinity_secret = must_ok(load_or_create_router_affinity_hash_secret(&secrets))
            .secret()
            .clone();

        let upstream_listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(error) => panic!("mock upstream should bind: {error}"),
        };
        let upstream_address = match upstream_listener.local_addr() {
            Ok(address) => address,
            Err(error) => panic!("mock upstream address should read: {error}"),
        };
        let (upstream_sender, upstream_receiver) = mpsc::channel();
        let upstream_thread = thread::spawn(move || {
            let (mut stream, _peer_address) = match upstream_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("mock upstream should accept: {error}"),
            };
            let mut request = String::new();
            if let Err(error) = stream.read_to_string(&mut request) {
                panic!("mock upstream should read request: {error}");
            }
            if let Err(error) = upstream_sender.send(request) {
                panic!("mock upstream request should record: {error}");
            }
            let response_body = b"data: {\"id\":\"resp_runtime\"}\n\n";
            if let Err(error) = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n",
                response_body.len()
            ) {
                panic!("mock upstream should write response headers: {error}");
            }
            if let Err(error) = stream.write_all(response_body) {
                panic!("mock upstream should write response: {error}");
            }
        });
        let endpoint = match UpstreamEndpoint::new(format!("http://{upstream_address}/v1")) {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("mock upstream endpoint should validate: {error}"),
        };
        let bind_address = match LoopbackBindAddress::new("127.0.0.1", 0) {
            Ok(address) => address,
            Err(error) => panic!("router bind address should validate: {error}"),
        };
        let config = LoopbackRouterRuntimeConfig::new(
            bind_address,
            endpoint,
            database_path.clone(),
            secret_path,
            LocalRouterTokenRecord::new(
                SecretString::new("current-token"),
                TokenGeneration::new(1),
            ),
        )
        .with_quota_clock(1_030, 60);
        let runtime = match LoopbackRouterRuntime::start(config) {
            Ok(runtime) => runtime,
            Err(error) => panic!("router runtime should start: {error}"),
        };
        let router_address = runtime.local_addr();
        let client_thread = thread::spawn(move || {
            send_loopback_request(
                router_address,
                "POST /v1/responses?runtime=true HTTP/1.1\r\n",
                br#"{"model":"gpt-5","runtime":true}"#,
            )
        });

        let handled = match runtime.serve_http_connections(1) {
            Ok(handled) => handled,
            Err(error) => panic!("router runtime should serve one connection: {error}"),
        };
        assert_eq!(handled, 1);

        let response = match client_thread.join() {
            Ok(response) => response,
            Err(error) => panic!("client thread panicked: {error:?}"),
        };
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.ends_with("\r\ndata: {\"id\":\"resp_runtime\"}\n\n"));

        let upstream_request = match upstream_receiver.recv() {
            Ok(request) => request,
            Err(error) => panic!("mock upstream request should be recorded: {error}"),
        };
        assert!(upstream_request.starts_with("POST /v1/responses?runtime=true HTTP/1.1\r\n"));
        assert!(upstream_request.contains("authorization: Bearer runtime-upstream-token\r\n"));
        assert!(!upstream_request.contains("X-Codex-Router-Token"));
        assert!(!upstream_request.contains("current-token"));

        match upstream_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("mock upstream thread panicked: {error:?}"),
        }

        let owner_hash = must_ok(hash_previous_response_id(
            &affinity_secret,
            &must_ok(PreviousResponseId::new("resp_runtime")),
        ));
        let runtime_state = must_ok(SqliteStateStore::open(&database_path));
        let owner_lookup = must_ok(AffinityRepository::load_previous_response_owner(
            &runtime_state,
            &owner_hash,
            codex_router_core::routes::RouteBand::Responses.as_str(),
        ));
        let PreviousResponseAffinityOwnerLookup::Found(owner) = owner_lookup else {
            panic!("runtime should persist response owner row: {owner_lookup:?}");
        };
        assert_eq!(owner.account_id(), account.account_id());
        assert_eq!(owner.credential_generation(), 1);
    }

    #[test]
    fn loopback_router_runtime_reuses_held_account_inside_cooldown() {
        let temp_dir = ProxyTestTempDir::new("runtime_cross_connection_balance");
        let database_path = temp_dir.path().join("state.sqlite");
        let secret_path = temp_dir.path().join("secrets");
        let state = match SqliteStateStore::open(&database_path) {
            Ok(state) => state,
            Err(error) => panic!("state store should open: {error}"),
        };
        let secrets = match FileSecretStore::open(&secret_path) {
            Ok(secrets) => secrets,
            Err(error) => panic!("secret store should open: {error}"),
        };
        let alpha = AccountRecord::new(account_id("acct_alpha"), "alpha", AccountStatus::Enabled);
        let beta = AccountRecord::new(account_id("acct_beta"), "beta", AccountStatus::Enabled);
        persist_account_with_snapshot_and_token(&state, &secrets, &alpha, 50, "alpha-token");
        persist_account_with_snapshot_and_token(&state, &secrets, &beta, 50, "beta-token");

        let upstream_listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(error) => panic!("mock upstream should bind: {error}"),
        };
        let upstream_address = match upstream_listener.local_addr() {
            Ok(address) => address,
            Err(error) => panic!("mock upstream address should read: {error}"),
        };
        let (upstream_sender, upstream_receiver) = mpsc::channel();
        let upstream_thread = thread::spawn(move || {
            for _connection_index in 0..2 {
                let (mut stream, _peer_address) = match upstream_listener.accept() {
                    Ok(connection) => connection,
                    Err(error) => panic!("mock upstream should accept: {error}"),
                };
                let request = read_test_http_request(&mut stream);
                let authorization = request
                    .lines()
                    .find(|line| line.starts_with("authorization: "))
                    .unwrap_or("<missing>")
                    .to_owned();
                if let Err(error) = upstream_sender.send(authorization) {
                    panic!("mock upstream authorization should record: {error}");
                }
                if let Err(error) =
                    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                {
                    panic!("mock upstream should write response: {error}");
                }
            }
        });
        let endpoint = match UpstreamEndpoint::new(format!("http://{upstream_address}/v1")) {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("mock upstream endpoint should validate: {error}"),
        };
        let bind_address = match LoopbackBindAddress::new("127.0.0.1", 0) {
            Ok(address) => address,
            Err(error) => panic!("router bind address should validate: {error}"),
        };
        let config = LoopbackRouterRuntimeConfig::new(
            bind_address,
            endpoint,
            database_path,
            secret_path,
            LocalRouterTokenRecord::new(
                SecretString::new("current-token"),
                TokenGeneration::new(1),
            ),
        )
        .with_quota_clock(1_030, 60);
        let runtime = match LoopbackRouterRuntime::start(config) {
            Ok(runtime) => runtime,
            Err(error) => panic!("router runtime should start: {error}"),
        };
        let router_address = runtime.local_addr();
        let first_client_thread = thread::spawn(move || {
            send_loopback_request(
                router_address,
                "POST /v1/responses HTTP/1.1\r\n",
                br#"{"model":"gpt-5","turn":1}"#,
            )
        });
        let second_client_thread = thread::spawn(move || {
            send_loopback_request(
                router_address,
                "POST /v1/responses HTTP/1.1\r\n",
                br#"{"model":"gpt-5","turn":2}"#,
            )
        });

        let handled = match runtime.serve_protocol_connections(2) {
            Ok(handled) => handled,
            Err(error) => panic!("router runtime should serve two connections: {error}"),
        };
        assert_eq!(handled, 2);
        for client_thread in [first_client_thread, second_client_thread] {
            let response = match client_thread.join() {
                Ok(response) => response,
                Err(error) => panic!("client thread panicked: {error:?}"),
            };
            assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        }

        let first_authorization = match upstream_receiver.recv() {
            Ok(authorization) => authorization,
            Err(error) => panic!("first upstream auth should record: {error}"),
        };
        let second_authorization = match upstream_receiver.recv() {
            Ok(authorization) => authorization,
            Err(error) => panic!("second upstream auth should record: {error}"),
        };
        assert_eq!(first_authorization, "authorization: Bearer alpha-token");
        assert_eq!(second_authorization, "authorization: Bearer alpha-token");

        match upstream_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("mock upstream thread panicked: {error:?}"),
        }
    }

    #[test]
    fn assembled_loopback_router_runtime_writes_redacted_private_audit_events() {
        let temp_dir = ProxyTestTempDir::new("assembled_runtime_audit");
        let database_path = temp_dir.path().join("state.sqlite");
        let secret_path = temp_dir.path().join("secrets");
        let audit_path = temp_dir.path().join("audit").join("events.jsonl");
        let state = match SqliteStateStore::open(&database_path) {
            Ok(state) => state,
            Err(error) => panic!("state store should open: {error}"),
        };
        let secrets = match FileSecretStore::open(&secret_path) {
            Ok(secrets) => secrets,
            Err(error) => panic!("secret store should open: {error}"),
        };
        let account = AccountRecord::new(
            account_id("acct_audit_raw_id_canary"),
            "raw-account-email-canary@example.com",
            AccountStatus::Enabled,
        );
        persist_account_with_snapshot_and_token(
            &state,
            &secrets,
            &account,
            70,
            "audit-upstream-token-canary",
        );

        let upstream_listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(error) => panic!("mock upstream should bind: {error}"),
        };
        let upstream_address = match upstream_listener.local_addr() {
            Ok(address) => address,
            Err(error) => panic!("mock upstream address should read: {error}"),
        };
        let upstream_thread = thread::spawn(move || {
            let (mut stream, _peer_address) = match upstream_listener.accept() {
                Ok(connection) => connection,
                Err(error) => {
                    panic!("mock upstream should accept only authorized request: {error}")
                }
            };
            let _request = read_test_http_request(&mut stream);
            if let Err(error) =
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\ndata: ok\n\n")
            {
                panic!("mock upstream should write response: {error}");
            }
        });
        let endpoint = match UpstreamEndpoint::new(format!("http://{upstream_address}/v1")) {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("mock upstream endpoint should validate: {error}"),
        };
        let bind_address = match LoopbackBindAddress::new("127.0.0.1", 0) {
            Ok(address) => address,
            Err(error) => panic!("router bind address should validate: {error}"),
        };
        let config = LoopbackRouterRuntimeConfig::new(
            bind_address,
            endpoint,
            database_path,
            secret_path,
            LocalRouterTokenRecord::new(
                SecretString::new("audit-local-token-canary"),
                TokenGeneration::new(1),
            ),
        )
        .with_quota_clock(1_030, 60)
        .with_audit_file(audit_path.clone());
        let runtime = match LoopbackRouterRuntime::start(config) {
            Ok(runtime) => runtime,
            Err(error) => panic!("router runtime should start: {error}"),
        };
        let router_address = runtime.local_addr();
        let server_thread = thread::spawn(move || match runtime.serve_http_connections(2) {
            Ok(handled) => handled,
            Err(error) => panic!("router runtime should serve audit connections: {error}"),
        });

        let unauthorized_response = send_loopback_request_with_token(
            router_address,
            None,
            br#"{"prompt":"prompt-body-canary","unauthorized":true}"#,
        );
        assert!(unauthorized_response.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
        let authorized_response = send_loopback_request_with_token(
            router_address,
            Some("audit-local-token-canary"),
            br#"{"prompt":"prompt-body-canary","authorized":true}"#,
        );
        assert!(authorized_response.starts_with("HTTP/1.1 200 OK\r\n"));

        match server_thread.join() {
            Ok(handled) => assert_eq!(handled, 2),
            Err(error) => panic!("server thread panicked: {error:?}"),
        }
        match upstream_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("mock upstream thread panicked: {error:?}"),
        }

        let audit_contents = match fs::read_to_string(&audit_path) {
            Ok(contents) => contents,
            Err(error) => panic!("audit file should exist: {error}"),
        };
        assert_eq!(audit_contents.lines().count(), 2);
        assert!(audit_contents.contains("\"transport_kind\":\"http\""));
        assert!(audit_contents.contains("\"route_kind\":\"responses\""));
        assert!(audit_contents.contains("\"local_auth_result\":\"missing\""));
        assert!(audit_contents.contains("\"local_auth_result\":\"valid\""));
        assert!(audit_contents.contains("\"response_commit_state\":\"not_committed\""));
        assert!(audit_contents.contains("\"response_commit_state\":\"committed\""));
        assert!(audit_contents.contains("\"account_hash\""));
        assert!(!audit_contents.contains("audit-local-token-canary"));
        assert!(!audit_contents.contains("audit-upstream-token-canary"));
        assert!(!audit_contents.contains("prompt-body-canary"));
        assert!(!audit_contents.contains("raw-account-email-canary@example.com"));
        assert!(!audit_contents.contains("acct_audit_raw_id_canary"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = match fs::metadata(&audit_path) {
                Ok(metadata) => metadata.permissions().mode() & 0o777,
                Err(error) => panic!("audit metadata should read: {error}"),
            };
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn assembled_loopback_router_runtime_redacts_http_and_websocket_audit_events() {
        assembled_loopback_router_runtime_writes_redacted_private_audit_events();
        loopback_router_runtime_dispatches_websocket_upgrade_to_tunnel();
    }

    #[test]
    fn audit_append_failure_reports_through_audit_failure_reporter_without_secret_leak() {
        let temp_dir = ProxyTestTempDir::new("audit_failure_reporter");
        let blocked_parent = temp_dir.path().join("audit-parent-is-file");
        match fs::write(&blocked_parent, "not-a-directory") {
            Ok(()) => {}
            Err(error) => panic!("blocked parent fixture should write: {error}"),
        }
        let sink = AuditFileSink::new(blocked_parent.join("events.jsonl"));
        let reporter = RecordingAuditFailureReporter::default();
        let event = AuditEvent::proxy_decision(AuditEventFields {
            request_id: RequestId::new("request-audit-failure"),
            route_kind: AuditRouteKind::Responses,
            transport_kind: TransportKind::Http,
            local_auth_result: LocalAuthAuditResult::Valid,
            outcome: AuditOutcome::Allowed,
            decision_reason: "allowed",
            response_commit_state: ResponseCommitState::Committed,
            account_hash: Some("acct_hash_without_secret".to_owned()),
            error_class: None,
        });

        append_audit_event_with_reporter(&sink, &event, &reporter);
        let diagnostics = reporter.diagnostics.borrow();

        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].contains("audit append failed"));
        assert!(!diagnostics[0].contains("access-token-canary"));
        assert!(!diagnostics[0].contains("refresh-token-canary"));
        assert!(!diagnostics[0].contains("local-token-canary"));
    }

    #[derive(Default)]
    struct RecordingAuditFailureReporter {
        diagnostics: RefCell<Vec<String>>,
    }

    impl AuditFailureReporter for RecordingAuditFailureReporter {
        fn report_audit_failure(&self, diagnostic: &str) {
            self.diagnostics.borrow_mut().push(diagnostic.to_owned());
        }
    }

    #[test]
    fn assembled_loopback_router_runtime_streams_sse_before_upstream_eof() {
        let temp_dir = ProxyTestTempDir::new("assembled_runtime_streams_sse");
        let database_path = temp_dir.path().join("state.sqlite");
        let secret_path = temp_dir.path().join("secrets");
        let state = match SqliteStateStore::open(&database_path) {
            Ok(state) => state,
            Err(error) => panic!("state store should open: {error}"),
        };
        let secrets = match FileSecretStore::open(&secret_path) {
            Ok(secrets) => secrets,
            Err(error) => panic!("secret store should open: {error}"),
        };
        let account = AccountRecord::new(
            account_id("acct_runtime_streaming"),
            "runtime-streaming",
            AccountStatus::Enabled,
        );
        persist_account_with_snapshot_and_token(
            &state,
            &secrets,
            &account,
            70,
            "runtime-streaming-token",
        );

        let upstream_listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(error) => panic!("mock upstream should bind: {error}"),
        };
        let upstream_address = match upstream_listener.local_addr() {
            Ok(address) => address,
            Err(error) => panic!("mock upstream address should read: {error}"),
        };
        let (release_sender, release_receiver) = mpsc::channel();
        let upstream_thread = thread::spawn(move || {
            let (mut stream, _peer_address) = match upstream_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("mock upstream should accept: {error}"),
            };
            let _request = read_test_http_request(&mut stream);
            if let Err(error) = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\ndata: first\n\n",
            ) {
                panic!("mock upstream should write first event: {error}");
            }
            if let Err(error) = stream.flush() {
                panic!("mock upstream should flush first event: {error}");
            }
            let _ = release_receiver.recv_timeout(Duration::from_secs(2));
            if let Err(error) = stream.write_all(b"data: second\n\n") {
                panic!("mock upstream should write second event: {error}");
            }
        });
        let endpoint = match UpstreamEndpoint::new(format!("http://{upstream_address}/v1")) {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("mock upstream endpoint should validate: {error}"),
        };
        let bind_address = match LoopbackBindAddress::new("127.0.0.1", 0) {
            Ok(address) => address,
            Err(error) => panic!("router bind address should validate: {error}"),
        };
        let config = LoopbackRouterRuntimeConfig::new(
            bind_address,
            endpoint,
            database_path,
            secret_path,
            LocalRouterTokenRecord::new(
                SecretString::new("current-token"),
                TokenGeneration::new(1),
            ),
        )
        .with_quota_clock(1_030, 60);
        let runtime = match LoopbackRouterRuntime::start(config) {
            Ok(runtime) => runtime,
            Err(error) => panic!("router runtime should start: {error}"),
        };
        let router_address = runtime.local_addr();
        let server_thread = thread::spawn(move || match runtime.serve_http_connections(1) {
            Ok(handled) => handled,
            Err(error) => panic!("router runtime should serve one streaming connection: {error}"),
        });

        let mut client = match TcpStream::connect(router_address) {
            Ok(client) => client,
            Err(error) => panic!("client should connect to loopback listener: {error}"),
        };
        if let Err(error) = client.set_read_timeout(Some(Duration::from_millis(750))) {
            panic!("client read timeout should be set: {error}");
        }
        let request = concat!(
            "POST /v1/responses?stream=true HTTP/1.1\r\n",
            "Host: 127.0.0.1\r\n",
            "X-Codex-Router-Token: current-token\r\n",
            "Accept: text/event-stream\r\n",
            "Content-Length: 17\r\n",
            "\r\n",
            "{\"model\":\"gpt-5\"}"
        );
        if let Err(error) = client.write_all(request.as_bytes()) {
            panic!("client request write should succeed: {error}");
        }
        let response_prefix =
            read_until_contains(&mut client, "data: first\n\n", Duration::from_millis(750));
        let _ = release_sender.send(());
        let mut drain = Vec::new();
        let _ = client.read_to_end(&mut drain);
        drop(client);

        match server_thread.join() {
            Ok(handled) => assert_eq!(handled, 1),
            Err(error) => panic!("server thread panicked: {error:?}"),
        }
        match upstream_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("mock upstream thread panicked: {error:?}"),
        }
        let response = match response_prefix {
            Ok(response) => response,
            Err(error) => {
                panic!("client should receive first SSE event before upstream EOF: {error}");
            }
        };
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("content-type: text/event-stream\r\n"));
        assert!(response.contains("data: first\n\n"));
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn loopback_router_runtime_dispatches_websocket_upgrade_to_tunnel() {
        let temp_dir = ProxyTestTempDir::new("runtime_websocket");
        let database_path = temp_dir.path().join("state.sqlite");
        let secret_path = temp_dir.path().join("secrets");
        let audit_path = temp_dir.path().join("audit").join("events.jsonl");
        let state = match SqliteStateStore::open(&database_path) {
            Ok(state) => state,
            Err(error) => panic!("state store should open: {error}"),
        };
        let secrets = match FileSecretStore::open(&secret_path) {
            Ok(secrets) => secrets,
            Err(error) => panic!("secret store should open: {error}"),
        };
        let account =
            AccountRecord::new(account_id("acct_ws_runtime"), "ws", AccountStatus::Enabled);
        persist_account_with_snapshot_and_token(
            &state,
            &secrets,
            &account,
            90,
            "runtime-ws-upstream-token-canary",
        );

        let upstream_listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(error) => panic!("mock websocket upstream should bind: {error}"),
        };
        let upstream_address = match upstream_listener.local_addr() {
            Ok(address) => address,
            Err(error) => panic!("mock websocket upstream address should read: {error}"),
        };
        let (upstream_sender, upstream_receiver) = mpsc::channel();
        let upstream_thread = thread::spawn(move || {
            let (stream, _peer_address) = match upstream_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("mock websocket upstream should accept: {error}"),
            };
            let mut websocket = match accept_hdr(stream, |request: &Request, response: Response| {
                let authorization = request
                    .headers()
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("<missing>")
                    .to_owned();
                let local_token = request
                    .headers()
                    .get("x-codex-router-token")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                if let Err(error) = upstream_sender.send((authorization, local_token)) {
                    panic!("mock websocket upstream headers should record: {error}");
                }
                Ok(response)
            }) {
                Ok(websocket) => websocket,
                Err(error) => panic!("mock websocket upstream handshake should accept: {error}"),
            };
            let first_frame = match websocket.read() {
                Ok(message) => message,
                Err(error) => panic!("mock websocket upstream should read first frame: {error}"),
            };
            if let Err(error) = upstream_sender.send((first_frame.to_string(), None)) {
                panic!("mock websocket upstream first frame should record: {error}");
            }
            if let Err(error) = websocket.send(Message::text(r#"{"type":"response.completed"}"#)) {
                panic!("mock websocket upstream should send response: {error}");
            }
        });

        let bind_address = match LoopbackBindAddress::new("127.0.0.1", 0) {
            Ok(address) => address,
            Err(error) => panic!("router bind address should validate: {error}"),
        };
        let endpoint = match UpstreamEndpoint::new(format!("http://{upstream_address}/v1")) {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("mock endpoint should validate: {error}"),
        };
        let config = LoopbackRouterRuntimeConfig::new(
            bind_address,
            endpoint,
            database_path,
            secret_path,
            LocalRouterTokenRecord::new(
                SecretString::new("current-token"),
                TokenGeneration::new(1),
            ),
        )
        .with_quota_clock(1_030, 60)
        .with_max_websocket_upstream_messages(1)
        .with_audit_file(audit_path.clone());
        let runtime = match LoopbackRouterRuntime::start(config) {
            Ok(runtime) => runtime,
            Err(error) => panic!("router runtime should start: {error}"),
        };
        let router_address = runtime.local_addr();
        let client_thread = thread::spawn(move || {
            let mut request =
                match format!("ws://{router_address}/v1/responses").into_client_request() {
                    Ok(request) => request,
                    Err(error) => panic!("local websocket request should build: {error}"),
                };
            request.headers_mut().insert(
                "Authorization",
                HeaderValue::from_static("Bearer current-token"),
            );
            let (mut client, _response) = match connect(request) {
                Ok(connection) => connection,
                Err(error) => panic!("local websocket client should connect: {error}"),
            };
            let first_frame = r#"{"type":"response.create","runtime":true}"#;
            if let Err(error) = client.send(Message::text(first_frame)) {
                panic!("local websocket client should send first frame: {error}");
            }
            match client.read() {
                Ok(message) => message.to_string(),
                Err(error) => panic!("local websocket client should read response: {error}"),
            }
        });

        let handled = match runtime.serve_protocol_connections(1) {
            Ok(handled) => handled,
            Err(error) => panic!("router runtime should serve websocket connection: {error}"),
        };
        assert_eq!(handled, 1);
        let client_response = match client_thread.join() {
            Ok(response) => response,
            Err(error) => panic!("client thread panicked: {error:?}"),
        };
        assert_eq!(client_response, r#"{"type":"response.completed"}"#);
        let (authorization, local_token) = match upstream_receiver.recv() {
            Ok(recorded) => recorded,
            Err(error) => panic!("upstream handshake should record: {error}"),
        };
        assert_eq!(authorization, "Bearer runtime-ws-upstream-token-canary");
        assert_eq!(local_token, None);
        let (recorded_first_frame, _) = match upstream_receiver.recv() {
            Ok(recorded) => recorded,
            Err(error) => panic!("upstream first frame should record: {error}"),
        };
        assert_eq!(
            recorded_first_frame,
            r#"{"type":"response.create","runtime":true}"#
        );

        match upstream_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("mock websocket upstream thread panicked: {error:?}"),
        }

        let audit_contents = match fs::read_to_string(&audit_path) {
            Ok(contents) => contents,
            Err(error) => panic!("websocket audit file should exist: {error}"),
        };
        assert_eq!(audit_contents.lines().count(), 1);
        assert!(audit_contents.contains("\"transport_kind\":\"web_socket\""));
        assert!(audit_contents.contains("\"route_kind\":\"responses_web_socket\""));
        assert!(audit_contents.contains("\"local_auth_result\":\"valid\""));
        assert!(audit_contents.contains("\"response_commit_state\":\"committed\""));
        assert!(audit_contents.contains("\"account_hash\""));
        assert!(!audit_contents.contains("current-token"));
        assert!(!audit_contents.contains("runtime-ws-upstream-token-canary"));
        assert!(!audit_contents.contains(r#"{"type":"response.create","runtime":true}"#));
        assert!(!audit_contents.contains("acct_ws_runtime"));
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn loopback_router_runtime_reloads_local_auth_and_closes_old_token_websocket() {
        let temp_dir = ProxyTestTempDir::new("runtime_websocket_token_rotation");
        let database_path = temp_dir.path().join("state.sqlite");
        let secret_path = temp_dir.path().join("secrets");
        let state = match SqliteStateStore::open(&database_path) {
            Ok(state) => state,
            Err(error) => panic!("state store should open: {error}"),
        };
        let secrets = match FileSecretStore::open(&secret_path) {
            Ok(secrets) => secrets,
            Err(error) => panic!("secret store should open: {error}"),
        };
        let account = AccountRecord::new(
            account_id("acct_ws_rotation"),
            "ws-rotation",
            AccountStatus::Enabled,
        );
        persist_account_with_snapshot_and_token(
            &state,
            &secrets,
            &account,
            90,
            "runtime-ws-rotation-upstream-token",
        );

        let upstream_listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(error) => panic!("mock websocket upstream should bind: {error}"),
        };
        let upstream_address = match upstream_listener.local_addr() {
            Ok(address) => address,
            Err(error) => panic!("mock websocket upstream address should read: {error}"),
        };
        let (first_frame_sender, first_frame_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let upstream_thread = thread::spawn(move || {
            let (stream, _peer_address) = match upstream_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("mock websocket upstream should accept: {error}"),
            };
            let mut websocket =
                match accept_hdr(stream, |_request: &Request, response: Response| {
                    Ok(response)
                }) {
                    Ok(websocket) => websocket,
                    Err(error) => {
                        panic!("mock websocket upstream handshake should accept: {error}")
                    }
                };
            let first_frame = match websocket.read() {
                Ok(message) => message,
                Err(error) => panic!("mock websocket upstream should read first frame: {error}"),
            };
            if let Err(error) = first_frame_sender.send(first_frame.to_string()) {
                panic!("mock websocket upstream first frame should record: {error}");
            }
            let _released = release_receiver.recv_timeout(Duration::from_secs(2));
        });

        let bind_address = match LoopbackBindAddress::new("127.0.0.1", 0) {
            Ok(address) => address,
            Err(error) => panic!("router bind address should validate: {error}"),
        };
        let endpoint = match UpstreamEndpoint::new(format!("http://{upstream_address}/v1")) {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("mock endpoint should validate: {error}"),
        };
        let config = LoopbackRouterRuntimeConfig::new(
            bind_address,
            endpoint,
            database_path,
            secret_path,
            LocalRouterTokenRecord::new(SecretString::new("token-a"), TokenGeneration::new(1)),
        )
        .with_quota_clock(1_030, 60);
        let runtime = match LoopbackRouterRuntime::start(config) {
            Ok(runtime) => runtime,
            Err(error) => panic!("router runtime should start: {error}"),
        };
        let router_address = runtime.local_addr();
        let reloader = runtime.local_auth_reloader();
        let server_thread = thread::spawn(move || match runtime.serve_protocol_connections(1) {
            Ok(handled) => handled,
            Err(error) => panic!("router runtime should serve websocket connection: {error}"),
        });

        let mut request = match format!("ws://{router_address}/v1/responses").into_client_request()
        {
            Ok(request) => request,
            Err(error) => panic!("local websocket request should build: {error}"),
        };
        request
            .headers_mut()
            .insert("X-Codex-Router-Token", HeaderValue::from_static("token-a"));
        let (mut client, _response) = match connect(request) {
            Ok(connection) => connection,
            Err(error) => panic!("local websocket client should connect: {error}"),
        };
        if let Err(error) = client.send(Message::text(r#"{"type":"response.create"}"#)) {
            panic!("local websocket client should send first frame: {error}");
        }
        let recorded_first_frame = match first_frame_receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(frame) => frame,
            Err(error) => panic!("upstream should receive first frame before rotation: {error}"),
        };
        assert_eq!(recorded_first_frame, r#"{"type":"response.create"}"#);

        reloader.reload_local_auth(
            LocalRouterTokenRecord::new(SecretString::new("token-b"), TokenGeneration::new(2)),
            vec![LocalRouterTokenRecord::new(
                SecretString::new("token-a"),
                TokenGeneration::new(1),
            )],
        );
        match client.read() {
            Ok(message) => panic!("old-token websocket should close, got message: {message}"),
            Err(_error) => {}
        }
        if let Err(error) = release_sender.send(()) {
            panic!("upstream release should send: {error}");
        }

        match server_thread.join() {
            Ok(handled) => assert_eq!(handled, 1),
            Err(error) => panic!("server thread panicked: {error:?}"),
        }
        match upstream_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("mock websocket upstream thread panicked: {error:?}"),
        }
    }

    #[test]
    fn loopback_router_runtime_rejects_websocket_upgrade_without_token() {
        let temp_dir = ProxyTestTempDir::new("runtime_websocket_missing_token");
        let database_path = temp_dir.path().join("state.sqlite");
        let secret_path = temp_dir.path().join("secrets");
        let bind_address = match LoopbackBindAddress::new("127.0.0.1", 0) {
            Ok(address) => address,
            Err(error) => panic!("router bind address should validate: {error}"),
        };
        let endpoint = match UpstreamEndpoint::new("http://127.0.0.1:1/v1") {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("mock endpoint should validate: {error}"),
        };
        let config = LoopbackRouterRuntimeConfig::new(
            bind_address,
            endpoint,
            database_path,
            secret_path,
            LocalRouterTokenRecord::new(
                SecretString::new("current-token"),
                TokenGeneration::new(1),
            ),
        );
        let runtime = match LoopbackRouterRuntime::start(config) {
            Ok(runtime) => runtime,
            Err(error) => panic!("router runtime should start: {error}"),
        };
        let router_address = runtime.local_addr();
        let server_thread = thread::spawn(move || match runtime.serve_protocol_connections(1) {
            Ok(handled) => handled,
            Err(error) => panic!("router runtime should serve rejected websocket: {error}"),
        });

        let request = match format!("ws://{router_address}/v1/responses").into_client_request() {
            Ok(request) => request,
            Err(error) => panic!("local websocket request should build: {error}"),
        };
        let connect_succeeded = match connect(request) {
            Ok((client, _response)) => {
                drop(client);
                true
            }
            Err(_error) => false,
        };

        match server_thread.join() {
            Ok(handled) => assert_eq!(handled, 1),
            Err(error) => panic!("server thread panicked: {error:?}"),
        }
        assert!(
            !connect_succeeded,
            "missing-token websocket upgrade should fail before local accept"
        );
    }

    #[test]
    fn loopback_router_runtime_rejects_unsupported_websocket_path_before_accept() {
        let temp_dir = ProxyTestTempDir::new("runtime_websocket_unsupported_path");
        let database_path = temp_dir.path().join("state.sqlite");
        let secret_path = temp_dir.path().join("secrets");
        let bind_address = match LoopbackBindAddress::new("127.0.0.1", 0) {
            Ok(address) => address,
            Err(error) => panic!("router bind address should validate: {error}"),
        };
        let endpoint = match UpstreamEndpoint::new("http://127.0.0.1:1/v1") {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("mock endpoint should validate: {error}"),
        };
        let config = LoopbackRouterRuntimeConfig::new(
            bind_address,
            endpoint,
            database_path,
            secret_path,
            LocalRouterTokenRecord::new(
                SecretString::new("current-token"),
                TokenGeneration::new(1),
            ),
        );
        let runtime = match LoopbackRouterRuntime::start(config) {
            Ok(runtime) => runtime,
            Err(error) => panic!("router runtime should start: {error}"),
        };
        let router_address = runtime.local_addr();
        let server_thread = thread::spawn(move || match runtime.serve_protocol_connections(1) {
            Ok(handled) => handled,
            Err(error) => panic!("router runtime should serve rejected websocket: {error}"),
        });

        let mut request = match format!("ws://{router_address}/v1/realtime").into_client_request() {
            Ok(request) => request,
            Err(error) => panic!("local websocket request should build: {error}"),
        };
        request.headers_mut().insert(
            "X-Codex-Router-Token",
            HeaderValue::from_static("current-token"),
        );
        let connect_succeeded = match connect(request) {
            Ok((client, _response)) => {
                drop(client);
                true
            }
            Err(_error) => false,
        };

        match server_thread.join() {
            Ok(handled) => assert_eq!(handled, 1),
            Err(error) => panic!("server thread panicked: {error:?}"),
        }
        assert!(
            !connect_succeeded,
            "unsupported websocket path should fail before local accept"
        );
    }

    #[test]
    fn loopback_router_runtime_bounds_websocket_wait_for_first_frame() {
        let temp_dir = ProxyTestTempDir::new("runtime_websocket_no_first_frame");
        let database_path = temp_dir.path().join("state.sqlite");
        let secret_path = temp_dir.path().join("secrets");
        let bind_address = match LoopbackBindAddress::new("127.0.0.1", 0) {
            Ok(address) => address,
            Err(error) => panic!("router bind address should validate: {error}"),
        };
        let endpoint = match UpstreamEndpoint::new("http://127.0.0.1:1/v1") {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("mock endpoint should validate: {error}"),
        };
        let config = LoopbackRouterRuntimeConfig::new(
            bind_address,
            endpoint,
            database_path,
            secret_path,
            LocalRouterTokenRecord::new(
                SecretString::new("current-token"),
                TokenGeneration::new(1),
            ),
        );
        let runtime = match LoopbackRouterRuntime::start(config) {
            Ok(runtime) => runtime,
            Err(error) => panic!("router runtime should start: {error}"),
        };
        let router_address = runtime.local_addr();
        let (done_sender, done_receiver) = mpsc::channel();
        let server_thread = thread::spawn(move || {
            let handled = match runtime.serve_protocol_connections(1) {
                Ok(handled) => handled,
                Err(error) => panic!("router runtime should serve bounded websocket: {error}"),
            };
            if let Err(error) = done_sender.send(handled) {
                panic!("server completion should send: {error}");
            }
        });

        let mut request = match format!("ws://{router_address}/v1/responses").into_client_request()
        {
            Ok(request) => request,
            Err(error) => panic!("local websocket request should build: {error}"),
        };
        request.headers_mut().insert(
            "X-Codex-Router-Token",
            HeaderValue::from_static("current-token"),
        );
        let (client, _response) = match connect(request) {
            Ok(connection) => connection,
            Err(error) => panic!("local websocket client should connect: {error}"),
        };
        let bounded_result = done_receiver.recv_timeout(Duration::from_millis(750));
        drop(client);

        match server_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("server thread panicked: {error:?}"),
        }
        match bounded_result {
            Ok(handled) => assert_eq!(handled, 1),
            Err(error) => {
                panic!("server should stop waiting for first websocket frame promptly: {error}");
            }
        }
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn loopback_router_runtime_continues_after_rejected_connection() {
        let temp_dir = ProxyTestTempDir::new("runtime_rejected_then_websocket");
        let database_path = temp_dir.path().join("state.sqlite");
        let secret_path = temp_dir.path().join("secrets");
        let state = match SqliteStateStore::open(&database_path) {
            Ok(state) => state,
            Err(error) => panic!("state store should open: {error}"),
        };
        let secrets = match FileSecretStore::open(&secret_path) {
            Ok(secrets) => secrets,
            Err(error) => panic!("secret store should open: {error}"),
        };
        let account = AccountRecord::new(
            account_id("acct_ws_after_reject"),
            "ws-after-reject",
            AccountStatus::Enabled,
        );
        persist_account_with_snapshot_and_token(
            &state,
            &secrets,
            &account,
            90,
            "runtime-after-reject-token",
        );

        let upstream_listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(error) => panic!("mock websocket upstream should bind: {error}"),
        };
        let upstream_address = match upstream_listener.local_addr() {
            Ok(address) => address,
            Err(error) => panic!("mock websocket upstream address should read: {error}"),
        };
        let (upstream_sender, upstream_receiver) = mpsc::channel();
        let upstream_thread = thread::spawn(move || {
            let (stream, _peer_address) = match upstream_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("mock websocket upstream should accept: {error}"),
            };
            let mut websocket = match accept_hdr(stream, |request: &Request, response: Response| {
                let authorization = request
                    .headers()
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("<missing>")
                    .to_owned();
                if let Err(error) = upstream_sender.send(authorization) {
                    panic!("mock websocket upstream headers should record: {error}");
                }
                Ok(response)
            }) {
                Ok(websocket) => websocket,
                Err(error) => panic!("mock websocket upstream handshake should accept: {error}"),
            };
            let first_frame = match websocket.read() {
                Ok(message) => message,
                Err(error) => panic!("mock websocket upstream should read first frame: {error}"),
            };
            if let Err(error) = upstream_sender.send(first_frame.to_string()) {
                panic!("mock websocket upstream first frame should record: {error}");
            }
            if let Err(error) = websocket.send(Message::text(r#"{"type":"response.completed"}"#)) {
                panic!("mock websocket upstream should send response: {error}");
            }
        });

        let bind_address = match LoopbackBindAddress::new("127.0.0.1", 0) {
            Ok(address) => address,
            Err(error) => panic!("router bind address should validate: {error}"),
        };
        let endpoint = match UpstreamEndpoint::new(format!("http://{upstream_address}/v1")) {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("mock endpoint should validate: {error}"),
        };
        let config = LoopbackRouterRuntimeConfig::new(
            bind_address,
            endpoint,
            database_path,
            secret_path,
            LocalRouterTokenRecord::new(
                SecretString::new("current-token"),
                TokenGeneration::new(1),
            ),
        )
        .with_quota_clock(1_030, 60)
        .with_max_websocket_upstream_messages(1);
        let runtime = match LoopbackRouterRuntime::start(config) {
            Ok(runtime) => runtime,
            Err(error) => panic!("router runtime should start: {error}"),
        };
        let router_address = runtime.local_addr();
        let rejected_client_thread = thread::spawn(move || {
            let mut client = match TcpStream::connect(router_address) {
                Ok(client) => client,
                Err(error) => panic!("rejected client should connect: {error}"),
            };
            if let Err(error) = client.write_all(
                b"GET /v1/unsupported HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\n\r\n",
            ) {
                panic!("rejected client request should write: {error}");
            }
            if let Err(error) = client.shutdown(Shutdown::Write) {
                panic!("rejected client should shutdown write side: {error}");
            }
            let mut ignored_response = String::new();
            let _ = client.read_to_string(&mut ignored_response);
        });
        let websocket_client_thread = thread::spawn(move || {
            match rejected_client_thread.join() {
                Ok(()) => {}
                Err(error) => panic!("rejected client thread panicked: {error:?}"),
            }
            let mut request =
                match format!("ws://{router_address}/v1/responses").into_client_request() {
                    Ok(request) => request,
                    Err(error) => panic!("local websocket request should build: {error}"),
                };
            request.headers_mut().insert(
                "X-Codex-Router-Token",
                HeaderValue::from_static("current-token"),
            );
            let (mut client, _response) = match connect(request) {
                Ok(connection) => connection,
                Err(error) => panic!("local websocket client should connect after reject: {error}"),
            };
            if let Err(error) = client.send(Message::text(
                r#"{"type":"response.create","after_reject":true}"#,
            )) {
                panic!("local websocket client should send first frame: {error}");
            }
            match client.read() {
                Ok(message) => message.to_string(),
                Err(error) => panic!("local websocket client should read response: {error}"),
            }
        });

        let handled = match runtime.serve_protocol_connections(2) {
            Ok(handled) => handled,
            Err(error) => {
                panic!("router runtime should continue after rejected connection: {error}")
            }
        };
        assert_eq!(handled, 2);
        let client_response = match websocket_client_thread.join() {
            Ok(response) => response,
            Err(error) => panic!("websocket client thread panicked: {error:?}"),
        };
        assert_eq!(client_response, r#"{"type":"response.completed"}"#);
        let authorization = match upstream_receiver.recv() {
            Ok(recorded) => recorded,
            Err(error) => panic!("upstream handshake should record: {error}"),
        };
        assert_eq!(authorization, "Bearer runtime-after-reject-token");
        let recorded_first_frame = match upstream_receiver.recv() {
            Ok(recorded) => recorded,
            Err(error) => panic!("upstream first frame should record: {error}"),
        };
        assert_eq!(
            recorded_first_frame,
            r#"{"type":"response.create","after_reject":true}"#
        );

        match upstream_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("mock websocket upstream thread panicked: {error:?}"),
        }
    }

    fn send_loopback_request(
        server_address: std::net::SocketAddr,
        request_line: &str,
        body: &[u8],
    ) -> String {
        let mut client = match TcpStream::connect(server_address) {
            Ok(client) => client,
            Err(error) => panic!("client should connect to loopback listener: {error}"),
        };
        let request = format!(
            "{request_line}Host: 127.0.0.1\r\nX-Codex-Router-Token: current-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            String::from_utf8_lossy(body)
        );
        if let Err(error) = client.write_all(request.as_bytes()) {
            panic!("client request write should succeed: {error}");
        }
        if let Err(error) = client.shutdown(Shutdown::Write) {
            panic!("client write shutdown should succeed: {error}");
        }
        let mut response = String::new();
        if let Err(error) = client.read_to_string(&mut response) {
            panic!("client response read should succeed: {error}");
        }

        response
    }

    fn send_loopback_request_with_token(
        server_address: std::net::SocketAddr,
        token: Option<&str>,
        body: &[u8],
    ) -> String {
        let mut client = match TcpStream::connect(server_address) {
            Ok(client) => client,
            Err(error) => panic!("client should connect to loopback listener: {error}"),
        };
        let token_header = token
            .map(|token| format!("X-Codex-Router-Token: {token}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "POST /v1/responses HTTP/1.1\r\nHost: 127.0.0.1\r\n{token_header}Content-Length: {}\r\n\r\n{}",
            body.len(),
            String::from_utf8_lossy(body)
        );
        if let Err(error) = client.write_all(request.as_bytes()) {
            panic!("client request write should succeed: {error}");
        }
        if let Err(error) = client.shutdown(Shutdown::Write) {
            panic!("client write shutdown should succeed: {error}");
        }
        let mut response = String::new();
        if let Err(error) = client.read_to_string(&mut response) {
            panic!("client response read should succeed: {error}");
        }

        response
    }

    fn read_test_http_request(stream: &mut TcpStream) -> String {
        let mut request_bytes = Vec::new();
        let header_length = loop {
            if let Some(header_end) = request_bytes
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|position| position + 4)
            {
                break header_end;
            }
            let mut buffer = [0_u8; 1024];
            let read = match stream.read(&mut buffer) {
                Ok(read) => read,
                Err(error) => panic!("mock upstream should read request bytes: {error}"),
            };
            if read == 0 {
                panic!("mock upstream request ended before headers completed");
            }
            request_bytes.extend_from_slice(&buffer[..read]);
        };
        let headers = String::from_utf8_lossy(&request_bytes[..header_length]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or_default();
        let body_end = header_length + content_length;
        while request_bytes.len() < body_end {
            let mut buffer = [0_u8; 1024];
            let read = match stream.read(&mut buffer) {
                Ok(read) => read,
                Err(error) => panic!("mock upstream should read request body: {error}"),
            };
            if read == 0 {
                panic!("mock upstream request ended before body completed");
            }
            request_bytes.extend_from_slice(&buffer[..read]);
        }

        String::from_utf8_lossy(&request_bytes[..body_end]).into_owned()
    }

    fn read_until_contains(
        stream: &mut TcpStream,
        needle: &str,
        deadline_after: Duration,
    ) -> std::io::Result<String> {
        let deadline = Instant::now() + deadline_after;
        let mut bytes = Vec::new();
        loop {
            if String::from_utf8_lossy(&bytes).contains(needle) {
                return Ok(String::from_utf8_lossy(&bytes).into_owned());
            }
            if Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("timed out waiting for `{needle}`"),
                ));
            }

            let mut buffer = [0_u8; 128];
            match stream.read(&mut buffer) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        format!("EOF before `{needle}`"),
                    ));
                }
                Ok(read) => bytes.extend_from_slice(&buffer[..read]),
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        || error.kind() == std::io::ErrorKind::TimedOut =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn local_auth_gate() -> ProxyLocalAuthGate {
        let current = LocalRouterTokenRecord::new(
            SecretString::new("current-token"),
            TokenGeneration::new(1),
        );
        ProxyLocalAuthGate::new(LocalRouterAuth::new(current, Vec::new()))
    }

    fn quota_account(
        account_id: &str,
        remaining_headroom: u32,
        freshness: SnapshotFreshness,
    ) -> QuotaAwareAccountState {
        let account_id = match codex_router_core::ids::AccountId::new(account_id) {
            Ok(account_id) => account_id,
            Err(error) => panic!("account id should parse: {error}"),
        };
        QuotaAwareAccountState::new(account_id, remaining_headroom, freshness)
    }

    fn persist_account_with_snapshot_and_token(
        state: &SqliteStateStore,
        secrets: &FileSecretStore,
        account: &AccountRecord,
        remaining_headroom: u32,
        upstream_token: &str,
    ) {
        let account_with_generation = account.clone().with_active_credential_generation(1);
        if let Err(error) = AccountStateRepository::upsert_account(state, &account_with_generation)
        {
            panic!("account should persist: {error}");
        }
        let snapshot = PersistedQuotaSnapshot::new(
            account.account_id().clone(),
            QuotaSnapshotSource::MockEndpoint,
        )
        .with_observed_unix_seconds(1_000)
        .with_route_band("responses", remaining_headroom)
        .with_stale_penalty(false);
        if let Err(error) = QuotaSnapshotRepository::upsert_snapshot(state, &snapshot) {
            panic!("quota snapshot should persist: {error}");
        }
        let selector_window = PersistedSelectorQuotaWindow::new(
            account.account_id().clone(),
            "responses",
            18_000,
            if remaining_headroom == 0 {
                SelectorQuotaWindowStatus::Ineligible
            } else {
                SelectorQuotaWindowStatus::Eligible
            },
        )
        .with_remaining_headroom(remaining_headroom)
        .with_effective(true)
        .with_observed_unix_seconds(test_unix_seconds())
        .with_reset_unix_seconds(selector_reset_seconds(18_000));
        if let Err(error) = SelectorQuotaRepository::upsert_selector_window(state, &selector_window)
        {
            panic!("selector quota window should persist: {error}");
        }
        let weekly_selector_window = PersistedSelectorQuotaWindow::new(
            account.account_id().clone(),
            "responses",
            604_800,
            if remaining_headroom == 0 {
                SelectorQuotaWindowStatus::Ineligible
            } else {
                SelectorQuotaWindowStatus::Eligible
            },
        )
        .with_remaining_headroom(remaining_headroom)
        .with_effective(false)
        .with_observed_unix_seconds(test_unix_seconds())
        .with_reset_unix_seconds(selector_reset_seconds(604_800));
        if let Err(error) =
            SelectorQuotaRepository::upsert_selector_window(state, &weekly_selector_window)
        {
            panic!("weekly selector quota window should persist: {error}");
        }
        let token_key = match account_credential_bundle_key(account.account_id(), 1) {
            Ok(token_key) => token_key,
            Err(error) => panic!("token key should build: {error}"),
        };
        let bundle = match AccountCredentialBundle::imported_codex_auth(
            upstream_token,
            Some(format!("{upstream_token}-refresh")),
        )
        .to_secret_string()
        {
            Ok(bundle) => bundle,
            Err(error) => panic!("credential bundle should serialize: {error}"),
        };
        if let Err(error) = secrets.write_secret(&token_key, &bundle) {
            panic!("upstream token should persist: {error}");
        }
    }

    fn persist_account_with_selector_windows(
        state: &SqliteStateStore,
        account: &AccountRecord,
        route_bands: &[&str],
        remaining_headroom: u32,
    ) {
        let account_with_generation = account.clone().with_active_credential_generation(1);
        if let Err(error) = AccountStateRepository::upsert_account(state, &account_with_generation)
        {
            panic!("account should persist: {error}");
        }
        for route_band in route_bands {
            let selector_window = PersistedSelectorQuotaWindow::new(
                account.account_id().clone(),
                *route_band,
                18_000,
                SelectorQuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(remaining_headroom)
            .with_effective(true)
            .with_observed_unix_seconds(test_unix_seconds())
            .with_reset_unix_seconds(selector_reset_seconds(18_000));
            if let Err(error) =
                SelectorQuotaRepository::upsert_selector_window(state, &selector_window)
            {
                panic!("selector quota window should persist: {error}");
            }
            let weekly_selector_window = PersistedSelectorQuotaWindow::new(
                account.account_id().clone(),
                *route_band,
                604_800,
                SelectorQuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(remaining_headroom)
            .with_effective(false)
            .with_observed_unix_seconds(test_unix_seconds())
            .with_reset_unix_seconds(selector_reset_seconds(604_800));
            if let Err(error) =
                SelectorQuotaRepository::upsert_selector_window(state, &weekly_selector_window)
            {
                panic!("weekly selector quota window should persist: {error}");
            }
        }
    }

    fn persist_account_with_selector_window_specs(
        state: &SqliteStateStore,
        account: &AccountRecord,
        route_band: &str,
        windows: &[(u64, u32, bool)],
    ) {
        let account_with_generation = account.clone().with_active_credential_generation(1);
        if let Err(error) = AccountStateRepository::upsert_account(state, &account_with_generation)
        {
            panic!("account should persist: {error}");
        }
        for (limit_window_seconds, remaining_headroom, effective) in windows {
            let selector_window = PersistedSelectorQuotaWindow::new(
                account.account_id().clone(),
                route_band,
                *limit_window_seconds,
                SelectorQuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(*remaining_headroom)
            .with_effective(*effective)
            .with_observed_unix_seconds(test_unix_seconds())
            .with_reset_unix_seconds(selector_reset_seconds(*limit_window_seconds));
            if let Err(error) =
                SelectorQuotaRepository::upsert_selector_window(state, &selector_window)
            {
                panic!("selector quota window should persist: {error}");
            }
        }
    }

    fn persist_account_with_selector_window_status_specs(
        state: &SqliteStateStore,
        account: &AccountRecord,
        route_band: &str,
        windows: &[(u64, u32, bool, SelectorQuotaWindowStatus)],
    ) {
        let account_with_generation = account.clone().with_active_credential_generation(1);
        if let Err(error) = AccountStateRepository::upsert_account(state, &account_with_generation)
        {
            panic!("account should persist: {error}");
        }
        for (limit_window_seconds, remaining_headroom, effective, status) in windows {
            let selector_window = PersistedSelectorQuotaWindow::new(
                account.account_id().clone(),
                route_band,
                *limit_window_seconds,
                *status,
            )
            .with_remaining_headroom(*remaining_headroom)
            .with_effective(*effective)
            .with_observed_unix_seconds(test_unix_seconds())
            .with_reset_unix_seconds(selector_reset_seconds(*limit_window_seconds));
            if let Err(error) =
                SelectorQuotaRepository::upsert_selector_window(state, &selector_window)
            {
                panic!("selector quota window should persist: {error}");
            }
        }
    }

    fn persist_previous_response_owner(
        state: &SqliteStateStore,
        previous_response_id: &str,
        affinity_secret: &RouterAffinityHashSecret,
        account_id: &codex_router_core::ids::AccountId,
    ) -> Result<(), codex_router_state::sqlite::StateStoreError> {
        let previous_response_id = match PreviousResponseId::new(previous_response_id) {
            Ok(previous_response_id) => previous_response_id,
            Err(error) => panic!("previous response id should parse: {error}"),
        };
        let affinity_key_hash =
            match hash_previous_response_id(affinity_secret, &previous_response_id) {
                Ok(affinity_key_hash) => affinity_key_hash,
                Err(error) => panic!("affinity hash should compute: {error}"),
            };
        let owner = PreviousResponseAffinityOwnerRecord::new(
            affinity_key_hash,
            account_id.clone(),
            1,
            codex_router_core::routes::RouteBand::Responses,
            AffinitySourceTransport::HttpSse,
            test_unix_seconds(),
        );

        AffinityRepository::write_previous_response_owner(state, &owner)
    }

    fn test_affinity_secret() -> RouterAffinityHashSecret {
        match RouterAffinityHashSecret::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ) {
            Ok(secret) => secret,
            Err(error) => panic!("test affinity secret should parse: {error}"),
        }
    }

    fn account_id(value: &str) -> codex_router_core::ids::AccountId {
        match codex_router_core::ids::AccountId::new(value) {
            Ok(account_id) => account_id,
            Err(error) => panic!("account id should parse: {error}"),
        }
    }

    fn test_unix_seconds() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs())
    }

    fn selector_reset_seconds(limit_window_seconds: u64) -> u64 {
        test_unix_seconds().saturating_add(limit_window_seconds)
    }

    struct ProxyTestTempDir {
        path: PathBuf,
    }

    impl ProxyTestTempDir {
        fn new(name: &str) -> Self {
            let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = env::temp_dir().join(format!(
                "codex-router-proxy-{name}-{}-{unique}",
                std::process::id()
            ));
            if path.exists() {
                remove_dir_all(&path);
            }
            if let Err(error) = fs::create_dir(&path) {
                panic!(
                    "failed to create test directory {}: {error}",
                    path.display()
                );
            }

            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for ProxyTestTempDir {
        fn drop(&mut self) {
            if self.path.exists() {
                remove_dir_all(&self.path);
            }
        }
    }

    fn must_ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("expected Ok, got error: {error}"),
        }
    }

    fn http_response_from_one_connection(
        handle_stream: impl FnOnce(TcpStream) + Send + 'static,
    ) -> String {
        let address = match LoopbackBindAddress::new("127.0.0.1", 0) {
            Ok(address) => address,
            Err(error) => panic!("loopback address should validate: {error}"),
        };
        let runtime = match LoopbackServerRuntime::bind(address) {
            Ok(runtime) => runtime,
            Err(error) => panic!("loopback bind should succeed: {error}"),
        };
        let listener = match runtime.listener().try_clone() {
            Ok(listener) => listener,
            Err(error) => panic!("listener clone should succeed: {error}"),
        };
        let server_thread = thread::spawn(move || {
            let (stream, _peer_address) = match listener.accept() {
                Ok(accepted) => accepted,
                Err(error) => panic!("server should accept one client: {error}"),
            };
            handle_stream(stream);
        });
        let mut client = match TcpStream::connect(runtime.local_addr()) {
            Ok(client) => client,
            Err(error) => panic!("client should connect to loopback listener: {error}"),
        };
        let request = concat!(
            "POST /v1/responses HTTP/1.1\r\n",
            "Host: 127.0.0.1\r\n",
            "X-Codex-Router-Token: current-token\r\n",
            "Content-Length: 17\r\n",
            "\r\n",
            "{\"model\":\"gpt-5\"}"
        );
        must_ok(client.write_all(request.as_bytes()));
        must_ok(client.shutdown(Shutdown::Write));
        let mut response = String::new();
        must_ok(client.read_to_string(&mut response));
        match server_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("server thread panicked: {error:?}"),
        }
        response
    }

    fn remove_dir_all(path: &Path) {
        if let Err(error) = fs::remove_dir_all(path) {
            panic!(
                "failed to remove test directory {}: {error}",
                path.display()
            );
        }
    }

    struct RecordingUpstream {
        response: HttpProxyResponse,
        recorded: RefCell<Vec<UpstreamHttpRequest>>,
    }

    impl RecordingUpstream {
        fn new(response: HttpProxyResponse) -> Self {
            Self {
                response,
                recorded: RefCell::new(Vec::new()),
            }
        }

        fn take_recorded(&self) -> Vec<UpstreamHttpRequest> {
            self.recorded.take()
        }
    }

    impl UpstreamHttpTransport for RecordingUpstream {
        fn send(&self, request: UpstreamHttpRequest) -> Result<HttpProxyResponse, HttpProxyError> {
            self.recorded.borrow_mut().push(request);
            Ok(self.response.clone())
        }
    }

    impl StreamingUpstreamHttpTransport for RecordingUpstream {
        fn send_streaming(
            &self,
            request: UpstreamHttpRequest,
        ) -> Result<StreamingHttpProxyResponse, HttpProxyError> {
            self.recorded.borrow_mut().push(request);
            Ok(StreamingHttpProxyResponse::from_buffered(
                self.response.clone(),
            ))
        }
    }

    struct RecordingSelector {
        recorded: RefCell<Vec<(String, TokenGeneration)>>,
    }

    impl RecordingSelector {
        fn new() -> Self {
            Self {
                recorded: RefCell::new(Vec::new()),
            }
        }

        fn take_recorded(&self) -> Vec<(String, TokenGeneration)> {
            self.recorded.take()
        }
    }

    impl AccountDecisionSelector for RecordingSelector {
        fn select_upstream_account(
            &self,
            request: &HttpProxyRequest,
            token_generation: TokenGeneration,
            _affinity_secret: Option<&RouterAffinityHashSecret>,
        ) -> Result<SelectedAccountDecision, HttpProxyError> {
            self.recorded
                .borrow_mut()
                .push((request.path().to_owned(), token_generation));
            let account_id = match codex_router_core::ids::AccountId::new("acct_selected") {
                Ok(account_id) => account_id,
                Err(error) => {
                    return Err(HttpProxyError::Upstream {
                        message: format!("test account id failed: {error}"),
                    });
                }
            };
            Ok(SelectedAccountDecision::new(account_id, "test_selection"))
        }
    }

    struct RejectingSelector {
        reason: QuotaAwareAccountSelectorError,
        recorded: RefCell<Vec<(String, TokenGeneration)>>,
    }

    impl RejectingSelector {
        fn new(reason: QuotaAwareAccountSelectorError) -> Self {
            Self {
                reason,
                recorded: RefCell::new(Vec::new()),
            }
        }
    }

    impl AccountDecisionSelector for RejectingSelector {
        fn select_upstream_account(
            &self,
            request: &HttpProxyRequest,
            token_generation: TokenGeneration,
            _affinity_secret: Option<&RouterAffinityHashSecret>,
        ) -> Result<SelectedAccountDecision, HttpProxyError> {
            self.recorded
                .borrow_mut()
                .push((request.path().to_owned(), token_generation));
            Err(HttpProxyError::Selection {
                reason: self.reason,
            })
        }
    }

    struct RecordingProviderCredentialResolver {
        access_token: SecretString,
        recorded: RefCell<Vec<String>>,
    }

    impl RecordingProviderCredentialResolver {
        fn new(access_token: &str) -> Self {
            Self {
                access_token: SecretString::new(access_token),
                recorded: RefCell::new(Vec::new()),
            }
        }

        fn take_recorded(&self) -> Vec<String> {
            self.recorded.take()
        }
    }

    impl ProviderCredentialResolver for RecordingProviderCredentialResolver {
        fn resolve_provider_credentials(
            &self,
            account_id: &codex_router_core::ids::AccountId,
        ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
            self.recorded
                .borrow_mut()
                .push(account_id.as_str().to_owned());
            Ok(ResolvedProviderCredential::new(
                account_id.clone(),
                self.access_token.clone(),
                1,
            ))
        }
    }

    struct RejectingProviderCredentialResolver {
        reason: CredentialResolverError,
        recorded: RefCell<Vec<String>>,
    }

    impl RejectingProviderCredentialResolver {
        fn new(reason: CredentialResolverError) -> Self {
            Self {
                reason,
                recorded: RefCell::new(Vec::new()),
            }
        }

        fn take_recorded(&self) -> Vec<String> {
            self.recorded.take()
        }
    }

    impl ProviderCredentialResolver for RejectingProviderCredentialResolver {
        fn resolve_provider_credentials(
            &self,
            account_id: &codex_router_core::ids::AccountId,
        ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
            self.recorded
                .borrow_mut()
                .push(account_id.as_str().to_owned());
            Err(self.reason.clone())
        }
    }

    #[derive(Clone)]
    struct RecordingRefreshClient {
        expected_account_id: String,
        expected_refresh_token: String,
        response: AccountCredentialBundle,
        calls: std::sync::Arc<AtomicUsize>,
    }

    impl RecordingRefreshClient {
        fn new(
            expected_account_id: &str,
            expected_refresh_token: &str,
            response: AccountCredentialBundle,
        ) -> Self {
            Self {
                expected_account_id: expected_account_id.to_owned(),
                expected_refresh_token: expected_refresh_token.to_owned(),
                response,
                calls: std::sync::Arc::new(AtomicUsize::new(0)),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl CredentialRefreshClient for RecordingRefreshClient {
        fn refresh_credentials(
            &self,
            account_id: &codex_router_core::ids::AccountId,
            refresh_token: &SecretString,
        ) -> Result<AccountCredentialBundle, CredentialResolverError> {
            assert_eq!(account_id.as_str(), self.expected_account_id);
            assert_eq!(refresh_token.expose_secret(), self.expected_refresh_token);
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.response.clone())
        }
    }

    struct ChannelUpstream {
        request_sender: mpsc::Sender<UpstreamHttpRequest>,
        response: HttpProxyResponse,
    }

    impl ChannelUpstream {
        fn new(
            request_sender: mpsc::Sender<UpstreamHttpRequest>,
            response: HttpProxyResponse,
        ) -> Self {
            Self {
                request_sender,
                response,
            }
        }
    }

    impl UpstreamHttpTransport for ChannelUpstream {
        fn send(&self, request: UpstreamHttpRequest) -> Result<HttpProxyResponse, HttpProxyError> {
            if let Err(error) = self.request_sender.send(request) {
                return Err(HttpProxyError::Upstream {
                    message: format!("recording channel closed: {error}"),
                });
            }

            Ok(self.response.clone())
        }
    }

    #[test]
    fn websocket_first_response_create_frame_selects_and_forwards_unchanged() {
        let router = WebSocketProtocolRouter::new(FirstFramePolicy::new(1024));
        let frame = WebSocketFrame::Text(
            br#"{"type":"response.create","unknown_codex_field":{"kept":true}}"#.to_vec(),
        );
        let decision = match router.route_first_frame(
            WebSocketHandshakeRequest::new()
                .with_header(Header::new("X-Codex-Router-Token", "local-token"))
                .with_header(Header::new("Host", "127.0.0.1:8787"))
                .with_header(Header::new("Sec-WebSocket-Key", "client-key"))
                .with_header(Header::new("Authorization", "Bearer wrong"))
                .with_header(Header::new("ChatGPT-Account-Id", "hostile-account-id"))
                .with_header(Header::new("Connection", "upgrade"))
                .with_header(Header::new("Upgrade", "websocket"))
                .with_header(Header::new("OpenAI-Beta", "responses=v1")),
            frame.clone(),
            SecretString::new("selected-upstream-token"),
            Some("chatgpt-account-id-canary"),
        ) {
            Ok(decision) => decision,
            Err(error) => panic!("valid first frame should route: {error:?}"),
        };

        let WebSocketFirstFrameDecision::OpenUpstream {
            headers,
            first_frame,
            ..
        } = decision;
        assert_eq!(first_frame, frame);
        assert_eq!(headers.value("openai-beta"), Some("responses=v1"));
        assert_eq!(
            headers.values("authorization"),
            vec!["Bearer selected-upstream-token"]
        );
        assert_eq!(
            headers.value("chatgpt-account-id"),
            Some("chatgpt-account-id-canary")
        );
        assert_eq!(headers.value("x-codex-router-token"), None);
        assert_eq!(headers.value("host"), None);
        assert_eq!(headers.value("sec-websocket-key"), None);
        assert_eq!(headers.value("connection"), None);
        assert_eq!(headers.value("upgrade"), None);
    }

    #[test]
    fn websocket_first_direct_response_create_payload_selects_and_forwards_unchanged() {
        let router = WebSocketProtocolRouter::new(FirstFramePolicy::new(1024));
        let frame = WebSocketFrame::Text(
            br#"{"model":"gpt-5.5","input":[],"stream":true,"unknown_codex_field":{"kept":true}}"#
                .to_vec(),
        );

        let decision = match router.route_first_frame(
            WebSocketHandshakeRequest::new()
                .with_header(Header::new("X-Codex-Router-Token", "local-token"))
                .with_header(Header::new("Authorization", "Bearer wrong")),
            frame.clone(),
            SecretString::new("selected-upstream-token"),
            Some("chatgpt-account-id-canary"),
        ) {
            Ok(decision) => decision,
            Err(error) => panic!("valid direct first frame should route: {error:?}"),
        };

        let WebSocketFirstFrameDecision::OpenUpstream {
            headers,
            first_frame,
            ..
        } = decision;
        assert_eq!(first_frame, frame);
        assert_eq!(
            headers.values("authorization"),
            vec!["Bearer selected-upstream-token"]
        );
        assert_eq!(
            headers.value("chatgpt-account-id"),
            Some("chatgpt-account-id-canary")
        );
        assert_eq!(headers.value("x-codex-router-token"), None);
    }

    #[test]
    fn authenticated_websocket_router_selects_after_local_auth_and_first_frame() {
        let protocol_router = WebSocketProtocolRouter::new(FirstFramePolicy::new(1024));
        let selector = RecordingSelector::new();
        let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
        let auth_gate = local_auth_gate();
        let router =
            AuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router);
        let frame = WebSocketFrame::Text(br#"{"type":"response.create"}"#.to_vec());

        let decision = match router.route_first_frame(
            WebSocketHandshakeRequest::new()
                .with_header(Header::new("X-Codex-Router-Token", "current-token"))
                .with_header(Header::new("Authorization", "Bearer wrong")),
            frame.clone(),
        ) {
            Ok(decision) => decision,
            Err(error) => panic!("authenticated websocket should route: {error:?}"),
        };

        assert_eq!(
            selector.take_recorded(),
            vec![("/v1/responses".to_owned(), TokenGeneration::new(1))]
        );
        let WebSocketFirstFrameDecision::OpenUpstream {
            headers,
            first_frame,
            ..
        } = decision;
        assert_eq!(first_frame, frame);
        assert_eq!(
            headers.values("authorization"),
            vec!["Bearer selected-upstream-token"]
        );
        assert_eq!(headers.value("x-codex-router-token"), None);
    }

    #[test]
    fn authenticated_websocket_router_rejects_first_frame_before_selection_or_credentials() {
        let protocol_router = WebSocketProtocolRouter::new(FirstFramePolicy::new(1024));
        let selector = RecordingSelector::new();
        let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
        let auth_gate = local_auth_gate();
        let router =
            AuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router);

        assert_eq!(
            router.route_first_frame(
                WebSocketHandshakeRequest::new()
                    .with_header(Header::new("X-Codex-Router-Token", "current-token")),
                WebSocketFrame::Text(br#"{"type":"not.response.create"}"#.to_vec()),
            ),
            Err(WebSocketCloseReason::UnexpectedFirstFrame)
        );
        assert!(selector.take_recorded().is_empty());
        assert!(resolver.take_recorded().is_empty());
    }

    #[test]
    fn authenticated_websocket_router_routes_previous_response_affinity_owner() {
        let temp_dir = ProxyTestTempDir::new("websocket-router-affinity");
        let database_path = temp_dir.path().join("state.sqlite");
        let state = must_ok(SqliteStateStore::open(&database_path));
        let alpha = AccountRecord::new(account_id("acct_alpha"), "alpha", AccountStatus::Enabled);
        let beta = AccountRecord::new(account_id("acct_beta"), "beta", AccountStatus::Enabled);
        persist_account_with_selector_window_specs(
            &state,
            &alpha,
            "responses",
            &[(18_000, 100, true), (604_800, 100, false)],
        );
        persist_account_with_selector_window_specs(
            &state,
            &beta,
            "responses",
            &[(18_000, 100, true), (604_800, 100, false)],
        );
        let affinity_secret = test_affinity_secret();
        if let Err(error) = persist_previous_response_owner(
            &state,
            "resp_beta",
            &affinity_secret,
            beta.account_id(),
        ) {
            panic!("affinity owner should persist: {error}");
        }

        let protocol_router = WebSocketProtocolRouter::new(FirstFramePolicy::new(1024));
        let selector = RepositoryBackedAccountSelector::new(&state);
        let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
        let auth_gate = local_auth_gate();
        let router =
            AuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

        let decision = match router.route_first_frame(
            WebSocketHandshakeRequest::new()
                .with_header(Header::new("X-Codex-Router-Token", "current-token")),
            WebSocketFrame::Text(
                br#"{"type":"response.create","previous_response_id":"resp_beta"}"#.to_vec(),
            ),
        ) {
            Ok(decision) => decision,
            Err(error) => panic!("websocket affinity owner should route: {error:?}"),
        };

        assert_eq!(resolver.take_recorded(), vec!["acct_beta".to_owned()]);
        let WebSocketFirstFrameDecision::OpenUpstream { headers, .. } = decision;
        assert_eq!(
            headers.values("authorization"),
            vec!["Bearer selected-upstream-token"]
        );
    }

    #[test]
    fn authenticated_websocket_router_refreshes_expired_access_token_before_upstream_open() {
        let temp_dir = ProxyTestTempDir::new("websocket-router-refresh");
        let state = must_ok(SqliteStateStore::open(
            &temp_dir.path().join("state.sqlite"),
        ));
        let secrets = must_ok(FileSecretStore::open(temp_dir.path().join("secrets")));
        let account_id = account_id("acct_selected");
        let account = AccountRecord::new(account_id.clone(), "selected", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let expired_key = must_ok(account_credential_bundle_key(&account_id, 1));
        must_ok(
            secrets.write_secret(
                &expired_key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        "stale-websocket-access-token",
                        Some("websocket-refresh-token".to_owned()),
                    )
                    .with_expires_unix_seconds(900)
                    .to_secret_string(),
                ),
            ),
        );
        let refresh_client = RecordingRefreshClient::new(
            "acct_selected",
            "websocket-refresh-token",
            AccountCredentialBundle::imported_codex_auth(
                "refreshed-websocket-access-token",
                Some("refreshed-websocket-refresh-token".to_owned()),
            )
            .with_expires_unix_seconds(2_000),
        );
        let resolver =
            RouterCredentialResolver::new(&state, &secrets, refresh_client.clone(), 1_000);
        let protocol_router = WebSocketProtocolRouter::new(FirstFramePolicy::new(1024));
        let selector = RecordingSelector::new();
        let auth_gate = local_auth_gate();
        let router =
            AuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router);
        let frame = WebSocketFrame::Text(br#"{"type":"response.create"}"#.to_vec());

        let decision = match router.route_first_frame(
            WebSocketHandshakeRequest::new()
                .with_header(Header::new("X-Codex-Router-Token", "current-token"))
                .with_header(Header::new(
                    "Authorization",
                    "Bearer stale-websocket-access-token",
                )),
            frame,
        ) {
            Ok(decision) => decision,
            Err(error) => panic!("expired websocket credential should refresh: {error:?}"),
        };

        let WebSocketFirstFrameDecision::OpenUpstream { headers, .. } = decision;
        assert_eq!(
            headers.values("authorization"),
            vec!["Bearer refreshed-websocket-access-token"]
        );
        assert_eq!(refresh_client.calls(), 1);
        let loaded_account = must_ok(AccountStateRepository::load_account(&state, &account_id))
            .unwrap_or_else(|| panic!("account should remain registered"));
        assert_eq!(loaded_account.active_credential_generation(), Some(2));
    }

    #[test]
    fn proxy_credential_resolver_refreshes_expired_bundle_through_runtime_wrapper() {
        let temp_dir = ProxyTestTempDir::new("proxy-runtime-resolver-refresh");
        let state_database_path = temp_dir.path().join("state.sqlite");
        let secret_store_root = temp_dir.path().join("secrets");
        let state = must_ok(SqliteStateStore::open(&state_database_path));
        let secrets = must_ok(FileSecretStore::open(&secret_store_root));
        let account_id = account_id("acct_proxy_runtime_refresh");
        let account = AccountRecord::new(account_id.clone(), "runtime", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let expired_key = must_ok(account_credential_bundle_key(&account_id, 1));
        must_ok(
            secrets.write_secret(
                &expired_key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        "expired-proxy-runtime-access-token",
                        Some("proxy-runtime-refresh-token".to_owned()),
                    )
                    .with_expires_unix_seconds(900)
                    .to_secret_string(),
                ),
            ),
        );
        let refresh_client = RecordingRefreshClient::new(
            "acct_proxy_runtime_refresh",
            "proxy-runtime-refresh-token",
            AccountCredentialBundle::imported_codex_auth(
                "refreshed-proxy-runtime-access-token",
                Some("refreshed-proxy-runtime-refresh-token".to_owned()),
            )
            .with_expires_unix_seconds(2_000),
        );
        let resolver = must_ok(ProxyCredentialResolver::open_with_refresh_client(
            &state_database_path,
            &secret_store_root,
            1_000,
            refresh_client.clone(),
        ));

        let resolved = must_ok(resolver.resolve_provider_credentials(&account_id));

        assert_eq!(
            resolved.access_token().expose_secret(),
            "refreshed-proxy-runtime-access-token"
        );
        assert_eq!(refresh_client.calls(), 1);
        let loaded_account = must_ok(AccountStateRepository::load_account(&state, &account_id))
            .unwrap_or_else(|| panic!("account should remain registered"));
        assert_eq!(loaded_account.active_credential_generation(), Some(2));
    }

    #[test]
    fn authenticated_websocket_router_missing_refresh_token_fails_closed_before_upstream_open() {
        let temp_dir = ProxyTestTempDir::new("websocket-router-missing-refresh");
        let state = must_ok(SqliteStateStore::open(
            &temp_dir.path().join("state.sqlite"),
        ));
        let secrets = must_ok(FileSecretStore::open(temp_dir.path().join("secrets")));
        let account_id = account_id("acct_selected");
        let account = AccountRecord::new(account_id.clone(), "selected", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        let expired_key = must_ok(account_credential_bundle_key(&account_id, 1));
        must_ok(
            secrets.write_secret(
                &expired_key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        "stale-websocket-access-token",
                        None,
                    )
                    .with_expires_unix_seconds(900)
                    .to_secret_string(),
                ),
            ),
        );
        let resolver =
            RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_000);
        let protocol_router = WebSocketProtocolRouter::new(FirstFramePolicy::new(1024));
        let selector = RecordingSelector::new();
        let auth_gate = local_auth_gate();
        let router =
            AuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router);

        assert_eq!(
            router.route_first_frame(
                WebSocketHandshakeRequest::new()
                    .with_header(Header::new("X-Codex-Router-Token", "current-token")),
                WebSocketFrame::Text(br#"{"type":"response.create"}"#.to_vec()),
            ),
            Err(WebSocketCloseReason::ProviderCredential)
        );
        assert_eq!(
            selector.take_recorded(),
            vec![("/v1/responses".to_owned(), TokenGeneration::new(1))]
        );
    }

    #[test]
    fn authenticated_websocket_router_rejects_missing_local_token_before_selection() {
        let protocol_router = WebSocketProtocolRouter::new(FirstFramePolicy::new(1024));
        let selector = RecordingSelector::new();
        let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
        let auth_gate = local_auth_gate();
        let router =
            AuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router);

        assert_eq!(
            router.route_first_frame(
                WebSocketHandshakeRequest::new(),
                WebSocketFrame::Text(br#"{"type":"response.create"}"#.to_vec()),
            ),
            Err(WebSocketCloseReason::LocalAuth {
                reason: LocalAuthError::Missing
            })
        );
        assert!(selector.take_recorded().is_empty());
    }

    #[test]
    fn authenticated_websocket_router_accepts_codex_env_key_authorization_bearer() {
        let protocol_router = WebSocketProtocolRouter::new(FirstFramePolicy::new(1024));
        let selector = RecordingSelector::new();
        let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
        let auth_gate = local_auth_gate();
        let router =
            AuthenticatedWebSocketRouter::new(&auth_gate, &selector, &resolver, &protocol_router);

        let decision = match router.route_first_frame(
            WebSocketHandshakeRequest::new()
                .with_header(Header::new("Authorization", "Bearer current-token")),
            WebSocketFrame::Text(br#"{"type":"response.create"}"#.to_vec()),
        ) {
            Ok(decision) => decision,
            Err(error) => panic!("authorization bearer should satisfy local auth: {error:?}"),
        };

        assert_eq!(
            selector.take_recorded(),
            vec![("/v1/responses".to_owned(), TokenGeneration::new(1))]
        );
        let WebSocketFirstFrameDecision::OpenUpstream {
            token_generation,
            headers,
            ..
        } = decision;
        assert_eq!(token_generation, TokenGeneration::new(1));
        assert_eq!(
            headers.values("authorization"),
            vec!["Bearer selected-upstream-token"]
        );
    }

    #[test]
    fn websocket_first_frame_rejects_hostile_preselection_cases() {
        let router = WebSocketProtocolRouter::new(FirstFramePolicy::new(32));

        assert_eq!(
            router
                .route_first_frame(
                    WebSocketHandshakeRequest::new(),
                    WebSocketFrame::Binary(vec![1, 2, 3]),
                    SecretString::new("selected-upstream-token"),
                    None,
                )
                .err(),
            Some(WebSocketCloseReason::UnsupportedFirstFrameType)
        );
        assert_eq!(
            router
                .route_first_frame(
                    WebSocketHandshakeRequest::new(),
                    WebSocketFrame::Text(br#"{"type":"not.response.create"}"#.to_vec()),
                    SecretString::new("selected-upstream-token"),
                    None,
                )
                .err(),
            Some(WebSocketCloseReason::UnexpectedFirstFrame)
        );
        assert_eq!(
            router
                .route_first_frame(
                    WebSocketHandshakeRequest::new(),
                    WebSocketFrame::Text(br#"{}"#.to_vec()),
                    SecretString::new("selected-upstream-token"),
                    None,
                )
                .err(),
            Some(WebSocketCloseReason::UnexpectedFirstFrame)
        );
        assert_eq!(
            router
                .route_first_frame(
                    WebSocketHandshakeRequest::new(),
                    WebSocketFrame::Text(
                        br#"{"type":"response.create","padding":"too-large"}"#.to_vec()
                    ),
                    SecretString::new("selected-upstream-token"),
                    None,
                )
                .err(),
            Some(WebSocketCloseReason::FirstFrameTooLarge)
        );
        assert_eq!(
            router
                .route_first_frame(
                    WebSocketHandshakeRequest::new(),
                    WebSocketFrame::Text(br#"{"type":"#.to_vec()),
                    SecretString::new("selected-upstream-token"),
                    None,
                )
                .err(),
            Some(WebSocketCloseReason::MalformedFirstFrame)
        );
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn blocking_websocket_tunnel_preserves_first_frame_and_sanitizes_upstream_handshake() {
        let upstream_listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(error) => panic!("mock websocket upstream should bind: {error}"),
        };
        let upstream_address = match upstream_listener.local_addr() {
            Ok(address) => address,
            Err(error) => panic!("mock websocket upstream address should read: {error}"),
        };
        let (upstream_sender, upstream_receiver) = mpsc::channel();
        let upstream_thread = thread::spawn(move || {
            let (stream, _peer_address) = match upstream_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("mock websocket upstream should accept: {error}"),
            };
            let mut websocket = match accept_hdr(stream, |request: &Request, response: Response| {
                let authorization = request
                    .headers()
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("<missing>")
                    .to_owned();
                let local_token = request
                    .headers()
                    .get("x-codex-router-token")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                if let Err(error) = upstream_sender.send((authorization, local_token)) {
                    panic!("mock websocket upstream headers should record: {error}");
                }
                Ok(response)
            }) {
                Ok(websocket) => websocket,
                Err(error) => panic!("mock websocket upstream handshake should accept: {error}"),
            };
            let first_frame = match websocket.read() {
                Ok(message) => message,
                Err(error) => panic!("mock websocket upstream should read first frame: {error}"),
            };
            if let Err(error) = upstream_sender.send((first_frame.to_string(), None)) {
                panic!("mock websocket upstream first frame should record: {error}");
            }
            if let Err(error) = websocket.send(Message::text(r#"{"type":"response.completed"}"#)) {
                panic!("mock websocket upstream should send response: {error}");
            }
        });

        let router_listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(error) => panic!("router websocket listener should bind: {error}"),
        };
        let router_address = match router_listener.local_addr() {
            Ok(address) => address,
            Err(error) => panic!("router websocket address should read: {error}"),
        };
        let selector = RecordingSelector::new();
        let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
        let auth_gate = local_auth_gate();
        let protocol_router = WebSocketProtocolRouter::new(FirstFramePolicy::new(1024));
        let upstream_url = format!("ws://{upstream_address}/v1/responses");
        let router_thread = thread::spawn(move || {
            let (stream, _peer_address) = match router_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("router websocket should accept local client: {error}"),
            };
            let tunnel =
                BlockingWebSocketTunnel::new(&auth_gate, &selector, &resolver, &protocol_router);
            match tunnel.handle_connection(stream, upstream_url.as_str(), 1) {
                Ok(()) => {}
                Err(error) => panic!("websocket tunnel should complete: {error}"),
            }
        });

        let mut request = match format!("ws://{router_address}/v1/responses").into_client_request()
        {
            Ok(request) => request,
            Err(error) => panic!("local websocket request should build: {error}"),
        };
        request.headers_mut().insert(
            "X-Codex-Router-Token",
            HeaderValue::from_static("current-token"),
        );
        request.headers_mut().insert(
            "Authorization",
            HeaderValue::from_static("Bearer hostile-client-auth"),
        );
        let (mut client, _response) = match connect(request) {
            Ok(connection) => connection,
            Err(error) => panic!("local websocket client should connect: {error}"),
        };
        let first_frame = r#"{"type":"response.create","unknown_codex_field":{"kept":true}}"#;
        if let Err(error) = client.send(Message::text(first_frame)) {
            panic!("local websocket client should send first frame: {error}");
        }
        let response = match client.read() {
            Ok(message) => message,
            Err(error) => panic!("local websocket client should read upstream response: {error}"),
        };

        assert_eq!(response.to_string(), r#"{"type":"response.completed"}"#);
        let (authorization, local_token) = match upstream_receiver.recv() {
            Ok(recorded) => recorded,
            Err(error) => panic!("upstream handshake should be recorded: {error}"),
        };
        assert_eq!(authorization, "Bearer selected-upstream-token");
        assert_eq!(local_token, None);
        let (recorded_first_frame, _) = match upstream_receiver.recv() {
            Ok(recorded) => recorded,
            Err(error) => panic!("upstream first frame should be recorded: {error}"),
        };
        assert_eq!(recorded_first_frame, first_frame);

        match router_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("router websocket thread panicked: {error:?}"),
        }
        match upstream_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("mock websocket upstream thread panicked: {error:?}"),
        }
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn blocking_websocket_tunnel_pins_one_upstream_account_for_multiple_turns() {
        let upstream_listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(error) => panic!("mock websocket upstream should bind: {error}"),
        };
        let upstream_address = match upstream_listener.local_addr() {
            Ok(address) => address,
            Err(error) => panic!("mock websocket upstream address should read: {error}"),
        };
        let (upstream_sender, upstream_receiver) = mpsc::channel();
        let upstream_thread = thread::spawn(move || {
            let (stream, _peer_address) = match upstream_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("mock websocket upstream should accept: {error}"),
            };
            let mut websocket = match accept_hdr(stream, |request: &Request, response: Response| {
                let authorization = request
                    .headers()
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("<missing>")
                    .to_owned();
                if let Err(error) = upstream_sender.send(("auth".to_owned(), authorization)) {
                    panic!("mock websocket upstream auth should record: {error}");
                }
                Ok(response)
            }) {
                Ok(websocket) => websocket,
                Err(error) => panic!("mock websocket upstream handshake should accept: {error}"),
            };
            for turn in 1..=2 {
                let frame = match websocket.read() {
                    Ok(message) => message,
                    Err(error) => {
                        panic!("mock websocket upstream should read turn {turn}: {error}")
                    }
                };
                if let Err(error) =
                    upstream_sender.send((format!("turn-{turn}"), frame.to_string()))
                {
                    panic!("mock websocket upstream turn should record: {error}");
                }
                if let Err(error) =
                    websocket.send(Message::text(r#"{"type":"response.completed"}"#))
                {
                    panic!("mock websocket upstream should complete turn {turn}: {error}");
                }
            }
        });

        let router_listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(error) => panic!("router websocket listener should bind: {error}"),
        };
        let router_address = match router_listener.local_addr() {
            Ok(address) => address,
            Err(error) => panic!("router websocket address should read: {error}"),
        };
        let selector = RecordingSelector::new();
        let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
        let auth_gate = local_auth_gate();
        let protocol_router = WebSocketProtocolRouter::new(FirstFramePolicy::new(1024));
        let upstream_url = format!("ws://{upstream_address}/v1/responses");
        let router_thread = thread::spawn(move || {
            let (stream, _peer_address) = match router_listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("router websocket should accept local client: {error}"),
            };
            let tunnel =
                BlockingWebSocketTunnel::new(&auth_gate, &selector, &resolver, &protocol_router);
            match tunnel.handle_connection(stream, upstream_url.as_str(), 1) {
                Ok(()) => {}
                Err(error) => panic!("websocket tunnel should complete: {error}"),
            }
        });

        let mut request = match format!("ws://{router_address}/v1/responses").into_client_request()
        {
            Ok(request) => request,
            Err(error) => panic!("local websocket request should build: {error}"),
        };
        request.headers_mut().insert(
            "X-Codex-Router-Token",
            HeaderValue::from_static("current-token"),
        );
        let (mut client, _response) = match connect(request) {
            Ok(connection) => connection,
            Err(error) => panic!("local websocket client should connect: {error}"),
        };
        for turn in 1..=2 {
            let frame = format!(r#"{{"type":"response.create","turn":{turn}}}"#);
            if let Err(error) = client.send(Message::text(frame)) {
                panic!("local websocket client should send turn {turn}: {error}");
            }
            let response = match client.read() {
                Ok(message) => message,
                Err(error) => panic!("local websocket client should read turn {turn}: {error}"),
            };
            assert_eq!(response.to_string(), r#"{"type":"response.completed"}"#);
        }
        if let Err(error) = client.close(None) {
            panic!("local websocket client should close: {error}");
        }

        assert_eq!(
            upstream_receiver.recv().unwrap_or_else(|error| {
                panic!("upstream auth should record: {error}");
            }),
            (
                "auth".to_owned(),
                "Bearer selected-upstream-token".to_owned()
            )
        );
        assert_eq!(
            upstream_receiver.recv().unwrap_or_else(|error| {
                panic!("upstream first turn should record: {error}");
            }),
            (
                "turn-1".to_owned(),
                r#"{"type":"response.create","turn":1}"#.to_owned()
            )
        );
        assert_eq!(
            upstream_receiver.recv().unwrap_or_else(|error| {
                panic!("upstream second turn should record: {error}");
            }),
            (
                "turn-2".to_owned(),
                r#"{"type":"response.create","turn":2}"#.to_owned()
            )
        );

        match router_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("router websocket thread panicked: {error:?}"),
        }
        match upstream_thread.join() {
            Ok(()) => {}
            Err(error) => panic!("mock websocket upstream thread panicked: {error:?}"),
        }
    }
}
