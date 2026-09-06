//! Abrupt process loss cannot promote persisted observations into live coverage.
#[cfg(test)]
mod tests {
    use communication_client::ControlClient;
    use communication_protocol::{
        AddressPage, CoverageState, EndpointRef, JournalPosition, LifecycleChange,
        LifecycleObservation,
    };
    use communication_service::{ServiceIdentity, serve_control_connection};
    use lifecycle_observation::{LifecycleStore, ObservationJournal};
    use serde_json::json;
    use std::{
        io::Write, os::unix::process::ExitStatusExt, path::Path, process::Stdio, sync::Arc,
        time::Duration,
    };
    use tokio::io::{AsyncBufReadExt, BufReader};

    const SERVICE: &str = "00000000-0000-4000-8000-000000000001";
    const OLD_EPOCH: &str = "00000000-0000-4000-8000-000000000002";
    const NEW_EPOCH: &str = "00000000-0000-4000-8000-000000000004";
    const FIXTURE_PATH_ENV: &str = "COMMUNICATION_PROCESS_CRASH_FIXTURE";

    #[tokio::test]
    async fn killed_writer_preserves_commits_but_invalidates_public_live_coverage() {
        // Arrange: a distinct process owns the real journal and public Control server.
        let path = std::env::temp_dir().join(format!(
            "process-crash-journal-{}.sqlite",
            std::process::id()
        ));
        assert!(!path.exists(), "fixture must not reuse a database");
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::crash_fixture_process",
                "--ignored",
                "--nocapture",
            ])
            .env(FIXTURE_PATH_ENV, &path)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let before: AddressPage = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let line = lines
                    .next_line()
                    .await
                    .unwrap()
                    .expect("child must publish readiness");
                if let Some(snapshot) = line.strip_prefix("CRASH_READY:") {
                    break serde_json::from_str(snapshot).unwrap();
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(before.coverage.state, CoverageState::Observing);
        assert_eq!(before.entries.len(), 1);

        // Act: SIGKILL skips graceful close and the final coverageLost append.
        child.start_kill().unwrap();
        let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status.signal(), Some(9));
        let store = open_store(&path).await;
        let (mut client, server) = public_reader(Arc::clone(&store), NEW_EPOCH).await;
        let after = client.list_addresses(&endpoint(), 100, None).await.unwrap();
        let history = client
            .read_journal(
                &endpoint(),
                JournalPosition {
                    journal_id: before.watermark.journal_id.clone(),
                    sequence: 0,
                },
                100,
                0,
            )
            .await
            .unwrap();

        // Assert: durable facts and position survive, but their old scope is historical.
        assert_eq!(
            serde_json::to_value(&after.entries).unwrap(),
            serde_json::to_value(&before.entries).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&after.watermark).unwrap(),
            serde_json::to_value(&before.watermark).unwrap()
        );
        assert_eq!(after.coverage.state, CoverageState::Initializing);
        assert!(after.coverage.observer_id.is_none());
        assert!(after.coverage.generation.is_none());
        assert!(after.entries[0].status_scope.is_some());
        assert_eq!(history.records.len(), 3);
        assert!(
            history
                .records
                .iter()
                .any(|row| matches!(row.observation.change, LifecycleChange::CoverageRestored))
        );
        assert!(
            !history
                .records
                .iter()
                .any(|row| matches!(row.observation.change, LifecycleChange::CoverageLost))
        );
        client.close().await.unwrap();
        server.await.unwrap().unwrap();
        Arc::try_unwrap(store)
            .unwrap_or_else(|_| panic!("store retained"))
            .close()
            .await;
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    #[ignore = "subprocess fixture; parent kills this process after public readiness"]
    async fn crash_fixture_process() {
        let path = std::env::var_os(FIXTURE_PATH_ENV).expect("parent fixture path");
        let store = open_store(Path::new(&path)).await;
        let scope = json!({"endpoint":endpoint(), "generation":{"serviceEpoch":OLD_EPOCH,"generation":1},
            "observerId":"00000000-0000-4000-8000-000000000003"});
        for (source, subject, change) in [
            (
                "inventoryRead",
                json!({"kind":"thread","address":{"endpoint":endpoint(),"nativeThreadId":"owned-fixture"}}),
                json!({"kind":"threadDiscovered"}),
            ),
            (
                "inventoryRead",
                json!({"kind":"thread","address":{"endpoint":endpoint(),"nativeThreadId":"owned-fixture"}}),
                json!({"kind":"threadStatus","status":{"type":"idle"},"ordering":"established"}),
            ),
            (
                "observerLifecycle",
                json!({"kind":"backend"}),
                json!({"kind":"coverageRestored"}),
            ),
        ] {
            let observation: LifecycleObservation = serde_json::from_value(json!({
                "observedAt":"2026-09-06T12:00:00Z", "source":source, "scope":scope,
                "subject":subject, "change":change,
            }))
            .unwrap();
            store.append(&observation, 100).await.unwrap();
        }
        let (mut client, _server) = public_reader(Arc::clone(&store), OLD_EPOCH).await;
        let before = client.list_addresses(&endpoint(), 100, None).await.unwrap();
        println!("CRASH_READY:{}", serde_json::to_string(&before).unwrap());
        std::io::stdout().flush().unwrap();
        std::future::pending::<()>().await;
    }

    fn endpoint() -> EndpointRef {
        serde_json::from_value(json!({"serviceId":SERVICE,"endpointId":"codex-local"})).unwrap()
    }

    async fn open_store(path: &Path) -> Arc<LifecycleStore> {
        let journal = ObservationJournal::open(path, SERVICE.to_owned().try_into().unwrap())
            .await
            .unwrap();
        let store = Arc::new(LifecycleStore::new(journal));
        // Same startup preparation invoked by Host before admitting public readers.
        store.prepare(100).await.unwrap();
        store
    }

    async fn public_reader(
        store: Arc<LifecycleStore>,
        epoch: &str,
    ) -> (ControlClient, tokio::task::JoinHandle<std::io::Result<()>>) {
        let identity = ServiceIdentity::new(SERVICE, epoch, &format!("sha256:{}", "a".repeat(64))).unwrap()
            .with_endpoints(vec![serde_json::from_value(json!({"endpoint":endpoint(),"label":"Fixture Codex",
                "availability":{"state":"unprobed"},"channels":[{"kind":"nativeCodex","transport":"unixWebSocket",
                "path":"native.sock","schemaDigest":null,"generation":null}]})).unwrap()]).unwrap()
            .with_journal(store);
        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let task = tokio::spawn(serve_control_connection(server, identity));
        (
            ControlClient::initialize(client, "process-recovery-proof", "1")
                .await
                .unwrap(),
            task,
        )
    }
}
