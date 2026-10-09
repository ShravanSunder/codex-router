//! Separate grant carriers validate typed frame association and physical descriptors.
use crate::{ChildGrantFrame, JsonMessage, ListenerKind, MAX_FRAME_BYTES};
use codex_router_descriptor_boundary::{
    BoundaryError, DescriptorGate, MAX_RIGHTS, OwnedSocket, UnixReceipt, fatal_receipt,
    validate_pipe,
};
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    os::fd::{AsFd, OwnedFd},
    path::PathBuf,
};

/// Receiver-local expected descriptor kind and address; never a child-grant frame field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum DescriptorSpec {
    PipeRead,
    PipeWrite,
    UnixListener { path: PathBuf },
    TcpListener { address: SocketAddr },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChildGrantExpectationKind {
    Bootstrap,
    ListenerGrant,
}

impl ChildGrantExpectationKind {
    fn is_bootstrap(self) -> bool {
        matches!(self, Self::Bootstrap)
    }
}

/// Receiver-local descriptor and logical-listener association for one expected frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChildGrantExpectation {
    kind: ChildGrantExpectationKind,
    listeners: Vec<ListenerKind>,
    descriptors: Vec<DescriptorSpec>,
}

impl ChildGrantExpectation {
    pub fn bootstrap(
        listeners: Vec<ListenerKind>,
        descriptors: Vec<DescriptorSpec>,
    ) -> Result<Self, BoundaryError> {
        Self::new(ChildGrantExpectationKind::Bootstrap, listeners, descriptors)
    }

    pub fn listener_grant(
        listeners: Vec<ListenerKind>,
        descriptors: Vec<DescriptorSpec>,
    ) -> Result<Self, BoundaryError> {
        Self::new(
            ChildGrantExpectationKind::ListenerGrant,
            listeners,
            descriptors,
        )
    }

    fn new(
        kind: ChildGrantExpectationKind,
        listeners: Vec<ListenerKind>,
        descriptors: Vec<DescriptorSpec>,
    ) -> Result<Self, BoundaryError> {
        validate_descriptor_shape(kind.is_bootstrap(), &descriptors, listeners.len())?;
        Ok(Self {
            kind,
            listeners,
            descriptors,
        })
    }

    fn validate_frame(&self, frame: &ChildGrantFrame) -> Result<(), BoundaryError> {
        if self.kind.is_bootstrap() != frame.is_bootstrap()
            || self.listeners.as_slice() != frame.listener_kinds()
        {
            return Err(BoundaryError::DescriptorKind);
        }
        Ok(())
    }
}

pub struct ReceivedChildGrant {
    frame: ChildGrantFrame,
    descriptors: Vec<OwnedFd>,
}

impl ReceivedChildGrant {
    pub fn into_parts(self) -> (ChildGrantFrame, Vec<OwnedFd>) {
        (self.frame, self.descriptors)
    }
}

pub struct GrantSender {
    socket: Option<OwnedSocket>,
}

pub struct GrantReceiver {
    receipt: Option<UnixReceipt>,
}

fn validate_descriptor_shape(
    bootstrap: bool,
    descriptors: &[DescriptorSpec],
    listener_count: usize,
) -> Result<(), BoundaryError> {
    if descriptors.len() > MAX_RIGHTS {
        return Err(BoundaryError::TooLarge);
    }
    let listeners = if bootstrap {
        if !matches!(descriptors.first(), Some(DescriptorSpec::PipeRead))
            || !matches!(descriptors.get(1), Some(DescriptorSpec::PipeWrite))
        {
            return Err(BoundaryError::DescriptorKind);
        }
        descriptors.get(2..).ok_or(BoundaryError::DescriptorKind)?
    } else {
        descriptors
    };
    if listeners.len() != listener_count
        || listeners.iter().any(|kind| {
            !matches!(
                kind,
                DescriptorSpec::UnixListener { .. } | DescriptorSpec::TcpListener { .. }
            )
        })
    {
        return Err(BoundaryError::DescriptorKind);
    }
    Ok(())
}

fn validate_descriptor(fd: &OwnedFd, spec: &DescriptorSpec) -> Result<(), BoundaryError> {
    match spec {
        DescriptorSpec::PipeRead => validate_pipe(fd.as_fd(), false),
        DescriptorSpec::PipeWrite => validate_pipe(fd.as_fd(), true),
        DescriptorSpec::UnixListener { path } => {
            codex_router_descriptor_boundary::validate_unconnected_unix_stream(fd.as_fd(), path)
        }
        DescriptorSpec::TcpListener { address } => {
            codex_router_descriptor_boundary::validate_unconnected_tcp_stream(fd.as_fd(), *address)
        }
    }
}

impl GrantSender {
    pub fn new(socket: OwnedSocket) -> Self {
        Self {
            socket: Some(socket),
        }
    }

    pub async fn send(
        &mut self,
        frame: &ChildGrantFrame,
        expected_descriptors: &[DescriptorSpec],
        rights: &[OwnedFd],
    ) -> Result<(), BoundaryError> {
        frame.validate().map_err(|_| BoundaryError::TooLarge)?;
        validate_descriptor_shape(
            frame.is_bootstrap(),
            expected_descriptors,
            frame.listener_kinds().len(),
        )?;
        if rights.len() != expected_descriptors.len() {
            return Err(BoundaryError::DescriptorKind);
        }
        for (descriptor, spec) in rights.iter().zip(expected_descriptors) {
            validate_descriptor(descriptor, spec)?;
        }
        let message = JsonMessage::encode(frame)?;
        let mut bytes = u32::try_from(message.bytes().len())
            .map_err(|_| BoundaryError::TooLarge)?
            .to_be_bytes()
            .to_vec();
        bytes.extend(message.bytes());
        let socket = self.socket.take().ok_or(BoundaryError::Closed)?;
        let borrowed: Vec<_> = rights.iter().map(AsFd::as_fd).collect();
        let mut written = 0;
        while let Some(remaining) = bytes.get(written..).filter(|slice| !slice.is_empty()) {
            let count = socket
                .send(remaining, if written == 0 { &borrowed } else { &[] })
                .await?;
            if count == 0 {
                return Err(BoundaryError::WriteZero);
            }
            written += count;
        }
        self.socket = Some(socket);
        Ok(())
    }
}

impl GrantReceiver {
    /// Run only in a receiving process allowed to exit on malformed grant or ancillary input.
    pub fn new(socket: OwnedSocket) -> Self {
        Self {
            receipt: Some(UnixReceipt::new(socket)),
        }
    }

    pub async fn receive(
        &mut self,
        expected: &ChildGrantExpectation,
        gate: &DescriptorGate,
    ) -> Result<ReceivedChildGrant, BoundaryError> {
        let receipt = self.receipt.take().ok_or(BoundaryError::Closed)?;
        let expected_descriptors = expected.descriptors.as_slice();
        let mut prefix = [0; 4];
        let chunk = receipt
            .read_validated(
                prefix.get_mut(..1).ok_or(BoundaryError::UnexpectedEof)?,
                true,
                gate,
                |rights| {
                    if rights.len() != expected_descriptors.len() {
                        return Err(BoundaryError::DescriptorKind);
                    }
                    for (descriptor, spec) in rights.iter().zip(expected_descriptors) {
                        validate_descriptor(descriptor, spec)?;
                    }
                    Ok(())
                },
            )
            .await?;
        if chunk.bytes == 0 {
            return Err(BoundaryError::UnexpectedEof);
        }
        Self::remaining(
            &receipt,
            prefix.get_mut(1..).ok_or(BoundaryError::UnexpectedEof)?,
            gate,
        )
        .await?;
        let length = u32::from_be_bytes(prefix) as usize;
        if length > MAX_FRAME_BYTES {
            fatal_receipt();
        }
        let mut bytes = vec![0; length];
        Self::remaining(&receipt, &mut bytes, gate).await?;
        let grant = receipt
            .validate_or_exit(gate, move || {
                let frame: ChildGrantFrame = serde_json::from_slice(&bytes)?;
                frame.validate().map_err(|_| BoundaryError::TooLarge)?;
                expected.validate_frame(&frame)?;
                if chunk.descriptors.len() != expected_descriptors.len() {
                    return Err(BoundaryError::DescriptorKind);
                }
                for (descriptor, spec) in chunk.descriptors.iter().zip(expected_descriptors) {
                    validate_descriptor(descriptor, spec)?;
                }
                Ok(ReceivedChildGrant {
                    frame,
                    descriptors: chunk.descriptors,
                })
            })
            .await;
        self.receipt = Some(receipt);
        Ok(grant)
    }

    async fn remaining(
        receipt: &UnixReceipt,
        bytes: &mut [u8],
        gate: &DescriptorGate,
    ) -> Result<(), BoundaryError> {
        let mut offset = 0;
        while let Some(remaining) = bytes.get_mut(offset..).filter(|slice| !slice.is_empty()) {
            let chunk = receipt.read(remaining, false, gate).await?;
            if chunk.bytes == 0 {
                return Err(BoundaryError::UnexpectedEof);
            }
            offset += chunk.bytes;
        }
        Ok(())
    }
}
