//! Pure two-root producer report classification and validation.
use super::runtime_fixture::{ActorRootExpectation, ExpectedResources, ensure};
use crate::{
    owned_resource_cleanup::{
        CleanupAction, CleanupObservation, CleanupOutcome, MatrixCleanupReport,
    },
    owned_resources::MatrixCreationHistory,
    proof_context::ProofResult,
};
use collaboration_client::{board::EndReason, protocol::ThreadSubscriptionState};

#[derive(Clone, Copy, Debug)]
pub(super) enum ActualHelperPrefix {
    Completed,
    Cooldown,
    Unqualified,
}

pub(super) fn missing_installed_pairs(
    history: &MatrixCreationHistory,
    installed: &ExpectedResources,
) -> Vec<ActorRootExpectation> {
    history
        .actor_roots
        .iter()
        .filter(|owned| {
            !installed
                .pairs
                .iter()
                .any(|row| row.actor == owned.actor && row.root_message_id == owned.root_message_id)
        })
        .map(|owned| ActorRootExpectation {
            actor: owned.actor.clone(),
            root_message_id: owned.root_message_id.clone(),
        })
        .collect()
}

pub(super) fn check_actual_helper_report(
    report: &MatrixCleanupReport,
    installed: &ExpectedResources,
    actors: Option<&[collaboration_client::board::Identity; 2]>,
    prefix: ActualHelperPrefix,
) -> ProofResult<()> {
    let (owned_count, installed_count, expected_counts) = match prefix {
        ActualHelperPrefix::Completed => (4, 4, [0, 0, 4, 4]),
        ActualHelperPrefix::Cooldown => (3, 2, [0, 0, 3, 3]),
        ActualHelperPrefix::Unqualified => {
            return Err("producer did not reach a source-qualified prefix".into());
        }
    };
    ensure(
        report.unresolved.is_empty()
            && report.history.wake.is_none()
            && report.history.instruction.is_none()
            && report.history.schedule.is_none(),
        "actual helper invented automation ownership",
    )?;
    ensure(
        report.history.actor_roots.len() == owned_count && installed.pairs.len() == installed_count,
        "literal owned/installed helper prefix counts changed",
    )?;
    for row in &installed.pairs {
        ensure(
            report
                .history
                .actor_roots
                .iter()
                .filter(|owned| {
                    owned.actor == row.actor && owned.root_message_id == row.root_message_id
                })
                .count()
                == 1,
            "installed row lost authoritative ownership",
        )?;
    }
    let absent = missing_installed_pairs(&report.history, installed);
    match prefix {
        ActualHelperPrefix::Completed => ensure(
            absent.is_empty(),
            "completed helper retained an uninstalled pair",
        )?,
        ActualHelperPrefix::Cooldown => {
            let [sender, reader] = actors.ok_or("actual helper actors were not established")?;
            let [first, second] = installed.pairs.as_slice() else {
                return Err("cooldown did not retain two actual installed rows".into());
            };
            let [missing] = absent.as_slice() else {
                return Err("cooldown did not retain its never-created sender/root pair".into());
            };
            ensure(
                first.actor == *sender
                    && second.actor == *reader
                    && first.root_message_id == second.root_message_id
                    && missing.actor == *sender
                    && missing.root_message_id != first.root_message_id,
                "cooldown prefix changed first-root rows or second-root ownership",
            )?;
        }
        ActualHelperPrefix::Unqualified => {
            return Err("unqualified producer prefix reached report validation".into());
        }
    }
    let mut counts = [0_usize; 4];
    for attempt in &report.attempts {
        let (actor, root_message_id, unsubscribe) = match &attempt.action {
            CleanupAction::Unsubscribe {
                actor,
                root_message_id,
            } => {
                counts[2] += 1;
                (actor, root_message_id, true)
            }
            CleanupAction::Unwatch {
                actor,
                root_message_id,
            } => {
                counts[3] += 1;
                (actor, root_message_id, false)
            }
            CleanupAction::CancelWake { .. } | CleanupAction::DisableSchedule { .. } => {
                return Err("actual helper cleanup invented automation action".into());
            }
        };
        ensure(
            report
                .history
                .actor_roots
                .iter()
                .any(|owned| &owned.actor == actor && &owned.root_message_id == root_message_id),
            "cleanup action targeted an unowned helper pair",
        )?;
        if installed
            .pairs
            .iter()
            .any(|row| &row.actor == actor && &row.root_message_id == root_message_id)
        {
            let retired = if unsubscribe {
                matches!(
                    &attempt.outcome,
                    CleanupOutcome::Returned(CleanupObservation::Subscription {
                        state: ThreadSubscriptionState::Ended,
                        end_reason: Some(EndReason::Cancelled),
                        pending_count: 0,
                    })
                )
            } else {
                matches!(
                    &attempt.outcome,
                    CleanupOutcome::Returned(CleanupObservation::Watch { watching: false })
                )
            };
            ensure(
                retired,
                "installed helper row did not return exact local retirement",
            )?;
        } else {
            // Preserve exact observed typed rejection/uncertainty; neither means no effect or success.
            ensure(
                matches!(
                    &attempt.outcome,
                    CleanupOutcome::Rejected(_) | CleanupOutcome::Unknown(_)
                ),
                "never-created helper pair fabricated a Returned cleanup observation",
            )?;
        }
    }
    ensure(
        counts == expected_counts,
        "literal one-attempt helper cleanup counts changed",
    )?;
    ensure(
        report.attempts.len() == expected_counts.into_iter().sum::<usize>(),
        "helper cleanup attempt record was lost",
    )?;
    for owned in &report.history.actor_roots {
        let unsubscribes = report.attempts.iter().filter(|attempt| matches!(&attempt.action,
            CleanupAction::Unsubscribe { actor, root_message_id } if actor == &owned.actor && root_message_id == &owned.root_message_id)).count();
        let unwatches = report.attempts.iter().filter(|attempt| matches!(&attempt.action,
            CleanupAction::Unwatch { actor, root_message_id } if actor == &owned.actor && root_message_id == &owned.root_message_id)).count();
        ensure(
            unsubscribes == 1 && unwatches == 1,
            "owned helper pair was replayed or omitted",
        )?;
    }
    Ok(())
}
