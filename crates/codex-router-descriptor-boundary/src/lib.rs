//! Dependency-neutral owned carriers and receiving-process containment.
mod boundary_error;
mod descriptor_gate;
mod owned_pipe;
mod owned_socket;
mod pipe_records;
mod reader_lifetime;
mod unix_receipt;
pub use boundary_error::BoundaryError;
pub use descriptor_gate::DescriptorGate;
pub use owned_pipe::validate_pipe;
pub use owned_pipe::{OwnedPipe, PipeReader, PipeWriter};
pub use owned_socket::{OwnedListener, OwnedSocket, SocketWriter};
pub use pipe_records::{
    MAX_DATA_BYTES, MAX_RECORD_BYTES, RecordWriter, SocketReadRecord, SocketReadStream,
};
pub use reader_lifetime::{ReaderLease, supervise_reader};
pub use unix_receipt::{MAX_RIGHTS, ReceivedBytes, UnixReceipt, fatal_receipt};

mod listener_validation;
pub use listener_validation::{validate_unconnected_tcp_stream, validate_unconnected_unix_stream};
