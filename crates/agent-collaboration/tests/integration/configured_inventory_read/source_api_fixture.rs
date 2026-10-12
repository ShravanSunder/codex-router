//! Real loopback MCP/API/SQLite read wiring; catalog fixtures are not a debug Host.
use super::router_connection_registry::RouterConnectionRegistry;
use crate::presentation::session_picker::{PickerSourceContext, SourceInventoryRequest};
use crate::sessions::*;
use collaboration_client::protocol::{EndpointRef, RouterExecutableRelation};
use collaboration_service::{
    CollaborationApplication, NativeControlBackend, NativeGenerationGate, ServiceIdentity,
};
use std::{
    io::ErrorKind,
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::PathBuf,
    time::Duration,
};
use tokio_util::sync::CancellationToken;

mod recorded_api_listener;

pub(crate) struct SourceApiFixture {
    _root: tempfile::TempDir,
    pub(super) url: String,
    pub(super) endpoint: EndpointRef,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    native: UnixListener,
    database: PathBuf,
    original: Vec<u8>,
    recorder: recorded_api_listener::RecordedApiListener,
}

impl SourceApiFixture {
    pub(crate) async fn start(service: &str, model: &str) -> Self {
        Self::start_with_provider(service, model, false).await
    }

    pub(super) async fn start_with_provider(
        service: &str,
        model: &str,
        include_provider: bool,
    ) -> Self {
        let root = tempfile::Builder::new()
            .prefix("sel-mcp-")
            .tempdir_in("/tmp")
            .unwrap();
        let directory = std::fs::canonicalize(root.path()).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        let source_data = directory.join("source-catalog");
        std::fs::create_dir(&source_data).unwrap();
        let database = source_data.join("state_5.sqlite");
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(&database)
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT, cwd TEXT, model_provider TEXT, model TEXT, reasoning_effort TEXT, source TEXT, thread_source TEXT, git_branch TEXT, git_origin_url TEXT, name TEXT, title TEXT, preview TEXT, first_user_message TEXT NOT NULL DEFAULT '', created_at_ms INTEGER, updated_at_ms INTEGER, recency_at_ms INTEGER, archived INTEGER); CREATE INDEX idx_threads_updated_at_ms ON threads(updated_at_ms DESC, id DESC);")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO threads (id,cwd,model,reasoning_effort,title,first_user_message,source,thread_source,updated_at_ms,recency_at_ms,archived) VALUES ('shared-source-id','/source/project',?,'high','Source session','User message','cli','cli',2000,2000,0)")
            .bind(model).execute(&pool).await.unwrap();
        pool.close().await;
        let original = std::fs::read(&database).unwrap();
        let native = UnixListener::bind(directory.join("native.sock")).unwrap();
        native.set_nonblocking(true).unwrap();
        let endpoint: EndpointRef = serde_json::from_value(
            serde_json::json!({"serviceId":service,"endpointId":"codex-local"}),
        )
        .unwrap();
        let description = serde_json::from_value(serde_json::json!({
            "endpoint":endpoint,"label":"Source fixture","availability":{"state":"unprobed"},
            "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":null,"generation":null}]
        })).unwrap();
        let mut identity = ServiceIdentity::new(service, "00000000-0000-4000-8000-000000000009")
            .unwrap()
            .with_endpoints(vec![description])
            .unwrap()
            .with_native_backend(NativeControlBackend {
                endpoint: endpoint.clone(),
                gate: NativeGenerationGate::default(),
                codex_home: source_data,
            })
            .unwrap();
        if include_provider {
            use collaboration_client::protocol::{
                ProviderRequestedPolicy, RouterAccess, SessionRef,
            };
            use collaboration_service::{
                ProviderOperationStore, ProviderSessionEventHub, ProviderSessionRecord,
            };
            let target: SessionRef = serde_json::from_value(serde_json::json!({
                "endpoint":{"serviceId":service,"endpointId":"claude-local"},"sessionId":"shared-source-id"
            })).unwrap();
            let owner: SessionRef = serde_json::from_value(serde_json::json!({
                "endpoint":endpoint,"sessionId":"fixture-owner"
            }))
            .unwrap();
            let store = std::sync::Arc::new(tokio::sync::Mutex::new(
                ProviderOperationStore::open(&directory.join("provider-operations.sqlite"))
                    .await
                    .unwrap(),
            ));
            store
                .lock()
                .await
                .record_session(&ProviderSessionRecord {
                    target: target.clone(),
                    working_directory: "/source/provider".to_owned().try_into().unwrap(),
                    requested_policy: ProviderRequestedPolicy {
                        access: RouterAccess::WriteRestricted,
                    },
                    created_by: owner.clone().into(),
                    approver: owner.into(),
                    updated_at_ms: 2000,
                })
                .await
                .unwrap();
            identity = identity
                .with_provider_operation_store(std::sync::Arc::clone(&store))
                .with_provider_session_hub(std::sync::Arc::new(
                    ProviderSessionEventHub::with_capacity(store, 16),
                ));
            identity.endpoint_directory().publish(serde_json::from_value(serde_json::json!({
                "endpoint":target.endpoint,"label":"Source provider","availability":{"state":"unprobed"},
                "channels":[{"kind":"externalProvider","transport":"stdioAcp","bindingId":"fixture-provider","bindingGeneration":1,
                    "runtime":{"provider":"claudeCode","runtimeName":"fixture"},"capabilities":[{"name":"create","status":"supported","evidence":"advertised"}]}]
            })).unwrap()).unwrap();
        }
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let shutdown = CancellationToken::new();
        let (_sender, relation) = tokio::sync::watch::channel(RouterExecutableRelation::Match);
        let api = collaboration_mcp::collaboration_api_router(
            &collaboration_mcp::CollaborationApiConfig {
                application: CollaborationApplication::new(identity),
                service_directory: directory,
                native_definitions: None,
                router_executable_relation: relation,
                concurrent_requests: collaboration_mcp::DEFAULT_CONCURRENT_REQUESTS,
                shutdown: shutdown.clone(),
            },
            collaboration_mcp::CollaborationApiListener::LoopbackTcp(address),
        );
        let task = tokio::spawn(collaboration_mcp::serve_collaboration_api(
            listener,
            api,
            shutdown.clone(),
        ));
        let recorder = recorded_api_listener::RecordedApiListener::start(
            format!("http://{address}/mcp"),
            shutdown.clone(),
        )
        .await;
        Self {
            _root: root,
            url: recorder.url.clone(),
            endpoint,
            shutdown,
            task,
            native,
            database,
            original,
            recorder,
        }
    }

    pub(crate) fn request(&self, name: &str) -> SourceInventoryRequest {
        let registry = RouterConnectionRegistry::parse(&serde_json::json!({"version":1,"routers":[{
            "name":name,"connection":{"kind":"remote","serviceId":self.endpoint.service_id,"mcpUrl":self.url}
        }]}).to_string()).unwrap();
        SourceInventoryRequest {
            source_context: PickerSourceContext::ConfiguredHosted(registry.routers[0].clone()),
            query: SessionsPickerDataQuery {
                root: SessionsPickerRoot::Any,
                provider: SessionsProvider::Any,
                source: SessionsSource::All,
                sort: SessionsSort::Updated,
                search: String::new(),
                include_empty_sessions: false,
            },
            request_generation: 17,
        }
    }

    pub(crate) fn api_call_count(&self, method: &str) -> usize {
        self.recorder.call_count(method)
    }

    pub(crate) fn loader(&self) -> SessionsPickerRecordLoader {
        session_picker_record_loader(
            CliContext::new(Vec::new()),
            discover_repository_identity(std::path::Path::new("/tmp")),
            None,
        )
    }

    pub(crate) async fn finish(self) {
        self.shutdown.cancel();
        self.recorder.finish().await;
        tokio::time::timeout(Duration::from_secs(2), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            self.native.accept().unwrap_err().kind(),
            ErrorKind::WouldBlock,
            "stored read must not attach native work"
        );
        assert_eq!(
            std::fs::read(self.database).unwrap(),
            self.original,
            "source reads must preserve catalog bytes"
        );
    }
}
