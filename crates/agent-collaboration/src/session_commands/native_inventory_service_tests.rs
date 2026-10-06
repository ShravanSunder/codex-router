//! Actual Control service and source-local SQLite paging, without a live native backend.
use super::*;
use crate::sessions::picker_runtime_inventory::native_inventory_binding::{
    NativeEndpointSelector, NativeInventoryContext, bind_native_inventory,
};
use collaboration_service::{
    LocalControlService, ManifestPublication, NativeControlBackend, NativeGenerationGate,
    ServiceIdentity,
};
use std::{io::ErrorKind, os::unix::net::UnixListener, path::PathBuf};
use tokio_util::sync::CancellationToken;

struct StoredServiceFixture {
    root: tempfile::TempDir,
    publication: ManifestPublication,
    stop: CancellationToken,
    server: tokio::task::JoinHandle<()>,
    native_listener: UnixListener,
    database: PathBuf,
    original_database: Vec<u8>,
}

impl StoredServiceFixture {
    async fn open(service_id: &str, model: &str) -> (ControlClient, Self) {
        let root = tempfile::Builder::new()
            .prefix("sel-pages-")
            .tempdir_in("/tmp")
            .unwrap();
        let directory = std::fs::canonicalize(root.path()).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        let home = directory.join("source-home");
        std::fs::create_dir(&home).unwrap();
        let database = home.join("state_5.sqlite");
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&database)
            .create_if_missing(true);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT, cwd TEXT, model_provider TEXT, model TEXT, reasoning_effort TEXT, source TEXT, thread_source TEXT, git_branch TEXT, git_origin_url TEXT, name TEXT, title TEXT, preview TEXT, first_user_message TEXT NOT NULL DEFAULT '', created_at_ms INTEGER, updated_at_ms INTEGER, recency_at_ms INTEGER, archived INTEGER); CREATE INDEX idx_threads_updated_at_ms ON threads(updated_at_ms DESC, id DESC);")
            .execute(&pool).await.unwrap();
        for (session_id, message, timestamp) in [
            ("empty-source", "", 3000),
            ("shared-source-id", "User message", 2000),
        ] {
            sqlx::query("INSERT INTO threads (id,cwd,model,reasoning_effort,name,title,first_user_message,source,thread_source,updated_at_ms,recency_at_ms,archived) VALUES (?, '/source/owned-project', ?, 'high', NULL, 'Source session', ?, 'cli', 'cli', ?, ?, 0)")
                .bind(session_id).bind(model).bind(message).bind(timestamp).bind(timestamp).execute(&pool).await.unwrap();
        }
        pool.close().await;
        let original_database = std::fs::read(&database).unwrap();
        let native_listener = UnixListener::bind(directory.join("native.sock")).unwrap();
        native_listener.set_nonblocking(true).unwrap();
        let endpoint: EndpointRef =
            serde_json::from_value(json!({"serviceId":service_id,"endpointId":"codex-local"}))
                .unwrap();
        let description = serde_json::from_value(json!({
            "endpoint":endpoint,"label":"Source paging fixture","availability":{"state":"unprobed"},
            "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":null,"generation":null}]
        })).unwrap();
        let digest = format!("sha256:{}", "a".repeat(64));
        let identity = ServiceIdentity::new(service_id, EPOCH, &digest)
            .unwrap()
            .with_endpoints(vec![description])
            .unwrap()
            .with_native_backend(NativeControlBackend {
                endpoint,
                gate: NativeGenerationGate::default(),
                codex_home: home,
            })
            .unwrap();
        let service = LocalControlService::bind(&directory.join("control.sock"), identity).unwrap();
        let manifest = serde_json::from_value(json!({
            "version":2,"serviceId":service_id,"serviceEpoch":EPOCH,"machineLabel":"Source paging fixture",
            "control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest,
            "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
        })).unwrap();
        let publication = ManifestPublication::publish(&directory, &manifest).unwrap();
        let stop = CancellationToken::new();
        let server_stop = stop.clone();
        let server = tokio::spawn(async move {
            service.run(server_stop).await.unwrap();
        });
        let client = ControlClient::connect(&directory, "source-paging-test", "1")
            .await
            .unwrap();
        (
            client,
            Self {
                root,
                publication,
                stop,
                server,
                native_listener,
                database,
                original_database,
            },
        )
    }

    async fn seed_subagent_fixture(&mut self) {
        use sqlx::Connection;
        let mut connection = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(&self.database),
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO threads (id,cwd,model,reasoning_effort,name,title,first_user_message,source,thread_source,updated_at_ms,recency_at_ms,archived) VALUES ('source-subagent','/source/owned-project','subagent-model','high',NULL,'Source subagent','Source user message','cli','subagent',1000,1000,0)")
            .execute(&mut connection).await.unwrap();
        connection.close().await.unwrap();
        // This is fixture arrangement before any session read, not part of the read proof.
        self.original_database = std::fs::read(&self.database).unwrap();
    }

    async fn finish(self) {
        assert_eq!(
            self.native_listener.accept().unwrap_err().kind(),
            ErrorKind::WouldBlock,
            "stored paging cannot attach or create native work"
        );
        self.stop.cancel();
        self.server.await.unwrap();
        assert_eq!(
            std::fs::read(&self.database).unwrap(),
            self.original_database,
            "source catalog reads must leave stored bytes intact"
        );
        drop(self.publication);
        drop(self.root);
    }
}

#[tokio::test]
async fn real_source_catalog_respects_interactive_subagent_and_all_page_filters() {
    let (mut client, mut fixture) = StoredServiceFixture::open(SERVICE, "interactive-model").await;
    fixture.seed_subagent_fixture().await;
    for (source, expected_ids) in [
        (NativeSessionSource::Interactive, vec!["shared-source-id"]),
        (NativeSessionSource::Subagents, vec!["source-subagent"]),
        (
            NativeSessionSource::All,
            vec!["shared-source-id", "source-subagent"],
        ),
    ] {
        let mut request = paging_request(NativeSessionView::Stored);
        request.scope = NativeSessionScope::Any;
        request.query = None;
        request.source = source;
        request.page_size = 1;
        let mut pager = NativeInventoryPager::new(request, None).unwrap();
        let mut actual_ids = Vec::new();
        while let Some(page) = pager.next_page(&mut client).await.unwrap() {
            for summary in page.sessions {
                assert!(source == NativeSessionSource::All || source == summary.source);
                actual_ids.push(String::from(summary.target.session_id));
            }
        }
        assert_eq!(
            actual_ids, expected_ids,
            "source-owned paging must keep its requested filter"
        );
    }
    client.close().await.unwrap();
    fixture.finish().await;
}

#[tokio::test]
async fn real_source_services_page_sparse_stored_catalogs_without_runtime_generation_or_effects() {
    let mut observed = Vec::new();
    for (service_id, model) in [
        (SERVICE, "gpt-6.1-sol"),
        ("00000000-0000-4000-8000-000000000003", "gpt-6-luna"),
    ] {
        let (mut client, fixture) = StoredServiceFixture::open(service_id, model).await;
        let inventory = client.list_endpoints().await.unwrap();
        let binding = bind_native_inventory(
            &inventory,
            client.identity(),
            &service_id.to_owned().try_into().unwrap(),
            NativeEndpointSelector::UniqueNative,
            NativeSessionView::Stored,
        )
        .unwrap();
        assert_eq!(binding.context, NativeInventoryContext::Stored);
        let mut request = paging_request(NativeSessionView::Stored);
        request.endpoint = binding.endpoint.clone();
        request.scope = NativeSessionScope::Any;
        request.query = Some("Source".to_owned());
        request.page_size = 1;
        let mut pager = NativeInventoryPager::new(request, None).unwrap();
        let sparse = pager.next_page(&mut client).await.unwrap().unwrap();
        assert!(sparse.sessions.is_empty());
        assert!(
            sparse.next_cursor.is_some(),
            "a filtered-out first row is not exhaustion"
        );
        assert!(sparse.generation.is_none());
        assert_eq!(
            sparse.endpoint, binding.endpoint,
            "empty rows do not erase source binding"
        );
        let second = pager.next_page(&mut client).await.unwrap().unwrap();
        assert!(second.generation.is_none());
        assert_eq!(second.sessions.len(), 1);
        assert_eq!(second.sessions[0].model.as_deref(), Some(model));
        assert_eq!(second.sessions[0].reasoning_effort.as_deref(), Some("high"));
        let row = SessionPickerRecord::from_native_summary(&second.sessions[0]);
        assert_eq!(
            row.identity,
            SessionPickerIdentity::HostedCodex(second.sessions[0].target.clone())
        );
        assert_eq!(
            row.provenance,
            crate::sessions::SessionRowProvenance::ObservedHosted
        );
        assert_eq!(row.model.as_deref(), Some(model));
        assert_eq!(row.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(row.cwd.as_deref(), Some("/source/owned-project"));
        assert_eq!(row.native_source, Some(NativeSessionSource::Interactive));
        assert_eq!(row.recency_at_ms, Some(2000));
        assert!(row.normalized_cwd.is_none() && row.provider.is_none());
        assert!(row.created_at_ms.is_none() && row.conversation_source.is_none());
        assert_eq!(row.runtime_status, PickerRuntimeStatus::Unknown);
        assert_eq!(
            String::from(second.sessions[0].working_directory.clone()),
            "/source/owned-project"
        );
        observed.push(second.sessions[0].target.clone());
        let exhausted = pager.next_page(&mut client).await.unwrap().unwrap();
        assert!(exhausted.sessions.is_empty() && exhausted.next_cursor.is_none());
        assert!(pager.next_page(&mut client).await.unwrap().is_none());
        client.close().await.unwrap();
        fixture.finish().await;
    }
    assert_eq!(observed[0].session_id, observed[1].session_id);
    assert_ne!(
        observed[0].endpoint, observed[1].endpoint,
        "equal native IDs cannot erase their actual source service"
    );
}

#[tokio::test]
async fn real_source_service_rejects_an_inventory_request_for_another_service() {
    let (mut client, fixture) =
        StoredServiceFixture::open("00000000-0000-4000-8000-000000000003", "gpt-6-luna").await;
    let mut pager =
        NativeInventoryPager::new(paging_request(NativeSessionView::Stored), None).unwrap();
    assert!(pager.next_page(&mut client).await.is_err());
    assert!(
        pager.next_page(&mut client).await.is_err(),
        "source rejection cannot turn into exhausted success or a retry"
    );
    let _closed = client.close().await;
    fixture.finish().await;
}
