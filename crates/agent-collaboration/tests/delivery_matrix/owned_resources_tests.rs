//! Pure permanent ownership-state cases; no client/service/store or runtime is invoked.
use super::{
    CleanupIntent, CreationKind, InstructionCreation, MatrixCleanupPlan, OwnedMatrixResources,
    OwnershipError, ScheduleCreation, UnresolvedCreation, WakeCreation,
};
use crate::proof_context::ProofResult;
use collaboration_client::{
    board::{
        EndpointId, Identity, MessageId, ServiceId, SessionEndpointRef, SessionId, SessionRef,
    },
    protocol::{InstructionId, OperationId, ScheduleId, WakeupId},
};

fn wake_operation() -> ProofResult<OperationId> {
    Ok(OperationId::try_from(
        "019f0000-0000-7000-8000-000000000001".to_owned(),
    )?)
}
fn instruction_operation() -> ProofResult<OperationId> {
    Ok(OperationId::try_from(
        "019f0000-0000-7000-8000-000000000002".to_owned(),
    )?)
}
fn schedule_operation() -> ProofResult<OperationId> {
    Ok(OperationId::try_from(
        "019f0000-0000-7000-8000-000000000003".to_owned(),
    )?)
}
fn different_operation() -> ProofResult<OperationId> {
    Ok(OperationId::try_from(
        "019f0000-0000-7000-8000-000000000004".to_owned(),
    )?)
}
fn wake_id() -> ProofResult<WakeupId> {
    Ok(WakeupId::try_from(
        "019f0000-0000-7000-8000-000000000011".to_owned(),
    )?)
}
fn instruction_id() -> ProofResult<InstructionId> {
    Ok(InstructionId::try_from(
        "019f0000-0000-7000-8000-000000000012".to_owned(),
    )?)
}
fn schedule_id() -> ProofResult<ScheduleId> {
    Ok(ScheduleId::try_from(
        "019f0000-0000-7000-8000-000000000013".to_owned(),
    )?)
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Actor {
    Sender,
    Reader,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Root {
    First,
    Second,
    Third,
}
fn actor(value: Actor) -> ProofResult<Identity> {
    Ok(Identity::Session {
        session: SessionRef {
            endpoint: SessionEndpointRef {
                service_id: ServiceId::try_from("00000000-0000-4000-8000-000000000001".to_owned())?,
                endpoint_id: EndpointId::try_from("codex-local".to_owned())?,
            },
            session_id: SessionId::try_from(
                match value {
                    Actor::Sender => "matrix-sender",
                    Actor::Reader => "matrix-reader",
                }
                .to_owned(),
            )?,
        },
    })
}
fn root_id(value: Root) -> ProofResult<MessageId> {
    Ok(MessageId::try_from(
        match value {
            Root::First => "019f0000-0000-7000-8000-000000000021",
            Root::Second => "019f0000-0000-7000-8000-000000000022",
            Root::Third => "019f0000-0000-7000-8000-000000000023",
        }
        .to_owned(),
    )?)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum History {
    Absent,
    Submitted,
    Known,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Intent {
    CancelWake,
    DisableSchedule,
    Unsubscribe(Actor, Root),
    Unwatch(Actor, Root),
}
#[derive(Debug, Eq, PartialEq)]
struct PlanView {
    intents: Vec<Intent>,
    unresolved: Vec<CreationKind>,
    wake: History,
    instruction: History,
    schedule: History,
}
fn pair_view(actor_value: &Identity, root: &MessageId) -> ProofResult<(Actor, Root)> {
    let actor_label = if *actor_value == actor(Actor::Sender)? {
        Actor::Sender
    } else if *actor_value == actor(Actor::Reader)? {
        Actor::Reader
    } else {
        return Err("cleanup intent changed its actual actor identity".into());
    };
    let root_label = if *root == root_id(Root::First)? {
        Root::First
    } else if *root == root_id(Root::Second)? {
        Root::Second
    } else {
        return Err("cleanup intent changed its actual root identity".into());
    };
    Ok((actor_label, root_label))
}
fn view(plan: &MatrixCleanupPlan) -> ProofResult<PlanView> {
    let mut intents = Vec::new();
    for action in &plan.intents {
        intents.push(match action {
            CleanupIntent::CancelWake(actual) if *actual == wake_id()? => Intent::CancelWake,
            CleanupIntent::DisableSchedule(actual) if *actual == schedule_id()? => {
                Intent::DisableSchedule
            }
            CleanupIntent::Unsubscribe {
                actor,
                root_message_id,
            } => {
                let (actor, root) = pair_view(actor, root_message_id)?;
                Intent::Unsubscribe(actor, root)
            }
            CleanupIntent::Unwatch {
                actor,
                root_message_id,
            } => {
                let (actor, root) = pair_view(actor, root_message_id)?;
                Intent::Unwatch(actor, root)
            }
            _ => return Err("cleanup intent changed its actual wake or schedule identity".into()),
        });
    }
    let mut unresolved = Vec::new();
    for entry in &plan.unresolved {
        unresolved.push(match entry {
            UnresolvedCreation::Wake { operation_id } if *operation_id == wake_operation()? => {
                CreationKind::Wake
            }
            UnresolvedCreation::Instruction { operation_id }
                if *operation_id == instruction_operation()? =>
            {
                CreationKind::Instruction
            }
            UnresolvedCreation::Schedule { operation_id }
                if *operation_id == schedule_operation()? =>
            {
                CreationKind::Schedule
            }
            _ => return Err("unresolved creation lost its actual submitted operation".into()),
        });
    }
    let wake = match &plan.history.wake {
        None => History::Absent,
        Some(WakeCreation::Submitted { operation_id }) if *operation_id == wake_operation()? => {
            History::Submitted
        }
        Some(WakeCreation::Known {
            operation_id,
            wakeup_id,
        }) if *operation_id == wake_operation()? && *wakeup_id == wake_id()? => History::Known,
        _ => return Err("wake creation history changed identity".into()),
    };
    let instruction = match &plan.history.instruction {
        None => History::Absent,
        Some(InstructionCreation::Submitted { operation_id })
            if *operation_id == instruction_operation()? =>
        {
            History::Submitted
        }
        Some(InstructionCreation::Known {
            operation_id,
            instruction_id: actual,
        }) if *operation_id == instruction_operation()? && *actual == instruction_id()? => {
            History::Known
        }
        _ => return Err("instruction history changed identity".into()),
    };
    let schedule = match &plan.history.schedule {
        None => History::Absent,
        Some(ScheduleCreation::Submitted { operation_id })
            if *operation_id == schedule_operation()? =>
        {
            History::Submitted
        }
        Some(ScheduleCreation::Known {
            operation_id,
            schedule_id: actual,
        }) if *operation_id == schedule_operation()? && *actual == schedule_id()? => History::Known,
        _ => return Err("schedule creation history changed identity".into()),
    };
    let historical_pairs = plan
        .history
        .actor_roots
        .iter()
        .map(|pair| pair_view(&pair.actor, &pair.root_message_id))
        .collect::<ProofResult<Vec<_>>>()?;
    let unsubscribe_pairs = intents
        .iter()
        .filter_map(|intent| match intent {
            Intent::Unsubscribe(actor, root) => Some((*actor, *root)),
            _ => None,
        })
        .collect::<Vec<_>>();
    if historical_pairs != unsubscribe_pairs
        || historical_pairs.len() * 2
            != intents
                .iter()
                .filter(|intent| matches!(intent, Intent::Unsubscribe(..) | Intent::Unwatch(..)))
                .count()
    {
        return Err("actor/root history does not match exact cleanup-pair ownership".into());
    }
    Ok(PlanView {
        intents,
        unresolved,
        wake,
        instruction,
        schedule,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Prefix {
    SenderFirstBeforeCreate,
    ReaderFirstBeforeJoin,
    FirstPolicyBeforeSecondRoot,
    SenderSecondBeforeCreate,
    ReaderSecondBeforeJoin,
    WakeSubmitted,
    WakeReturned,
    InstructionSubmitted,
    InstructionReturned,
    ScheduleSubmitted,
    ScheduleReturnedBeforePrepare,
    PrepareFailed,
    EnableFailed,
}
fn at_prefix(prefix: Prefix) -> ProofResult<OwnedMatrixResources> {
    let mut owned = OwnedMatrixResources::default();
    owned.register_actor_root(actor(Actor::Sender)?, root_id(Root::First)?)?;
    if prefix == Prefix::SenderFirstBeforeCreate {
        return Ok(owned);
    }
    owned.register_actor_root(actor(Actor::Reader)?, root_id(Root::First)?)?;
    if prefix == Prefix::ReaderFirstBeforeJoin {
        return Ok(owned);
    }
    owned.register_actor_root(actor(Actor::Reader)?, root_id(Root::First)?)?;
    if prefix == Prefix::FirstPolicyBeforeSecondRoot {
        return Ok(owned);
    }
    owned.register_actor_root(actor(Actor::Sender)?, root_id(Root::Second)?)?;
    if prefix == Prefix::SenderSecondBeforeCreate {
        return Ok(owned);
    }
    owned.register_actor_root(actor(Actor::Reader)?, root_id(Root::Second)?)?;
    if prefix == Prefix::ReaderSecondBeforeJoin {
        return Ok(owned);
    }
    owned.register_actor_root(actor(Actor::Reader)?, root_id(Root::Second)?)?;
    owned.record_wake_submission(wake_operation()?)?;
    if prefix == Prefix::WakeSubmitted {
        return Ok(owned);
    }
    owned.record_wake_receipt(&wake_operation()?, wake_id()?)?;
    if prefix == Prefix::WakeReturned {
        return Ok(owned);
    }
    owned.record_instruction_submission(instruction_operation()?)?;
    if prefix == Prefix::InstructionSubmitted {
        return Ok(owned);
    }
    owned.record_instruction_receipt(&instruction_operation()?, instruction_id()?)?;
    if prefix == Prefix::InstructionReturned {
        return Ok(owned);
    }
    owned.record_schedule_submission(schedule_operation()?)?;
    if prefix == Prefix::ScheduleSubmitted {
        return Ok(owned);
    }
    owned.record_schedule_receipt(&schedule_operation()?, schedule_id()?)?;
    // Prepare/enable are later service outcomes, not new ownership states or calls here.
    Ok(owned)
}

#[test]
fn prefix_failures_preserve_literal_owned_intents_and_history() -> ProofResult<()> {
    use Actor::{Reader, Sender};
    use CreationKind::{Instruction, Schedule, Wake};
    use History::{Absent, Known, Submitted};
    use Intent::{CancelWake, DisableSchedule, Unsubscribe, Unwatch};
    use Prefix::*;
    use Root::{First, Second};
    struct Row {
        prefix: Prefix,
        intents: &'static [Intent],
        unresolved: &'static [CreationKind],
        wake: History,
        instruction: History,
        schedule: History,
    }
    // Literal expected rows follow actual C order: pairs, wake, instruction, schedule.
    let rows = [
        Row {
            prefix: SenderFirstBeforeCreate,
            intents: &[Unsubscribe(Sender, First), Unwatch(Sender, First)],
            unresolved: &[],
            wake: Absent,
            instruction: Absent,
            schedule: Absent,
        },
        Row {
            prefix: ReaderFirstBeforeJoin,
            intents: &[
                Unsubscribe(Sender, First),
                Unwatch(Sender, First),
                Unsubscribe(Reader, First),
                Unwatch(Reader, First),
            ],
            unresolved: &[],
            wake: Absent,
            instruction: Absent,
            schedule: Absent,
        },
        Row {
            prefix: FirstPolicyBeforeSecondRoot,
            intents: &[
                Unsubscribe(Sender, First),
                Unwatch(Sender, First),
                Unsubscribe(Reader, First),
                Unwatch(Reader, First),
            ],
            unresolved: &[],
            wake: Absent,
            instruction: Absent,
            schedule: Absent,
        },
        Row {
            prefix: SenderSecondBeforeCreate,
            intents: &[
                Unsubscribe(Sender, First),
                Unwatch(Sender, First),
                Unsubscribe(Reader, First),
                Unwatch(Reader, First),
                Unsubscribe(Sender, Second),
                Unwatch(Sender, Second),
            ],
            unresolved: &[],
            wake: Absent,
            instruction: Absent,
            schedule: Absent,
        },
        Row {
            prefix: ReaderSecondBeforeJoin,
            intents: &[
                Unsubscribe(Sender, First),
                Unwatch(Sender, First),
                Unsubscribe(Reader, First),
                Unwatch(Reader, First),
                Unsubscribe(Sender, Second),
                Unwatch(Sender, Second),
                Unsubscribe(Reader, Second),
                Unwatch(Reader, Second),
            ],
            unresolved: &[],
            wake: Absent,
            instruction: Absent,
            schedule: Absent,
        },
        Row {
            prefix: WakeSubmitted,
            intents: &[
                Unsubscribe(Sender, First),
                Unwatch(Sender, First),
                Unsubscribe(Reader, First),
                Unwatch(Reader, First),
                Unsubscribe(Sender, Second),
                Unwatch(Sender, Second),
                Unsubscribe(Reader, Second),
                Unwatch(Reader, Second),
            ],
            unresolved: &[Wake],
            wake: Submitted,
            instruction: Absent,
            schedule: Absent,
        },
        Row {
            prefix: WakeReturned,
            intents: &[
                CancelWake,
                Unsubscribe(Sender, First),
                Unwatch(Sender, First),
                Unsubscribe(Reader, First),
                Unwatch(Reader, First),
                Unsubscribe(Sender, Second),
                Unwatch(Sender, Second),
                Unsubscribe(Reader, Second),
                Unwatch(Reader, Second),
            ],
            unresolved: &[],
            wake: Known,
            instruction: Absent,
            schedule: Absent,
        },
        Row {
            prefix: InstructionSubmitted,
            intents: &[
                CancelWake,
                Unsubscribe(Sender, First),
                Unwatch(Sender, First),
                Unsubscribe(Reader, First),
                Unwatch(Reader, First),
                Unsubscribe(Sender, Second),
                Unwatch(Sender, Second),
                Unsubscribe(Reader, Second),
                Unwatch(Reader, Second),
            ],
            unresolved: &[Instruction],
            wake: Known,
            instruction: Submitted,
            schedule: Absent,
        },
        Row {
            prefix: InstructionReturned,
            intents: &[
                CancelWake,
                Unsubscribe(Sender, First),
                Unwatch(Sender, First),
                Unsubscribe(Reader, First),
                Unwatch(Reader, First),
                Unsubscribe(Sender, Second),
                Unwatch(Sender, Second),
                Unsubscribe(Reader, Second),
                Unwatch(Reader, Second),
            ],
            unresolved: &[],
            wake: Known,
            instruction: Known,
            schedule: Absent,
        },
        Row {
            prefix: ScheduleSubmitted,
            intents: &[
                CancelWake,
                Unsubscribe(Sender, First),
                Unwatch(Sender, First),
                Unsubscribe(Reader, First),
                Unwatch(Reader, First),
                Unsubscribe(Sender, Second),
                Unwatch(Sender, Second),
                Unsubscribe(Reader, Second),
                Unwatch(Reader, Second),
            ],
            unresolved: &[Schedule],
            wake: Known,
            instruction: Known,
            schedule: Submitted,
        },
        Row {
            prefix: ScheduleReturnedBeforePrepare,
            intents: &[
                CancelWake,
                DisableSchedule,
                Unsubscribe(Sender, First),
                Unwatch(Sender, First),
                Unsubscribe(Reader, First),
                Unwatch(Reader, First),
                Unsubscribe(Sender, Second),
                Unwatch(Sender, Second),
                Unsubscribe(Reader, Second),
                Unwatch(Reader, Second),
            ],
            unresolved: &[],
            wake: Known,
            instruction: Known,
            schedule: Known,
        },
        Row {
            prefix: PrepareFailed,
            intents: &[
                CancelWake,
                DisableSchedule,
                Unsubscribe(Sender, First),
                Unwatch(Sender, First),
                Unsubscribe(Reader, First),
                Unwatch(Reader, First),
                Unsubscribe(Sender, Second),
                Unwatch(Sender, Second),
                Unsubscribe(Reader, Second),
                Unwatch(Reader, Second),
            ],
            unresolved: &[],
            wake: Known,
            instruction: Known,
            schedule: Known,
        },
        Row {
            prefix: EnableFailed,
            intents: &[
                CancelWake,
                DisableSchedule,
                Unsubscribe(Sender, First),
                Unwatch(Sender, First),
                Unsubscribe(Reader, First),
                Unwatch(Reader, First),
                Unsubscribe(Sender, Second),
                Unwatch(Sender, Second),
                Unsubscribe(Reader, Second),
                Unwatch(Reader, Second),
            ],
            unresolved: &[],
            wake: Known,
            instruction: Known,
            schedule: Known,
        },
    ];
    for row in rows {
        let plan = at_prefix(row.prefix)?.into_cleanup_plan();
        let actual = view(&plan)?;
        let expected = PlanView {
            intents: row.intents.to_vec(),
            unresolved: row.unresolved.to_vec(),
            wake: row.wake,
            instruction: row.instruction,
            schedule: row.schedule,
        };
        if actual != expected {
            return Err(format!(
                "ownership prefix {:?} differs from literal expectation",
                row.prefix
            )
            .into());
        }
    }
    Ok(())
}

#[test]
fn empty_owner_has_no_actions_unresolved_creation_or_history() -> ProofResult<()> {
    let plan = OwnedMatrixResources::default().into_cleanup_plan();
    if view(&plan)?
        != (PlanView {
            intents: vec![],
            unresolved: vec![],
            wake: History::Absent,
            instruction: History::Absent,
            schedule: History::Absent,
        })
    {
        return Err("empty ownership produced cleanup or historical state".into());
    }
    Ok(())
}

#[test]
fn unknown_creations_are_metadata_only_and_never_dispatch_intents() -> ProofResult<()> {
    let mut owned = OwnedMatrixResources::default();
    owned.record_wake_submission(wake_operation()?)?;
    owned.record_instruction_submission(instruction_operation()?)?;
    owned.record_schedule_submission(schedule_operation()?)?;
    let plan = owned.into_cleanup_plan();
    let expected = PlanView {
        intents: vec![],
        unresolved: vec![
            CreationKind::Wake,
            CreationKind::Instruction,
            CreationKind::Schedule,
        ],
        wake: History::Submitted,
        instruction: History::Submitted,
        schedule: History::Submitted,
    };
    if view(&plan)? != expected {
        return Err("unknown creation invented a resource/action or lost its operation".into());
    }
    Ok(())
}

#[test]
fn identical_policy_pairs_dedupe_and_fifth_distinct_pair_is_refused() -> ProofResult<()> {
    let mut owned = at_prefix(Prefix::ReaderSecondBeforeJoin)?;
    owned.register_actor_root(actor(Actor::Sender)?, root_id(Root::First)?)?;
    owned.register_actor_root(actor(Actor::Reader)?, root_id(Root::Second)?)?;
    if owned.register_actor_root(actor(Actor::Sender)?, root_id(Root::Third)?)
        != Err(OwnershipError::ActorRootLimitReached)
    {
        return Err("fifth distinct actor/root pair was not refused before dispatch".into());
    }
    let plan = owned.into_cleanup_plan();
    use Actor::{Reader, Sender};
    use Intent::{Unsubscribe, Unwatch};
    use Root::{First, Second};
    let expected = vec![
        Unsubscribe(Sender, First),
        Unwatch(Sender, First),
        Unsubscribe(Reader, First),
        Unwatch(Reader, First),
        Unsubscribe(Sender, Second),
        Unwatch(Sender, Second),
        Unsubscribe(Reader, Second),
        Unwatch(Reader, Second),
    ];
    if view(&plan)?.intents != expected || plan.history.actor_roots.len() != 4 {
        return Err("pair deduplication or refusal changed actual four-pair ownership".into());
    }
    Ok(())
}

#[test]
fn second_submission_is_refused_without_replacing_any_original_operation() -> ProofResult<()> {
    let mut owned = OwnedMatrixResources::default();
    owned.record_wake_submission(wake_operation()?)?;
    owned.record_instruction_submission(instruction_operation()?)?;
    owned.record_schedule_submission(schedule_operation()?)?;
    if owned.record_wake_submission(different_operation()?)
        != Err(OwnershipError::CreationAlreadySubmitted(CreationKind::Wake))
        || owned.record_instruction_submission(different_operation()?)
            != Err(OwnershipError::CreationAlreadySubmitted(
                CreationKind::Instruction,
            ))
        || owned.record_schedule_submission(different_operation()?)
            != Err(OwnershipError::CreationAlreadySubmitted(
                CreationKind::Schedule,
            ))
    {
        return Err("second creation submission overwrote ownership".into());
    }
    let plan = owned.into_cleanup_plan();
    if view(&plan)?.unresolved
        != vec![
            CreationKind::Wake,
            CreationKind::Instruction,
            CreationKind::Schedule,
        ]
    {
        return Err("rejected second submission replaced an original operation identity".into());
    }
    Ok(())
}

#[test]
fn receipts_require_matching_submission_and_cannot_rebind_known_resources() -> ProofResult<()> {
    let mut owned = OwnedMatrixResources::default();
    if owned.record_wake_receipt(&wake_operation()?, wake_id()?)
        != Err(OwnershipError::ReceiptWithoutSubmission(CreationKind::Wake))
        || owned.record_instruction_receipt(&instruction_operation()?, instruction_id()?)
            != Err(OwnershipError::ReceiptWithoutSubmission(
                CreationKind::Instruction,
            ))
        || owned.record_schedule_receipt(&schedule_operation()?, schedule_id()?)
            != Err(OwnershipError::ReceiptWithoutSubmission(
                CreationKind::Schedule,
            ))
    {
        return Err("receipt without submission was accepted".into());
    }
    owned.record_wake_submission(wake_operation()?)?;
    owned.record_instruction_submission(instruction_operation()?)?;
    owned.record_schedule_submission(schedule_operation()?)?;
    if owned.record_wake_receipt(&different_operation()?, wake_id()?)
        != Err(OwnershipError::ReceiptOperationMismatch(CreationKind::Wake))
        || owned.record_instruction_receipt(&different_operation()?, instruction_id()?)
            != Err(OwnershipError::ReceiptOperationMismatch(
                CreationKind::Instruction,
            ))
        || owned.record_schedule_receipt(&different_operation()?, schedule_id()?)
            != Err(OwnershipError::ReceiptOperationMismatch(
                CreationKind::Schedule,
            ))
    {
        return Err("receipt rebound a different operation".into());
    }
    owned.record_wake_receipt(&wake_operation()?, wake_id()?)?;
    owned.record_instruction_receipt(&instruction_operation()?, instruction_id()?)?;
    owned.record_schedule_receipt(&schedule_operation()?, schedule_id()?)?;
    if owned.record_wake_receipt(
        &wake_operation()?,
        WakeupId::try_from("019f0000-0000-7000-8000-000000000099".to_owned())?,
    ) != Err(OwnershipError::ReceiptAlreadyRecorded(CreationKind::Wake))
        || owned.record_instruction_receipt(
            &instruction_operation()?,
            InstructionId::try_from("019f0000-0000-7000-8000-000000000099".to_owned())?,
        ) != Err(OwnershipError::ReceiptAlreadyRecorded(
            CreationKind::Instruction,
        ))
        || owned.record_schedule_receipt(
            &schedule_operation()?,
            ScheduleId::try_from("019f0000-0000-7000-8000-000000000099".to_owned())?,
        ) != Err(OwnershipError::ReceiptAlreadyRecorded(
            CreationKind::Schedule,
        ))
    {
        return Err("returned-resource rebinding was accepted".into());
    }
    let plan = owned.into_cleanup_plan();
    let actual = view(&plan)?;
    if actual.intents != vec![Intent::CancelWake, Intent::DisableSchedule]
        || actual.wake != History::Known
        || actual.instruction != History::Known
        || actual.schedule != History::Known
        || !actual.unresolved.is_empty()
    {
        return Err("rejected receipts changed original resources or instruction history".into());
    }
    Ok(())
}

#[test]
fn full_owner_has_exact_ten_actions_and_retains_instruction_without_deletion() -> ProofResult<()> {
    let plan = at_prefix(Prefix::EnableFailed)?.into_cleanup_plan();
    let actual = view(&plan)?;
    use Actor::{Reader, Sender};
    use Intent::{CancelWake, DisableSchedule, Unsubscribe, Unwatch};
    use Root::{First, Second};
    let expected = vec![
        CancelWake,
        DisableSchedule,
        Unsubscribe(Sender, First),
        Unwatch(Sender, First),
        Unsubscribe(Reader, First),
        Unwatch(Reader, First),
        Unsubscribe(Sender, Second),
        Unwatch(Sender, Second),
        Unsubscribe(Reader, Second),
        Unwatch(Reader, Second),
    ];
    if actual.intents != expected
        || actual.intents.len() != 10
        || actual.instruction != History::Known
        || !actual.unresolved.is_empty()
    {
        return Err(
            "full owned resources exceeded exact cleanup bounds or deleted instruction history"
                .into(),
        );
    }
    Ok(())
}

#[test]
fn schedule_only_ownership_does_not_require_or_invent_other_creation_slots() -> ProofResult<()> {
    let mut owned = OwnedMatrixResources::default();
    owned.record_schedule_submission(schedule_operation()?)?;
    owned.record_schedule_receipt(&schedule_operation()?, schedule_id()?)?;
    let expected = PlanView {
        intents: vec![Intent::DisableSchedule],
        unresolved: vec![],
        wake: History::Absent,
        instruction: History::Absent,
        schedule: History::Known,
    };
    if view(&owned.into_cleanup_plan())? != expected {
        return Err(
            "independent schedule ownership invented or required unrelated resources".into(),
        );
    }
    Ok(())
}
