use codex_router_descriptor_boundary::BoundaryError;
use codex_router_keeper_protocol::NativeProbeJobConversionError;
#[derive(Debug, thiserror::Error)]
pub enum NativeProbeError {
    #[error("native receiver parent must be Keeper")]
    ParentRole,
    #[error("native receiver projected fingerprint differs from compiled Keeper")]
    Fingerprint,
    #[error("native receiver inherited pipe/frame failed: {0}")]
    Frame(#[from] BoundaryError),
    #[error("native probe image launch failed: {0}")]
    Image(#[from] crate::ImageError),
    #[error("native probe pipe conversion failed: {0}")]
    Stdio(#[from] std::io::Error),
    #[error("native probe duration projection failed: {0}")]
    Duration(#[from] NativeProbeJobConversionError),
    #[error("native probe result violates native observation invariants: {0}")]
    Observation(#[from] codex_native_integration::AppServerObservationValidationError),
    #[error("native probe original deadline cannot be represented")]
    DeadlineOverflow,
    #[error("native probe original readiness budget expired")]
    NativeBudgetExpired,
    #[error("native probe original collection deadline expired")]
    CollectionExpired,
    #[error("native probe operation cancelled; caller must drain retained ownership")]
    Cancelled,
    #[error("native probe owns an unfinished operation; no new job admitted")]
    Busy,
    #[error("native probe has no active job")]
    NotStarted,
    #[error("native probe failure has unfinished cleanup; drain the same owner")]
    CleanupRequired,
    #[error("native probe hello absent or different from projected Keeper/fingerprint")]
    Hello,
    #[error("native probe requires exactly one job/result and clean pipe EOF")]
    RecordCount,
    #[error("native probe exited abnormally after output")]
    AbnormalExit,
    #[error("native probe expected anonymous stdio pipes")]
    MissingPipe,
    #[error("native probe owned group observation/cleanup failed: {0}")]
    Group(#[from] crate::GroupStopError),
    #[error("native probe group cleanup remains unfinished")]
    CleanupIncomplete,
}
