//! Real owner-only API, source files, generation gate and native WebSockets.
//! Backend protocol payloads are fixed literals; no Codex or model is launched.
use super::*;
use collaboration_client::protocol::{CodexGeneration, ServiceManifest};
use collaboration_service::{
    CollaborationApplication, EndpointDirectory, ManifestPublication, NativeGenerationGate,
    NativeRelayListener, ServiceIdentity,
};
use futures_util::{SinkExt, StreamExt};
use std::{os::unix::fs::PermissionsExt, path::PathBuf, task::Poll};
use tokio::net::{UnixListener, UnixStream};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};
use tokio_util::sync::CancellationToken;

const SOURCE_SERVICE: &str = "00000000-0000-4000-8000-000000000021";
const SOURCE_EPOCH: &str = "00000000-0000-4000-8000-000000000022";
const INITIALIZE_FRAME: &str = "{\"id\":\"source-proof\",\"method\":\"initialize\"}";
const INITIALIZE_REPLY: &str = "{\"id\":\"source-proof\",\"result\":{}}";

struct SourceNativeFixture {
    root: tempfile::TempDir,
    project: PathBuf,
    context: CliContext,
    request: SourceRequest,
    directory: EndpointDirectory,
    gate: NativeGenerationGate,
    first_backend: UnixListener,
    replacement_backend: UnixListener,
    shutdown: CancellationToken,
    relay: tokio::task::JoinHandle<std::io::Result<()>>,
    api: collaboration_mcp::test_support::ServedCollaborationApi,
    _manifest: ManifestPublication,
}

fn generation(number: u64) -> CodexGeneration {
    CodexGeneration {
        service_epoch: SOURCE_EPOCH.to_owned().try_into().unwrap(),
        generation: number.try_into().unwrap(),
    }
}

impl SourceNativeFixture {
    async fn start() -> Self {
        let root = tempfile::Builder::new()
            .prefix("source-pin-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap();
        let service_root = std::fs::canonicalize(root.path()).unwrap();
        let project = service_root.join("source-project");
        std::fs::create_dir(&project).unwrap();
        let codex_home = service_root.join("fixture-codex-home");
        std::fs::create_dir(&codex_home).unwrap();
        std::fs::write(
            codex_home.join(
                SourceClientProfile::RouterDebug
                    .native_profile()
                    .file_name(),
            ),
            "model_provider = \"codex-router\"\n",
        )
        .unwrap();
        let context = CliContext::new(vec![(
            "CODEX_HOME".to_owned(),
            codex_home.to_str().unwrap().to_owned(),
        )]);
        let identity = ServiceIdentity::new(SOURCE_SERVICE, SOURCE_EPOCH).unwrap();
        let directory = identity.endpoint_directory();
        let gate = NativeGenerationGate::default();
        let first_backend = UnixListener::bind(service_root.join("first.sock")).unwrap();
        let replacement_backend = UnixListener::bind(service_root.join("second.sock")).unwrap();
        let shutdown = CancellationToken::new();
        let listener =
            NativeRelayListener::bind(&service_root.join("codex-native.sock"), gate.clone())
                .unwrap();
        let relay = tokio::spawn(listener.run(shutdown.clone()));
        let api = collaboration_mcp::test_support::ServedCollaborationApi::start(
            &service_root,
            CollaborationApplication::new(identity),
        )
        .await
        .unwrap();
        let mut manifest: ServiceManifest =
            serde_json::from_slice(&std::fs::read(service_root.join("service.json")).unwrap())
                .unwrap();
        manifest.native_schema_digest =
            Some(format!("sha256:{}", "b".repeat(64)).try_into().unwrap());
        let manifest = ManifestPublication::publish(&service_root, &manifest).unwrap();
        let request = SourceRequest {
            service_directory: service_root
                .to_str()
                .unwrap()
                .to_owned()
                .try_into()
                .unwrap(),
            expected_service: SOURCE_SERVICE.to_owned().try_into().unwrap(),
            operation: SourceOperation::Prepare {
                action: SourceSessionAction::New {
                    directory: project.to_str().unwrap().to_owned().try_into().unwrap(),
                },
                client_profile: SourceClientProfile::RouterDebug,
            },
        };
        let fixture = Self {
            root,
            project,
            context,
            request,
            directory,
            gate,
            first_backend,
            replacement_backend,
            shutdown,
            relay,
            api,
            _manifest: manifest,
        };
        fixture.publish_generation(1);
        fixture
    }

    fn publish_generation(&self, number: u64) {
        self.gate.retire().unwrap();
        let backend_name = if number == 1 {
            "first.sock"
        } else {
            "second.sock"
        };
        self.gate
            .activate(
                generation(number),
                self.api.directory().join(backend_name),
                None,
            )
            .unwrap();
        self.directory.publish(serde_json::from_value(serde_json::json!({
            "endpoint":{"serviceId":SOURCE_SERVICE,"endpointId":"codex-local"},
            "label":"Source native proof","availability":{"state":"available","observedAt":"2026-10-15T00:00:00Z"},
            "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"codex-native.sock",
                "schemaDigest":format!("sha256:{}", "b".repeat(64)),"generation":generation(number)}]
        })).unwrap()).unwrap();
    }

    async fn prepare(&self) -> PreparedSourceLaunch {
        let SourceReply::Prepared { launch } =
            execute_read(&self.request, &self.context).await.unwrap()
        else {
            panic!("prepare must return a prepared launch")
        };
        *launch
    }

    async fn connect(
        &self,
        prepared: &PreparedSourceLaunch,
    ) -> (NativeTransportConnection, WebSocketStream<UnixStream>) {
        let (connection, backend) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(
                establish_prepared_connection(&self.request, prepared, &self.context),
                async {
                    let (socket, _) = self.first_backend.accept().await.unwrap();
                    tokio_tungstenite::accept_async(socket).await.unwrap()
                },
            )
        })
        .await
        .unwrap();
        (connection.unwrap(), backend)
    }

    async fn finish(self) {
        self.shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(2), self.relay)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        self.api.stop().await.unwrap();
        drop(self.root);
    }
}

#[tokio::test]
async fn source_native_prepare_reads_actual_api_source_directory_and_rejects_wrong_service() {
    let fixture = SourceNativeFixture::start().await;
    let prepared = fixture.prepare().await;
    assert_eq!(
        prepared.working_directory.as_str(),
        fixture.project.to_str().unwrap()
    );
    assert_eq!(prepared.generation, generation(1));
    assert_eq!(String::from(prepared.endpoint.service_id), SOURCE_SERVICE);
    assert!(
        matches!(
            futures_util::poll!(Box::pin(fixture.first_backend.accept())),
            Poll::Pending
        ),
        "Prepare cannot attach native work"
    );
    let mut wrong = fixture.request.clone();
    wrong.expected_service = "00000000-0000-4000-8000-000000000023"
        .to_owned()
        .try_into()
        .unwrap();
    assert!(matches!(
        execute_read(&wrong, &fixture.context).await,
        Err(SourceRejection::WrongService)
    ));
    fixture.finish().await;
}

#[tokio::test]
async fn source_native_postcheck_rejects_replacement_after_preflight_without_application_frames() {
    let fixture = SourceNativeFixture::start().await;
    let prepared = fixture.prepare().await;
    let (reached, reached_receiver) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let request = fixture.request.clone();
    let context = fixture.context.clone();
    let action = tokio::spawn(async move {
        establish_connection_at_boundary(&request, &prepared, &context, async move {
            reached.send(()).unwrap();
            released.await.unwrap();
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), reached_receiver)
        .await
        .unwrap()
        .unwrap();
    fixture.publish_generation(2);
    release.send(()).unwrap();
    let (result, mut backend) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(action, async {
            let (socket, _) = fixture.replacement_backend.accept().await.unwrap();
            tokio_tungstenite::accept_async(socket).await.unwrap()
        })
    })
    .await
    .unwrap();
    assert!(matches!(
        result.unwrap(),
        Err(SourceRejection::StaleBinding)
    ));
    let application_frames = tokio::time::timeout(Duration::from_secs(2), async {
        let mut frames = 0;
        while let Some(frame) = backend.next().await {
            match frame {
                Ok(Message::Text(_) | Message::Binary(_)) => frames += 1,
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
        frames
    })
    .await
    .unwrap();
    assert_eq!(
        application_frames, 0,
        "replacement must receive no initialize or action"
    );
    drop(backend);
    assert!(matches!(
        futures_util::poll!(Box::pin(fixture.first_backend.accept())),
        Poll::Pending
    ));
    fixture.finish().await;
}

#[tokio::test]
async fn source_native_retirement_after_postcheck_cannot_reopen_replacement() {
    let fixture = SourceNativeFixture::start().await;
    let prepared = fixture.prepare().await;
    let (mut connection, mut backend) = fixture.connect(&prepared).await;
    fixture.publish_generation(2);
    let closed = tokio::time::timeout(Duration::from_secs(2), connection.stream.next())
        .await
        .unwrap();
    assert!(matches!(
        closed,
        None | Some(Err(_)) | Some(Ok(Message::Close(_)))
    ));
    assert!(
        connection
            .stream
            .send(Message::Text(INITIALIZE_FRAME.into()))
            .await
            .is_err()
    );
    let backend_closed = tokio::time::timeout(Duration::from_secs(2), backend.next())
        .await
        .unwrap();
    assert!(matches!(
        backend_closed,
        None | Some(Err(_)) | Some(Ok(Message::Close(_)))
    ));
    assert!(matches!(
        futures_util::poll!(Box::pin(fixture.replacement_backend.accept())),
        Poll::Pending
    ));
    drop(connection);
    drop(backend);
    fixture.finish().await;
}

#[tokio::test]
async fn source_native_verified_connection_forwards_only_the_consumers_literal_request() {
    let fixture = SourceNativeFixture::start().await;
    let prepared = fixture.prepare().await;
    let (mut connection, mut backend) = fixture.connect(&prepared).await;
    // Real upgrade/postcheck send no native initialization before the consumer.
    assert!(matches!(
        futures_util::poll!(Box::pin(backend.next())),
        Poll::Pending
    ));
    connection
        .stream
        .send(Message::Text(INITIALIZE_FRAME.into()))
        .await
        .unwrap();
    let received = tokio::time::timeout(Duration::from_secs(2), backend.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(received, Message::Text(INITIALIZE_FRAME.into()));
    backend
        .send(Message::Text(INITIALIZE_REPLY.into()))
        .await
        .unwrap();
    let received = tokio::time::timeout(Duration::from_secs(2), connection.stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(received, Message::Text(INITIALIZE_REPLY.into()));
    drop(connection);
    drop(backend);
    fixture.finish().await;
}
