//! Private ownership metadata for one matrix run; no service calls or cleanup execution.
use collaboration_client::{
    board::{Identity, MessageId},
    protocol::{InstructionId, OperationId, ScheduleId, WakeupId},
};
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CreationKind {
    Wake,
    Instruction,
    Schedule,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum OwnershipError {
    CreationAlreadySubmitted(CreationKind),
    ReceiptWithoutSubmission(CreationKind),
    ReceiptAlreadyRecorded(CreationKind),
    ReceiptOperationMismatch(CreationKind),
    ActorRootLimitReached,
}
impl fmt::Display for OwnershipError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreationAlreadySubmitted(kind) => write!(
                formatter,
                "{kind:?} creation is already owned; second submission refused"
            ),
            Self::ReceiptWithoutSubmission(kind) => {
                write!(formatter, "{kind:?} receipt has no recorded submission")
            }
            Self::ReceiptAlreadyRecorded(kind) => write!(
                formatter,
                "{kind:?} returned resource is already recorded; rebinding refused"
            ),
            Self::ReceiptOperationMismatch(kind) => write!(
                formatter,
                "{kind:?} receipt operation differs from recorded submission"
            ),
            Self::ActorRootLimitReached => {
                formatter.write_str("matrix ownership is limited to four distinct actor/root pairs")
            }
        }
    }
}
impl std::error::Error for OwnershipError {}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum WakeCreation {
    Submitted {
        operation_id: OperationId,
    },
    Known {
        operation_id: OperationId,
        wakeup_id: WakeupId,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum InstructionCreation {
    Submitted {
        operation_id: OperationId,
    },
    Known {
        operation_id: OperationId,
        instruction_id: InstructionId,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum ScheduleCreation {
    Submitted {
        operation_id: OperationId,
    },
    Known {
        operation_id: OperationId,
        schedule_id: ScheduleId,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct OwnedActorRoot {
    pub(super) actor: Identity,
    pub(super) root_message_id: MessageId,
}

#[derive(Debug, Default)]
pub(super) struct OwnedMatrixResources {
    wake: Option<WakeCreation>,
    instruction: Option<InstructionCreation>,
    schedule: Option<ScheduleCreation>,
    actor_roots: Vec<OwnedActorRoot>,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum CleanupIntent {
    CancelWake(WakeupId),
    DisableSchedule(ScheduleId),
    Unsubscribe {
        actor: Identity,
        root_message_id: MessageId,
    },
    Unwatch {
        actor: Identity,
        root_message_id: MessageId,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum UnresolvedCreation {
    Wake { operation_id: OperationId },
    Instruction { operation_id: OperationId },
    Schedule { operation_id: OperationId },
}

#[derive(Debug)]
pub(super) struct MatrixCreationHistory {
    pub(super) wake: Option<WakeCreation>,
    pub(super) instruction: Option<InstructionCreation>,
    pub(super) schedule: Option<ScheduleCreation>,
    pub(super) actor_roots: Vec<OwnedActorRoot>,
}

#[derive(Debug)]
pub(super) struct MatrixCleanupPlan {
    pub(super) intents: Vec<CleanupIntent>,
    pub(super) unresolved: Vec<UnresolvedCreation>,
    pub(super) history: MatrixCreationHistory,
}

impl OwnedMatrixResources {
    pub(super) fn record_wake_submission(
        &mut self,
        operation_id: OperationId,
    ) -> Result<(), OwnershipError> {
        if self.wake.is_some() {
            return Err(OwnershipError::CreationAlreadySubmitted(CreationKind::Wake));
        }
        self.wake = Some(WakeCreation::Submitted { operation_id });
        Ok(())
    }
    pub(super) fn record_wake_receipt(
        &mut self,
        operation_id: &OperationId,
        wakeup_id: WakeupId,
    ) -> Result<(), OwnershipError> {
        let recorded_operation = match self.wake.as_ref() {
            None => return Err(OwnershipError::ReceiptWithoutSubmission(CreationKind::Wake)),
            Some(WakeCreation::Known { .. }) => {
                return Err(OwnershipError::ReceiptAlreadyRecorded(CreationKind::Wake));
            }
            Some(WakeCreation::Submitted {
                operation_id: recorded,
            }) if recorded == operation_id => recorded.clone(),
            Some(WakeCreation::Submitted { .. }) => {
                return Err(OwnershipError::ReceiptOperationMismatch(CreationKind::Wake));
            }
        };
        self.wake = Some(WakeCreation::Known {
            operation_id: recorded_operation,
            wakeup_id,
        });
        Ok(())
    }
    pub(super) fn record_instruction_submission(
        &mut self,
        operation_id: OperationId,
    ) -> Result<(), OwnershipError> {
        if self.instruction.is_some() {
            return Err(OwnershipError::CreationAlreadySubmitted(
                CreationKind::Instruction,
            ));
        }
        self.instruction = Some(InstructionCreation::Submitted { operation_id });
        Ok(())
    }
    pub(super) fn record_instruction_receipt(
        &mut self,
        operation_id: &OperationId,
        instruction_id: InstructionId,
    ) -> Result<(), OwnershipError> {
        let recorded_operation = match self.instruction.as_ref() {
            None => {
                return Err(OwnershipError::ReceiptWithoutSubmission(
                    CreationKind::Instruction,
                ));
            }
            Some(InstructionCreation::Known { .. }) => {
                return Err(OwnershipError::ReceiptAlreadyRecorded(
                    CreationKind::Instruction,
                ));
            }
            Some(InstructionCreation::Submitted {
                operation_id: recorded,
            }) if recorded == operation_id => recorded.clone(),
            Some(InstructionCreation::Submitted { .. }) => {
                return Err(OwnershipError::ReceiptOperationMismatch(
                    CreationKind::Instruction,
                ));
            }
        };
        self.instruction = Some(InstructionCreation::Known {
            operation_id: recorded_operation,
            instruction_id,
        });
        Ok(())
    }
    pub(super) fn record_schedule_submission(
        &mut self,
        operation_id: OperationId,
    ) -> Result<(), OwnershipError> {
        if self.schedule.is_some() {
            return Err(OwnershipError::CreationAlreadySubmitted(
                CreationKind::Schedule,
            ));
        }
        self.schedule = Some(ScheduleCreation::Submitted { operation_id });
        Ok(())
    }
    pub(super) fn record_schedule_receipt(
        &mut self,
        operation_id: &OperationId,
        schedule_id: ScheduleId,
    ) -> Result<(), OwnershipError> {
        let recorded_operation = match self.schedule.as_ref() {
            None => {
                return Err(OwnershipError::ReceiptWithoutSubmission(
                    CreationKind::Schedule,
                ));
            }
            Some(ScheduleCreation::Known { .. }) => {
                return Err(OwnershipError::ReceiptAlreadyRecorded(
                    CreationKind::Schedule,
                ));
            }
            Some(ScheduleCreation::Submitted {
                operation_id: recorded,
            }) if recorded == operation_id => recorded.clone(),
            Some(ScheduleCreation::Submitted { .. }) => {
                return Err(OwnershipError::ReceiptOperationMismatch(
                    CreationKind::Schedule,
                ));
            }
        };
        self.schedule = Some(ScheduleCreation::Known {
            operation_id: recorded_operation,
            schedule_id,
        });
        Ok(())
    }
    pub(super) fn register_actor_root(
        &mut self,
        actor: Identity,
        root_message_id: MessageId,
    ) -> Result<(), OwnershipError> {
        let pair = OwnedActorRoot {
            actor,
            root_message_id,
        };
        if self.actor_roots.contains(&pair) {
            return Ok(());
        }
        if self.actor_roots.len() >= 4 {
            return Err(OwnershipError::ActorRootLimitReached);
        }
        self.actor_roots.push(pair);
        Ok(())
    }

    pub(super) fn into_cleanup_plan(self) -> MatrixCleanupPlan {
        let mut intents = Vec::new();
        let mut unresolved = Vec::new();
        match self.wake.as_ref() {
            Some(WakeCreation::Known { wakeup_id, .. }) => {
                intents.push(CleanupIntent::CancelWake(wakeup_id.clone()))
            }
            Some(WakeCreation::Submitted { operation_id }) => {
                unresolved.push(UnresolvedCreation::Wake {
                    operation_id: operation_id.clone(),
                })
            }
            None => {}
        }
        match self.instruction.as_ref() {
            Some(InstructionCreation::Submitted { operation_id }) => {
                unresolved.push(UnresolvedCreation::Instruction {
                    operation_id: operation_id.clone(),
                })
            }
            Some(InstructionCreation::Known { .. }) | None => {}
        }
        match self.schedule.as_ref() {
            Some(ScheduleCreation::Known { schedule_id, .. }) => {
                intents.push(CleanupIntent::DisableSchedule(schedule_id.clone()))
            }
            Some(ScheduleCreation::Submitted { operation_id }) => {
                unresolved.push(UnresolvedCreation::Schedule {
                    operation_id: operation_id.clone(),
                })
            }
            None => {}
        }
        for pair in &self.actor_roots {
            intents.push(CleanupIntent::Unsubscribe {
                actor: pair.actor.clone(),
                root_message_id: pair.root_message_id.clone(),
            });
            intents.push(CleanupIntent::Unwatch {
                actor: pair.actor.clone(),
                root_message_id: pair.root_message_id.clone(),
            });
        }
        MatrixCleanupPlan {
            intents,
            unresolved,
            history: MatrixCreationHistory {
                wake: self.wake,
                instruction: self.instruction,
                schedule: self.schedule,
                actor_roots: self.actor_roots,
            },
        }
    }
}

#[cfg(test)]
#[path = "owned_resources_tests.rs"]
mod tests;
