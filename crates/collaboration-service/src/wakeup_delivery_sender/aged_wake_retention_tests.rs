//! An aged durable wake can fire again after its previous push is physically pruned.
use super::{WakeDeliverySender, build_wake_push_draft};
use crate::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext,
    DeliveryContractError, DeliveryFuture, MachineIdentity, ServiceIdentity,
    SessionMessageDelivery, serve_control_connection,
};
use agent_automation::{
    CessationEvidence, ExpiryRule, NativeEffectEvidence, PreparationEffect, RouteEffectEvidence,
    SubmissionEffect, TimingRule, WakeState,
};
use automation_storage::{AutomationStore, WakeCreate, WakeEvaluation};
use collaboration_client::{ClientError, ControlClient};
use collaboration_protocol::{
    CodexGeneration, DeliveryOutcome, DeliveryReceipt, MachineId, OperationId, PushDeliveryState,
    PushHeaderFacts, PushKind, PushLineInput, PushOrigin, PushRecord, PushRecordShowParams,
    RouterLink, RouterOriginRef, SavedMessage, SessionReachability, SessionRef, UuidIdentity,
    render_push_line,
};
use serde_json::json;
use std::{error::Error, sync::Arc, time::Duration};
use tokio::sync::Mutex;

type TestResult<TValue = ()> = Result<TValue, Box<dyn Error + Send + Sync>>;
const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
const INTERVAL_SECONDS: u32 = 31 * 24 * 60 * 60;
const WAKE_BODY: &str = "Report the same durable wake's new firing";

fn ensure(condition: bool, detail: &'static str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(detail.into())
    }
}

struct StoredWakeRoute {
    store: Arc<Mutex<AutomationStore>>,
    observed: Mutex<Vec<crate::layer_zero::DeliveryRequest>>,
    generation: CodexGeneration,
}

impl SessionMessageDelivery for StoredWakeRoute {
    fn deliver<'a>(
        &'a self,
        request: crate::layer_zero::DeliveryRequest,
        sink: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt> {
        Box::pin(async move {
            let stored = self
                .store
                .lock()
                .await
                .get_push_record(&request.payload.push_id)
                .await
                .map_err(|_| DeliveryContractError::InvalidEvidence)?;
            if !stored.is_some_and(|record| {
                record.delivery_state == PushDeliveryState::Attempted
                    && record.target == request.target
                    && record.body.as_deref() == Some(WAKE_BODY)
            }) {
                return Err(DeliveryContractError::InvalidEvidence);
            }
            let mut effects = NativeEffectEvidence {
                target: Some(request.target.clone()),
                generation: Some(self.generation.clone()),
                client_user_message_id: Some(request.correlation.as_str().to_owned()),
                native_turn_id: None,
                native_submission_id: None,
                allocation: PreparationEffect::NotRequested,
                resume: PreparationEffect::NotRequested,
                submission: SubmissionEffect::Dispatching,
                cessation: CessationEvidence::NotApplicable,
            };
            sink.record(RouteEffectEvidence::CodexAppServer(effects.clone()))
                .await?;
            effects.submission = SubmissionEffect::Accepted;
            effects.native_turn_id = Some(format!("turn-{}", request.payload.push_id.as_str()));
            sink.record(RouteEffectEvidence::CodexAppServer(effects))
                .await?;
            self.observed.lock().await.push(request);
            Ok(DeliveryReceipt {
                outcome: DeliveryOutcome::Started,
                reachability: Some(SessionReachability::CodexAppServer),
                client: None,
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

async fn require_expired_link(
    identity: &ServiceIdentity,
    target: &SessionRef,
    link: &str,
) -> TestResult {
    // A rejected Control call retires its client connection. Each old-link
    // lookup therefore uses a separate real connection from the fresh lookup.
    let (socket, server) = tokio::net::UnixStream::pair()?;
    let control_task = tokio::spawn(serve_control_connection(server, identity.clone()));
    let mut client = ControlClient::initialize(socket, "expired-wake-link-proof", "1").await?;
    let result = client
        .router_show(PushRecordShowParams {
            caller: target.clone(),
            reference: link.to_owned(),
        })
        .await;
    let valid_rejection = ensure(
        matches!(result, Err(ClientError::Rejected { code: -32050, data: Some(data) })
        if data.get("kind").and_then(serde_json::Value::as_str) == Some("notFound")),
        "pruned wake push link must be notFound through real Control",
    );
    client.close().await?;
    tokio::time::timeout(Duration::from_secs(5), control_task).await???;
    valid_rejection
}

async fn evaluate_and_dispatch(
    store: &Arc<Mutex<AutomationStore>>,
    sender: &WakeDeliverySender,
    wakeup_id: &agent_automation::WakeupId,
    now_ms: i64,
) -> TestResult<(PushRecord, agent_automation::OccurrenceId)> {
    let WakeEvaluation::Fired {
        fire, delivery_id, ..
    } = store
        .lock()
        .await
        .evaluate_wakeup::<SavedMessage>(wakeup_id, now_ms, build_wake_push_draft)
        .await?
    else {
        return Err("same recurring definition must create a new actual firing".into());
    };
    let origin = RouterOriginRef::Wake {
        wakeup_id: fire.wakeup_id,
        occurrence_id: fire.occurrence_id.clone(),
    };
    sender.dispatch(Arc::clone(store), delivery_id).await?;
    let record = store
        .lock()
        .await
        .get_push_record_by_origin_reference(&origin)
        .await?
        .ok_or("actual firing did not store its typed push")?;
    ensure(
        record.delivery_state == PushDeliveryState::Delivered,
        "actual wake sender must settle its route acceptance",
    )?;
    Ok((record, fire.occurrence_id))
}

#[tokio::test]
async fn aged_wake_definition_fires_fresh_push_after_old_link_is_pruned() -> TestResult {
    let root = tempfile::tempdir()?;
    let store = Arc::new(Mutex::new(
        AutomationStore::open(&root.path().join("automation.sqlite")).await?,
    ));
    let service_id = UuidIdentity::try_from(SERVICE_ID.to_owned())?;
    let machine = MachineIdentity::new(service_id.clone(), Some("aged-wake-fixture"))?;
    let message: SavedMessage = serde_json::from_value(json!({
        "target":{"endpoint":{"serviceId":SERVICE_ID,"endpointId":"codex-local"},"sessionId":"aged-wake-target"},
        "content":{"kind":"humanUser","text":WAKE_BODY},"delivery":"auto",
    }))?;
    let target = message.target.clone();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let interval_ms = i64::from(INTERVAL_SECONDS) * 1000;
    let anchor_ms = now_ms - 2 * interval_ms;
    let wake = store
        .lock()
        .await
        .create_wakeup(&WakeCreate {
            operation_id: OperationId::generate(),
            message,
            timing: TimingRule::Interval {
                seconds: INTERVAL_SECONDS,
            },
            expiry: ExpiryRule::None,
            now_ms: anchor_ms,
        })
        .await?;
    let definition = serde_json::to_value(&wake.definition)?;
    let wakeup_id = wake.definition.wakeup_id.clone();
    let route = Arc::new(StoredWakeRoute {
        store: Arc::clone(&store),
        observed: Mutex::new(Vec::new()),
        generation: serde_json::from_value(json!({"serviceEpoch":SERVICE_ID,"generation":1}))?,
    });
    let delivery: Arc<dyn SessionMessageDelivery> = route.clone();
    let identity = ServiceIdentity::new(SERVICE_ID, SERVICE_ID)?
        .with_machine_identity(machine.clone())?
        .with_automation_store(Arc::clone(&store))
        .with_session_delivery(delivery);
    let sender = WakeDeliverySender {
        delivery: identity
            .session_message_delivery()
            .ok_or("composed wake delivery missing")?,
        configuration: crate::AutomationConfigurationHandle::default(),
        machine_identity: machine.clone(),
    };
    let (socket, server) = tokio::net::UnixStream::pair()?;
    let control_task = tokio::spawn(serve_control_connection(server, identity.clone()));
    let mut client = ControlClient::initialize(socket, "aged-wake-retention-proof", "1").await?;

    let (old_push, old_occurrence) =
        evaluate_and_dispatch(&store, &sender, &wakeup_id, anchor_ms + interval_ms).await?;
    let now = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(now_ms)
        .ok_or("valid maintenance time")?;
    ensure(
        old_push.created_at < now - chrono::Duration::days(30),
        "production firing timestamp must make the first delivered push strictly older than 30 days",
    )?;
    let old_link = RouterLink::new(
        MachineId::from(service_id.clone()),
        old_push.push_id.clone(),
    )
    .to_string();
    ensure(
        store.lock().await.prune_push_records(now, 500).await? == 1,
        "retention must physically prune the aged push",
    )?;
    ensure(
        store
            .lock()
            .await
            .get_push_record(&old_push.push_id)
            .await?
            .is_none(),
        "aged push must be physically absent",
    )?;
    require_expired_link(&identity, &target, &old_link).await?;
    let retained = store
        .lock()
        .await
        .read_wakeup::<SavedMessage>(&wakeup_id)
        .await?;
    ensure(
        retained.state == WakeState::Active
            && retained.pending_delivery_id.is_none()
            && serde_json::to_value(&retained.definition)? == definition,
        "pruning a push must preserve the original active durable wake definition",
    )?;

    let (fresh_push, fresh_occurrence) =
        evaluate_and_dispatch(&store, &sender, &wakeup_id, now_ms).await?;
    ensure(
        fresh_push.push_id != old_push.push_id
            && fresh_occurrence != old_occurrence
            && fresh_push.created_at == now
            && fresh_push.kind == PushKind::Wake
            && fresh_push.origin == PushOrigin::Router(PushKind::Wake)
            && fresh_push.header_facts == PushHeaderFacts::Wake,
        "same aged wake must fire a fresh current push and occurrence",
    )?;
    let fresh_link = RouterLink::new(MachineId::from(service_id), fresh_push.push_id.clone());
    let shown = client
        .router_show(PushRecordShowParams {
            caller: target.clone(),
            reference: fresh_link.to_string(),
        })
        .await?;
    ensure(
        shown.record.push_id == fresh_push.push_id
            && shown.record.body.as_deref() == Some(WAKE_BODY),
        "fresh firing must be readable with its full body through Control",
    )?;
    require_expired_link(&identity, &target, &old_link).await?;
    let observed = route.observed.lock().await;
    let [old_request, fresh_request] = observed.as_slice() else {
        return Err("both actual firings must cross the service route exactly once".into());
    };
    let expected_line = render_push_line(&PushLineInput {
        link: fresh_link,
        machine_label: machine.machine_label().clone(),
        origin: fresh_push.origin.clone(),
        header_facts: fresh_push.header_facts.clone(),
        body: fresh_push.body.clone(),
    })?;
    ensure(
        old_request.payload.push_id == old_push.push_id
            && fresh_request.payload.push_id == fresh_push.push_id
            && fresh_request.correlation.as_str() == fresh_push.push_id.as_str()
            && fresh_request.payload.line.as_str() == expected_line
            && fresh_request.payload.line.as_str().lines().count() == 1
            && fresh_request.payload.load_policy == crate::LoadPolicy::MayLoad,
        "new firing must deliver exactly the stored neutral wake line with fresh identity",
    )?;
    drop(observed);
    let current = store
        .lock()
        .await
        .read_wakeup::<SavedMessage>(&wakeup_id)
        .await?;
    ensure(
        serde_json::to_value(&current.definition)? == definition
            && current.pending_delivery_id.is_none(),
        "both firings must settle the same unchanged definition",
    )?;

    client.close().await?;
    tokio::time::timeout(Duration::from_secs(5), control_task).await???;
    drop(sender);
    drop(route);
    drop(identity);
    Arc::try_unwrap(store)
        .map_err(|_| "wake fixture store remained shared")?
        .into_inner()
        .close()
        .await?;
    Ok(())
}
