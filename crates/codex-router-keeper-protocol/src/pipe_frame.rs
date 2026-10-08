//! Full JSON encoded bounds are checked independently from typed downstream admission.
use codex_router_descriptor_boundary::{BoundaryError, PipeReader, PipeWriter};
use serde::{Serialize, de::DeserializeOwned};
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub struct JsonMessage(Vec<u8>);
impl JsonMessage {
    pub fn encode<TMessage: Serialize>(message: &TMessage) -> Result<Self, BoundaryError> {
        Self::from_bytes(serde_json::to_vec(message)?)
    }
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, BoundaryError> {
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(BoundaryError::TooLarge);
        }
        serde_json::from_slice::<serde::de::IgnoredAny>(&bytes)?;
        Ok(Self(bytes))
    }
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
    /// Decoding shape is not domain validation; the consumer must TryFrom before effects.
    pub fn decode<TWire: DeserializeOwned>(&self) -> Result<TWire, BoundaryError> {
        Ok(serde_json::from_slice(&self.0)?)
    }
}
pub struct PipeFrameReader {
    pipe: Option<PipeReader>,
}
pub struct PipeFrameWriter {
    pipe: Option<PipeWriter>,
}
impl PipeFrameReader {
    pub fn new(pipe: PipeReader) -> Self {
        Self { pipe: Some(pipe) }
    }
    pub async fn receive(&mut self) -> Result<Option<JsonMessage>, BoundaryError> {
        let pipe = self.pipe.take().ok_or(BoundaryError::Closed)?;
        let mut prefix = [0; 4];
        if pipe
            .read(prefix.get_mut(..1).ok_or(BoundaryError::UnexpectedEof)?)
            .await?
            == 0
        {
            return Ok(None);
        }
        pipe.read_exact(prefix.get_mut(1..).ok_or(BoundaryError::UnexpectedEof)?)
            .await?;
        let length = u32::from_be_bytes(prefix) as usize;
        if length > MAX_FRAME_BYTES {
            return Err(BoundaryError::TooLarge);
        }
        let mut bytes = vec![0; length];
        pipe.read_exact(&mut bytes).await?;
        let message = JsonMessage::from_bytes(bytes)?;
        self.pipe = Some(pipe);
        Ok(Some(message))
    }
}
impl PipeFrameWriter {
    pub fn new(pipe: PipeWriter) -> Self {
        Self { pipe: Some(pipe) }
    }
    /// Exclusive mutable ownership serializes frames. Dropped/failed partial futures close the pipe.
    pub async fn send(&mut self, message: &JsonMessage) -> Result<(), BoundaryError> {
        let pipe = self.pipe.take().ok_or(BoundaryError::Closed)?;
        pipe.write_all(
            &u32::try_from(message.bytes().len())
                .map_err(|_| BoundaryError::TooLarge)?
                .to_be_bytes(),
        )
        .await?;
        pipe.write_all(message.bytes()).await?;
        self.pipe = Some(pipe);
        Ok(())
    }
}
