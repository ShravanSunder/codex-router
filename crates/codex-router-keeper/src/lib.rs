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
