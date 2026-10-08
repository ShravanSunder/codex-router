//! The localhost address the collaboration API's TCP listener binds.
use std::{net::SocketAddr, str::FromStr};

/// A socket address on the loopback interface; nothing else may serve the API over TCP.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoopbackBindAddress(SocketAddr);

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum LoopbackBindAddressError {
    #[error("MCP listener address must be loopback")]
    NonLoopback,
    #[error("invalid MCP listener address")]
    InvalidAddress,
}

impl LoopbackBindAddress {
    pub fn new(address: SocketAddr) -> Result<Self, LoopbackBindAddressError> {
        if !address.ip().is_loopback() {
            return Err(LoopbackBindAddressError::NonLoopback);
        }
        Ok(Self(address))
    }

    pub fn parse(value: &str) -> Result<Self, LoopbackBindAddressError> {
        let address =
            SocketAddr::from_str(value).map_err(|_| LoopbackBindAddressError::InvalidAddress)?;
        Self::new(address)
    }

    #[must_use]
    pub const fn socket_addr(&self) -> SocketAddr {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::LoopbackBindAddress;

    #[test]
    fn only_loopback_addresses_are_accepted() {
        assert!(LoopbackBindAddress::parse("127.0.0.1:0").is_ok());
        assert!(LoopbackBindAddress::parse("[::1]:0").is_ok());
        assert!(LoopbackBindAddress::parse("0.0.0.0:0").is_err());
        assert!(LoopbackBindAddress::parse("192.0.2.10:8080").is_err());
    }
}
