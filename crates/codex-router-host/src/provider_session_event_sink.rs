//! Ordered Host publication into the Router-owned provider Session hub.

use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use acp_client_runtime::{
    EventSinkOverflow, HistoryReplayFuture, HistoryReplayUnavailable, SessionEventSink,
};
use collaboration_service::ProviderSessionEventHub;
use message_board::{SessionEndpointRef, SessionId, SessionRef};
use session_event_model::SessionEvent;
use tokio::{
    sync::{Mutex, mpsc, oneshot},
    task::JoinHandle,
};

enum HubCommand {
    Publish {
        session: SessionRef,
        event: SessionEvent,
    },
    BeginReplay {
        session: SessionRef,
        completion: oneshot::Sender<Result<(), HistoryReplayUnavailable>>,
    },
}

/// One provider connection has one queue and one consumer. The synchronous ACP
/// reader only enqueues; replay waits behind every earlier publication.
pub(crate) struct HubSessionEventSink {
    endpoint: SessionEndpointRef,
    sender: StdMutex<Option<mpsc::UnboundedSender<HubCommand>>>,
    consumer: Mutex<Option<JoinHandle<()>>>,
    backlog: Arc<AtomicUsize>,
    backlog_warned: StdMutex<HashSet<String>>,
    not_publishing: Arc<StdMutex<HashSet<String>>>,
}

impl HubSessionEventSink {
    pub(crate) fn new(hub: Arc<ProviderSessionEventHub>, endpoint: SessionEndpointRef) -> Self {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let backlog = Arc::new(AtomicUsize::new(0));
        let not_publishing = Arc::new(StdMutex::new(HashSet::new()));
        let consumer_backlog = Arc::clone(&backlog);
        let consumer_failures = Arc::clone(&not_publishing);
        let consumer = tokio::spawn(async move {
            while let Some(command) = receiver.recv().await {
                consumer_backlog.fetch_sub(1, Ordering::Relaxed);
                match command {
                    HubCommand::Publish { session, event } => {
                        if let Err(error) = hub.publish(session.clone(), event).await {
                            tracing::error!(%error, session_id = %session.session_id.as_str(), "provider Session event publication failed");
                            consumer_failures
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .insert(session.session_id.as_str().to_owned());
                        }
                    }
                    HubCommand::BeginReplay {
                        session,
                        completion,
                    } => {
                        let result = hub
                            .begin_history_replay(session.clone())
                            .await
                            .map(|_| ())
                            .map_err(|error| {
                                tracing::error!(%error, session_id = %session.session_id.as_str(), "provider Session replay reset failed");
                                HistoryReplayUnavailable
                            });
                        let _ = completion.send(result);
                    }
                }
            }
        });
        Self {
            endpoint,
            sender: StdMutex::new(Some(sender)),
            consumer: Mutex::new(Some(consumer)),
            backlog,
            backlog_warned: StdMutex::new(HashSet::new()),
            not_publishing,
        }
    }

    fn session(&self, session_id: &str) -> Result<SessionRef, HistoryReplayUnavailable> {
        let session_id =
            SessionId::try_from(session_id.to_owned()).map_err(|_| HistoryReplayUnavailable)?;
        Ok(SessionRef {
            endpoint: self.endpoint.clone(),
            session_id,
        })
    }

    fn enqueue(&self, session_id: &str, command: HubCommand) -> Result<(), EventSinkOverflow> {
        if self
            .not_publishing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(session_id)
        {
            return Err(EventSinkOverflow);
        }
        self.backlog.fetch_add(1, Ordering::Relaxed);
        let sent = self
            .sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|sender| sender.send(command).is_ok());
        if !sent {
            self.backlog.fetch_sub(1, Ordering::Relaxed);
            self.not_publishing
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(session_id.to_owned());
            tracing::error!(session_id, "provider Session event consumer is unavailable");
            return Err(EventSinkOverflow);
        }
        if self.backlog.load(Ordering::Relaxed) > 10_000
            && self
                .backlog_warned
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(session_id.to_owned())
        {
            tracing::warn!(session_id, "provider Session event backlog exceeds 10000");
        }
        Ok(())
    }

    pub(crate) async fn shutdown(&self) -> Result<(), HistoryReplayUnavailable> {
        self.sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(consumer) = self.consumer.lock().await.take() {
            consumer.await.map_err(|_| HistoryReplayUnavailable)?;
        }
        Ok(())
    }
}

impl SessionEventSink for HubSessionEventSink {
    fn begin_history_replay(&self, session_id: &str) -> HistoryReplayFuture<'_> {
        let session = self.session(session_id);
        let session_id = session_id.to_owned();
        Box::pin(async move {
            let session = session?;
            let (completion, result) = oneshot::channel();
            self.enqueue(
                &session_id,
                HubCommand::BeginReplay {
                    session,
                    completion,
                },
            )
            .map_err(|_| HistoryReplayUnavailable)?;
            result.await.map_err(|_| HistoryReplayUnavailable)?
        })
    }

    fn publish(&self, session_id: &str, event: SessionEvent) -> Result<(), EventSinkOverflow> {
        let session = self.session(session_id).map_err(|_| EventSinkOverflow)?;
        self.enqueue(session_id, HubCommand::Publish { session, event })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acp_client_runtime::{ApprovalPortOutcome, InteractionPort, SessionEventSink};
    use collaboration_protocol::{
        CodexGeneration, EndpointId as ControlEndpointId, EndpointRef as ControlEndpointRef,
        GenerationNumber, OperationId, SessionId as ControlSessionId,
        SessionRef as ControlSessionRef, UuidIdentity,
    };
    use collaboration_service::{
        NativeControlBackend, NativeGenerationGate, ProviderOperationStore,
        ProviderSessionEventHub, ServiceInteractionBroker, SessionEventHub,
    };
    use message_board::{EndpointId, ServiceId, SessionEndpointRef, SessionId, SessionRef};
    use session_event_model::{SessionEvent, TurnOutcome};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[tokio::test]
    async fn typed_broker_request_and_decision_publish_pending_and_resolution_in_order() {
        let root = tempfile::tempdir().expect("temporary store");
        let service_id = UuidIdentity::try_from("00000000-0000-4000-8000-000000000001".to_owned())
            .expect("service ID");
        let provider_endpoint = ControlEndpointRef {
            service_id: service_id.clone(),
            endpoint_id: ControlEndpointId::try_from("fixture-provider".to_owned())
                .expect("provider endpoint"),
        };
        let approver_endpoint = ControlEndpointRef {
            service_id: service_id.clone(),
            endpoint_id: ControlEndpointId::try_from("codex-local".to_owned())
                .expect("approver endpoint"),
        };
        let target = ControlSessionRef {
            endpoint: provider_endpoint,
            session_id: ControlSessionId::try_from("session-one".to_owned()).expect("session"),
        };
        let approver = ControlSessionRef {
            endpoint: approver_endpoint.clone(),
            session_id: ControlSessionId::try_from("approver".to_owned()).expect("approver"),
        };
        let broker = ServiceInteractionBroker::load(
            service_id.clone(),
            NativeControlBackend {
                codex_home: root.path().to_owned(),
                endpoint: approver_endpoint,
                gate: NativeGenerationGate::default(),
            },
            root.path().join("approval-routes.json"),
        )
        .await
        .expect("broker");
        let store = ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("provider store");
        let hub = Arc::new(ProviderSessionEventHub::new(Arc::new(Mutex::new(store))));
        let endpoint = SessionEndpointRef {
            service_id: ServiceId::try_from(String::from(service_id)).expect("service"),
            endpoint_id: EndpointId::try_from("fixture-provider".to_owned()).expect("endpoint"),
        };
        let session = SessionRef {
            endpoint: endpoint.clone(),
            session_id: SessionId::try_from("session-one".to_owned()).expect("session"),
        };
        let sink = Arc::new(HubSessionEventSink::new(Arc::clone(&hub), endpoint));
        let port = Arc::new(crate::acp_interaction_port::HostInteractionPort::default());
        port.install_broker(Arc::clone(&broker)).await;
        port.install_event_sink(Arc::clone(&sink) as Arc<dyn SessionEventSink>)
            .await;
        sink.publish(
            "session-one",
            SessionEvent::TurnStarted {
                turn_id: "turn-one".into(),
                input_id: "input-one".into(),
            },
        )
        .expect("turn start");
        let request = serde_json::from_value(serde_json::json!({
            "requestId":"approval-one", "title":"Run command", "options":[
                {"optionId":"allow-once","label":"Allow once","choice":{"effect":"allow","scope":"once"}}
            ]
        }))
        .expect("approval request");
        let request_task = tokio::spawn({
            let port = Arc::clone(&port);
            let context = crate::ExternalProviderApprovalContext {
                requester: approver.clone(),
                approver: approver.clone(),
                target,
                operation_id: OperationId::generate(),
                binding_generation: CodexGeneration {
                    service_epoch: UuidIdentity::try_from(
                        "00000000-0000-4000-8000-000000000001".to_owned(),
                    )
                    .expect("epoch"),
                    generation: GenerationNumber::try_from(1).expect("generation"),
                },
                binding_retirement: tokio_util::sync::CancellationToken::new(),
            };
            async move {
                port.request_approval(
                    context,
                    request,
                    tokio_util::sync::CancellationToken::new(),
                    tokio_util::sync::CancellationToken::new(),
                )
                .await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Ok(attached) = hub.attach(session.clone()).await
                    && attached.snapshot.len() == 2
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("pending interaction published");
        let actor = message_board::Identity::Session {
            session: serde_json::from_value(serde_json::to_value(&approver).expect("actor JSON"))
                .expect("actor"),
        };
        broker
            .decide_typed_interaction("approval-one", &actor, "allow-once", false)
            .await
            .expect("approver decision");
        assert_eq!(
            request_task.await.expect("port task"),
            ApprovalPortOutcome::Selected {
                option_id: "allow-once".into(),
            }
        );
        sink.shutdown().await.expect("drain");
        let attached = hub.attach(session).await.expect("attach history");
        assert_eq!(attached.snapshot.len(), 3);
        assert!(matches!(
            &attached.snapshot[0].event,
            SessionEvent::TurnStarted { .. }
        ));
        assert!(matches!(
            &attached.snapshot[1].event,
            SessionEvent::InteractionRequested { .. }
        ));
        assert!(matches!(
            &attached.snapshot[2].event,
            SessionEvent::InteractionResolved { .. }
        ));
    }

    #[tokio::test]
    async fn publication_before_replay_reset_is_never_reintroduced_after_it() {
        let root = tempfile::tempdir().expect("temporary store");
        let store = ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("provider store");
        let hub = Arc::new(ProviderSessionEventHub::new(Arc::new(Mutex::new(store))));
        let endpoint = SessionEndpointRef {
            service_id: ServiceId::try_from("00000000-0000-4000-8000-000000000001".to_owned())
                .expect("service"),
            endpoint_id: EndpointId::try_from("fixture-provider".to_owned()).expect("endpoint"),
        };
        let session = SessionRef {
            endpoint: endpoint.clone(),
            session_id: SessionId::try_from("session-one".to_owned()).expect("session"),
        };
        let sink = HubSessionEventSink::new(Arc::clone(&hub), endpoint);
        sink.publish(
            "session-one",
            SessionEvent::TurnStarted {
                turn_id: "old-turn".into(),
                input_id: "old-input".into(),
            },
        )
        .expect("start old turn");
        sink.publish(
            "session-one",
            SessionEvent::TurnEnded {
                turn_id: "old-turn".into(),
                outcome: TurnOutcome::Lost {
                    reason: "providerRetired".into(),
                },
            },
        )
        .expect("publish loss");
        sink.begin_history_replay("session-one")
            .await
            .expect("reset after loss");
        sink.publish(
            "session-one",
            SessionEvent::TurnStarted {
                turn_id: "replayed-turn".into(),
                input_id: "replayed-input".into(),
            },
        )
        .expect("replayed start");
        sink.shutdown().await.expect("drained consumer");
        assert!(sink.begin_history_replay("session-one").await.is_err());
        let attached = hub.attach(session).await.expect("attach replay");
        assert_eq!(attached.snapshot.len(), 1);
        assert!(matches!(
            &attached.snapshot[0].event,
            SessionEvent::TurnStarted { turn_id, .. } if turn_id == "replayed-turn"
        ));
    }
}
