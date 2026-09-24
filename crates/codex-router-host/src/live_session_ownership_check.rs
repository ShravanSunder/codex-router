//! Provider load veto for a session currently owned by another live client.
use collaboration_protocol::SessionRef;
use collaboration_service::DeliveryFuture;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveSessionOwnership {
    NotLive,
    LiveWritable,
    LiveUnsupported,
}

pub trait LiveSessionOwnershipCheck: Send + Sync {
    fn check<'a>(&'a self, target: &'a SessionRef) -> DeliveryFuture<'a, LiveSessionOwnership>;
}
