//! Stored discovery publishes only metadata observations, even without a live backend.
#[cfg(test)]
mod tests {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use collaboration_client::{ClientError, ControlClient, JournalStatus};
    use collaboration_protocol::{NativeSessionListParams, NativeSessionView};
    use collaboration_service::{
        LocalControlService, ManifestPublication, NativeControlBackend, NativeGenerationGate,
        ServiceIdentity,
    };
    use lifecycle_observation::{LifecycleStore, ObservationJournal};
    use serde_json::json;
    use std::{os::unix::fs::DirBuilderExt, path::PathBuf, sync::Arc};
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn paged_stored_discovery_records_addresses_without_claiming_live_state() {
        // Arrange: real read-only native catalog and distinct lifecycle storage; no native socket.
        let root = PathBuf::from(format!("/tmp/stored-journal-{}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        let home = root.join("native-home");
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
        sqlx::raw_sql("CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT, cwd TEXT, model_provider TEXT, model TEXT, reasoning_effort TEXT, source TEXT, thread_source TEXT, git_branch TEXT, git_origin_url TEXT, name TEXT, title TEXT, preview TEXT, first_user_message TEXT, created_at_ms INTEGER, updated_at_ms INTEGER, recency_at_ms INTEGER, archived INTEGER); CREATE INDEX idx_threads_updated_at_ms ON threads(updated_at_ms DESC, id DESC);")
            .execute(&pool).await.unwrap();
        let title = "PRIVATE_TITLE".repeat(60_000);
        for (id, time) in [("stored-b", 2000_i64), ("stored-a", 1000_i64)] {
            sqlx::query("INSERT INTO threads (id,cwd,model,reasoning_effort,name,title,preview,first_user_message,updated_at_ms,archived) VALUES (?, '/private-fixture-workspace', 'gpt-5.6-sol', 'medium', ?, 'PRIVATE_TITLE', 'PRIVATE_BODY', 'PRIVATE_BODY', ?, 0)")
                .bind(id).bind(&title).bind(time).execute(&pool).await.unwrap();
        }
        pool.close().await;
        let original_catalog = std::fs::read(&database).unwrap();
        let id = "00000000-0000-4000-8000-000000000001";
        let epoch = "00000000-0000-4000-8000-000000000002";
        let endpoint: collaboration_protocol::EndpointRef =
            serde_json::from_value(json!({"serviceId":id,"endpointId":"codex-local"})).unwrap();
        let journal = ObservationJournal::open(
            &root.join("journal.sqlite"),
            id.to_owned().try_into().unwrap(),
        )
        .await
        .unwrap();
        let store = Arc::new(LifecycleStore::new(journal));
        let digest = format!("sha256:{}", "a".repeat(64));
        let description = serde_json::from_value(json!({"endpoint":endpoint,"label":"Stored fixture","availability":{"state":"unprobed"},"channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"absent-native.sock","schemaDigest":null,"generation":null}]})).unwrap();
        let identity = ServiceIdentity::new(id, epoch, &digest)
            .unwrap()
            .with_endpoints(vec![description])
            .unwrap()
            .with_journal(Arc::clone(&store))
            .with_native_backend(NativeControlBackend {
                endpoint: endpoint.clone(),
                gate: NativeGenerationGate::default(),
                codex_home: home.clone(),
            })
            .unwrap();
        let listener = LocalControlService::bind(&root.join("control.sock"), identity).unwrap();
        let manifest = serde_json::from_value(json!({"version":2,"serviceId":id,"serviceEpoch":epoch,"control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}})).unwrap();
        let publication = ManifestPublication::publish(&root, &manifest).unwrap();
        let stop = CancellationToken::new();
        let server = tokio::spawn(listener.run(stop.clone()));
        let mut client = ControlClient::connect(&root, "stored-journal-test", "1")
            .await
            .unwrap();
        let request = |cursor| NativeSessionListParams {
            endpoint: endpoint.clone(),
            view: NativeSessionView::Stored,
            scope: collaboration_protocol::NativeSessionScope::Any,
            source: collaboration_protocol::NativeSessionSource::All,
            query: None,
            page_size: 100,
            cursor,
        };
        // Act: page from the catalog through the public SDK, then read the public address book.
        let first = client.list_sessions(request(None)).await.unwrap();
        let second = client
            .list_sessions(request(first.next_cursor.clone()))
            .await
            .unwrap();
        let addresses = client.list_addresses(&endpoint, 100, None).await.unwrap();
        let JournalStatus::Available { bounds } = client.journal_status().await.unwrap() else {
            panic!("storage should be available");
        };
        let history = client
            .read_journal(
                &endpoint,
                collaboration_protocol::JournalPosition {
                    journal_id: bounds.journal_id,
                    sequence: 0,
                },
                100,
                0,
            )
            .await
            .unwrap();
        // Cursor mutations must not silently restart paging from its first row.
        let cursor = first.next_cursor.as_ref().unwrap();
        let mut malformed: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(cursor).unwrap()).unwrap();
        malformed["stored_id"] = serde_json::Value::Null;
        let invalid_cursor = client
            .list_sessions(request(Some(
                URL_SAFE_NO_PAD.encode(serde_json::to_vec(&malformed).unwrap()),
            )))
            .await;
        client.close().await.unwrap();
        stop.cancel();
        server.await.unwrap().unwrap();
        drop(publication);
        Arc::try_unwrap(store)
            .unwrap_or_else(|_| panic!("journal still shared"))
            .close()
            .await;
        let unchanged_catalog = std::fs::read(&database).unwrap();
        std::fs::remove_file(root.join("journal.sqlite")).unwrap();
        std::fs::remove_file(database).unwrap();
        std::fs::remove_dir(home).unwrap();
        std::fs::remove_dir(root).unwrap();
        // Assert: stored references are historical discovery, never invented runtime status.
        assert_eq!(
            String::from(first.sessions.first().unwrap().target.session_id.clone()),
            "stored-b"
        );
        assert_eq!(
            String::from(second.sessions.first().unwrap().target.session_id.clone()),
            "stored-a"
        );
        assert_eq!(addresses.entries.len(), 2);
        assert_eq!(
            first.sessions.len(),
            1,
            "page must shrink to its frame budget"
        );
        assert_eq!(second.sessions.len(), 1);
        assert_eq!(
            first.sessions.first().unwrap().name.as_deref(),
            Some(title.as_str())
        );
        assert_eq!(
            second.sessions.first().unwrap().name.as_deref(),
            Some(title.as_str())
        );
        assert_eq!(first.sessions.first().unwrap().title, "PRIVATE_TITLE");
        assert_eq!(second.sessions.first().unwrap().title, "PRIVATE_TITLE");
        assert!(second.next_cursor.is_none());
        assert!(
            addresses
                .entries
                .iter()
                .all(|entry| entry.last_status.is_none() && entry.status_scope.is_none())
        );
        assert_ne!(
            addresses.coverage.state,
            collaboration_protocol::CoverageState::Observing
        );
        assert_eq!(history.records.len(), 2);
        assert!(
            history
                .records
                .iter()
                .all(|record| record.observation.scope.generation.is_none()
                    && matches!(
                        record.observation.change,
                        collaboration_protocol::LifecycleChange::ThreadDiscovered
                    ))
        );
        let encoded = serde_json::to_string(&history).unwrap();
        assert!(
            !encoded.contains("PRIVATE_TITLE")
                && !encoded.contains("PRIVATE_BODY")
                && !encoded.contains("private-fixture-workspace")
        );
        assert_eq!(original_catalog, unchanged_catalog);
        assert!(matches!(
            invalid_cursor,
            Err(ClientError::Rejected { code: -32602, .. })
        ));
    }

    /// Pages one stored listing to exhaustion through the real Control service.
    ///
    /// Returns every page in order so a caller can assert both the row content and that
    /// paging never loses a row across a cursor boundary.
    async fn stored_listing_pages(
        root: &std::path::Path,
        scope: collaboration_protocol::NativeSessionScope,
        source: collaboration_protocol::NativeSessionSource,
        query: Option<&str>,
        page_size: u32,
    ) -> Vec<collaboration_protocol::NativeSessionListResult> {
        let home = root.join("native-home");
        let id = "00000000-0000-4000-8000-000000000011";
        let epoch = "00000000-0000-4000-8000-000000000012";
        let endpoint: collaboration_protocol::EndpointRef =
            serde_json::from_value(json!({"serviceId":id,"endpointId":"codex-local"})).unwrap();
        let journal = ObservationJournal::open(
            &root.join("journal.sqlite"),
            id.to_owned().try_into().unwrap(),
        )
        .await
        .unwrap();
        let store = Arc::new(LifecycleStore::new(journal));
        let digest = format!("sha256:{}", "a".repeat(64));
        let description = serde_json::from_value(json!({"endpoint":endpoint,"label":"Scoped fixture","availability":{"state":"unprobed"},"channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"absent-native.sock","schemaDigest":null,"generation":null}]})).unwrap();
        let identity = ServiceIdentity::new(id, epoch, &digest)
            .unwrap()
            .with_endpoints(vec![description])
            .unwrap()
            .with_journal(Arc::clone(&store))
            .with_native_backend(NativeControlBackend {
                endpoint: endpoint.clone(),
                gate: NativeGenerationGate::default(),
                codex_home: home,
            })
            .unwrap();
        let listener = LocalControlService::bind(&root.join("control.sock"), identity).unwrap();
        let manifest = serde_json::from_value(json!({"version":2,"serviceId":id,"serviceEpoch":epoch,"control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}})).unwrap();
        let publication = ManifestPublication::publish(root, &manifest).unwrap();
        let stop = CancellationToken::new();
        let server = tokio::spawn(listener.run(stop.clone()));
        let mut client = ControlClient::connect(root, "scoped-listing-test", "1")
            .await
            .unwrap();
        let mut pages = Vec::new();
        let mut cursor = None;
        loop {
            let page = client
                .list_sessions(NativeSessionListParams {
                    endpoint: endpoint.clone(),
                    view: NativeSessionView::Stored,
                    scope: scope.clone(),
                    source,
                    query: query.map(str::to_owned),
                    page_size,
                    cursor: cursor.take(),
                })
                .await
                .unwrap();
            cursor = page.next_cursor.clone();
            pages.push(page);
            if cursor.is_none() {
                break;
            }
        }
        client.close().await.unwrap();
        stop.cancel();
        server.await.unwrap().unwrap();
        drop(publication);
        Arc::try_unwrap(store)
            .unwrap_or_else(|_| panic!("journal still shared"))
            .close()
            .await;
        std::fs::remove_file(root.join("journal.sqlite")).unwrap();
        pages
    }

    fn session_ids(pages: &[collaboration_protocol::NativeSessionListResult]) -> Vec<String> {
        pages
            .iter()
            .flat_map(|page| page.sessions.iter())
            .map(|session| String::from(session.target.session_id.clone()))
            .collect()
    }

    #[tokio::test]
    async fn stored_listing_applies_repo_scope_source_and_query_and_omits_unknown_effort() {
        // Arrange: a live checkout, its work fork under a different origin, and a subagent.
        let root = PathBuf::from(format!("/tmp/stored-scoped-{}", std::process::id()));
        let _ignored = std::fs::remove_dir_all(&root);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        let home = root.join("native-home");
        std::fs::create_dir(&home).unwrap();
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(home.join("state_5.sqlite"))
            .create_if_missing(true);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT, cwd TEXT, model_provider TEXT, model TEXT, reasoning_effort TEXT, source TEXT, thread_source TEXT, git_branch TEXT, git_origin_url TEXT, name TEXT, title TEXT, preview TEXT, first_user_message TEXT, created_at_ms INTEGER, updated_at_ms INTEGER, recency_at_ms INTEGER, archived INTEGER); CREATE INDEX idx_threads_updated_at_ms ON threads(updated_at_ms DESC, id DESC);")
            .execute(&pool).await.unwrap();
        for (id, cwd, origin, effort, thread_source, name, time) in [
            (
                "in-repo-known-effort",
                "/dev/ai-tools/crates",
                None,
                Some("high"),
                "user",
                "Astra review seat",
                400_i64,
            ),
            (
                "in-repo-unknown-effort",
                "/dev/ai-tools",
                None,
                None,
                "user",
                "Sol review seat",
                300,
            ),
            (
                "in-repo-subagent",
                "/dev/ai-tools",
                None,
                Some("low"),
                "subagent",
                "review worker",
                200,
            ),
            (
                "work-fork",
                "/dev/ai-tools-work",
                Some("https://github.com/shravan/ai-tools-work.git"),
                Some("medium"),
                "user",
                "review seat in the fork",
                100,
            ),
            // A different repository whose origin differs only by letter case. SQLite
            // LIKE admits it; only the canonical predicate on the decoded row rejects it.
            (
                "case-only-origin",
                "/elsewhere/mirror",
                Some("https://github.com/Shravan/AI-Tools.git"),
                Some("medium"),
                "user",
                "review seat in the mirror",
                250,
            ),
        ] {
            sqlx::query("INSERT INTO threads (id,cwd,model,reasoning_effort,source,thread_source,git_origin_url,name,title,updated_at_ms,recency_at_ms,archived) VALUES (?, ?, 'gpt-5.6-sol', ?, 'cli', ?, ?, ?, 'derived', ?, ?, 0)")
                .bind(id).bind(cwd).bind(effort).bind(thread_source).bind(origin).bind(name).bind(time).bind(time)
                .execute(&pool).await.unwrap();
        }
        pool.close().await;
        let scope = collaboration_protocol::NativeSessionScope::Repo {
            live_roots: vec![PathBuf::from("/dev/ai-tools")],
            normalized_origin: Some("github.com/shravan/ai-tools".to_owned()),
            basename: "ai-tools".to_owned(),
            fallback_cwd: None,
        };

        // Act
        let interactive = stored_listing_pages(
            &root,
            scope.clone(),
            collaboration_protocol::NativeSessionSource::Interactive,
            Some("REVIEW"),
            100,
        )
        .await;
        let everything = stored_listing_pages(
            &root,
            scope.clone(),
            collaboration_protocol::NativeSessionSource::All,
            None,
            100,
        )
        .await;
        // One row per page, so every rejected row sits on a cursor boundary.
        let one_at_a_time = stored_listing_pages(
            &root,
            scope,
            collaboration_protocol::NativeSessionSource::All,
            None,
            1,
        )
        .await;

        std::fs::remove_file(home.join("state_5.sqlite")).unwrap();
        std::fs::remove_dir(home).unwrap();
        std::fs::remove_dir(root).unwrap();

        // Assert: the work fork never enters a repository-scoped listing.
        let interactive_ids = session_ids(&interactive);
        assert_eq!(
            interactive_ids,
            ["in-repo-known-effort", "in-repo-unknown-effort"]
        );
        let all_ids = session_ids(&everything);
        assert_eq!(
            all_ids,
            [
                "in-repo-known-effort",
                "in-repo-unknown-effort",
                "in-repo-subagent"
            ],
            "the Control page resolves --repo through the canonical predicate, so neither \
             the work fork nor a case-only origin difference can enter it"
        );
        // Assert: paging one row at a time crosses every rejected row without a gap.
        assert_eq!(
            session_ids(&one_at_a_time),
            all_ids,
            "a page may be short after the predicate rejects a row, but never skip one"
        );
        // Assert: the effort is read, never guessed, and absent when the column is NULL.
        assert_eq!(
            everything[0]
                .sessions
                .iter()
                .map(|session| session.reasoning_effort.clone())
                .collect::<Vec<_>>(),
            [Some("high".to_owned()), None, Some("low".to_owned())]
        );
        let encoded = serde_json::to_value(&everything[0].sessions[1]).unwrap();
        assert!(
            encoded.get("reasoningEffort").is_none(),
            "an unknown effort must be absent, not null"
        );
    }
}
