use codex_native_integration::{
    NativePayloadSchemas, NativeProtocolConnection, NativeSchemaBundle,
};
use communication_protocol::{
    LifecycleChange, LifecycleSubject, ObservationOrdering, ObservationScope,
};
use futures_util::{SinkExt, StreamExt};
use lifecycle_observation::{
    JournalPosition, LifecycleStore, NativeObservationInputs, NativeObservationStream,
    ObservationJournal,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::Role},
};

#[tokio::test]
async fn event_for_another_thread_during_read_triggers_its_own_reconciliation() {
    // Arrange: thread a changes while the observer waits for thread b's read.
    let path = std::env::temp_dir().join(format!("native-reread-{}.sqlite", std::process::id()));
    let scope: ObservationScope = serde_json::from_value(json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"generation":{"serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":1},"observerId":"00000000-0000-4000-8000-000000000003"})).unwrap_or_else(|error| panic!("scope: {error}"));
    let journal = ObservationJournal::open(&path, scope.observer_id.clone())
        .await
        .unwrap_or_else(|error| panic!("journal: {error}"));
    let position = JournalPosition {
        journal_id: journal.journal_id().clone(),
        sequence: 0,
    };
    let store = Arc::new(LifecycleStore::new(journal));
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
            .unwrap_or_else(|error| panic!("schema: {error}")),
    )]))
    .unwrap_or_else(|error| panic!("bundle: {error}"));
    let schemas = Arc::new(
        NativePayloadSchemas::from_bundle(&bundle)
            .unwrap_or_else(|error| panic!("schemas: {error}")),
    );
    let (client, server) =
        tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("pair: {error}"));
    let client = NativeProtocolConnection::from_websocket(
        WebSocketStream::from_raw_socket(client, Role::Client, None).await,
    );
    let fixture = tokio::spawn(async move {
        let mut server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
        for thread in [None, Some("a"), Some("b")] {
            let request = receive_request(&mut server)
                .await
                .unwrap_or_else(|error| panic!("request: {error}"));
            let result = if let Some(thread) = thread {
                assert_eq!(request["params"]["threadId"], thread);
                if thread == "b" {
                    server.send(Message::Text(json!({"method":"thread/status/changed","params":{"threadId":"a","status":{"type":"active","activeFlags":[]}}}).to_string().into())).await.unwrap_or_else(|error| panic!("event: {error}"));
                }
                json!({"thread":{"id":thread,"status":{"type":"idle"}}})
            } else {
                json!({"data":["a","b"],"nextCursor":null})
            };
            server
                .send(Message::Text(
                    json!({"id":request["id"],"result":result})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap_or_else(|error| panic!("response: {error}"));
        }
        let reread =
            tokio::time::timeout(Duration::from_secs(1), receive_request(&mut server)).await;
        if let Ok(Ok(request)) = reread {
            assert_eq!(request["params"]["threadId"], "a");
            server.send(Message::Text(json!({"id":request["id"],"result":{"thread":{"id":"a","status":{"type":"idle"}}}}).to_string().into())).await.unwrap_or_else(|error| panic!("reread response: {error}"));
            true
        } else {
            false
        }
    });
    // Act.
    let observer = NativeObservationStream::new(NativeObservationInputs {
        connection: client,
        store: Arc::clone(&store),
        scope: scope.clone(),
        schemas,
    })
    .unwrap_or_else(|error| panic!("observer: {error}"));
    let _closed = observer
        .run(tokio_util::sync::CancellationToken::new())
        .await;
    let reread = fixture
        .await
        .unwrap_or_else(|error| panic!("fixture: {error}"));
    let page = store
        .read_wait(&scope.endpoint, position, 100, 0)
        .await
        .unwrap_or_else(|error| panic!("journal read: {error}"));
    Arc::try_unwrap(store)
        .unwrap_or_else(|_| panic!("store retained"))
        .close()
        .await;
    std::fs::remove_file(path).unwrap_or_else(|error| panic!("cleanup: {error}"));
    // Assert: the final a status comes from a new read, not the unordered event.
    assert!(reread, "buffered cross-thread event must schedule a reread");
    let last = page.records.iter().rev().find(|record| matches!(&record.observation.subject, LifecycleSubject::Thread { address } if String::from(address.native_thread_id.clone()) == "a") && matches!(record.observation.change, LifecycleChange::ThreadStatus { .. }));
    assert!(matches!(
        last.map(|record| &record.observation.change),
        Some(LifecycleChange::ThreadStatus {
            ordering: ObservationOrdering::Established,
            ..
        })
    ));
}

async fn receive_request(
    socket: &mut WebSocketStream<tokio::net::UnixStream>,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let frame = socket.next().await.ok_or("native request missing")??;
    Ok(serde_json::from_str(frame.to_text()?)?)
}
