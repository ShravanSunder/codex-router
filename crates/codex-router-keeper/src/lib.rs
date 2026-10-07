//! Retained singleton and listener authority; business policy belongs to role consumers.
mod listener_address;
mod listener_registry;
mod registry_error;
mod singleton_authority;
pub use listener_address::ListenerAddress;
pub use listener_registry::{GrantedListener, ListenerRegistry};
pub use registry_error::RegistryError;
pub use singleton_authority::SingletonAuthority;

mod generation_publication;
mod publication_error;
mod publication_node;
pub use generation_publication::GenerationEndpointPublisher;
pub use publication_error::PublicationError;

mod group_stop_error;
mod group_stop_progress;
pub mod lifecycle_bounds;
mod owned_process_group;
pub use codex_router_keeper_protocol::GroupStopResult;
pub use group_stop_error::GroupStopError;
pub use group_stop_progress::{GroupStopProgress, GroupStopStatus};
pub use lifecycle_bounds::{GroupStopProfile, GroupStopTiming};
pub use owned_process_group::OwnedProcessGroup;

mod image_error;
pub use image_error::ImageError;
mod image_directory;
mod image_identity;
mod image_warmup;
pub use lifecycle_bounds::PREPARE_DEADLINE;
mod retained_image_registry;
pub use retained_image_registry::{ImageLease, ImageRegistry};
mod slot_image_state;
pub use slot_image_state::{ImageCommitRelease, SlotImageState};
