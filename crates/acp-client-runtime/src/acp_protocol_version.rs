//! Client-owned ACP protocol version at the public error boundary.

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct AcpProtocolVersion(u16);

impl AcpProtocolVersion {
    pub const fn new(version: u16) -> Self {
        Self(version)
    }

    pub(crate) const fn from_sdk(version: agent_client_protocol::schema::ProtocolVersion) -> Self {
        Self(version.as_u16())
    }
}

impl std::fmt::Debug for AcpProtocolVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Keep the pre-extraction diagnostic text stable for Host callers.
        formatter
            .debug_tuple("ProtocolVersion")
            .field(&self.0)
            .finish()
    }
}
