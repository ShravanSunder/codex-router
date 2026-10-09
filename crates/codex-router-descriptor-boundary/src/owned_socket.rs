//! Gated socket creation and duplication; readiness never holds the creation gate.
use crate::{
    BoundaryError, DescriptorGate, MAX_RIGHTS, boundary_error::io_error, owned_pipe::configure,
};
use rustix::net::{
    AddressFamily, SendAncillaryBuffer, SendAncillaryMessage, SendFlags, SocketAddrUnix, SocketType,
};
use std::{
    io::IoSlice,
    mem::MaybeUninit,
    net::SocketAddr,
    os::fd::{AsFd, BorrowedFd, OwnedFd},
    path::Path,
    task::{Context, Poll},
};
use tokio::io::unix::AsyncFd;
pub struct OwnedSocket {
    pub(crate) descriptor: AsyncFd<OwnedFd>,
}
pub struct OwnedListener {
    descriptor: AsyncFd<OwnedFd>,
}
pub struct SocketWriter {
    socket: OwnedSocket,
}
fn configure_socket(fd: &OwnedFd) -> Result<(), BoundaryError> {
    configure(fd)?;
    #[cfg(target_vendor = "apple")]
    rustix::net::sockopt::set_socket_nosigpipe(fd, true).map_err(io_error)?;
    Ok(())
}
fn send_bytes(
    descriptor: &OwnedFd,
    bytes: &[u8],
    rights: &[BorrowedFd<'_>],
) -> std::io::Result<usize> {
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_RIGHTS))];
    let mut control = SendAncillaryBuffer::new(&mut space);
    if !rights.is_empty() && !control.push(SendAncillaryMessage::ScmRights(rights)) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "rights capacity",
        ));
    }
    let flags = SendFlags::DONTWAIT;
    #[cfg(not(target_vendor = "apple"))]
    let flags = flags | SendFlags::NOSIGNAL;
    rustix::net::sendmsg(descriptor, &[IoSlice::new(bytes)], &mut control, flags)
        .map_err(std::io::Error::from)
}
impl OwnedSocket {
    pub async fn pair(gate: &DescriptorGate) -> Result<(Self, Self), BoundaryError> {
        let _shared = gate.creation().await;
        let (first, second) = rustix::net::socketpair(
            AddressFamily::UNIX,
            SocketType::STREAM,
            rustix::net::SocketFlags::empty(),
            None,
        )
        .map_err(io_error)?;
        configure_socket(&first)?;
        configure_socket(&second)?;
        Ok((
            Self {
                descriptor: AsyncFd::new(first)?,
            },
            Self {
                descriptor: AsyncFd::new(second)?,
            },
        ))
    }
    pub async fn from_owned(fd: OwnedFd, gate: &DescriptorGate) -> Result<Self, BoundaryError> {
        let _shared = gate.creation().await;
        if rustix::net::sockopt::socket_type(&fd).map_err(io_error)? != SocketType::STREAM {
            return Err(BoundaryError::DescriptorKind);
        }
        configure_socket(&fd)?;
        Ok(Self {
            descriptor: AsyncFd::new(fd)?,
        })
    }
    pub async fn duplicate(&self, gate: &DescriptorGate) -> Result<Self, BoundaryError> {
        let _shared = gate.creation().await;
        let fd = rustix::io::dup(self.as_fd()).map_err(io_error)?;
        configure_socket(&fd)?;
        Ok(Self {
            descriptor: AsyncFd::new(fd)?,
        })
    }
    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.descriptor.get_ref().as_fd()
    }
    pub fn into_owned(self) -> OwnedFd {
        self.descriptor.into_inner()
    }
    pub fn into_writer(self) -> SocketWriter {
        SocketWriter { socket: self }
    }
    pub async fn connect_unix(path: &Path, gate: &DescriptorGate) -> Result<Self, BoundaryError> {
        let address = SocketAddrUnix::new(path).map_err(io_error)?;
        let socket = {
            let _shared = gate.creation().await;
            let fd = rustix::net::socket(AddressFamily::UNIX, SocketType::STREAM, None)
                .map_err(io_error)?;
            configure_socket(&fd)?;
            Self {
                descriptor: AsyncFd::new(fd)?,
            }
        };
        match rustix::net::connect(socket.as_fd(), &address) {
            Ok(()) => {}
            Err(error)
                if error == rustix::io::Errno::INPROGRESS
                    || error == rustix::io::Errno::WOULDBLOCK =>
            {
                let _ready = socket.descriptor.writable().await?;
                rustix::net::sockopt::socket_error(socket.as_fd())
                    .map_err(io_error)?
                    .map_err(io_error)?;
            }
            Err(error) => return Err(io_error(error)),
        }
        Ok(socket)
    }
    /// Duplicate inherited stdin safely and replace stdin so the duplicate is its sole carrier owner.
    pub async fn inherit_stdin(gate: &DescriptorGate) -> Result<Self, BoundaryError> {
        let fd = {
            let _shared = gate.creation().await;
            let fd = rustix::io::dup(rustix::stdio::stdin()).map_err(io_error)?;
            configure_socket(&fd)?;
            let null = std::fs::File::open("/dev/null")?;
            rustix::stdio::dup2_stdin(&null).map_err(io_error)?;
            fd
        };
        Self::from_owned(fd, gate).await
    }
    pub async fn connect_tcp(
        address: SocketAddr,
        gate: &DescriptorGate,
    ) -> Result<Self, BoundaryError> {
        let family = if address.is_ipv4() {
            AddressFamily::INET
        } else {
            AddressFamily::INET6
        };
        let socket = {
            let _shared = gate.creation().await;
            let fd = rustix::net::socket(family, SocketType::STREAM, None).map_err(io_error)?;
            configure_socket(&fd)?;
            Self {
                descriptor: AsyncFd::new(fd)?,
            }
        };
        match rustix::net::connect(socket.as_fd(), &address) {
            Ok(()) => {}
            Err(error)
                if error == rustix::io::Errno::INPROGRESS
                    || error == rustix::io::Errno::WOULDBLOCK =>
            {
                let _ready = socket.descriptor.writable().await?;
                rustix::net::sockopt::socket_error(socket.as_fd())
                    .map_err(io_error)?
                    .map_err(io_error)?;
            }
            Err(error) => return Err(io_error(error)),
        }
        Ok(socket)
    }
    pub async fn send(
        &self,
        bytes: &[u8],
        rights: &[BorrowedFd<'_>],
    ) -> Result<usize, BoundaryError> {
        if rights.len() > MAX_RIGHTS {
            return Err(BoundaryError::TooLarge);
        }
        loop {
            let mut ready = self.descriptor.writable().await?;
            match ready.try_io(|fd| send_bytes(fd.get_ref(), bytes, rights)) {
                Err(_) => continue,
                Ok(Err(error)) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Ok(result) => return result.map_err(Into::into),
            }
        }
    }
    pub(crate) fn poll_send_without_rights(
        &self,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        loop {
            let mut ready = match self.descriptor.poll_write_ready(context) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(ready)) => ready,
            };
            match ready.try_io(|fd| send_bytes(fd.get_ref(), bytes, &[])) {
                Err(_) => continue,
                Ok(Err(error)) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Ok(result) => return Poll::Ready(result),
            }
        }
    }
    pub(crate) fn shutdown_write(&self) -> Result<(), BoundaryError> {
        rustix::net::shutdown(self.as_fd(), rustix::net::Shutdown::Write).map_err(io_error)
    }
}
impl SocketWriter {
    pub async fn write_all(&self, bytes: &[u8]) -> Result<(), BoundaryError> {
        let mut written = 0;
        while let Some(remaining) = bytes.get(written..).filter(|slice| !slice.is_empty()) {
            let count = self.socket.send(remaining, &[]).await?;
            if count == 0 {
                return Err(BoundaryError::WriteZero);
            }
            written += count;
        }
        Ok(())
    }
    pub fn shutdown_write(&self) -> Result<(), BoundaryError> {
        self.socket.shutdown_write()
    }
}
impl OwnedListener {
    /// Adopt an owned, validated association. Portable predicates do not prove listen state.
    pub async fn from_unix_owned(
        fd: OwnedFd,
        path: &Path,
        gate: &DescriptorGate,
    ) -> Result<Self, BoundaryError> {
        let _shared = gate.creation().await;
        crate::validate_unconnected_unix_stream(fd.as_fd(), path)?;
        configure_socket(&fd)?;
        Ok(Self {
            descriptor: AsyncFd::new(fd)?,
        })
    }
    /// Actual listening is established by its creator and an accept, not these portable queries.
    pub async fn from_tcp_owned(
        fd: OwnedFd,
        address: SocketAddr,
        gate: &DescriptorGate,
    ) -> Result<Self, BoundaryError> {
        let _shared = gate.creation().await;
        crate::validate_unconnected_tcp_stream(fd.as_fd(), address)?;
        configure_socket(&fd)?;
        Ok(Self {
            descriptor: AsyncFd::new(fd)?,
        })
    }
    pub fn into_owned(self) -> OwnedFd {
        self.descriptor.into_inner()
    }
    pub fn tcp_address(&self) -> Result<SocketAddr, BoundaryError> {
        SocketAddr::try_from(rustix::net::getsockname(self.as_fd()).map_err(io_error)?)
            .map_err(|_| BoundaryError::DescriptorKind)
    }

    pub async fn bind_unix(path: &Path, gate: &DescriptorGate) -> Result<Self, BoundaryError> {
        let _shared = gate.creation().await;
        let listener = std::os::unix::net::UnixListener::bind(path)?;
        let fd = OwnedFd::from(listener);
        configure_socket(&fd)?;
        Ok(Self {
            descriptor: AsyncFd::new(fd)?,
        })
    }
    pub async fn bind_tcp(
        address: SocketAddr,
        gate: &DescriptorGate,
    ) -> Result<Self, BoundaryError> {
        let _shared = gate.creation().await;
        let listener = std::net::TcpListener::bind(address)?;
        let fd = OwnedFd::from(listener);
        configure_socket(&fd)?;
        Ok(Self {
            descriptor: AsyncFd::new(fd)?,
        })
    }
    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.descriptor.get_ref().as_fd()
    }
    pub async fn accept(&self, gate: &DescriptorGate) -> Result<OwnedSocket, BoundaryError> {
        loop {
            let mut ready = self.descriptor.readable().await?;
            let _shared = gate.creation().await;
            match ready.try_io(|fd| {
                let accepted = rustix::net::accept(fd.get_ref()).map_err(std::io::Error::from)?;
                configure_socket(&accepted).map_err(std::io::Error::other)?;
                Ok(accepted)
            }) {
                Err(_) => continue,
                Ok(Err(error)) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Ok(Ok(fd)) => {
                    return Ok(OwnedSocket {
                        descriptor: AsyncFd::new(fd)?,
                    });
                }
                Ok(Err(error)) => return Err(error.into()),
            }
        }
    }
}
