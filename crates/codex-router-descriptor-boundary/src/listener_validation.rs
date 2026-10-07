//! Kernel unconnected stream shape/address validation without logical endpoint or role ownership.
use crate::{BoundaryError, boundary_error::io_error};
use std::{net::SocketAddr, os::fd::BorrowedFd, path::Path};
fn validate_unconnected(descriptor: BorrowedFd<'_>) -> Result<(), BoundaryError> {
    if rustix::net::sockopt::socket_type(descriptor).map_err(io_error)?
        != rustix::net::SocketType::STREAM
    {
        return Err(BoundaryError::DescriptorKind);
    }
    match rustix::net::getpeername(descriptor) {
        Err(rustix::io::Errno::NOTCONN) => Ok(()),
        _ => Err(BoundaryError::DescriptorKind),
    }
}
pub fn validate_unconnected_unix_stream(
    descriptor: BorrowedFd<'_>,
    path: &Path,
) -> Result<(), BoundaryError> {
    if !path.is_absolute() {
        return Err(BoundaryError::DescriptorKind);
    }
    validate_unconnected(descriptor)?;
    let expected =
        rustix::net::SocketAddrAny::from(rustix::net::SocketAddrUnix::new(path).map_err(io_error)?);
    if rustix::net::getsockname(descriptor).map_err(io_error)? != expected {
        return Err(BoundaryError::DescriptorKind);
    }
    Ok(())
}
pub fn validate_unconnected_tcp_stream(
    descriptor: BorrowedFd<'_>,
    address: SocketAddr,
) -> Result<(), BoundaryError> {
    validate_unconnected(descriptor)?;
    if rustix::net::getsockname(descriptor).map_err(io_error)?
        != rustix::net::SocketAddrAny::from(address)
    {
        return Err(BoundaryError::DescriptorKind);
    }
    Ok(())
}
