//! First-fire wait outcome is folded into the one CLI result by its caller.
use collaboration_client::protocol::{FireReceipt, WakeShowRequest, WakeupId};
use collaboration_client::{CollaborationClient, WakeWaitError};
use std::path::Path;

pub(crate) async fn wait(
    directory: &Path,
    wakeup_id: WakeupId,
) -> Result<FireReceipt, WakeWaitError> {
    let client = CollaborationClient::connect(
        directory,
        "agent-collaboration-wake-wait",
        env!("CARGO_PKG_VERSION"),
    )
    .await?;
    client
        .subscribe_wakeup(WakeShowRequest { wakeup_id })
        .await?
        .wait_until_first_fire()
        .await
}
