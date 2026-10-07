//! Pipe frames and separate process-fatal descriptor grants, without business-role schemas.
mod pipe_frame;
pub use pipe_frame::{JsonMessage, MAX_FRAME_BYTES, PipeFrameReader, PipeFrameWriter};

mod descriptor_grant;
pub use descriptor_grant::{
    DescriptorSpec, GrantEnvelope, GrantPhase, GrantReceiver, GrantSender, ValidatedGrant,
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
