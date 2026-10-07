//! Errors expose transport disposition, never business-role policy.
#[derive(Debug, thiserror::Error)]
pub enum BoundaryError {
    #[error("descriptor I/O failed")]
    Io(#[from] std::io::Error),
    #[error("descriptor kind or direction is invalid")]
    DescriptorKind,
    #[error("record/frame exceeds its encoded bound")]
    TooLarge,
    #[error("partial or unexpected pipe EOF")]
    UnexpectedEof,
    #[error("invalid byte record")]
    InvalidRecord,
    #[error("JSON record invalid")]
    Json(#[from] serde_json::Error),
    #[error("transport direction is closed")]
    Closed,
    #[error("transport task cancelled")]
    Cancelled,
    #[error("receiver wait task failed")]
    WaitTask,
    #[error("receiver exited abnormally")]
    ReceiverExit,
    #[error("write made no progress")]
    WriteZero,
}
pub(crate) fn io_error(error: rustix::io::Errno) -> BoundaryError {
    std::io::Error::from(error).into()
}
