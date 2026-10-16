//! Actual collaboration API and source-local SQLite paging, without a live native backend.
use super::*;
use crate::sessions::picker_runtime_inventory::native_inventory_binding::{
    NativeEndpointSelector, NativeInventoryContext, bind_native_inventory,
};
use collaboration_service::{
    CollaborationApplication, NativeControlBackend, NativeGenerationGate, ServiceIdentity,
};
use std::{io::ErrorKind, os::unix::net::UnixListener, path::PathBuf};

struct StoredServiceFixture {
    root: tempfile::TempDir,
    served: collaboration_mcp::test_support::ServedCollaborationApi,
    native_listener: UnixListener,
    database: PathBuf,
    original_database: Vec<u8>,
}

impl StoredServiceFixture {
    async fn open(service_id: &str, model: &str) -> (CollaborationClient, Self) {
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
        let identity = ServiceIdentity::new(service_id, EPOCH)
            .unwrap()
            .with_endpoints(vec![description])
            .unwrap()
            .with_native_backend(NativeControlBackend {
                endpoint,
                gate: NativeGenerationGate::default(),
                codex_home: home,
            })
            .unwrap();
        let served = collaboration_mcp::test_support::ServedCollaborationApi::start(
            &directory,
            CollaborationApplication::new(identity),
        )
        .await
        .unwrap();
        let client = CollaborationClient::connect(&directory, "source-paging-test", "1")
            .await
            .unwrap();
        (
            client,
            Self {
                root,
                served,
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
        self.served.stop().await.unwrap();
        assert_eq!(
            std::fs::read(&self.database).unwrap(),
            self.original_database,
            "source catalog reads must leave stored bytes intact"
        );
        drop(self.root);
    }
}

#[tokio::test]
async fn stored_source_predicate_divergence_keeps_valid_later_pages_available() {
    use sqlx::Connection;
    let (mut client, mut fixture) = StoredServiceFixture::open(SERVICE, "interactive-model").await;
    fixture.seed_subagent_fixture().await;
    let mut connection = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(&fixture.database),
    )
    .await
    .unwrap();
    for (session_id, source, thread_source, timestamp) in [
        ("user-tagged-subagent", "nested-subagent", "user", 5000),
        ("mixed-case-source", "SubAgent", "cli", 4000),
    ] {
        sqlx::query("INSERT INTO threads (id,cwd,model,title,first_user_message,source,thread_source,updated_at_ms,recency_at_ms,archived) VALUES (?, '/source/owned-project', 'edge-model', 'Source edge', 'User message', ?, ?, ?, ?, 0)")
            .bind(session_id).bind(source).bind(thread_source).bind(timestamp).bind(timestamp)
            .execute(&mut connection).await.unwrap();
    }
    connection.close().await.unwrap();
    // Baseline follows fixture arrangement; only subsequent reads are proved immutable.
    fixture.original_database = std::fs::read(&fixture.database).unwrap();
    for (source, expected_ids, expected_sparse_pages) in [
        (
            NativeSessionSource::Interactive,
            vec!["shared-source-id"],
            2,
        ),
        (
            NativeSessionSource::Subagents,
            vec!["user-tagged-subagent", "source-subagent"],
            1,
        ),
        (
            NativeSessionSource::All,
            vec![
                "user-tagged-subagent",
                "mixed-case-source",
                "shared-source-id",
                "source-subagent",
            ],
            1,
        ),
    ] {
        let mut request = paging_request(NativeSessionView::Stored);
        request.scope = NativeSessionScope::Any;
        request.query = None;
        request.source = source;
        request.page_size = 1;
        let mut pager = NativeInventoryPager::new(request, None).unwrap();
        let mut actual_ids = Vec::new();
        let mut sparse_pages = 0;
        while let Some(page) = pager.next_page(&mut client).await.unwrap() {
            assert!(page.generation.is_none());
            if page.sessions.is_empty() && page.next_cursor.is_some() {
                sparse_pages += 1;
            }
            for summary in page.sessions {
                assert!(source == NativeSessionSource::All || source == summary.source);
                actual_ids.push(String::from(summary.target.session_id));
            }
        }
        assert_eq!(actual_ids, expected_ids);
        assert_eq!(sparse_pages, expected_sparse_pages);
    }
    fixture.finish().await;
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
    fixture.finish().await;
}

#[tokio::test]
async fn concurrent_source_services_keep_equal_session_ids_and_cross_source_reads_isolated() {
    let second_service = "00000000-0000-4000-8000-000000000003";
    let ((mut first_client, first_fixture), (mut second_client, second_fixture)) = tokio::join!(
        StoredServiceFixture::open(SERVICE, "first-source-model"),
        StoredServiceFixture::open(second_service, "second-source-model"),
    );
    let (first_inventory, second_inventory) = tokio::join!(
        first_client.list_endpoints(),
        second_client.list_endpoints(),
    );
    let first_binding = bind_native_inventory(
        &first_inventory.unwrap(),
        first_client.identity(),
        &SERVICE.to_owned().try_into().unwrap(),
        NativeEndpointSelector::UniqueNative,
        NativeSessionView::Stored,
    )
    .unwrap();
    let second_binding = bind_native_inventory(
        &second_inventory.unwrap(),
        second_client.identity(),
        &second_service.to_owned().try_into().unwrap(),
        NativeEndpointSelector::UniqueNative,
        NativeSessionView::Stored,
    )
    .unwrap();

    let request_for = |endpoint: EndpointRef| {
        let mut request = paging_request(NativeSessionView::Stored);
        request.endpoint = endpoint;
        request.scope = NativeSessionScope::Any;
        request.query = Some("Source".to_owned());
        request.page_size = 1;
        request
    };
    // A live second service must not make a request sent to the first service valid.
    // Each API call is independent; the failed pager remains rejected. Keep its
    // negative request separate and verify both healthy source readers still work.
    let mut rejected_client = CollaborationClient::connect(
        &std::fs::canonicalize(first_fixture.root.path()).unwrap(),
        "wrong-source-probe",
        "1",
    )
    .await
    .unwrap();
    let mut wrong_source =
        NativeInventoryPager::new(request_for(second_binding.endpoint.clone()), None).unwrap();
    assert!(wrong_source.next_page(&mut rejected_client).await.is_err());
    assert!(wrong_source.next_page(&mut rejected_client).await.is_err());

    let mut first_pager =
        NativeInventoryPager::new(request_for(first_binding.endpoint.clone()), None).unwrap();
    let mut second_pager =
        NativeInventoryPager::new(request_for(second_binding.endpoint.clone()), None).unwrap();
    let (first_sparse, second_sparse) = tokio::join!(
        first_pager.next_page(&mut first_client),
        second_pager.next_page(&mut second_client),
    );
    for sparse in [first_sparse, second_sparse] {
        let page = sparse.unwrap().unwrap();
        assert!(page.sessions.is_empty() && page.next_cursor.is_some());
        assert!(page.generation.is_none());
    }
    let (first_page, second_page) = tokio::join!(
        first_pager.next_page(&mut first_client),
        second_pager.next_page(&mut second_client),
    );
    let first_page = first_page.unwrap().unwrap();
    let second_page = second_page.unwrap().unwrap();
    assert_eq!(first_page.endpoint, first_binding.endpoint);
    assert_eq!(second_page.endpoint, second_binding.endpoint);
    assert_eq!(first_page.sessions.len(), 1);
    assert_eq!(second_page.sessions.len(), 1);
    let first_record = SessionPickerRecord::from_native_summary(&first_page.sessions[0]);
    let second_record = SessionPickerRecord::from_native_summary(&second_page.sessions[0]);
    assert_eq!(
        first_page.sessions[0].target.session_id,
        "shared-source-id".to_owned().try_into().unwrap()
    );
    assert_eq!(
        first_page.sessions[0].target.session_id,
        second_page.sessions[0].target.session_id
    );
    assert_ne!(first_record.identity, second_record.identity);
    assert_eq!(first_record.model.as_deref(), Some("first-source-model"));
    assert_eq!(second_record.model.as_deref(), Some("second-source-model"));
    assert!(first_record.normalized_cwd.is_none() && second_record.normalized_cwd.is_none());
    tokio::join!(first_fixture.finish(), second_fixture.finish());
}
