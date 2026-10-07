//! Explicit half-close records distinguish ordinary socket EOF from reader process loss.
use crate::{BoundaryError, PipeReader, PipeWriter};
use serde::{Deserialize, Serialize};
pub const MAX_RECORD_BYTES: usize = 1024 * 1024;
pub const MAX_DATA_BYTES: usize = 16 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum RecordWire {
    Data { bytes: Vec<u8> },
    ReadHalfClosed,
}
pub enum SocketReadRecord {
    Data(Vec<u8>),
    ReadHalfClosed,
}
impl SocketReadRecord {
    pub fn data(bytes: Vec<u8>) -> Result<Self, BoundaryError> {
        if bytes.is_empty() || bytes.len() > MAX_DATA_BYTES {
            return Err(BoundaryError::InvalidRecord);
        }
        Ok(Self::Data(bytes))
    }
}
pub struct RecordWriter {
    writer: Option<PipeWriter>,
    half_closed: bool,
}
pub struct SocketReadStream {
    reader: Option<PipeReader>,
    half_closed: bool,
}
impl RecordWriter {
    pub fn new(writer: PipeWriter) -> Self {
        Self {
            writer: Some(writer),
            half_closed: false,
        }
    }
    pub async fn send(&mut self, record: SocketReadRecord) -> Result<(), BoundaryError> {
        if self.half_closed {
            return Err(BoundaryError::Closed);
        }
        let wire = match record {
            SocketReadRecord::Data(bytes) => {
                let _validated = SocketReadRecord::data(bytes.clone())?;
                RecordWire::Data { bytes }
            }
            SocketReadRecord::ReadHalfClosed => RecordWire::ReadHalfClosed,
        };
        let bytes = serde_json::to_vec(&wire)?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(BoundaryError::TooLarge);
        }
        let writer = self.writer.take().ok_or(BoundaryError::Closed)?;
        writer
            .write_all(
                &u32::try_from(bytes.len())
                    .map_err(|_| BoundaryError::TooLarge)?
                    .to_be_bytes(),
            )
            .await?;
        writer.write_all(&bytes).await?;
        self.half_closed = matches!(wire, RecordWire::ReadHalfClosed);
        self.writer = Some(writer);
        Ok(())
    }
}
impl SocketReadStream {
    pub fn new(reader: PipeReader) -> Self {
        Self {
            reader: Some(reader),
            half_closed: false,
        }
    }
    pub async fn next_record(&mut self) -> Result<SocketReadRecord, BoundaryError> {
        if self.half_closed {
            return Ok(SocketReadRecord::ReadHalfClosed);
        }
        let reader = self.reader.take().ok_or(BoundaryError::Closed)?;
        let mut prefix = [0; 4];
        reader.read_exact(&mut prefix).await?;
        let length = u32::from_be_bytes(prefix) as usize;
        if length > MAX_RECORD_BYTES {
            return Err(BoundaryError::TooLarge);
        }
        let mut bytes = vec![0; length];
        reader.read_exact(&mut bytes).await?;
        let record = match serde_json::from_slice::<RecordWire>(&bytes)? {
            RecordWire::Data { bytes } => SocketReadRecord::data(bytes)?,
            RecordWire::ReadHalfClosed => {
                self.half_closed = true;
                SocketReadRecord::ReadHalfClosed
            }
        };
        self.reader = Some(reader);
        Ok(record)
    }
}
