//! Portable work carries instructions and continuity, never live native destination bindings.
use crate::{InstructionId, InstructionText, ScheduleDefinition, ScheduleId};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortableInstruction {
    pub instruction_id: InstructionId,
    pub text: InstructionText,
    pub source_revision_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortableContinuity<TTarget> {
    pub text: InstructionText,
    pub source_run_id: String,
    pub source_target: TTarget,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortableSchedulePackage<TTarget, TEndpoint> {
    pub schedule_id: ScheduleId,
    pub source_change_id: String,
    pub instruction: PortableInstruction,
    pub definition: ScheduleDefinition<TTarget, TEndpoint>,
    pub continuity: Option<PortableContinuity<TTarget>>,
}
#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error("schedule package exceeds the supported UTF-8 frame budget")]
    TooLarge,
    #[error("schedule package must contain ordered complete LF-terminated JSONL records")]
    InvalidEncoding,
    #[error("schedule package version, record kind or fields are unsupported")]
    InvalidRecord,
    #[error("schedule package footer count or SHA-256 does not match its exact source lines")]
    IntegrityMismatch,
    #[error("portable instruction identity or unprepared destination is inconsistent")]
    InvalidDefinition,
}
