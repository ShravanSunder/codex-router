//! Public SDK readers consume actual native reconciliation and persisted SQLite state.
#[cfg(test)]
mod tests {
    use codex_native_integration::{
        NativePayloadSchemas, NativeProtocolConnection, NativeSchemaBundle,
    };
    use communication_client::ControlClient;
    use communication_protocol::{CoverageState, LifecycleChange, ObservationScope};
    use communication_service::{ServiceIdentity, serve_control_connection};
    use futures_util::{SinkExt, StreamExt};
    use lifecycle_observation::{
        LifecycleStore, NativeObservationInputs, NativeObservationStream, ObservationJournal,
    };
    use serde_json::{Value, json};
    use std::{collections::BTreeMap, sync::Arc, time::Duration};
    use tokio_tungstenite::{
        WebSocketStream,
        tungstenite::{Message, protocol::Role},
    };

    #[tokio::test]
    async fn public_snapshots_preserve_pages_and_reopen_invalidates_observation_coverage() {
        // Arrange: real SQLite, real native carrier and public Control reader, with an
        // independent native fixture that permits only inventory and metadata reads.
        let path =
            std::env::temp_dir().join(format!("public-observation-{}.sqlite", std::process::id()));
        let scope: ObservationScope = serde_json::from_value(json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
        "generation":{"serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":1},
        "observerId":"00000000-0000-4000-8000-000000000003"
    }))
    .unwrap_or_else(|error| panic!("scope: {error}"));
        let journal = ObservationJournal::open(&path, scope.endpoint.service_id.clone())
            .await
            .unwrap_or_else(|error| panic!("open: {error}"));
        let initial_position = communication_protocol::JournalPosition {
            journal_id: journal.journal_id().clone(),
            sequence: 0,
        };
        let store = Arc::new(LifecycleStore::new(journal));
        let (client, server) =
            tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("pair: {error}"));
        let native = NativeProtocolConnection::from_websocket(
            WebSocketStream::from_raw_socket(client, Role::Client, None).await,
        );
        let (change_sender, change_receiver) = tokio::sync::oneshot::channel();
        let (stop_sender, stop_receiver) = tokio::sync::oneshot::channel();
        let fixture = tokio::spawn(async move {
            let mut wire = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
            let list = read_request(&mut wire).await;
            assert_eq!(list["method"], "thread/loaded/list");
            write_message(
                &mut wire,
                json!({"id":list["id"],"result":{"data":["a","b"],"nextCursor":null}}),
            )
            .await;
            for thread in ["a", "b"] {
                answer_thread_read(&mut wire, thread, "idle").await;
            }
            change_receiver
                .await
                .unwrap_or_else(|error| panic!("change signal: {error}"));
            write_message(
                &mut wire,
                json!({"method":"thread/closed","params":{"threadId":"b"}}),
            )
            .await;
            answer_thread_read(&mut wire, "b", "notLoaded").await;
            stop_receiver
                .await
                .unwrap_or_else(|error| panic!("stop signal: {error}"));
        });
        let observer = NativeObservationStream::new(NativeObservationInputs {
            connection: native,
            store: Arc::clone(&store),
            scope: scope.clone(),
            schemas: fixture_schemas(),
        })
        .unwrap_or_else(|error| panic!("observer: {error}"));
        let observer = tokio::spawn(observer.run(tokio_util::sync::CancellationToken::new()));
        let (mut client, server_task) = connect_public_reader(&scope, Arc::clone(&store)).await;

        // Act: wait through the public journal, capture page one, then change native state.
        let ready_position = tokio::time::timeout(Duration::from_secs(3), async {
            let mut position = initial_position.clone();
            loop {
                let page = client
                    .read_journal(&scope.endpoint, position, 100, 1000)
                    .await
                    .unwrap_or_else(|error| panic!("journal: {error}"));
                position = page.next;
                if page.records.iter().any(|record| {
                    matches!(record.observation.change, LifecycleChange::CoverageRestored)
                }) {
                    break position;
                }
            }
        })
        .await
        .unwrap_or_else(|error| panic!("coverage readiness: {error}"));
        let first = client
            .list_addresses(&scope.endpoint, 1, None)
            .await
            .unwrap_or_else(|error| panic!("snapshot: {error}"));
        assert_eq!(first.coverage.state, CoverageState::Observing);
        assert_eq!(first.entries.len(), 1);
        assert_eq!(
            String::from(first.entries[0].address.native_thread_id.clone()),
            "a"
        );
        change_sender
            .send(())
            .unwrap_or_else(|_| panic!("observer fixture gone"));
        tokio::time::timeout(Duration::from_secs(3), async {
            let mut position = ready_position;
            loop {
                let page = client
                    .read_journal(&scope.endpoint, position, 100, 1000)
                    .await
                    .unwrap_or_else(|error| panic!("changed journal: {error}"));
                position = page.next;
                if page.records.iter().any(|record| {
                    matches!(
                        record.observation.change,
                        LifecycleChange::ThreadStatus {
                            status: communication_protocol::NativeThreadStatus::NotLoaded,
                            ordering: communication_protocol::ObservationOrdering::Established
                        }
                    )
                }) {
                    break;
                }
            }
        })
        .await
        .unwrap_or_else(|error| panic!("reconciliation: {error}"));
        let second = client
            .list_addresses(&scope.endpoint, 1, first.next_cursor.as_deref())
            .await
            .unwrap_or_else(|error| panic!("second page: {error}"));
        // Assert: paging preserves its captured state even after the real reducer advances.
        assert_eq!(second.snapshot_id, first.snapshot_id);
        assert_eq!(
            serialized_value(&second.watermark),
            serialized_value(&first.watermark)
        );
        assert_eq!(
            serialized_value(&second.coverage),
            serialized_value(&first.coverage)
        );
        assert_eq!(second.entries.len(), 1);
        assert_eq!(
            String::from(second.entries[0].address.native_thread_id.clone()),
            "b"
        );
        assert_eq!(
            second.entries[0].last_status,
            Some(communication_protocol::NativeThreadStatus::Idle)
        );
        let changed = client
            .list_addresses(&scope.endpoint, 100, None)
            .await
            .unwrap_or_else(|error| panic!("changed snapshot: {error}"));
        assert!(changed.entries.iter().any(|entry| entry.last_status
            == Some(communication_protocol::NativeThreadStatus::NotLoaded)));

        // Simulate loss of the observation owner without running its final coverage-loss
        // path, then recreate all public storage/connection state from the same database.
        observer.abort();
        assert!(observer.await.unwrap_err().is_cancelled());
        stop_sender
            .send(())
            .unwrap_or_else(|_| panic!("fixture closed"));
        fixture
            .await
            .unwrap_or_else(|error| panic!("fixture: {error}"));
        client
            .close()
            .await
            .unwrap_or_else(|error| panic!("close: {error}"));
        server_task
            .await
            .unwrap_or_else(|error| panic!("server task: {error}"))
            .unwrap_or_else(|error| panic!("server: {error}"));
        Arc::try_unwrap(store)
            .unwrap_or_else(|_| panic!("store shared"))
            .close()
            .await;
        let journal = ObservationJournal::open(&path, scope.endpoint.service_id.clone())
            .await
            .unwrap_or_else(|error| panic!("reopen: {error}"));
        let store = Arc::new(LifecycleStore::new(journal));
        let (mut client, server_task) = connect_public_reader(&scope, Arc::clone(&store)).await;
        let restored = client
            .list_addresses(&scope.endpoint, 100, None)
            .await
            .unwrap_or_else(|error| panic!("restored snapshot: {error}"));
        assert_eq!(
            serialized_value(&restored.entries),
            serialized_value(&changed.entries)
        );
        assert_eq!(
            serialized_value(&restored.watermark),
            serialized_value(&changed.watermark)
        );
        assert_eq!(restored.coverage.state, CoverageState::Initializing);
        assert!(restored.coverage.observer_id.is_none());
        let history = client
            .read_journal(&scope.endpoint, initial_position, 100, 0)
            .await
            .unwrap_or_else(|error| panic!("restored journal: {error}"));
        assert!(
            !history
                .records
                .iter()
                .any(|record| matches!(record.observation.change, LifecycleChange::CoverageLost))
        );
        client
            .close()
            .await
            .unwrap_or_else(|error| panic!("close: {error}"));
        server_task
            .await
            .unwrap_or_else(|error| panic!("server task: {error}"))
            .unwrap_or_else(|error| panic!("server: {error}"));
        Arc::try_unwrap(store)
            .unwrap_or_else(|_| panic!("store shared"))
            .close()
            .await;
        std::fs::remove_file(path).unwrap_or_else(|error| panic!("cleanup: {error}"));
    }

    async fn connect_public_reader(
        scope: &ObservationScope,
        store: Arc<LifecycleStore>,
    ) -> (ControlClient, tokio::task::JoinHandle<std::io::Result<()>>) {
        let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001", "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    ).unwrap_or_else(|error| panic!("identity: {error}"))
    .with_endpoints(vec![serde_json::from_value(json!({"endpoint":scope.endpoint,"label":"Fixture Codex","availability":{"state":"unprobed"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":null,"generation":null}]}))
        .unwrap_or_else(|error| panic!("endpoint: {error}"))])
    .unwrap_or_else(|error| panic!("endpoints: {error}"))
    .with_journal(store);
        let (client, server) =
            tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("Control pair: {error}"));
        let task = tokio::spawn(serve_control_connection(server, identity));
        let client = ControlClient::initialize(client, "public-observation-test", "1")
            .await
            .unwrap_or_else(|error| panic!("initialize: {error}"));
        (client, task)
    }

    async fn read_request(wire: &mut WebSocketStream<tokio::net::UnixStream>) -> Value {
        let frame = wire
            .next()
            .await
            .unwrap_or_else(|| panic!("missing request"))
            .unwrap_or_else(|error| panic!("frame: {error}"));
        serde_json::from_str(
            frame
                .to_text()
                .unwrap_or_else(|error| panic!("text: {error}")),
        )
        .unwrap_or_else(|error| panic!("JSON: {error}"))
    }
    async fn write_message(wire: &mut WebSocketStream<tokio::net::UnixStream>, value: Value) {
        wire.send(Message::Text(value.to_string().into()))
            .await
            .unwrap_or_else(|error| panic!("send: {error}"));
    }
    async fn answer_thread_read(
        wire: &mut WebSocketStream<tokio::net::UnixStream>,
        thread: &str,
        status: &str,
    ) {
        let request = read_request(wire).await;
        assert_eq!(request["method"], "thread/read");
        assert_eq!(request["params"]["threadId"], thread);
        assert_eq!(request["params"]["includeTurns"], false);
        write_message(
            wire,
            json!({"id":request["id"],"result":{"thread":{"id":thread,"status":{"type":status}}}}),
        )
        .await;
    }
    fn fixture_schemas() -> Arc<NativePayloadSchemas> {
        let mut definitions = serde_json::Map::new();
        for name in [
            "ThreadRead",
            "ThreadResume",
            "ThreadStart",
            "ThreadLoadedList",
            "TurnStart",
            "TurnSteer",
            "TurnInterrupt",
        ] {
            definitions.insert(format!("{name}Params"), json!({"type":"object"}));
            definitions.insert(format!("{name}Response"), json!({"type":"object"}));
        }
        let bundle = NativeSchemaBundle::from_documents(BTreeMap::from([(
            "codex_app_server_protocol.schemas.json".to_owned(),
            serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))
                .unwrap_or_else(|error| panic!("schema JSON: {error}")),
        )]))
        .unwrap_or_else(|error| panic!("schema bundle: {error}"));
        Arc::new(
            NativePayloadSchemas::from_bundle(&bundle)
                .unwrap_or_else(|error| panic!("schema admission: {error}")),
        )
    }

    fn serialized_value(value: &impl serde::Serialize) -> Value {
        serde_json::to_value(value)
            .unwrap_or_else(|error| panic!("serialize public record: {error}"))
    }
}
