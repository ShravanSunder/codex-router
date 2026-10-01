//! Provider schedule outcomes and retry identity, including push retention expiry.
use super::route_tests::{
    FakeSettlementPlan, FakeSubmissionPlan, TestResult, provider_worker_fixture,
};
use agent_automation::{RouteEffectEvidence, RunPhase};
use automation_storage::StorageError;
use collaboration_protocol::{
    CodexGeneration, DeliveryNextAction, DeliveryOutcome, DeliveryRejection,
    DeliveryRejectionReason, EndpointRef, PushDeliveryState, PushHeaderFacts, PushKind, SessionRef,
};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn fake_provider_submission_and_settlement_variants_preserve_run_state() -> TestResult<()> {
    let cases = [
        (
            FakeSubmissionPlan::Reject,
            FakeSettlementPlan::Pending,
            RunPhase::Preparing,
            false,
        ),
        (
            FakeSubmissionPlan::Unknown,
            FakeSettlementPlan::Pending,
            RunPhase::Uncertain,
            false,
        ),
        (
            FakeSubmissionPlan::Accept,
            FakeSettlementPlan::Failed,
            RunPhase::Finished,
            false,
        ),
        (
            FakeSubmissionPlan::Accept,
            FakeSettlementPlan::Interrupted,
            RunPhase::Finished,
            false,
        ),
        (
            FakeSubmissionPlan::Accept,
            FakeSettlementPlan::Pending,
            RunPhase::Executing,
            false,
        ),
        (
            FakeSubmissionPlan::Accept,
            FakeSettlementPlan::WrittenWithoutCompletion,
            RunPhase::Executing,
            true,
        ),
    ];
    for (submission, settlement, expected_phase, expected_error) in cases {
        let (root, store, worker, run_id, observed_push_ids) =
            provider_worker_fixture(submission, settlement).await?;
        worker.step(run_id.clone()).await?;
        worker.step(run_id.clone()).await?;
        let busy = store
            .lock()
            .await
            .read_run::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(&run_id)
            .await?;
        if busy.phase != RunPhase::Preparing || busy.evidence.timing.is_some() {
            return Err("not-started-busy consumed a dispatch budget".into());
        }
        worker.step(run_id.clone()).await?;
        if matches!(submission, FakeSubmissionPlan::Accept) {
            let observed = worker.step(run_id.clone()).await;
            if expected_error {
                if !matches!(observed, Err(StorageError::InvalidRecord)) {
                    return Err("provider write-only settlement was not rejected".into());
                }
            } else {
                observed?;
            }
        }
        let settled = store
            .lock()
            .await
            .read_run::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(&run_id)
            .await?;
        if settled.phase != expected_phase {
            return Err(format!(
                "provider route phase mismatch: expected {expected_phase:?}, got {:?}",
                settled.phase
            )
            .into());
        }
        let observed_push_ids_snapshot = observed_push_ids
            .lock()
            .map_err(|_| "scheduled push id observation lock poisoned")?
            .clone();
        if observed_push_ids_snapshot.len() != 2
            || observed_push_ids_snapshot[0] != observed_push_ids_snapshot[1]
        {
            return Err("schedule retry did not reuse the same push id".into());
        }
        let push = store
            .lock()
            .await
            .get_push_record(&observed_push_ids_snapshot[0])
            .await?
            .ok_or("scheduled push record missing")?;
        let (expected_push_state, expected_push_outcome) = match submission {
            FakeSubmissionPlan::Reject => (
                PushDeliveryState::Pending,
                Some(DeliveryOutcome::Rejected(DeliveryRejection {
                    reason: DeliveryRejectionReason::Busy,
                    next_action: DeliveryNextAction::RetryLater,
                    client_code: None,
                    detail: Some("fixture rejection".into()),
                    claims: None,
                })),
            ),
            FakeSubmissionPlan::Unknown => (
                PushDeliveryState::OutcomeUnknown,
                Some(DeliveryOutcome::Unknown),
            ),
            FakeSubmissionPlan::Accept | FakeSubmissionPlan::RejectOnceThenAccept => {
                (PushDeliveryState::Delivered, Some(DeliveryOutcome::Started))
            }
        };
        if push.delivery_state != expected_push_state
            || push
                .last_outcome
                .as_ref()
                .map(|receipt| receipt.outcome.clone())
                != expected_push_outcome
        {
            return Err("scheduled push outcome did not match the run submission".into());
        }
        if matches!(submission, FakeSubmissionPlan::Unknown) {
            worker.step(run_id.clone()).await?;
            let observed_push_ids_after_observation = observed_push_ids
                .lock()
                .map_err(|_| "scheduled push id observation lock poisoned")?
                .clone();
            if observed_push_ids_after_observation.len() != 2 {
                return Err("unknown provider submission was automatically redispatched".into());
            }
            let uncertain_run = store
                .lock()
                .await
                .read_run::<
                    SessionRef,
                    EndpointRef,
                    CodexGeneration,
                    crate::stored_run_receipt::StoredRunReceipt,
                >(&run_id)
                .await?;
            if uncertain_run.phase != RunPhase::Uncertain {
                return Err("unknown provider submission left the uncertain phase".into());
            }
            let uncertain_push = store
                .lock()
                .await
                .get_push_record(&observed_push_ids_after_observation[0])
                .await?
                .ok_or("unknown schedule push missing")?;
            if uncertain_push.delivery_state != PushDeliveryState::OutcomeUnknown {
                return Err("unknown provider submission changed push state on observation".into());
            }
        }
        let Some(RouteEffectEvidence::ProviderAcp(provider_effect)) =
            settled.evidence.route.as_ref()
        else {
            return Err("scheduled run lost its selected provider evidence".into());
        };
        let PushHeaderFacts::ScheduleRun {
            schedule_id: push_schedule_id,
            run_id: push_run_id,
        } = &push.header_facts
        else {
            return Err("scheduled push lost its typed schedule origin".into());
        };
        if push.push_id != observed_push_ids_snapshot[0]
            || push.target != provider_effect.target
            || push_schedule_id != &settled.schedule_id
            || push_run_id != &settled.run_id
        {
            return Err("scheduled push identity, target, or route evidence diverged".into());
        }
        if matches!(submission, FakeSubmissionPlan::Reject) && settled.evidence.timing.is_some() {
            return Err("known rejection retained an execution budget".into());
        }
        match (submission, settlement) {
            (FakeSubmissionPlan::Accept, FakeSettlementPlan::Failed)
                if !matches!(
                    settled.worker_outcome,
                    Some(agent_automation::WorkerOutcome::Failed { .. })
                ) =>
            {
                return Err("failed settlement lost worker outcome".into());
            }
            (FakeSubmissionPlan::Accept, FakeSettlementPlan::Interrupted)
                if !matches!(
                    settled.worker_outcome,
                    Some(agent_automation::WorkerOutcome::Interrupted { .. })
                ) =>
            {
                return Err("interrupted settlement lost worker outcome".into());
            }
            _ => {}
        }
        drop(worker);
        let store = Arc::try_unwrap(store).map_err(|_| "store still referenced")?;
        store.into_inner().close().await?;
        for entry in std::fs::read_dir(&root)? {
            std::fs::remove_file(entry?.path())?;
        }
        std::fs::remove_dir(root)?;
    }
    Ok(())
}

#[tokio::test]
async fn known_rejected_provider_submission_retries_on_the_same_schedule_push_id() -> TestResult<()>
{
    let (root, store, worker, run_id, observed_push_ids) = provider_worker_fixture(
        FakeSubmissionPlan::RejectOnceThenAccept,
        FakeSettlementPlan::Pending,
    )
    .await?;
    worker.step(run_id.clone()).await?;
    worker.step(run_id.clone()).await?;
    worker.step(run_id.clone()).await?;

    let first_attempt_ids = observed_push_ids
        .lock()
        .map_err(|_| "scheduled push id observation lock poisoned")?
        .clone();
    if first_attempt_ids.len() != 2 || first_attempt_ids[0] != first_attempt_ids[1] {
        return Err("known rejection did not retain its original schedule push id".into());
    }
    let rejected_push = store
        .lock()
        .await
        .get_push_record(&first_attempt_ids[0])
        .await?
        .ok_or("rejected schedule push missing")?;
    if rejected_push.delivery_state != PushDeliveryState::Pending
        || !matches!(
            rejected_push
                .last_outcome
                .as_ref()
                .map(|receipt| &receipt.outcome),
            Some(DeliveryOutcome::Rejected(_))
        )
        || rejected_push.settled_at.is_none()
    {
        return Err("known rejection was not retained on a retryable pending push".into());
    }

    worker.step(run_id.clone()).await?;
    let accepted_run = store
        .lock()
        .await
        .read_run::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(&run_id)
        .await?;
    if accepted_run.phase != RunPhase::Executing {
        return Err("known rejection retry did not advance the Run after acceptance".into());
    }
    let all_attempt_ids = observed_push_ids
        .lock()
        .map_err(|_| "scheduled push id observation lock poisoned")?
        .clone();
    if all_attempt_ids.len() != 3
        || all_attempt_ids
            .iter()
            .any(|push_id| push_id != &first_attempt_ids[0])
    {
        return Err("schedule retry created or dispatched a second push id".into());
    }
    let delivered_push = store
        .lock()
        .await
        .get_push_record(&first_attempt_ids[0])
        .await?
        .ok_or("delivered schedule push missing")?;
    if delivered_push.delivery_state != PushDeliveryState::Delivered
        || !matches!(
            delivered_push
                .last_outcome
                .as_ref()
                .map(|receipt| &receipt.outcome),
            Some(DeliveryOutcome::Started)
        )
    {
        return Err("schedule push did not settle delivered after its retry".into());
    }
    drop(worker);
    let store = Arc::try_unwrap(store).map_err(|_| "store still referenced")?;
    store.into_inner().close().await?;
    for entry in std::fs::read_dir(&root)? {
        std::fs::remove_file(entry?.path())?;
    }
    std::fs::remove_dir(root)?;
    Ok(())
}

#[tokio::test]
async fn known_rejected_provider_submission_after_retention_uses_fresh_id_and_old_link_expires()
-> TestResult<()> {
    let (root, store, worker, run_id, observed_push_ids) = provider_worker_fixture(
        FakeSubmissionPlan::RejectOnceThenAccept,
        FakeSettlementPlan::Pending,
    )
    .await?;
    worker
        .step(run_id.clone())
        .await
        .map_err(|error| format!("initial target preparation failed: {error:?}"))?;
    worker
        .step(run_id.clone())
        .await
        .map_err(|error| format!("initial busy submission failed: {error:?}"))?;
    worker
        .step(run_id.clone())
        .await
        .map_err(|error| format!("known rejection persistence failed: {error:?}"))?;

    let rejected_ids = observed_push_ids
        .lock()
        .map_err(|_| "scheduled push id observation lock poisoned")?
        .clone();
    if rejected_ids.len() != 2 || rejected_ids[0] != rejected_ids[1] {
        return Err("pre-expiry rejection did not reuse the original schedule PushId".into());
    }
    let old_push = store
        .lock()
        .await
        .get_push_record(&rejected_ids[0])
        .await?
        .ok_or("pending schedule push missing before retention")?;
    if old_push.delivery_state != PushDeliveryState::Pending
        || !matches!(
            old_push
                .last_outcome
                .as_ref()
                .map(|receipt| &receipt.outcome),
            Some(DeliveryOutcome::Rejected(_))
        )
    {
        return Err("test did not reach a pending rejected schedule push".into());
    }
    let pruned_count = store
        .lock()
        .await
        .prune_push_records(old_push.created_at + chrono::Duration::days(31), 500)
        .await?;
    if pruned_count != 1
        || store
            .lock()
            .await
            .get_push_record(&old_push.push_id)
            .await?
            .is_some()
    {
        return Err("31-day retention did not prune the pending schedule push".into());
    }

    let service_id = String::from(old_push.target.endpoint.service_id.clone());
    let identity = crate::ServiceIdentity::new(
        &service_id,
        &service_id,
        &format!("sha256:{}", "a".repeat(64)),
    )
    .map_err(std::io::Error::other)?
    .with_automation_store(Arc::clone(&store));
    let old_link = crate::push_record_resolver::link_for(&old_push, &identity);
    let old_link_result = crate::push_record_resolver::show(
        json!(1),
        json!({"caller":old_push.target,"reference":old_link}),
        &identity,
    )
    .await;
    if old_link_result
        .pointer("/error/data/kind")
        .and_then(serde_json::Value::as_str)
        != Some("notFound")
        || !old_link_result
            .pointer("/error/data/message")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|message| message.contains("expired after 30 days"))
    {
        return Err("old schedule link did not report its expired record".into());
    }
    drop(identity);

    if let Err(error) = worker.step(run_id.clone()).await {
        let failed_run = store
            .lock()
            .await
            .read_run::<
                SessionRef,
                EndpointRef,
                CodexGeneration,
                crate::stored_run_receipt::StoredRunReceipt,
            >(&run_id)
            .await?;
        let observed_ids = observed_push_ids
            .lock()
            .map_err(|_| "scheduled push id observation lock poisoned")?
            .clone();
        let retained_origin_record = store
            .lock()
            .await
            .get_push_record_by_router_ref(
                old_push
                    .origin_router_ref
                    .as_deref()
                    .ok_or("old ScheduleRun push had no origin reference")?,
            )
            .await?;
        return Err(format!(
            "post-retention run retry failed: {error:?}; phase={:?}; observed_push_ids={observed_ids:?}; retained_origin_record_state={:?}",
            failed_run.phase,
            retained_origin_record.map(|record| record.delivery_state),
        )
        .into());
    }
    let run = store
        .lock()
        .await
        .read_run::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(&run_id)
        .await?;
    if run.phase != RunPhase::Executing {
        return Err("run retry did not proceed after its old push expired".into());
    }
    let all_push_ids = observed_push_ids
        .lock()
        .map_err(|_| "scheduled push id observation lock poisoned")?
        .clone();
    if all_push_ids.len() != 3
        || all_push_ids[0] != all_push_ids[1]
        || all_push_ids[2] == old_push.push_id
    {
        return Err("post-expiry retry did not create one fresh ScheduleRun PushId".into());
    }
    let fresh_push = store
        .lock()
        .await
        .get_push_record(&all_push_ids[2])
        .await?
        .ok_or("fresh schedule push missing after retry")?;
    let header_facts_match = match (&old_push.header_facts, &fresh_push.header_facts) {
        (
            PushHeaderFacts::ScheduleRun {
                schedule_id: old_schedule_id,
                run_id: old_run_id,
            },
            PushHeaderFacts::ScheduleRun {
                schedule_id: fresh_schedule_id,
                run_id: fresh_run_id,
            },
        ) => old_schedule_id == fresh_schedule_id && old_run_id == fresh_run_id,
        _ => false,
    };
    if fresh_push.kind != PushKind::ScheduleRun
        || fresh_push.target != old_push.target
        || fresh_push.origin_router_ref != old_push.origin_router_ref
        || fresh_push.body != old_push.body
        || !header_facts_match
        || fresh_push.delivery_state != PushDeliveryState::Delivered
        || !matches!(
            fresh_push
                .last_outcome
                .as_ref()
                .map(|receipt| &receipt.outcome),
            Some(DeliveryOutcome::Started)
        )
    {
        return Err("fresh schedule push was not delivered after retention".into());
    }

    drop(worker);
    let store = Arc::try_unwrap(store).map_err(|_| "store still referenced")?;
    store.into_inner().close().await?;
    for entry in std::fs::read_dir(&root)? {
        std::fs::remove_file(entry?.path())?;
    }
    std::fs::remove_dir(root)?;
    Ok(())
}
