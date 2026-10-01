#![allow(clippy::expect_used)]

use automation_storage::{AutomationStore, PushDeliveryState};
use collaboration_protocol::{
    DeliveryClientReceipt, DeliveryOutcome, DeliveryReceipt, EndpointId, EndpointRef,
    MessageContent, MessageDelivery, PushActivityRange, PushActivitySnapshot, PushHeaderFacts,
    PushId, PushKind, PushOrigin, PushRecordDraft, RouterOriginRef, SessionId,
    SessionMessageSendParams, SessionRef, UuidIdentity,
};
use collaboration_service::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext,
    DeliveryContractError, DeliveryFuture, DeliveryPrecondition, ServiceIdentity,
    SessionMessageDelivery, serve_control_connection,
};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    net::{
        UnixStream,
        unix::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::Mutex as TokioMutex,
    task::JoinHandle,
};

const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";

struct RecordingDelivery {
    requests: Mutex<Vec<collaboration_service::layer_zero::DeliveryRequest>>,
    store: Arc<TokioMutex<AutomationStore>>,
    stored_before_delivery: AtomicBool,
    planned_outcome: Mutex<Option<DeliveryOutcome>>,
}

impl RecordingDelivery {
    fn new(store: Arc<TokioMutex<AutomationStore>>) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            store,
            stored_before_delivery: AtomicBool::new(false),
            planned_outcome: Mutex::new(None),
        }
    }

    fn set_outcome(&self, outcome: DeliveryOutcome) {
        *self.planned_outcome.lock().expect("planned outcome") = Some(outcome);
    }
}

impl SessionMessageDelivery for RecordingDelivery {
    fn deliver<'a>(
        &'a self,
        request: collaboration_service::layer_zero::DeliveryRequest,
        _: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt> {
        Box::pin(async move {
            let record = self
                .store
                .lock()
                .await
                .get_push_record(&request.payload.push_id)
                .await
                .map_err(|_| DeliveryContractError::ClientOperation)?;
            if record.is_some_and(|record| record.delivery_state == PushDeliveryState::Attempted) {
                self.stored_before_delivery.store(true, Ordering::SeqCst);
            }
            self.requests
                .lock()
                .map_err(|_| DeliveryContractError::ClientOperation)?
                .push(request);
            Ok(DeliveryReceipt {
                outcome: self
                    .planned_outcome
                    .lock()
                    .map_err(|_| DeliveryContractError::ClientOperation)?
                    .clone()
                    .unwrap_or(DeliveryOutcome::PeerMessageWritten),
                reachability: Some(collaboration_protocol::SessionReachability::ClaudeCodePeer),
                client: Some(DeliveryClientReceipt::ClaudeCodePeer),
            })
        })
    }

    fn reconcile_attempt(
        &self,
        _: AttemptReconciliationContext,
    ) -> DeliveryFuture<'_, AttemptReconciliation> {
        Box::pin(async { Ok(AttemptReconciliation::StillUnknown) })
    }
}

struct ControlHarness {
    writer: OwnedWriteHalf,
    responses: Lines<BufReader<OwnedReadHalf>>,
    service: JoinHandle<Result<(), String>>,
    next_id: u64,
    owner: Option<collaboration_service::SubscriptionDeliveryService>,
}

impl ControlHarness {
    async fn start(identity: ServiceIdentity) -> Self {
        let (client, server) = UnixStream::pair().expect("control stream pair");
        let service = tokio::spawn(async move {
            serve_control_connection(server, identity)
                .await
                .map_err(|error| error.to_string())
        });
        let (reader, writer) = client.into_split();
        let mut harness = Self {
            writer,
            responses: BufReader::new(reader).lines(),
            service,
            next_id: 1,
            owner: None,
        };
        let initialized = harness
            .call(
                "control/initialize",
                json!({
                    "version":{"major":1,"minor":0},
                    "client":{"name":"push-surface-test","version":"1"}
                }),
            )
            .await;
        assert!(initialized.get("result").is_some(), "{initialized}");
        harness
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id.to_string();
        self.next_id += 1;
        let request = json!({
            "jsonrpc":"2.0",
            "id":id,
            "method":method,
            "params":params
        });
        self.writer
            .write_all(format!("{request}\n").as_bytes())
            .await
            .expect("write control request");
        loop {
            let line = self
                .responses
                .next_line()
                .await
                .expect("read control response")
                .expect("control response line");
            let response: Value = serde_json::from_str(&line).expect("control response JSON");
            if response.get("id") == Some(&json!(id)) {
                return response;
            }
        }
    }

    async fn close(self) {
        let Self {
            mut writer,
            responses: _,
            service,
            next_id: _,
            owner,
        } = self;
        writer.shutdown().await.expect("close control client");
        service.await.expect("service task").expect("service exit");
        if let Some(owner) = owner {
            owner.shutdown().await;
        }
    }
}

fn session(endpoint_id: &str, session_id: &str) -> SessionRef {
    SessionRef {
        endpoint: EndpointRef {
            service_id: UuidIdentity::try_from(SERVICE_ID.to_owned()).expect("service id"),
            endpoint_id: EndpointId::try_from(endpoint_id.to_owned()).expect("endpoint id"),
        },
        session_id: SessionId::try_from(session_id.to_owned()).expect("session id"),
    }
}

async fn setup() -> (
    tempfile::TempDir,
    Arc<TokioMutex<AutomationStore>>,
    Arc<RecordingDelivery>,
    ControlHarness,
) {
    let directory = tempfile::tempdir().expect("isolated push store");
    let store = Arc::new(TokioMutex::new(
        AutomationStore::open(&directory.path().join("automation.sqlite"))
            .await
            .expect("automation store"),
    ));
    let delivery = Arc::new(RecordingDelivery::new(Arc::clone(&store)));
    let identity = ServiceIdentity::new(
        SERVICE_ID,
        SERVICE_ID,
        &format!("sha256:{}", "a".repeat(64)),
    )
    .expect("service identity")
    .with_automation_store(Arc::clone(&store))
    .with_session_delivery(delivery.clone());
    let presence = Arc::new(RunningPresence);
    let owner = collaboration_service::SubscriptionDeliveryService::new(
        collaboration_service::SubscriptionDeliveryServiceProps {
            board_availability: collaboration_service::BoardAvailability::Unavailable,
            push_store: Arc::clone(&store),
            delivery: delivery.clone(),
            presence: presence.clone(),
            machine_identity: collaboration_service::MachineIdentity::new(
                UuidIdentity::try_from(SERVICE_ID.to_owned()).expect("service UUID"),
                Some("push-surface-test"),
            )
            .expect("machine identity"),
            clock: Arc::new(collaboration_service::SystemSubscriptionClock),
        },
    );
    owner.start().await.expect("single owner starts");
    let identity = identity.with_subscription_delivery_service(owner.clone(), presence);
    let mut control = ControlHarness::start(identity).await;
    control.owner = Some(owner);
    (directory, store, delivery, control)
}

async fn setup_without_automation_store() -> (
    tempfile::TempDir,
    Arc<TokioMutex<AutomationStore>>,
    Arc<RecordingDelivery>,
    ControlHarness,
) {
    let directory = tempfile::tempdir().expect("isolated unavailable-store fixture");
    let store = Arc::new(TokioMutex::new(
        AutomationStore::open(&directory.path().join("automation.sqlite"))
            .await
            .expect("stand-in database"),
    ));
    let delivery = Arc::new(RecordingDelivery::new(Arc::clone(&store)));
    let identity = ServiceIdentity::new(
        SERVICE_ID,
        SERVICE_ID,
        &format!("sha256:{}", "a".repeat(64)),
    )
    .expect("service identity")
    .with_session_delivery(delivery.clone());
    let control = ControlHarness::start(identity).await;
    (directory, store, delivery, control)
}

fn send_params(target: &SessionRef, sender: &SessionRef, text: &str) -> Value {
    serde_json::to_value(SessionMessageSendParams {
        target: target.clone(),
        message: MessageContent::Agent {
            sender: sender.clone(),
            text: text.to_owned().try_into().expect("message text"),
        },
        mode: MessageDelivery::Auto,
        generation_guard: None,
    })
    .expect("message request JSON")
}

#[tokio::test]
async fn message_send_persists_before_delivery_and_submits_one_linked_line() {
    let (_directory, store, delivery, mut control) = setup().await;
    let sender = session("claude-local", "sender-session");
    let target = session("codex-local", "target-session");

    let response = control
        .call(
            "message/send",
            send_params(&target, &sender, "snapshot body"),
        )
        .await;

    let result = response.get("result").expect("message send accepted");
    assert_eq!(result["receipt"]["outcome"]["kind"], "peerMessageWritten");
    let push_id = PushId::try_from(
        result["pushId"]
            .as_str()
            .expect("push id in delivery result")
            .to_owned(),
    )
    .expect("valid push id");
    assert_eq!(
        result["link"],
        format!("router://{SERVICE_ID}/push/{}", push_id.as_str())
    );
    assert!(delivery.stored_before_delivery.load(Ordering::SeqCst));

    let stored = store
        .lock()
        .await
        .get_push_record(&push_id)
        .await
        .expect("read stored message")
        .expect("message snapshot exists");
    assert_eq!(stored.body.as_deref(), Some("snapshot body"));
    assert_eq!(stored.origin, PushOrigin::Session(sender));
    assert_eq!(stored.delivery_state, PushDeliveryState::Delivered);

    {
        let requests = delivery
            .requests
            .lock()
            .expect("prepared delivery requests");
        assert_eq!(requests.len(), 1);
        let prepared = &requests[0];
        assert_eq!(prepared.payload.push_id, push_id);
        assert_eq!(
            prepared.payload.load_policy,
            collaboration_service::LoadPolicy::LoadedOnly
        );
        assert_eq!(prepared.correlation.as_str(), push_id.as_str());
        let text = &prepared.payload.line;
        assert_eq!(prepared.target, target);
        assert_eq!(text.as_str().lines().count(), 1);
        assert!(text.as_str().contains("snapshot body"));
        assert!(
            text.as_str()
                .contains(&result["link"].as_str().expect("link").to_owned())
        );
        assert!(!text.as_str().contains("Agent communication"));
        assert!(!text.as_str().contains("Self-declared sender:"));
    }
    control.close().await;
}

#[tokio::test]
async fn show_returns_stored_rejected_and_unknown_delivery_outcomes() {
    for (outcome, expected_state, expected_kind, expected_claims) in [
        (
            DeliveryOutcome::Rejected(collaboration_protocol::DeliveryRejection {
                reason: collaboration_protocol::DeliveryRejectionReason::LiveElsewhere,
                next_action: collaboration_protocol::DeliveryNextAction::InspectTarget,
                client_code: None,
                detail: Some("two terminals claim this session".to_owned()),
                claims: Some(vec![
                    collaboration_protocol::DeliveryPeerClaim {
                        pid: 52304,
                        name: Some("terminal-one".to_owned()),
                        cwd: Some("/workspace/one".to_owned()),
                    },
                    collaboration_protocol::DeliveryPeerClaim {
                        pid: 68833,
                        name: None,
                        cwd: None,
                    },
                ]),
            }),
            "rejected",
            "rejected",
            Some(json!([
                {"pid":52304,"name":"terminal-one","cwd":"/workspace/one"},
                {"pid":68833,"name":null,"cwd":null}
            ])),
        ),
        (DeliveryOutcome::Unknown, "outcome-unknown", "unknown", None),
    ] {
        let (_directory, _store, delivery, mut control) = setup().await;
        delivery.set_outcome(outcome);
        let sender = session("claude-local", "sender-session");
        let target = session("codex-local", "target-session");

        let sent = control
            .call(
                "message/send",
                send_params(&target, &sender, "stored outcome"),
            )
            .await;
        let result = sent.get("result").expect("send returns typed receipt");
        let link = result["link"].as_str().expect("push link").to_owned();
        assert_eq!(
            result
                .pointer("/receipt/outcome/kind")
                .and_then(Value::as_str),
            Some(expected_kind)
        );
        if let Some(expected_claims) = expected_claims.as_ref() {
            assert_eq!(
                result.pointer("/receipt/outcome/claims"),
                Some(expected_claims)
            );
        }

        let shown = control
            .call("router/show", json!({"caller":target,"reference":link}))
            .await;
        assert_eq!(
            shown
                .pointer("/result/record/deliveryState")
                .and_then(Value::as_str),
            Some(expected_state)
        );
        assert_eq!(
            shown
                .pointer("/result/record/lastOutcome/outcome/kind")
                .and_then(Value::as_str),
            Some(expected_kind)
        );
        if expected_kind == "rejected" {
            assert_eq!(
                shown
                    .pointer("/result/record/lastOutcome/outcome/detail")
                    .and_then(Value::as_str),
                Some("two terminals claim this session")
            );
            assert_eq!(
                shown.pointer("/result/record/lastOutcome/outcome/claims"),
                expected_claims.as_ref()
            );
        }
        control.close().await;
    }
}

#[tokio::test]
async fn router_show_enforces_participants_and_marks_read_only_for_the_target() {
    let (_directory, store, _delivery, mut control) = setup().await;
    let sender = session("claude-local", "sender-session");
    let target = session("codex-local", "target-session");
    let third_party = session("cursor-local", "third-party");
    let response = control
        .call(
            "message/send",
            send_params(&target, &sender, "private body"),
        )
        .await;
    let result = response.get("result").expect("message send accepted");
    let push_id = result["pushId"].as_str().expect("push id").to_owned();
    let link = result["link"].as_str().expect("push link").to_owned();

    let inbox_before_read = control
        .call("message/inbox", json!({"caller":target.clone(),"limit":50}))
        .await;
    assert_eq!(
        inbox_before_read["result"]["records"]
            .as_array()
            .expect("inbox records")
            .len(),
        1
    );
    assert!(
        inbox_before_read["result"]["records"][0]["line"]
            .as_str()
            .expect("one-line notice")
            .contains(&link)
    );
    let history = control
        .call(
            "message/history",
            json!({"caller":target.clone(),"with":sender.clone(),"limit":50}),
        )
        .await;
    assert_eq!(
        history["result"]["records"]
            .as_array()
            .expect("history")
            .len(),
        1
    );

    let sender_show = control
        .call("router/show", json!({"caller":sender,"reference":link}))
        .await;
    assert_eq!(sender_show["result"]["record"]["body"], "private body");
    assert!(sender_show["result"]["record"]["readAt"].is_null());
    assert!(
        store
            .lock()
            .await
            .get_push_record(&PushId::try_from(push_id.clone()).expect("push id"))
            .await
            .expect("read sender view")
            .expect("record exists")
            .read_at
            .is_none()
    );

    let denied = control
        .call(
            "router/show",
            json!({"caller":third_party,"reference":link}),
        )
        .await;
    assert_eq!(denied["error"]["data"]["kind"], "notPermitted");

    let target_show = control
        .call(
            "router/show",
            json!({"caller":target.clone(),"reference":push_id}),
        )
        .await;
    assert_eq!(target_show["result"]["record"]["body"], "private body");
    assert!(!target_show["result"]["record"]["readAt"].is_null());

    let inbox = control
        .call("message/inbox", json!({"caller":target,"limit":50}))
        .await;
    assert_eq!(
        inbox["result"]["records"]
            .as_array()
            .expect("inbox records")
            .len(),
        0
    );
    control.close().await;
}

#[tokio::test]
async fn missing_push_store_rejects_without_attempting_delivery() {
    let (_directory, stand_in_store, delivery, mut control) =
        setup_without_automation_store().await;
    let sender = session("claude-local", "sender-session");
    let target = session("codex-local", "target-session");

    let response = control
        .call(
            "message/send",
            send_params(&target, &sender, "must not send"),
        )
        .await;

    assert_eq!(response["error"]["data"]["kind"], "unavailable");
    assert!(
        delivery
            .requests
            .lock()
            .expect("delivery requests")
            .is_empty()
    );
    assert!(
        stand_in_store
            .lock()
            .await
            .list_direct_message_inbox(&automation_storage::PushInboxQuery { target, limit: 10 })
            .await
            .expect("read stand-in store")
            .is_empty()
    );
    control.close().await;
}

#[tokio::test]
async fn unverified_owner_message_is_compact_and_has_no_owner_show_bypass() {
    let (_directory, store, delivery, mut control) = setup().await;
    let target = session("codex-local", "target-session");
    let third_party = session("cursor-local", "third-party");
    let response = control
        .call(
            "message/send",
            serde_json::to_value(SessionMessageSendParams {
                target: target.clone(),
                message: MessageContent::HumanUser {
                    text: "human text".to_owned().try_into().expect("message text"),
                },
                mode: MessageDelivery::Auto,
                generation_guard: None,
            })
            .expect("human request JSON"),
        )
        .await;
    let result = response.get("result").expect("owner message accepted");
    let push_id = PushId::try_from(result["pushId"].as_str().expect("push id").to_owned())
        .expect("valid push id");
    let record = store
        .lock()
        .await
        .get_push_record(&push_id)
        .await
        .expect("read stored owner message")
        .expect("owner message record");
    assert_eq!(record.origin, PushOrigin::OwnerUnverified);
    assert_eq!(result["receipt"]["outcome"]["kind"], "peerMessageWritten");
    {
        let requests = delivery
            .requests
            .lock()
            .expect("prepared delivery requests");
        let delivered = &requests[0];
        assert_eq!(delivered.target, target);
        assert_eq!(delivered.mode, MessageDelivery::Auto);
        assert!(matches!(
            delivered.precondition,
            DeliveryPrecondition::Unpinned
        ));
        assert_eq!(delivered.correlation.as_str(), push_id.as_str());
        assert_eq!(delivered.payload.push_id, push_id);
        let text = &delivered.payload.line;
        assert!(text.as_str().starts_with("🧑 Owner (unverified)"));
        assert!(text.as_str().contains("\"human text\""));
        assert!(
            text.as_str()
                .contains(&format!("router://{SERVICE_ID}/push/{}", push_id.as_str()))
        );
        assert!(!text.as_str().contains("Self-declared sender:"));
    }

    let denied = control
        .call(
            "router/show",
            json!({"caller":third_party,"reference":push_id.as_str()}),
        )
        .await;
    assert_eq!(denied["error"]["data"]["kind"], "notPermitted");
    let owner_reply = control
        .call(
            "message/reply",
            json!({"caller":target,"reference":push_id.as_str(),"text":"answer"}),
        )
        .await;
    assert_eq!(
        owner_reply["error"]["data"]["kind"],
        "ownerReplyUnsupported"
    );
    control.close().await;
}

#[tokio::test]
async fn foreign_and_missing_links_have_distinct_errors() {
    let (_directory, _store, _delivery, mut control) = setup().await;
    let caller = session("codex-local", "caller");
    let push_id = uuid::Uuid::now_v7().to_string();
    let foreign = control
        .call(
            "router/show",
            json!({
                "caller":caller.clone(),
                "reference":format!("router://11111111-1111-7111-8111-111111111111/push/{push_id}")
            }),
        )
        .await;
    assert_eq!(foreign["error"]["data"]["kind"], "foreignMachine");
    assert!(
        foreign["error"]["message"]
            .as_str()
            .expect("foreign message")
            .contains("cross-machine fetch not available yet")
    );

    let missing = control
        .call("router/show", json!({"caller":caller,"reference":push_id}))
        .await;
    assert_eq!(missing["error"]["data"]["kind"], "notFound");
    assert!(
        missing["error"]["message"]
            .as_str()
            .expect("not-found message")
            .contains("expired after 30 days")
    );
    control.close().await;
}

#[tokio::test]
async fn reply_rejects_missing_reference_and_non_dm_records() {
    let (_directory, store, _delivery, mut control) = setup().await;
    let target = session("codex-local", "target-session");
    let missing_reference = control
        .call("message/reply", json!({"caller":target,"text":"answer"}))
        .await;
    assert_eq!(missing_reference["error"]["data"]["kind"], "invalidField");

    let range = PushActivityRange {
        root_message_id: message_board::MessageId::generate(),
        from_activity_sequence: message_board::ActivitySequence::ZERO,
        through_activity_sequence: message_board::ActivitySequence::ZERO,
    };
    let non_dm = PushRecordDraft {
        mode: None,
        guard: None,
        push_id: PushId::try_from(uuid::Uuid::now_v7().to_string()).expect("push id"),
        kind: PushKind::SubscriptionActivity,
        origin: PushOrigin::Router(PushKind::SubscriptionActivity),
        origin_router_ref: Some(
            RouterOriginRef::SubscriptionActivity {
                target: target.clone(),
                batch_id: message_board::BatchId::generate(),
            }
            .canonical_string()
            .expect("canonical subscription batch origin"),
        ),
        target: target.clone(),
        reply_to_push_id: None,
        header_facts: PushHeaderFacts::SubscriptionActivity {
            root_count: 1,
            message_count: 0,
            held_since: None,
            thread_resolved: false,
        },
        body: None,
        activity: Some(PushActivitySnapshot {
            ranges: vec![range],
            held: false,
            draining: false,
        }),
        created_at: chrono::Utc::now(),
    };
    let record = store
        .lock()
        .await
        .insert_push_record(non_dm)
        .await
        .expect("store activity record");
    let response = control
        .call(
            "message/reply",
            json!({"caller":target,"reference":record.push_id.as_str(),"text":"answer"}),
        )
        .await;
    assert_eq!(response["error"]["data"]["kind"], "notDirectMessage");
    control.close().await;
}

#[tokio::test]
async fn reply_uses_the_selected_message_id_after_a_later_dm_arrives() {
    let (_directory, store, delivery, mut control) = setup().await;
    let target = session("codex-local", "recipient-session");
    let first_sender = session("claude-local", "first-sender");
    let second_sender = session("cursor-local", "second-sender");
    let first = control
        .call("message/send", send_params(&target, &first_sender, "first"))
        .await;
    let first_id = first["result"]["pushId"]
        .as_str()
        .expect("first id")
        .to_owned();
    control
        .call(
            "message/send",
            send_params(&target, &second_sender, "second"),
        )
        .await;

    let reply = control
        .call(
            "message/reply",
            json!({"caller":target,"reference":first_id,"text":"reply to first"}),
        )
        .await;

    assert!(reply.get("result").is_some(), "reply response: {reply}");
    assert_eq!(
        reply["result"]["target"],
        serde_json::to_value(&first_sender).expect("target JSON")
    );
    let reply_id = PushId::try_from(
        reply["result"]["pushId"]
            .as_str()
            .expect("reply push id")
            .to_owned(),
    )
    .expect("reply id");
    let stored_reply = store
        .lock()
        .await
        .get_push_record(&reply_id)
        .await
        .expect("read reply")
        .expect("reply is stored");
    assert_eq!(
        stored_reply.reply_to_push_id.as_ref().map(PushId::as_str),
        Some(first_id.as_str())
    );
    assert_eq!(stored_reply.origin, PushOrigin::Session(target));
    {
        let requests = delivery
            .requests
            .lock()
            .expect("prepared delivery requests");
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[2].target, first_sender);
        assert_eq!(requests[2].payload.push_id, reply_id);
        let text = &requests[2].payload.line;
        assert!(text.as_str().contains("reply to first"));
    }
    control.close().await;
}

struct RunningPresence;
impl collaboration_service::TargetPresenceProbe for RunningPresence {
    fn presence(
        &self,
        _: &SessionRef,
    ) -> DeliveryFuture<'_, collaboration_service::TargetPresence> {
        Box::pin(async { Ok(collaboration_service::TargetPresence::Running) })
    }
}
