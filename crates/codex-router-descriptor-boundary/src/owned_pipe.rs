//! Validated anonymous pipe ends: no connected Unix byte reads.
use crate::{BoundaryError, DescriptorGate, boundary_error::io_error};
use rustix::{
    fs::{FileType, OFlags, fcntl_getfl, fcntl_setfl, fstat},
    io::{FdFlags, fcntl_getfd, fcntl_setfd},
};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use tokio::io::unix::AsyncFd;
pub struct OwnedPipe;
pub struct PipeReader {
    descriptor: AsyncFd<OwnedFd>,
}
pub struct PipeWriter {
    descriptor: AsyncFd<OwnedFd>,
}
pub(crate) fn configure(descriptor: &OwnedFd) -> Result<(), BoundaryError> {
    fcntl_setfd(
        descriptor,
        fcntl_getfd(descriptor).map_err(io_error)? | FdFlags::CLOEXEC,
    )
    .map_err(io_error)?;
    fcntl_setfl(
        descriptor,
        fcntl_getfl(descriptor).map_err(io_error)? | OFlags::NONBLOCK,
    )
    .map_err(io_error)?;
    Ok(())
}
pub fn validate_pipe(descriptor: BorrowedFd<'_>, write: bool) -> Result<(), BoundaryError> {
    if FileType::from_raw_mode(fstat(descriptor).map_err(io_error)?.st_mode) != FileType::Fifo {
        return Err(BoundaryError::DescriptorKind);
    }
    let flags = fcntl_getfl(descriptor).map_err(io_error)?;
    let direction = flags & OFlags::ACCMODE;
    if direction
        != if write {
            OFlags::WRONLY
        } else {
            OFlags::RDONLY
        }
    {
        return Err(BoundaryError::DescriptorKind);
    }
    Ok(())
}
impl OwnedPipe {
    pub async fn pair(gate: &DescriptorGate) -> Result<(PipeReader, PipeWriter), BoundaryError> {
        let _shared = gate.creation().await;
        let (read, write) = rustix::pipe::pipe().map_err(io_error)?;
        configure(&read)?;
        configure(&write)?;
        Ok((
            PipeReader {
                descriptor: AsyncFd::new(read)?,
            },
            PipeWriter {
                descriptor: AsyncFd::new(write)?,
            },
        ))
    }
}
impl PipeReader {
    pub async fn from_owned(
        descriptor: OwnedFd,
        gate: &DescriptorGate,
    ) -> Result<Self, BoundaryError> {
        let _shared = gate.creation().await;
        validate_pipe(descriptor.as_fd(), false)?;
        configure(&descriptor)?;
        Ok(Self {
            descriptor: AsyncFd::new(descriptor)?,
        })
    }
    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.descriptor.get_ref().as_fd()
    }
    pub fn into_owned(self) -> OwnedFd {
        self.descriptor.into_inner()
    }
    pub async fn read(&self, bytes: &mut [u8]) -> Result<usize, BoundaryError> {
        loop {
            let mut ready = self.descriptor.readable().await?;
            match ready.try_io(|fd| {
                rustix::io::read(fd.get_ref(), &mut *bytes).map_err(std::io::Error::from)
            }) {
                Err(_) => continue,
                Ok(Err(error)) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Ok(result) => return result.map_err(Into::into),
            }
        }
    }
    pub async fn read_exact(&self, bytes: &mut [u8]) -> Result<(), BoundaryError> {
        let mut offset = 0;
        while let Some(remaining) = bytes
            .get_mut(offset..)
            .filter(|remaining| !remaining.is_empty())
        {
            let count = self.read(remaining).await?;
            if count == 0 {
                return Err(BoundaryError::UnexpectedEof);
            }
            offset += count;
        }
        Ok(())
    }
}
impl PipeWriter {
    pub async fn from_owned(
        descriptor: OwnedFd,
        gate: &DescriptorGate,
    ) -> Result<Self, BoundaryError> {
        let _shared = gate.creation().await;
        validate_pipe(descriptor.as_fd(), true)?;
        configure(&descriptor)?;
        Ok(Self {
            descriptor: AsyncFd::new(descriptor)?,
        })
    }
    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.descriptor.get_ref().as_fd()
    }
    pub fn into_owned(self) -> OwnedFd {
        self.descriptor.into_inner()
    }
    pub async fn write_all(&self, bytes: &[u8]) -> Result<(), BoundaryError> {
        let mut offset = 0;
        while let Some(remaining) = bytes
            .get(offset..)
            .filter(|remaining| !remaining.is_empty())
        {
            let mut ready = self.descriptor.writable().await?;
            let count = match ready.try_io(|fd| {
                rustix::io::write(fd.get_ref(), remaining).map_err(std::io::Error::from)
            }) {
                Err(_) => continue,
                Ok(Err(error)) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Ok(result) => result?,
            };
            if count == 0 {
                return Err(BoundaryError::WriteZero);
            }
            offset += count;
        }
        Ok(())
    }
}
