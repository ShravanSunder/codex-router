//! Fatal ancillary receipt is legal only in a disposable receiver or pre-runtime image.
use crate::{BoundaryError, DescriptorGate, OwnedSocket, owned_pipe::configure};
use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags};
use std::{io::IoSliceMut, mem::MaybeUninit, os::fd::OwnedFd};
pub const MAX_RIGHTS: usize = 64;
pub fn fatal_receipt() -> ! {
    std::process::exit(70)
}
pub struct ReceivedBytes {
    pub bytes: usize,
    pub descriptors: Vec<OwnedFd>,
}
pub struct UnixReceipt {
    socket: OwnedSocket,
}
impl UnixReceipt {
    /// The caller must run this boundary in a process allowed to die on poisoned receipt.
    pub fn new(socket: OwnedSocket) -> Self {
        Self { socket }
    }
    pub fn as_socket(&self) -> &OwnedSocket {
        &self.socket
    }
    pub fn into_socket(self) -> OwnedSocket {
        self.socket
    }
    pub async fn read(
        &self,
        bytes: &mut [u8],
        allow_rights: bool,
        gate: &DescriptorGate,
    ) -> Result<ReceivedBytes, BoundaryError> {
        self.read_validated(bytes, allow_rights, gate, |_| Ok(()))
            .await
    }
    /// Descriptor predicates execute inside receipt exclusion, before any spawn can proceed.
    pub async fn read_validated<TValidate>(
        &self,
        bytes: &mut [u8],
        allow_rights: bool,
        gate: &DescriptorGate,
        validate: TValidate,
    ) -> Result<ReceivedBytes, BoundaryError>
    where
        TValidate: Fn(&[OwnedFd]) -> Result<(), BoundaryError>,
    {
        loop {
            let mut ready = self.socket.descriptor.readable().await?;
            let _shared = gate.creation().await;
            let result = ready.try_io(|fd| {
                let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_RIGHTS))];
                let mut control = RecvAncillaryBuffer::new(&mut space);
                let flags = RecvFlags::DONTWAIT;
                #[cfg(target_os = "linux")]
                let flags = flags | RecvFlags::CMSG_CLOEXEC;
                let received = rustix::net::recvmsg(
                    fd.get_ref(),
                    &mut [IoSliceMut::new(bytes)],
                    &mut control,
                    flags,
                )
                .map_err(std::io::Error::from)?;
                // Never drain a possibly truncated pinned parser. Process exit bypasses Drop.
                if received.flags.contains(ReturnFlags::CTRUNC) {
                    fatal_receipt();
                }
                let mut descriptors = Vec::new();
                for ancillary in control.drain() {
                    match ancillary {
                        RecvAncillaryMessage::ScmRights(rights) => {
                            if !allow_rights {
                                fatal_receipt();
                            }
                            for descriptor in rights {
                                if configure(&descriptor).is_err() {
                                    fatal_receipt();
                                }
                                descriptors.push(descriptor);
                                if descriptors.len() > MAX_RIGHTS {
                                    fatal_receipt();
                                }
                            }
                        }
                        _ => fatal_receipt(),
                    }
                }
                if received.bytes > 0 && validate(&descriptors).is_err() {
                    fatal_receipt();
                }
                Ok(ReceivedBytes {
                    bytes: received.bytes,
                    descriptors,
                })
            });
            match result {
                Err(_) => continue,
                Ok(Err(error)) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Ok(result) => return result.map_err(Into::into),
            }
        }
    }
    pub async fn validate_or_exit<T>(
        &self,
        gate: &DescriptorGate,
        validation: impl FnOnce() -> Result<T, BoundaryError>,
    ) -> T {
        let _shared = gate.creation().await;
        match validation() {
            Ok(value) => value,
            Err(_) => fatal_receipt(),
        }
    }
}
