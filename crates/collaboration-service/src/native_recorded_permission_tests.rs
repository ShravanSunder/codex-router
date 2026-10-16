//! Cold native message attachment must preserve the broker's recorded authority.
use crate::native_message_dispatch::{
    NativeMessageBody, NativeMessageOutcome, NativeMessageParams, NativeMessageRequest,
    dispatch_message,
};
use crate::{LoadPolicy, NativeControlBackend, NativeGenerationGate, ServiceInteractionBroker};
use codex_acp_adapter::{ApprovalBroker, ApprovalRoute};
use codex_native_integration::{NativePayloadSchemas, NativeSchemaBundle};
use collaboration_protocol::{CodexGeneration, EndpointDescription, MessageDelivery, SessionRef};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};
use tokio_tungstenite::tungstenite::Message;

type TestError = Box<dyn std::error::Error + Send + Sync>;
const SERVICE: &str = "3d26f886-1489-423f-bc3d-6c0c940ddbb2";
const EPOCH: &str = "110a92d0-e607-4947-86ca-45838ac90a96";
const THREAD: &str = "recorded-cold-thread";

fn backend(root: &Path) -> (NativeControlBackend, CodexGeneration, EndpointDescription) {
    let mut definitions = serde_json::Map::new();
    for name in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
        "ThreadTurnsList",
        "ThreadQueueAdd",
        "ThreadQueueList",
    ] {
        definitions.insert(format!("{name}Params"), json!({"type":"object"}));
        definitions.insert(format!("{name}Response"), json!({"type":"object"}));
    }
    let bundle = NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".into(),
        serde_json::to_vec(&json!({"definitions":{"v2":definitions}})).unwrap(),
    )]))
    .unwrap();
    let schemas = Arc::new(NativePayloadSchemas::from_bundle(&bundle).unwrap());
    let generation: CodexGeneration =
        serde_json::from_value(json!({"serviceEpoch":EPOCH,"generation":1})).unwrap();
    let endpoint =
        serde_json::from_value(json!({"serviceId":SERVICE,"endpointId":"codex-local"})).unwrap();
    let description = serde_json::from_value(json!({"endpoint":endpoint,"label":"fixture","availability":{"state":"available","observedAt":"2026-10-11T00:00:00Z"},"channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":schemas.schema_digest(),"generation":generation}]})).unwrap();
    let gate = NativeGenerationGate::default();
    gate.activate(generation.clone(), root.join("native.sock"), Some(schemas))
        .unwrap();
    (
        NativeControlBackend {
            endpoint,
            gate,
            codex_home: root.to_owned(),
        },
        generation,
        description,
    )
}

fn target() -> SessionRef {
    serde_json::from_value(
        json!({"endpoint":{"serviceId":SERVICE,"endpointId":"codex-local"},"sessionId":THREAD}),
    )
    .unwrap()
}

/// Literal independent policy oracle, never obtained from the production projector or RPC input.
fn expected_filesystem(scratch: &Path) -> Result<serde_json::Map<String, Value>, TestError> {
    let home = std::env::var("HOME")?;
    let scratch = std::fs::canonicalize(scratch)?;
    let mut filesystem =
        serde_json::Map::from_iter([(scratch.to_string_lossy().into_owned(), json!("write"))]);
    for location in [
        "Library/Caches",
        ".cache",
        "Library/pnpm",
        ".local/share/pnpm",
        ".local/state/pnpm",
        ".pnpm-store",
        ".npm",
        ".yarn",
        ".bun/install/cache",
        ".cargo",
        ".rustup",
        "go/pkg",
        ".gradle",
        ".m2",
        "Library/org.swift.swiftpm",
        ".swiftpm",
        "Library/Developer",
        ".local/share/uv",
        "Library/Application Support/uv",
        ".local/share/mise",
        ".local/state/mise",
    ] {
        let symlinked = Path::new(location)
            .ancestors()
            .filter(|part| !part.as_os_str().is_empty())
            .any(|part| {
                std::fs::symlink_metadata(Path::new(&home).join(part))
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
            });
        if !symlinked {
            filesystem.insert(format!("{home}/{location}"), json!("write"));
        }
    }
    for location in [
        "Library/pnpm/bin",
        ".local/share/mise/shims",
        ".yarn/bin",
        "Library/Caches/pnpm/dlx",
        "Library/Caches/ms-playwright",
        "Library/Caches/bazelisk",
        ".cache/uv/environments-v2",
        ".cargo/bin",
        ".cargo/env",
        ".cargo/env.fish",
        ".cargo/env.nu",
        ".cargo/env.ps1",
        ".cargo/env.tcsh",
        ".cargo/config",
        ".cargo/config.toml",
        ".cargo/credentials",
        ".cargo/credentials.toml",
        ".gradle/init.d",
        ".gradle/init.gradle",
        ".gradle/init.gradle.kts",
        ".gradle/gradle.properties",
        ".m2/settings.xml",
        ".m2/settings-security.xml",
        "Library/Application Support/uv/credentials",
    ] {
        filesystem.insert(format!("{home}/{location}"), json!("read"));
    }
    Ok(filesystem)
}

#[derive(Clone, Copy)]
enum RecordedAuthority {
    Valid,
    Absent,
    Expired,
    InvalidScratch,
}
#[derive(Clone, Copy)]
enum NativeReply {
    Matching,
    Narrowed,
    WrongDirectory,
    WrongThread,
}
struct ProbeCase {
    authority: RecordedAuthority,
    reply: NativeReply,
    directory_present: bool,
}
struct ProbeObservation {
    outcome: NativeMessageOutcome,
    resume: Option<Value>,
    turn_started: bool,
    expected_filesystem: serde_json::Map<String, Value>,
}

async fn cold_message_probe(case: ProbeCase) -> Result<ProbeObservation, TestError> {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let (backend, generation, endpoint) = backend(root);
    let broker = ServiceInteractionBroker::load(
        SERVICE.to_owned().try_into()?,
        backend.clone(),
        root.join("approval-routes.json"),
    )
    .await?;
    let scratch = root.join("scratch");
    tokio::fs::create_dir(&scratch).await?;
    if !matches!(case.authority, RecordedAuthority::Absent) {
        broker
            .register_route(ApprovalRoute {
                thread_id: THREAD.into(),
                created_by: target(),
                approver: target(),
                access: collaboration_protocol::RouterAccess::WorkspaceWrite,
                scratch_path: if matches!(case.authority, RecordedAuthority::InvalidScratch) {
                    "relative-scratch".into()
                } else {
                    scratch.to_string_lossy().into_owned()
                },
                root_message_id: None,
            })
            .await?;
    }
    // Reopen the real persisted route, rather than supplying an invented broker answer.
    drop(broker);
    let broker: Arc<dyn ApprovalBroker> = ServiceInteractionBroker::load(
        SERVICE.to_owned().try_into()?,
        backend.clone(),
        root.join("approval-routes.json"),
    )
    .await?;
    let weak_broker = Arc::downgrade(&broker);
    let retained_broker = if matches!(case.authority, RecordedAuthority::Expired) {
        drop(broker);
        None
    } else {
        Some(broker)
    };
    let listener = tokio::net::UnixListener::bind(root.join("native.sock"))?;
    let cwd = root.to_string_lossy().into_owned();
    let filesystem = expected_filesystem(&scratch)?;
    let response_roots = filesystem
        .iter()
        .filter(|(_, access)| *access == &json!("write"))
        .map(|(path, _)| path.clone())
        .collect::<Vec<_>>();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        let init: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("missing initialize")??
                .to_text()?,
        )?;
        socket
            .send(Message::Text(
                json!({"id":init["id"],"result":{}}).to_string().into(),
            ))
            .await?;
        socket.next().await.ok_or("missing initialized")??;
        let read: Value =
            serde_json::from_str(socket.next().await.ok_or("missing read")??.to_text()?)?;
        assert_eq!(read["method"], "thread/read");
        let mut thread = json!({"id":THREAD,"status":{"type":"notLoaded"}});
        if case.directory_present {
            thread["cwd"] = json!(cwd);
        }
        socket
            .send(Message::Text(
                json!({"id":read["id"],"result":{"thread":thread}})
                    .to_string()
                    .into(),
            ))
            .await?;
        let resume = match socket.next().await {
            Some(Ok(Message::Text(text))) => serde_json::from_str::<Value>(&text)?,
            _ => return Ok::<_, TestError>((None, false)),
        };
        assert_eq!(resume["method"], "thread/resume");
        let response_cwd = if matches!(case.reply, NativeReply::WrongDirectory) {
            "/wrong-cwd"
        } else {
            &cwd
        };
        let response_id = if matches!(case.reply, NativeReply::WrongThread) {
            "other-thread"
        } else {
            THREAD
        };
        let roots = if matches!(case.reply, NativeReply::Narrowed) {
            vec![]
        } else {
            response_roots
        };
        socket.send(Message::Text(json!({"id":resume["id"],"result":{"thread":{"id":response_id,"cwd":response_cwd},"cwd":response_cwd,"activePermissionProfile":{"id":"router-workspace-write"},"sandbox":{"type":"workspaceWrite","networkAccess":true,"writableRoots":roots}}}).to_string().into())).await?;
        let turn_started = match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                let turn: Value = serde_json::from_str(&text)?;
                assert_eq!(turn["method"], "turn/start");
                assert_eq!(turn.pointer("/params/threadId"), Some(&json!(THREAD)));
                socket
                    .send(Message::Text(
                        json!({"id":turn["id"],"result":{"turn":{"id":"accepted-turn"}}})
                            .to_string()
                            .into(),
                    ))
                    .await?;
                true
            }
            _ => false,
        };
        Ok::<_, TestError>((Some(resume), turn_started))
    });
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        dispatch_message(NativeMessageRequest {
            params: NativeMessageParams {
                target: target(),
                generation,
                body: NativeMessageBody::PreparedPush(
                    "permission preservation probe".to_owned().try_into()?,
                ),
                delivery: MessageDelivery::Auto,
                client_user_message_id: Some("permission-probe".to_owned().try_into()?),
            },
            id: json!("permission-probe"),
            service_id: &backend.endpoint.service_id,
            backend: &backend,
            endpoints: &[endpoint],
            held_connection: None,
            load_policy: LoadPolicy::MayLoad,
            approval_broker: Some(&weak_broker),
            recorded_route: None,
            attempt_deadline: None,
        }),
    )
    .await;
    // Join the native actor even if the caller times out: no detached test socket owner.
    let observation = tokio::time::timeout(Duration::from_secs(5), server).await???;
    drop(retained_broker);
    Ok(ProbeObservation {
        outcome: result?,
        resume: observation.0,
        turn_started: observation.1,
        expected_filesystem: filesystem,
    })
}

fn assert_projected_policy(observation: &ProbeObservation) {
    let resume = observation.resume.as_ref().expect("resume request");
    assert_eq!(
        resume.pointer("/params/permissions"),
        Some(&json!("router-workspace-write")),
        "cold resume must explicitly restore the recorded policy"
    );
    assert_eq!(
        resume.pointer("/params/config/default_permissions"),
        Some(&json!("router-workspace-write"))
    );
    assert_eq!(
        resume.pointer("/params/config/permissions.router-workspace-write.extends"),
        Some(&json!(":workspace"))
    );
    assert_eq!(
        resume.pointer("/params/config/permissions.router-workspace-write.filesystem"),
        Some(&json!(observation.expected_filesystem))
    );
    assert_eq!(resume.pointer("/params/excludeTurns"), Some(&json!(true)));
}

#[tokio::test]
async fn cold_native_message_projects_recorded_grants_before_turn_submission() {
    let observation = cold_message_probe(ProbeCase {
        authority: RecordedAuthority::Valid,
        reply: NativeReply::Narrowed,
        directory_present: true,
    })
    .await
    .expect("native fixture operation");
    assert_projected_policy(&observation);
    assert!(
        !observation.turn_started,
        "narrowed native grants must prevent turn submission"
    );
    let NativeMessageOutcome::Failed(failure) = observation.outcome else {
        panic!("narrowed resume was accepted");
    };
    assert_eq!(
        failure.pointer("/error/data/effects/resume"),
        Some(&json!("accepted"))
    );
    assert_eq!(
        failure.pointer("/error/data/effects/submission"),
        Some(&json!("notDispatched"))
    );
}

#[tokio::test]
async fn cold_native_message_with_matching_grants_submits_on_the_same_connection() {
    let observation = cold_message_probe(ProbeCase {
        authority: RecordedAuthority::Valid,
        reply: NativeReply::Matching,
        directory_present: true,
    })
    .await
    .expect("native fixture operation");
    assert_projected_policy(&observation);
    assert!(observation.turn_started);
    let NativeMessageOutcome::Accepted(receipt) = observation.outcome else {
        panic!("valid recorded resume refused");
    };
    assert_eq!(
        receipt.resume_effect,
        collaboration_protocol::AcceptedResumeEffect::Accepted
    );
}

#[tokio::test]
async fn cold_native_message_without_a_recorded_route_preserves_generic_attachment() {
    let observation = cold_message_probe(ProbeCase {
        authority: RecordedAuthority::Absent,
        reply: NativeReply::Matching,
        directory_present: false,
    })
    .await
    .expect("native fixture operation");
    assert_eq!(
        observation.resume.as_ref().unwrap()["params"],
        json!({"threadId":THREAD,"excludeTurns":true})
    );
    assert!(observation.turn_started);
    assert!(matches!(
        observation.outcome,
        NativeMessageOutcome::Accepted(_)
    ));
}

#[tokio::test]
async fn cold_native_message_rejects_missing_or_invalid_authority_before_resume() {
    for (authority, directory_present) in [
        (RecordedAuthority::Expired, true),
        (RecordedAuthority::InvalidScratch, true),
        (RecordedAuthority::Valid, false),
    ] {
        let observation = cold_message_probe(ProbeCase {
            authority,
            reply: NativeReply::Matching,
            directory_present,
        })
        .await
        .expect("native fixture operation");
        assert!(observation.resume.is_none());
        assert!(!observation.turn_started);
        let NativeMessageOutcome::Failed(failure) = observation.outcome else {
            panic!("invalid authority accepted");
        };
        assert_eq!(
            failure.pointer("/error/data/effects/resume"),
            Some(&json!("notRequested"))
        );
        assert_eq!(
            failure.pointer("/error/data/effects/submission"),
            Some(&json!("notDispatched"))
        );
    }
}

#[tokio::test]
async fn cold_native_message_rejects_changed_response_identity_and_directory() {
    for reply in [NativeReply::WrongDirectory, NativeReply::WrongThread] {
        let observation = cold_message_probe(ProbeCase {
            authority: RecordedAuthority::Valid,
            reply,
            directory_present: true,
        })
        .await
        .expect("native fixture operation");
        assert_projected_policy(&observation);
        assert!(!observation.turn_started);
        let NativeMessageOutcome::Failed(failure) = observation.outcome else {
            panic!("mismatched attachment accepted");
        };
        assert_eq!(
            failure.pointer("/error/data/effects/submission"),
            Some(&json!("notDispatched"))
        );
    }
}

#[tokio::test]
async fn native_delivery_does_not_retain_its_broker_owner() {
    let directory = tempfile::tempdir().expect("fixture directory");
    let (backend, _, _) = backend(directory.path());
    let service_id = backend.endpoint.service_id.clone();
    let broker = ServiceInteractionBroker::load(
        service_id.clone(),
        backend.clone(),
        directory.path().join("approval-routes.json"),
    )
    .await
    .expect("native fixture operation");
    let retained_broker = Arc::downgrade(&broker);
    let broker_port: Arc<dyn ApprovalBroker> = broker.clone();
    let weak_port = Arc::downgrade(&broker_port);
    let native_route: Arc<dyn crate::SessionDeliveryRoute> = Arc::new(
        crate::CodexAppServerDeliveryRoute::new(
            service_id.clone(),
            crate::EndpointDirectory::new(service_id),
            backend,
            Arc::new(crate::UnmaterializedThreadHolder::new()),
        )
        .with_approval_broker(&broker),
    );
    broker
        .install_session_delivery(Arc::new(crate::SessionDeliveryRouter::new(vec![
            native_route,
        ])))
        .expect("install delivery");
    drop(broker_port);
    drop(broker);
    assert!(
        retained_broker.upgrade().is_none(),
        "delivery route must not keep its owner alive"
    );
    assert!(weak_port.upgrade().is_none());
}

#[path = "native_loaded_permission_tests.rs"]
mod loaded_followup_tests;
