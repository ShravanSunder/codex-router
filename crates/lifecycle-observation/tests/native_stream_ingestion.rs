use codex_native_integration::NativeProtocolConnection;
use communication_protocol::{LifecycleChange, ObservationScope};
use futures_util::{SinkExt, StreamExt};
use lifecycle_observation::{
    JournalPosition, LifecycleStore, NativeObservationStream, ObservationJournal,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::Role},
};
#[tokio::test]
async fn native_stream_records_status_and_marks_loss_without_prompting() {
    let path = std::path::PathBuf::from(format!(
        "/tmp/native-ingestion-{}.sqlite",
        std::process::id()
    ));
    let scope:ObservationScope=serde_json::from_value(json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"generation":{"serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":1},"observerId":"00000000-0000-4000-8000-000000000003"})).unwrap_or_else(|e|panic!("scope: {e}"));
    let journal = ObservationJournal::open(&path, scope.observer_id.clone())
        .await
        .unwrap_or_else(|e| panic!("open: {e}"));
    let position = JournalPosition {
        journal_id: journal.journal_id().clone(),
        sequence: 0,
    };
    let store = Arc::new(LifecycleStore::new(journal));
    let (client, server) = tokio::net::UnixStream::pair().unwrap_or_else(|e| panic!("pair: {e}"));
    let client = NativeProtocolConnection::from_websocket(
        WebSocketStream::from_raw_socket(client, Role::Client, None).await,
    );
    let fixture = tokio::spawn(async move {
        let mut socket = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
        let frame = socket
            .next()
            .await
            .unwrap_or_else(|| panic!("inventory"))
            .unwrap_or_else(|e| panic!("frame: {e}"));
        let request: Value =
            serde_json::from_str(frame.to_text().unwrap_or_else(|e| panic!("text: {e}")))
                .unwrap_or_else(|e| panic!("JSON: {e}"));
        assert_eq!(request["method"], "thread/loaded/list");
        for value in [
            json!({"id":request["id"],"result":{"data":[],"nextCursor":null}}),
            json!({"method":"thread/status/changed","params":{"threadId":"t","status":{"type":"idle"}}}),
            json!({"method":"thread/status/changed","params":{"threadId":"t","status":{"type":"idle"}}}),
        ] {
            socket
                .send(Message::Text(value.to_string().into()))
                .await
                .unwrap_or_else(|e| panic!("send: {e}"));
        }
    });
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
    let bundle = codex_native_integration::NativeSchemaBundle::from_documents(
        std::collections::BTreeMap::from([(
            "codex_app_server_protocol.schemas.json".to_owned(),
            serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))
                .unwrap_or_else(|error| panic!("schema JSON: {error}")),
        )]),
    )
    .unwrap_or_else(|error| panic!("bundle: {error}"));
    let schemas = Arc::new(
        codex_native_integration::NativePayloadSchemas::from_bundle(&bundle)
            .unwrap_or_else(|error| panic!("fixture schemas: {error}")),
    );
    let observer = NativeObservationStream::new(lifecycle_observation::NativeObservationInputs {
        connection: client,
        store: Arc::clone(&store),
        scope: scope.clone(),
        schemas,
    })
    .unwrap_or_else(|e| panic!("observer: {e}"));
    assert!(
        observer
            .run(tokio_util::sync::CancellationToken::new())
            .await
            .is_err()
    );
    fixture.await.unwrap_or_else(|e| panic!("join: {e}"));
    let page = store
        .read_wait(&scope.endpoint, position, 100, 0)
        .await
        .unwrap_or_else(|e| panic!("read: {e}"));
    assert!(page.records.iter().any(|record| matches!(
        record.observation.change,
        LifecycleChange::ThreadStatus { .. }
    )));
    assert_eq!(
        page.records
            .iter()
            .filter(|record| matches!(
                record.observation.change,
                LifecycleChange::ThreadStatus { .. }
            ))
            .count(),
        1,
        "unchanged status in one continuous scope must not append twice"
    );
    assert!(matches!(
        page.records.last().map(|record| &record.observation.change),
        Some(LifecycleChange::CoverageLost)
    ));
    Arc::try_unwrap(store)
        .unwrap_or_else(|_| panic!("shared"))
        .close()
        .await;
    std::fs::remove_file(path).unwrap_or_else(|e| panic!("cleanup: {e}"));
}
