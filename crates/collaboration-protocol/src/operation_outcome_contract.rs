//! Method-specific outcomes cannot pair a schedule command with an unrelated result type.
use crate::{
    AutomationConfiguration, InstructionSnapshot, RunSnapshot, ScheduleSnapshot,
    WakeMutationResult, WakeSnapshot,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

macro_rules! operation_methods {
    ($name:ident { $($variant:ident => $method:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename = $method)] $variant),+ }
        impl $name { #[must_use] pub const fn as_str(self) -> &'static str { match self { $(Self::$variant => $method),+ } } }
    };
}
operation_methods!(AutomationOperationMethod {
    InstructionCreate => "instruction/create", InstructionUpdate => "instruction/update",
    ScheduleCreate => "schedule/create", ScheduleUpdate => "schedule/update", ScheduleEnable => "schedule/enable", ScheduleDisable => "schedule/disable", SchedulePrepare => "schedule/prepare", ScheduleImport => "schedule/import",
    WakeSend => "wake/send", WakePause => "wake/pause", WakeResume => "wake/resume", WakeCancel => "wake/cancel",
    SummaryRetry => "run/summaryRetry", SummarySkip => "run/summarySkip", Configure => "automation/configure",
});
operation_methods!(InstructionOperation { Create => "instruction/create", Update => "instruction/update" });
operation_methods!(ScheduleOperation { Create => "schedule/create", Update => "schedule/update", Enable => "schedule/enable", Disable => "schedule/disable", Prepare => "schedule/prepare", Import => "schedule/import" });
operation_methods!(WakeCreationOperation { Send => "wake/send" });
operation_methods!(WakeMutationOperation { Pause => "wake/pause", Resume => "wake/resume", Cancel => "wake/cancel" });
operation_methods!(RunRecoveryOperation { Retry => "run/summaryRetry", Skip => "run/summarySkip" });
operation_methods!(ConfigurationOperation { Configure => "automation/configure" });

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum OperationSuccess {
    Instruction {
        method: InstructionOperation,
        result: Box<InstructionSnapshot>,
    },
    Schedule {
        method: ScheduleOperation,
        result: Box<ScheduleSnapshot>,
    },
    WakeCreated {
        method: WakeCreationOperation,
        result: Box<WakeSnapshot>,
    },
    WakeChanged {
        method: WakeMutationOperation,
        result: Box<WakeMutationResult>,
    },
    RunRecovery {
        method: RunRecoveryOperation,
        result: Box<RunSnapshot>,
    },
    Configuration {
        method: ConfigurationOperation,
        result: AutomationConfiguration,
    },
}
impl OperationSuccess {
    #[must_use]
    pub const fn method_name(&self) -> &'static str {
        match self {
            Self::Instruction { method, .. } => method.as_str(),
            Self::Schedule { method, .. } => method.as_str(),
            Self::WakeCreated { method, .. } => method.as_str(),
            Self::WakeChanged { method, .. } => method.as_str(),
            Self::RunRecovery { method, .. } => method.as_str(),
            Self::Configuration { method, .. } => method.as_str(),
        }
    }
    #[must_use]
    pub fn resource_id(&self) -> &str {
        match self {
            Self::Instruction { result, .. } => result.instruction_id.as_str(),
            Self::Schedule { result, .. } => result.schedule_id.as_str(),
            Self::WakeCreated { result, .. } => result.definition.wakeup_id.as_str(),
            Self::WakeChanged { result, .. } => result.wakeup.definition.wakeup_id.as_str(),
            Self::RunRecovery { result, .. } => result.run_id.as_str(),
            Self::Configuration { .. } => "automationConfiguration",
        }
    }
}
