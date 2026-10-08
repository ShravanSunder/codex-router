//! Separate grant carriers validate transport phase, order, direction and listener address.
use crate::{JsonMessage, MAX_FRAME_BYTES};
use codex_router_descriptor_boundary::{
    BoundaryError, DescriptorGate, MAX_RIGHTS, OwnedSocket, UnixReceipt, fatal_receipt,
    validate_pipe,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    net::SocketAddr,
    os::fd::{AsFd, OwnedFd},
    path::PathBuf,
};
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GrantPhase {
    Bootstrap,
    ListenerGrant,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum DescriptorSpec {
    PipeRead,
    PipeWrite,
    UnixListener { path: PathBuf },
    TcpListener { address: SocketAddr },
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantEnvelope<TContext> {
    pub phase: GrantPhase,
    pub descriptors: Vec<DescriptorSpec>,
    pub context: TContext,
}
/// Only carrier association is validated here. The context still needs its domain-owned TryFrom before effects.
pub struct ValidatedGrant<TContext> {
    pub context: TContext,
    pub descriptors: Vec<OwnedFd>,
}
pub struct GrantSender {
    socket: Option<OwnedSocket>,
}
pub struct GrantReceiver {
    receipt: Option<UnixReceipt>,
}
fn shape(phase: GrantPhase, descriptors: &[DescriptorSpec]) -> Result<(), BoundaryError> {
    if descriptors.len() > MAX_RIGHTS {
        return Err(BoundaryError::TooLarge);
    }
    let listeners = match phase {
        GrantPhase::Bootstrap => {
            if !matches!(descriptors.first(), Some(DescriptorSpec::PipeRead))
                || !matches!(descriptors.get(1), Some(DescriptorSpec::PipeWrite))
            {
                return Err(BoundaryError::DescriptorKind);
            }
            descriptors.get(2..).ok_or(BoundaryError::DescriptorKind)?
        }
        GrantPhase::ListenerGrant => descriptors,
    };
    if listeners.iter().any(|kind| {
        !matches!(
            kind,
            DescriptorSpec::UnixListener { .. } | DescriptorSpec::TcpListener { .. }
        )
    }) {
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
    pub async fn send<TContext: Serialize>(
        &mut self,
        envelope: &GrantEnvelope<TContext>,
        rights: &[OwnedFd],
    ) -> Result<(), BoundaryError> {
        shape(envelope.phase, &envelope.descriptors)?;
        if rights.len() != envelope.descriptors.len() {
            return Err(BoundaryError::DescriptorKind);
        }
        let message = JsonMessage::encode(envelope)?;
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
    pub async fn receive<TContext: DeserializeOwned>(
        &mut self,
        phase: GrantPhase,
        expected: &[DescriptorSpec],
        gate: &DescriptorGate,
    ) -> Result<ValidatedGrant<TContext>, BoundaryError> {
        let receipt = self.receipt.take().ok_or(BoundaryError::Closed)?;
        let mut prefix = [0; 4];
        let chunk = receipt
            .read_validated(
                prefix.get_mut(..1).ok_or(BoundaryError::UnexpectedEof)?,
                true,
                gate,
                |rights| {
                    if rights.len() != expected.len() {
                        return Err(BoundaryError::DescriptorKind);
                    }
                    for (descriptor, spec) in rights.iter().zip(expected) {
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
                let envelope: GrantEnvelope<TContext> = serde_json::from_slice(&bytes)?;
                shape(envelope.phase, &envelope.descriptors)?;
                if envelope.phase != phase
                    || envelope.descriptors != expected
                    || chunk.descriptors.len() != expected.len()
                {
                    return Err(BoundaryError::DescriptorKind);
                }
                for (descriptor, spec) in chunk.descriptors.iter().zip(expected) {
                    validate_descriptor(descriptor, spec)?;
                }
                Ok(ValidatedGrant {
                    context: envelope.context,
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
