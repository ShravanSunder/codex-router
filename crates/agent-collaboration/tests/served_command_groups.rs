//! Command groups no other test runs against a real collaboration API: questions, the
//! message inbox, history and reply, journal status, session rename and turn interrupt
//! (refused by an application with no native route, as it refuses them today).
//! Each runs the compiled CLI against the served API and checks its JSON against the
//! published `FiniteCommandRecord` contract.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use collaboration_mcp::test_support::ServedCollaborationApi;
use collaboration_service::{CollaborationApplication, ServiceIdentity};
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::Path, sync::Arc};

const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000301";
const SERVICE_EPOCH: &str = "00000000-0000-4000-8000-000000000302";
const CALLER_SESSION_ID: &str = "served-command-caller";
const UNKNOWN_PUSH_ID: &str = "019f0000-0000-7000-8000-000000000999";

struct ServedFixture {
    directory: tempfile::TempDir,
    served: ServedCollaborationApi,
    journal: Arc<lifecycle_observation::LifecycleStore>,
}

impl ServedFixture {
    /// An application with an automation store, an interaction broker and a lifecycle
    /// journal, publishing a Codex endpoint whose native backend is absent.
    async fn start() -> Self {
        let directory = tempfile::tempdir_in("/tmp").expect("service directory");
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private service directory");
        let service_id: collaboration_protocol::UuidIdentity =
            SERVICE_ID.to_owned().try_into().expect("service ID");
        let endpoint: collaboration_protocol::EndpointRef =
            serde_json::from_value(codex_endpoint()).expect("endpoint");
        let broker = collaboration_service::ServiceInteractionBroker::load(
            service_id,
            collaboration_service::NativeControlBackend {
                endpoint,
                gate: collaboration_service::NativeGenerationGate::default(),
                codex_home: directory.path().to_owned(),
            },
            directory.path().join("approval-routes.json"),
        )
        .await
        .expect("interaction broker");
        let automation = Arc::new(tokio::sync::Mutex::new(
            automation_storage::AutomationStore::open(&directory.path().join("automation.sqlite"))
                .await
                .expect("automation store"),
        ));
        let journal = lifecycle_observation::ObservationJournal::open(
            &directory.path().join("session-registry.sqlite"),
            "00000000-0000-4000-8000-000000000303"
                .to_owned()
                .try_into()
                .expect("journal identity"),
        )
        .await
        .expect("journal");
        let journal = Arc::new(lifecycle_observation::LifecycleStore::new(journal));
        let description = serde_json::from_value(json!({
            "endpoint":codex_endpoint(),"label":"Codex fixture",
            "availability":{"state":"available","observedAt":"2026-10-08T00:00:00Z"},
            "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"codex-native.sock",
                "schemaDigest":null,"generation":{"serviceEpoch":SERVICE_EPOCH,"generation":1}}]
        }))
        .expect("Codex endpoint");
        let identity = ServiceIdentity::new(SERVICE_ID, SERVICE_EPOCH)
            .expect("service identity")
            .with_endpoints(vec![description])
            .expect("endpoint inventory")
            .with_automation_store(automation)
            .with_approval_broker(broker)
            .with_journal(Arc::clone(&journal));
        let served = ServedCollaborationApi::start(
            directory.path(),
            CollaborationApplication::new(identity),
        )
        .await
        .expect("served collaboration API");
        Self {
            directory,
            served,
            journal,
        }
    }

    fn path(&self) -> &Path {
        self.directory.path()
    }

    async fn finish(self) {
        self.served.stop().await.expect("collaboration API stops");
        if let Ok(journal) = Arc::try_unwrap(self.journal) {
            journal.close().await;
        }
    }
}

fn codex_endpoint() -> Value {
    json!({"serviceId":SERVICE_ID,"endpointId":"codex-local"})
}

fn session(session_id: &str) -> Value {
    json!({"endpoint":codex_endpoint(),"sessionId":session_id})
}

/// Runs the CLI as the caller Session and returns its exit status and one JSON record,
/// checked against the published finite-command contract.
async fn run_finite(directory: &Path, arguments: &[&str]) -> (Option<i32>, Value) {
    let (status, record) = run_json(directory, arguments).await;
    let schemas =
        collaboration_client::protocol::protocol_type_schemas().expect("protocol schema export");
    let validator = jsonschema::validator_for(
        schemas
            .get("FiniteCommandRecord")
            .expect("FiniteCommandRecord schema"),
    )
    .expect("finite record validator");
    if let Err(error) = validator.validate(&record) {
        panic!("{arguments:?} broke the published CLI record: {error}; {record}");
    }
    (status, record)
}

/// Runs the CLI as the caller Session and returns its exit status and one JSON record.
async fn run_json(directory: &Path, arguments: &[&str]) -> (Option<i32>, Value) {
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args(arguments)
        .args(["--json", "--service-directory"])
        .arg(directory)
        .env("CODEX_THREAD_ID", CALLER_SESSION_ID)
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CURSOR_CONVERSATION_ID")
        .output()
        .await
        .expect("CLI runs");
    let record: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{arguments:?} printed no JSON record ({error}): stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.code(), record)
}

#[tokio::test]
async fn questions_messages_and_journal_read_through_the_served_api() {
    let fixture = ServedFixture::start().await;

    let (status, questions) = run_finite(fixture.path(), &["question", "list", "--pending"]).await;
    assert_eq!(status, Some(0), "{questions}");
    assert_eq!(questions["kind"], "result", "{questions}");

    let human = json!({"kind":"human","humanId":"served-owner"}).to_string();
    let (status, answer) = run_finite(
        fixture.path(),
        &[
            "question",
            "answer",
            "--request-id",
            "absent-question",
            "--actor",
            &human,
            "--decline",
        ],
    )
    .await;
    assert_eq!(answer["kind"], "error", "{answer}");
    assert_eq!(status, Some(4), "{answer}");

    let (status, inbox) = run_finite(fixture.path(), &["message", "inbox"]).await;
    assert_eq!(status, Some(0), "{inbox}");
    assert_eq!(inbox["kind"], "result", "{inbox}");

    let with = session("served-command-peer").to_string();
    let (status, history) =
        run_finite(fixture.path(), &["message", "history", "--with", &with]).await;
    assert_eq!(status, Some(0), "{history}");
    assert_eq!(history["kind"], "result", "{history}");

    // A reply's error record also names its caller, which the finite-record contract does
    // not describe, so only its fields are checked.
    let (status, reply) = run_json(
        fixture.path(),
        &["message", "reply", UNKNOWN_PUSH_ID, "answer"],
    )
    .await;
    assert_eq!(reply["kind"], "error", "{reply}");
    assert_eq!(reply["error"]["serviceKind"], "notFound", "{reply}");
    assert_eq!(status, Some(4), "{reply}");

    let (status, journal) = run_finite(fixture.path(), &["journal", "status"]).await;
    assert_eq!(status, Some(0), "{journal}");
    assert_eq!(journal["kind"], "result", "{journal}");

    fixture.finish().await;
}

#[tokio::test]
async fn native_session_commands_refuse_an_endpoint_without_a_native_route() {
    let fixture = ServedFixture::start().await;

    let (status, renamed) = run_finite(
        fixture.path(),
        &[
            "session",
            "rename",
            "--endpoint",
            "codex-local",
            "--session",
            "served-command-thread",
            "--name",
            "Renamed",
        ],
    )
    .await;
    assert_eq!(renamed["kind"], "error", "{renamed}");
    assert_eq!(
        renamed["error"]["kind"], "unsupportedCapability",
        "{renamed}"
    );
    assert_eq!(status, Some(2), "{renamed}");

    let (status, interrupted) = run_finite(
        fixture.path(),
        &[
            "turn",
            "interrupt",
            "--endpoint",
            "codex-local",
            "--session",
            "served-command-thread",
            "--turn",
            "served-command-turn",
        ],
    )
    .await;
    assert_eq!(interrupted["kind"], "error", "{interrupted}");
    assert_eq!(
        interrupted["error"]["kind"], "unsupportedCapability",
        "{interrupted}"
    );
    assert_eq!(status, Some(2), "{interrupted}");

    fixture.finish().await;
}
