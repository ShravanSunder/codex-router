//! Observe asynchronous public receipt publication within the caller's budget.
use collaboration_protocol::{PushDeliveryState, PushRecord};
use std::{error::Error, time::Duration};
use tokio::time::{Instant, interval, timeout_at};

pub(crate) type ReceiptResult<TValue> = Result<TValue, Box<dyn Error + Send + Sync>>;

pub(crate) async fn wait_for_public_receipt(
    mut read_record: impl AsyncFnMut() -> ReceiptResult<PushRecord>,
    deadline: Instant,
) -> ReceiptResult<PushRecord> {
    let mut observation = interval(Duration::from_millis(250));
    loop {
        if Instant::now() >= deadline {
            return Err(
                "public push receipt was not published within the original observation budget"
                    .into(),
            );
        }
        timeout_at(deadline, observation.tick()).await?;
        let record = timeout_at(deadline, read_record()).await??;
        if Instant::now() >= deadline {
            return Err("public push receipt observation exceeded its original budget".into());
        }
        if !matches!(
            record.delivery_state,
            PushDeliveryState::Pending | PushDeliveryState::Held | PushDeliveryState::Attempted
        ) {
            return Ok(record);
        }
    }
}
