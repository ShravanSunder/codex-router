//! Pipe frames and separate process-fatal descriptor grants, without business-role schemas.
mod pipe_frame;
pub use pipe_frame::{JsonMessage, MAX_FRAME_BYTES, PipeFrameReader, PipeFrameWriter};

mod child_grant;
pub use child_grant::{ChildGrantFrame, ChildGrantFrameError, ChildLaunchContext};

mod descriptor_grant;
pub use descriptor_grant::{
    ChildGrantExpectation, DescriptorSpec, GrantReceiver, GrantSender, ReceivedChildGrant,
};

mod listener_kind;
pub use listener_kind::ListenerKind;

mod app_server_generation;
pub use app_server_generation::{
    GenerationId, GenerationIdentityError, GenerationNumber, KeeperEpoch,
};
mod default_endpoint;
pub use default_endpoint::{DefaultEndpointPath, EndpointPathError, GenerationAliasPath};

mod child_process_identity;
pub use child_process_identity::{ChildIdentityError, ChildPgid, ChildPid};

mod group_stop_result;
pub use group_stop_result::GroupStopResult;

mod component_fingerprint;
pub use component_fingerprint::{ComponentFingerprint, FingerprintError};
mod component_kind;
pub use component_kind::ComponentKind;
mod build_info;
pub use build_info::{BuildInfo, ComponentFingerprints};
mod slot_image;
pub use slot_image::{SlotImage, SlotImageError};
mod generation_evidence;
pub use generation_evidence::{
    GenerationEvidence, GenerationEvidenceError, GenerationSchemaAvailability,
    SchemaUnavailableReason,
};
mod generation_current_payload;
pub use generation_current_payload::GenerationCurrentPayload;
mod generation_preparation;
pub use generation_preparation::GenerationPreparation;

mod native_probe_failure;
pub use native_probe_failure::{NativeProbeFailure, NativeProbeStage};
mod native_probe_job;
pub use native_probe_job::{NativeProbeJob, NativeProbeJobConversionError, NativeProbeJobWire};
mod native_probe_result;
pub use native_probe_result::{
    NativeProbeResult, NativeProbeResultWire, RemoteControlObservationWire,
};
mod receiver_hello;
pub use receiver_hello::ReceiverHelloWire;
