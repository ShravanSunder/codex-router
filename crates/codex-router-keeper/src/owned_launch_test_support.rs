//! Test assertions consume every launch variant and drain unexpected cleanup debt.
use crate::{GroupLaunchOutcome, ImageLaunchOutcome, ImageLease, OwnedProcessGroup};
type TestError = Box<dyn std::error::Error + Send + Sync>;
pub(crate) async fn require_launched(
    outcome: GroupLaunchOutcome,
) -> Result<OwnedProcessGroup, TestError> {
    match outcome {
        GroupLaunchOutcome::Launched(group) => Ok(group),
        GroupLaunchOutcome::Refused { reason } => Err(reason.into()),
        GroupLaunchOutcome::CleanupPending {
            reason,
            mut cleanup,
        } => {
            cleanup
                .wait_for_cleanup(&tokio_util::sync::CancellationToken::new())
                .await?;
            Err(reason.into())
        }
    }
}
pub(crate) async fn require_image_launch(
    outcome: ImageLaunchOutcome,
) -> Result<(OwnedProcessGroup, ImageLease), TestError> {
    match outcome {
        ImageLaunchOutcome::Launched { group, image } => Ok((group, image)),
        ImageLaunchOutcome::Refused { reason } => Err(reason.into()),
        ImageLaunchOutcome::CleanupPending {
            reason,
            mut cleanup,
            image: _image,
        } => {
            cleanup
                .wait_for_cleanup(&tokio_util::sync::CancellationToken::new())
                .await?;
            Err(reason.into())
        }
    }
}
