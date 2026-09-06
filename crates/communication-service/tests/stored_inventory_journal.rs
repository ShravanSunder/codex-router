//! Stored discovery publishes only metadata observations, even without a live backend.
#[cfg(test)]
mod tests {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use communication_client::{ClientError, ControlClient, JournalStatus};
    use communication_protocol::{NativeSessionListParams, NativeSessionView};
    use communication_service::{
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
        sqlx::raw_sql("CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT, cwd TEXT, model_provider TEXT, model TEXT, source TEXT, thread_source TEXT, git_branch TEXT, git_origin_url TEXT, name TEXT, title TEXT, preview TEXT, first_user_message TEXT, created_at_ms INTEGER, updated_at_ms INTEGER, recency_at_ms INTEGER, archived INTEGER); CREATE INDEX idx_threads_recency_at_ms ON threads(recency_at_ms DESC, id DESC);")
            .execute(&pool).await.unwrap();
        let title = "PRIVATE_TITLE".repeat(60_000);
        for (id, time) in [("stored-b", 2000_i64), ("stored-a", 1000_i64)] {
            sqlx::query("INSERT INTO threads (id,cwd,name,title,preview,first_user_message,recency_at_ms,archived) VALUES (?, '/private-fixture-workspace', ?, 'PRIVATE_TITLE', 'PRIVATE_BODY', 'PRIVATE_BODY', ?, 0)")
                .bind(id).bind(&title).bind(time).execute(&pool).await.unwrap();
        }
        pool.close().await;
        let original_catalog = std::fs::read(&database).unwrap();
        let id = "00000000-0000-4000-8000-000000000001";
        let epoch = "00000000-0000-4000-8000-000000000002";
        let endpoint: communication_protocol::EndpointRef =
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
        let manifest = serde_json::from_value(json!({"version":1,"serviceId":id,"serviceEpoch":epoch,"control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest})).unwrap();
        let publication = ManifestPublication::publish(&root, &manifest).unwrap();
        let stop = CancellationToken::new();
        let server = tokio::spawn(listener.run(stop.clone()));
        let mut client = ControlClient::connect(&root, "stored-journal-test", "1")
            .await
            .unwrap();
        let request = |cursor| NativeSessionListParams {
            endpoint: endpoint.clone(),
            view: NativeSessionView::Stored,
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
                communication_protocol::JournalPosition {
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
        assert_eq!(first.sessions.first().unwrap().title, title);
        assert_eq!(second.sessions.first().unwrap().title, title);
        assert!(second.next_cursor.is_none());
        assert!(
            addresses
                .entries
                .iter()
                .all(|entry| entry.last_status.is_none() && entry.status_scope.is_none())
        );
        assert_ne!(
            addresses.coverage.state,
            communication_protocol::CoverageState::Observing
        );
        assert_eq!(history.records.len(), 2);
        assert!(
            history
                .records
                .iter()
                .all(|record| record.observation.scope.generation.is_none()
                    && matches!(
                        record.observation.change,
                        communication_protocol::LifecycleChange::ThreadDiscovered
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
}
