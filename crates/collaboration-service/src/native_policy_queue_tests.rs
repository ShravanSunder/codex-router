//! Real wake intent and reconciliation; native endpoint replies/schema are stand-ins.
use super::*;
use crate::{SessionDeliveryRouter, SessionMessageDelivery};
use agent_automation::{ExpiryRule, PreparationEffect, TimingRule};
use automation_storage::{AutomationStore, WakeCreate, WakeEvaluation};
use collaboration_protocol::{
    DeliveryClientReceipt, DeliveryOutcome, OperationId, RouterOriginRef, SavedMessage,
};

struct QueueObservation {
    intent: Option<PreparationEffect>,
    resume: PreparationEffect,
    receipt: Option<collaboration_protocol::DeliveryReceipt>,
}

#[derive(Clone, Copy)]
enum QueueProbeCase {
    AttachmentLost,
    MatchingItem,
    WrongItem,
    DuplicateItem,
    GenericRoute,
    GenericRegistersDuringNativeRead,
}
async fn queue_probe(case: QueueProbeCase) -> Result<QueueObservation, TestError> {
    let lose_attachment = matches!(case, QueueProbeCase::AttachmentLost);
    let reconcile = !lose_attachment;
    let fixture = if matches!(
        case,
        QueueProbeCase::GenericRoute | QueueProbeCase::GenericRegistersDuringNativeRead
    ) {
        RecordedPolicyFixture::create_without_route().await?
    } else {
        RecordedPolicyFixture::create().await?
    };
    let store = Arc::new(tokio::sync::Mutex::new(
        AutomationStore::open(&fixture.directory.path().join("automation.sqlite")).await?,
    ));
    let message: SavedMessage = serde_json::from_value(
        json!({"target":target(),"content":{"kind":"agent","sender":target(),"text":"Policy queue witness"},"delivery":"queue"}),
    )?;
    let wake = store
        .lock()
        .await
        .create_wakeup(&WakeCreate {
            operation_id: OperationId::generate(),
            message,
            timing: TimingRule::After { seconds: 1 },
            expiry: ExpiryRule::None,
            now_ms: 0,
        })
        .await?;
    let WakeEvaluation::Fired {
        delivery_id, fire, ..
    } = store
        .lock()
        .await
        .evaluate_wakeup::<SavedMessage>(
            &wake.definition.wakeup_id,
            1000,
            crate::wakeup_delivery_sender::build_wake_push_draft,
        )
        .await?
    else {
        return Err("wake not fired".into());
    };
    let push = store
        .lock()
        .await
        .get_push_record_by_origin_reference(&RouterOriginRef::Wake {
            wakeup_id: fire.wakeup_id,
            occurrence_id: fire.occurrence_id,
        })
        .await?
        .ok_or("push missing")?;
    let directory = crate::EndpointDirectory::new(fixture.backend.endpoint.service_id.clone());
    directory.publish(fixture.endpoint.clone())?;
    let route = Arc::new(
        crate::CodexAppServerDeliveryRoute::new(
            fixture.backend.endpoint.service_id.clone(),
            directory,
            fixture.backend.clone(),
            Arc::new(crate::UnmaterializedThreadHolder::new()),
        )
        .with_approval_broker(&fixture.broker),
    );
    let delivery: Arc<dyn SessionMessageDelivery> =
        Arc::new(SessionDeliveryRouter::new(vec![route]));
    let sender = crate::wakeup_delivery_sender::WakeDeliverySender {
        delivery: delivery.clone(),
        configuration: crate::AutomationConfigurationHandle::default(),
        machine_identity: crate::MachineIdentity::new(
            fixture.backend.endpoint.service_id.clone(),
            Some("Policy fixture"),
        )?,
    };
    let listener = tokio::net::UnixListener::bind(fixture.directory.path().join("native.sock"))?;
    let mutation_broker = fixture.broker.clone();
    let mutation_scratch = fixture
        .directory
        .path()
        .join("scratch")
        .to_string_lossy()
        .into_owned();
    let native_store = store.clone();
    let native_id = delivery_id.clone();
    let cwd = fixture.directory.path().to_string_lossy().into_owned();
    let roots = fixture
        .filesystem
        .iter()
        .filter(|(_, access)| *access == &json!("write"))
        .map(|(path, _)| path.clone())
        .collect::<Vec<_>>();
    let native = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        let mut intent = None;
        let mut queued = None;
        while let Some(Ok(Message::Text(text))) = socket.next().await {
            let request: Value = serde_json::from_str(&text)?;
            let method = request["method"].as_str().ok_or("method missing")?;
            let result = match method {
                "initialize" => json!({}),
                "initialized" => continue,
                "thread/read" => {
                    if matches!(case, QueueProbeCase::GenericRegistersDuringNativeRead) {
                        mutation_broker
                            .register_route(ApprovalRoute {
                                thread_id: THREAD.into(),
                                created_by: target(),
                                approver: target(),
                                access: collaboration_protocol::RouterAccess::WorkspaceWrite,
                                scratch_path: mutation_scratch.clone(),
                                root_message_id: None,
                            })
                            .await?;
                    }
                    json!({"thread":{"id":THREAD,"cwd":cwd,"status":{"type":"idle"}}})
                }
                "thread/resume" => {
                    let record=native_store.lock().await.read_delivery::<SessionRef,CodexGeneration,crate::stored_delivery_receipt::StoredDeliveryReceipt>(&native_id).await?;
                    let RouteEffectEvidence::CodexAppServer(effects) = record
                        .attempt
                        .ok_or("intent missing")?
                        .effects
                        .ok_or("effects missing")?
                    else {
                        return Err("native intent missing".into());
                    };
                    intent = Some(effects.resume);
                    if lose_attachment {
                        break;
                    }
                    json!({"thread":{"id":THREAD,"cwd":cwd},"cwd":cwd,"activePermissionProfile":{"id":"router-workspace-write"},"sandbox":{"type":"workspaceWrite","networkAccess":true,"writableRoots":roots}})
                }
                "thread/queue/add" => {
                    queued = Some(request.clone());
                    break;
                }
                _ => return Err("unexpected native mutation".into()),
            };
            socket
                .send(Message::Text(
                    json!({"id":request["id"],"result":result})
                        .to_string()
                        .into(),
                ))
                .await?;
        }
        drop(socket);
        if reconcile {
            let (stream, _) = listener.accept().await?;
            let mut socket = tokio_tungstenite::accept_async(stream).await?;
            while let Some(Ok(Message::Text(text))) = socket.next().await {
                let request: Value = serde_json::from_str(&text)?;
                let result = match request["method"].as_str() {
                    Some("initialize") => json!({}),
                    Some("initialized") => continue,
                    Some("thread/queue/list") => {
                        let queued = queued.as_ref().ok_or("queued request missing")?;
                        let mut item = json!({"id":"reconciled-input","clientUserMessageId":queued.pointer("/params/clientUserMessageId"),"input":queued.pointer("/params/input")});
                        if matches!(case, QueueProbeCase::WrongItem) {
                            item["clientUserMessageId"] = json!("unrelated-push");
                        }
                        let items = if matches!(case, QueueProbeCase::DuplicateItem) {
                            vec![item.clone(), item]
                        } else {
                            vec![item]
                        };
                        json!({"data":items,"nextCursor":null})
                    }
                    _ => return Err("reconciliation replayed input".into()),
                };
                socket
                    .send(Message::Text(
                        json!({"id":request["id"],"result":result})
                            .to_string()
                            .into(),
                    ))
                    .await?;
            }
        }
        Ok::<_, TestError>(intent)
    });
    tokio::time::timeout(
        Duration::from_secs(5),
        sender.dispatch(store.clone(), delivery_id.clone()),
    )
    .await??;
    let record=store.lock().await.read_delivery::<SessionRef,CodexGeneration,crate::stored_delivery_receipt::StoredDeliveryReceipt>(&delivery_id).await?;
    let RouteEffectEvidence::CodexAppServer(effects) = record
        .attempt
        .as_ref()
        .ok_or("attempt missing")?
        .effects
        .as_ref()
        .ok_or("effects missing")?
    else {
        return Err("wrong effects".into());
    };
    let resume = effects.resume;
    if effects.client_user_message_id.as_deref() != Some(push.push_id.as_str()) {
        return Err("wake correlation differs from stored push".into());
    }
    let receipt = if reconcile {
        crate::delivery_reconciliation::reconcile(&store, delivery.as_ref(), record).await?;
        let settled=store.lock().await.read_delivery::<SessionRef,CodexGeneration,crate::stored_delivery_receipt::StoredDeliveryReceipt>(&delivery_id).await?;
        settled
            .receipt
            .map(crate::stored_delivery_receipt::StoredDeliveryReceipt::into_public)
    } else {
        None
    };
    if matches!(
        case,
        QueueProbeCase::MatchingItem
            | QueueProbeCase::GenericRoute
            | QueueProbeCase::GenericRegistersDuringNativeRead
    ) && !matches!(
        receipt.as_ref().map(|receipt| &receipt.outcome),
        Some(DeliveryOutcome::Queued)
    ) {
        native.abort();
        let _ = native.await;
        return Ok(QueueObservation {
            intent: None,
            resume,
            receipt,
        });
    }
    let intent = tokio::time::timeout(Duration::from_secs(5), native).await???;
    Ok(QueueObservation {
        intent,
        resume,
        receipt,
    })
}

#[tokio::test]
async fn policy_queue_attachment_intent_causal() {
    let observed = queue_probe(QueueProbeCase::AttachmentLost).await.unwrap();
    assert_eq!(
        observed.intent,
        Some(PreparationEffect::Unknown),
        "possible resume must be durable before attachment"
    );
    assert_eq!(observed.resume, PreparationEffect::Unknown);
}

#[tokio::test]
async fn policy_queue_accepted_resume_reconciliation_causal() {
    let observed = queue_probe(QueueProbeCase::MatchingItem).await.unwrap();
    assert_eq!(observed.resume, PreparationEffect::Accepted);
    let Some(receipt) = observed.receipt else {
        panic!("exact queue item was not reconciled");
    };
    assert!(
        matches!(receipt.outcome, DeliveryOutcome::Queued),
        "exact queue item must resolve accepted-resume submission"
    );
    let Some(DeliveryClientReceipt::CodexAppServer(client)) = receipt.client else {
        panic!("client receipt missing");
    };
    assert_eq!(
        client.resume_effect,
        collaboration_protocol::AcceptedResumeEffect::Accepted
    );
}

#[tokio::test]
async fn policy_queue_wrong_or_duplicate_item_stays_uncertain() {
    for case in [QueueProbeCase::WrongItem, QueueProbeCase::DuplicateItem] {
        let observed = queue_probe(case).await.unwrap();
        assert_eq!(observed.resume, PreparationEffect::Accepted);
        assert!(matches!(
            observed.receipt.unwrap().outcome,
            DeliveryOutcome::Unknown
        ));
    }
}
#[tokio::test]
async fn policy_queue_generic_route_reconciles_without_inventing_resume() {
    let observed = queue_probe(QueueProbeCase::GenericRoute).await.unwrap();
    assert_eq!(observed.resume, PreparationEffect::NotRequested);
    let receipt = observed.receipt.unwrap();
    assert!(matches!(receipt.outcome, DeliveryOutcome::Queued));
    let Some(DeliveryClientReceipt::CodexAppServer(client)) = receipt.client else {
        panic!("client receipt missing");
    };
    assert_eq!(
        client.resume_effect,
        collaboration_protocol::AcceptedResumeEffect::NotRequested
    );
}
#[tokio::test]
async fn policy_queue_invalid_effects_never_inspect_or_accept_a_queue_item() {
    let fixture = RecordedPolicyFixture::create().await.unwrap();
    let listener =
        tokio::net::UnixListener::bind(fixture.directory.path().join("native.sock")).unwrap();
    let push: collaboration_protocol::PushId = OperationId::generate()
        .as_str()
        .to_owned()
        .try_into()
        .unwrap();
    for (resume, allocation, wrong_correlation, wrong_target, missing_generation) in [
        (
            PreparationEffect::Unknown,
            PreparationEffect::NotRequested,
            false,
            false,
            false,
        ),
        (
            PreparationEffect::Rejected,
            PreparationEffect::NotRequested,
            false,
            false,
            false,
        ),
        (
            PreparationEffect::NotDispatched,
            PreparationEffect::NotRequested,
            false,
            false,
            false,
        ),
        (
            PreparationEffect::Accepted,
            PreparationEffect::Accepted,
            false,
            false,
            false,
        ),
        (
            PreparationEffect::Accepted,
            PreparationEffect::NotRequested,
            true,
            false,
            false,
        ),
        (
            PreparationEffect::Accepted,
            PreparationEffect::NotRequested,
            false,
            true,
            false,
        ),
        (
            PreparationEffect::Accepted,
            PreparationEffect::NotRequested,
            false,
            false,
            true,
        ),
    ] {
        let RouteEffectEvidence::CodexAppServer(mut effects) = fixture.scheduled_run().recorded
        else {
            panic!("native fixture expected");
        };
        effects.resume = resume;
        effects.allocation = allocation;
        effects.client_user_message_id = Some(if wrong_correlation {
            "unrelated-push".into()
        } else {
            push.as_str().into()
        });
        if wrong_target {
            effects.target = Some(SessionRef {
                endpoint: target().endpoint,
                session_id: "unrelated-thread".to_owned().try_into().unwrap(),
            });
        }
        if missing_generation {
            effects.generation = None;
        }
        let context = crate::AttemptReconciliationContext {
            target: target(),
            prepared_push_id: push.clone(),
            mode: MessageDelivery::Queue,
            recorded: effects.into(),
        };
        let observed = tokio::select! {
            biased;
            _=listener.accept()=>panic!("invalid evidence reached queue inspection"),
            result=crate::codex_queue_reconciliation::reconcile(&fixture.backend,context)=>result,
        };
        assert!(matches!(
            observed,
            crate::AttemptReconciliation::StillUnknown
        ));
    }
}

#[tokio::test]
async fn policy_generic_membership_is_pinned_before_intent_and_native_io() {
    let observed = queue_probe(QueueProbeCase::GenericRegistersDuringNativeRead)
        .await
        .unwrap();
    assert!(
        observed.intent.is_none(),
        "generic attempt reclassified and resumed after native read"
    );
    assert_eq!(observed.resume, PreparationEffect::NotRequested);
    let receipt = observed.receipt.unwrap();
    assert!(matches!(receipt.outcome, DeliveryOutcome::Queued));
    let Some(DeliveryClientReceipt::CodexAppServer(client)) = receipt.client else {
        panic!("client receipt missing");
    };
    assert_eq!(
        client.resume_effect,
        collaboration_protocol::AcceptedResumeEffect::NotRequested
    );
}
