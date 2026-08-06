//! Foreground lifecycle control for one shared Codex app-server.

mod config;
mod domain;
mod instance;
mod operator_protocol;

pub use config::HostConfig;
pub use config::HostConfigInputs;
pub use config::HostCoordinationPaths;
pub use domain::AppServerCondition;
pub use domain::ExecutableRelation;
pub use domain::HostOperation;
pub use domain::HostPhase;
pub use domain::HostSnapshot;
pub use domain::HostSnapshotDimensions;
pub use domain::HostedReadiness;
pub use domain::LifecycleOutcome;
pub use domain::LifecycleOutcomeClassification;
pub use domain::RecoveryBudget;
pub use domain::RemoteControlCondition;
pub use domain::RouterCondition;
pub use instance::HostInstance;
pub use instance::InstanceAcquireError;
pub use instance::inherited_lock_marker;
pub use operator_protocol::OPERATOR_PROTOCOL_VERSION;
pub use operator_protocol::OperatorFrame;
pub use operator_protocol::OperatorProtocolError;
pub use operator_protocol::OperatorRequest;
pub use operator_protocol::TerminalClassification;
pub use operator_protocol::decode_operator_frame;
pub use operator_protocol::decode_operator_request;
pub use operator_protocol::encode_operator_frame;
pub use operator_protocol::encode_operator_request;
