//! Request context shared by native Control methods and the app-server client route.
use crate::NativeControlBackend;
use collaboration_protocol::{EndpointDescription, UuidIdentity};
use serde_json::Value;

pub(crate) struct NativeControlRequest<'a> {
    pub method: &'a str,
    pub params: Value,
    pub id: Value,
    pub service_id: &'a UuidIdentity,
    pub backend: Option<&'a NativeControlBackend>,
    pub endpoints: &'a [EndpointDescription],
    pub stored_observation:
        Option<crate::stored_inventory_observation::StoredInventoryObservation<'a>>,
    pub access_routes: Option<&'a crate::ServiceApprovalBroker>,
}
