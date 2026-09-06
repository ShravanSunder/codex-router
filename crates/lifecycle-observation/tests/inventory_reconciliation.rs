use communication_protocol::{ObservationOrdering, ObservationScope};
use lifecycle_observation::{InventoryReconciliation, ReadDisposition};
use serde_json::json;
#[test]
fn overlapping_reads_retry_boundedly_and_disconnect_discards_completion() {
    let scope:ObservationScope=serde_json::from_value(json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"generation":{"serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":1},"observerId":"00000000-0000-4000-8000-000000000003"})).unwrap_or_else(|e|panic!("scope: {e}"));
    let thread: communication_protocol::SessionId = "t"
        .to_owned()
        .try_into()
        .unwrap_or_else(|e| panic!("thread: {e}"));
    let mut reconciliation =
        InventoryReconciliation::new(scope).unwrap_or_else(|e| panic!("new: {e}"));
    for retry in [true, true, false] {
        let read = reconciliation
            .begin_read(thread.clone())
            .unwrap_or_else(|e| panic!("begin: {e}"));
        assert_eq!(
            reconciliation
                .notification(&thread)
                .unwrap_or_else(|e| panic!("event: {e}")),
            ObservationOrdering::Ambiguous
        );
        assert_eq!(
            reconciliation.finish_read(read),
            ReadDisposition::Ambiguous { retry }
        );
    }
    let pending = reconciliation
        .begin_read(thread)
        .unwrap_or_else(|e| panic!("begin: {e}"));
    reconciliation.disconnect();
    assert_eq!(
        reconciliation.finish_read(pending),
        ReadDisposition::Discard
    );
}
