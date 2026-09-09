//! Public communication contracts without process, storage or transport ownership.
mod cli_output_contract;
pub use cli_output_contract::{
    ConversationEffect, ConversationRecord, ConversationStage, FiniteCommandRecord,
    NativeObservationRecord, ObservationCloseReason,
};
mod backend_generation;
mod control_error_validation;
mod control_schema_document;
pub use control_error_validation::control_error_is_valid;
pub use control_schema_document::control_schema_document;
mod control_schema_identity;
pub use control_schema_identity::{ControlSchema, ControlSchemaError};
mod message_content;
mod native_schema_references;
mod native_session_catalog;
pub use message_content::{
    AcceptedResumeEffect, MessageContent, MessageDelivery, MessageInputKind, MessageRepresentation,
    MessageText, MessageTextError,
};
pub use native_control_contract::{
    NativeInputDisposition, NativeInputOperation, NativeSendAcceptance, NativeSendParams,
    NativeSendReceipt,
};
pub use native_session_catalog::{
    NativeSessionListParams, NativeSessionListResult, NativeSessionObservation,
    NativeSessionSummary, NativeSessionView,
};
mod control_initialization;
mod endpoint_inventory;
pub use control_initialization::{
    ControlClientInfo, ControlInitializationParams, ControlInitializationResult, ProtocolVersion,
};
pub use endpoint_inventory::{EndpointChange, EndpointInventory};
mod identity_schemas;
mod protocol_type_schemas;
pub use protocol_type_schemas::protocol_type_schemas;
mod native_control_contract;
pub use native_control_contract::{
    NativeInspectParams, NativeInspectResult, NativeInterruptKind, NativeInterruptParams,
    NativeInterruptResult,
};
mod endpoint_identity;
pub use backend_generation::{CodexGeneration, GenerationNumber};
pub use endpoint_identity::{
    EndpointId, EndpointRef, IdentityError, SessionId, SessionRef, UuidIdentity,
};
mod control_frame_decoder;
pub use control_frame_decoder::{ControlFrameDecoder, FrameError, MAX_CONTROL_FRAME_BYTES};
mod request_admission;
pub use request_admission::{AdmissionError, ControlAdmission};

mod endpoint_description;
pub use endpoint_description::{
    AcpCarrier, ChannelDescription, EndpointAvailability, EndpointDescription, NativeCarrier,
    ObservationTimestamp, SchemaDigest,
};

pub use endpoint_description::NonEmptyText;

mod service_manifest;
pub use service_manifest::{ControlSelector, ControlSocketPath, ControlTransport, ServiceManifest};
mod lifecycle_observation;
pub use lifecycle_observation::{
    BackendStatus, LifecycleChange, LifecycleObservation, LifecycleSubject, NativeActiveFlag,
    NativeThreadStatus, ObservationOrdering, ObservationScope, ObservationSource, TerminalStatus,
    ThreadAddress,
};

mod journal_contract;
pub use journal_contract::{
    AddressListParams, JournalBounds, JournalPage, JournalPosition, JournalReadParams,
    JournalStatus, LifecycleRecord,
};

mod address_book_contract;
pub use address_book_contract::{
    AddressEntry, AddressPage, ArchiveState, CoverageState, CoverageView, Existence,
    LifecycleDisposition, StatusOrdering,
};

mod acp_schema_catalog;
pub use acp_schema_catalog::{
    ACP_SCHEMA_BYTES, ACP_SCHEMA_DIGEST, AcpSchemaCatalog, AcpSchemaError,
};

mod instruction_contract;
pub use agent_automation::{InstructionId, InstructionText, OperationId, RevisionId, WakeupId};
pub use instruction_contract::{
    InstructionCreateParams, InstructionShowParams, InstructionSnapshot, InstructionUpdateParams,
};
pub use instruction_contract::{
    InstructionFailure, InstructionFailureKind, InstructionNextAction, InstructionStage,
    LocalMutationEvidence, LocalMutationState,
};
mod automation_timing_contract;
pub use automation_timing_contract::{
    ExpiryRequest, InvalidSeconds, PositiveSeconds, TimingRequest,
};
mod wakeup_contract;
pub use wakeup_contract::{
    FireKind, FireReceipt, SavedMessage, WakeDefinition, WakeMutationRequest, WakeSendRequest,
    WakeShowRequest, WakeSnapshot, WakeState,
};
mod wakeup_failure;
pub use wakeup_failure::{WakeFailure, WakeFailureReason, WakeFailureStage, WakeNextAction};
mod delivery_inspection_contract;
pub use delivery_inspection_contract::{
    CessationEvidence, DeliveryDisposition, DeliveryEvidence, DeliveryInspection,
    DeliveryShowRequest, DeliverySource, NativeEffectEvidence, PreparationEffect, SubmissionEffect,
    WakeMutationResult,
};

mod wakeup_subscription_contract;
pub use wakeup_subscription_contract::{
    NoMutation, UnknownFire, WaitNextAction, WaitStage, WaitUnavailable, WaitUnavailableEffects,
    WaitUnavailableKind, WakeChange, WakeChanged, WakeSubscription,
};

pub use wakeup_subscription_contract::{
    UnknownFirstFire, VerifyWakeupAddress, WakeNotFound, WakeNotFoundKind,
};
mod automation_page_contract;
pub use automation_page_contract::{
    AutomationPage, AutomationPageRequest, InvalidPageLimit, PageLimit,
};
