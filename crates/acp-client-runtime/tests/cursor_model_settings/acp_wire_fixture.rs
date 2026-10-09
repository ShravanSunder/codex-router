use std::{path::PathBuf, sync::Arc};

use acp_client_runtime::{
    AgentSessionClient, ApprovalPortOutcome, EventSinkClosed, ExternalProviderLaunch,
    HistoryReplayFuture, InteractionFuture, InteractionPort, ProviderPersistenceTarget,
    RefusedApprovalOffer, SessionEventSink,
};
use session_event_model::{ApprovalRequest, SessionEvent, SessionSettings};
use tokio_util::sync::CancellationToken;

pub struct NoopInteractionPort;

impl InteractionPort for NoopInteractionPort {
    type Context = ();
    type OperationId = u64;

    fn operation_id(_: &()) -> u64 {
        1
    }
    fn binding_retirement(_: &()) -> CancellationToken {
        CancellationToken::new()
    }
    fn request_approval(
        &self,
        _: (),
        _: ApprovalRequest,
        _: CancellationToken,
        _: CancellationToken,
    ) -> InteractionFuture<'_, ApprovalPortOutcome> {
        Box::pin(async { ApprovalPortOutcome::Cancelled })
    }
    fn request_question(
        &self,
        _: (),
        _: session_event_model::QuestionRequest,
        _: CancellationToken,
        _: CancellationToken,
    ) -> InteractionFuture<'_, session_event_model::QuestionResponse> {
        Box::pin(async { session_event_model::QuestionResponse::Cancelled })
    }
    fn record_refusal(&self, _: (), _: RefusedApprovalOffer) -> InteractionFuture<'_, ()> {
        Box::pin(async {})
    }
    fn cancel_all(&self, _: (), _: &'static str) -> InteractionFuture<'_, ()> {
        Box::pin(async {})
    }
    fn cancel_retired(&self) -> InteractionFuture<'_, ()> {
        Box::pin(async {})
    }
}

#[derive(Default)]
pub struct SettingsSink {
    pub settings: std::sync::Mutex<Vec<SessionSettings>>,
    pub changed: tokio::sync::Notify,
}

impl SessionEventSink for SettingsSink {
    fn begin_history_replay(&self, _: &str) -> HistoryReplayFuture<'_> {
        Box::pin(async { Ok(()) })
    }
    fn publish(&self, _: &str, event: SessionEvent) -> Result<(), EventSinkClosed> {
        if let SessionEvent::SettingsChanged { settings } = event {
            self.settings.lock().expect("settings lock").push(settings);
            self.changed.notify_one();
        }
        Ok(())
    }
}

pub struct AcpWireFixture {
    pub root: tempfile::TempDir,
    pub receipt: PathBuf,
    pub client: AgentSessionClient<NoopInteractionPort>,
    pub sink: Arc<SettingsSink>,
}

enum ClientInitialization {
    Standard,
    CursorWithMcp,
    #[cfg(feature = "test-observation")]
    CursorWithoutMcp,
}

impl AcpWireFixture {
    pub async fn start(scenario: &str, cursor_picker: bool) -> Self {
        Self::initialize(
            scenario,
            if cursor_picker {
                ClientInitialization::CursorWithMcp
            } else {
                ClientInitialization::Standard
            },
        )
        .await
    }

    #[cfg(feature = "test-observation")]
    pub async fn start_without_mcp(scenario: &str) -> Self {
        Self::initialize(scenario, ClientInitialization::CursorWithoutMcp).await
    }

    async fn initialize(scenario: &str, initialization: ClientInitialization) -> Self {
        let root = tempfile::tempdir().expect("fixture root");
        let receipt = root.path().join("wire.json");
        let launch = ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec![
                "-u".into(),
                "-c".into(),
                include_str!("scripted_acp_agent.py").into(),
                scenario.into(),
                receipt.to_string_lossy().into_owned(),
            ],
            environment: Vec::new(),
            persistence_target: ProviderPersistenceTarget::Unspecified,
        };
        let sink = Arc::new(SettingsSink::default());
        let client = match initialization {
            ClientInitialization::CursorWithMcp => {
                AgentSessionClient::initialize_with_mcp_http(
                    launch,
                    acp_client_runtime::ProviderModelPicker::CursorParameterized,
                    "fixture",
                    "http://127.0.0.1:1/mcp",
                    Arc::new(NoopInteractionPort),
                    sink.clone(),
                )
                .await
            }
            ClientInitialization::Standard => {
                AgentSessionClient::initialize(launch, Arc::new(NoopInteractionPort), sink.clone())
                    .await
            }
            #[cfg(feature = "test-observation")]
            ClientInitialization::CursorWithoutMcp => {
                AgentSessionClient::initialize_with_model_picker(
                    launch,
                    acp_client_runtime::ProviderModelPicker::CursorParameterized,
                    Arc::new(NoopInteractionPort),
                    sink.clone(),
                )
                .await
            }
        }
        .expect("peer initializes");
        Self {
            root,
            receipt,
            client,
            sink,
        }
    }

    pub fn requests(&self) -> Vec<serde_json::Value> {
        serde_json::from_slice(&std::fs::read(&self.receipt).expect("wire receipt"))
            .expect("wire JSON")
    }
}
