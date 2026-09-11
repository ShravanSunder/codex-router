//! Compile-time consumer contract: state operations can be awaited by Send tasks.
use std::path::Path;

use codex_router_core::ids::AccountId;

use crate::sqlite::{AsyncSqliteStateStore, AsyncWeeklyQuotaFloorMutationStore};

#[test]
fn public_state_operations_can_be_awaited_by_send_consumers() {
    fn require_send<TFuture: Send>(_: TFuture) {}

    fn check_operations(
        state: &AsyncSqliteStateStore,
        weekly_floor: &AsyncWeeklyQuotaFloorMutationStore,
        account_id: &AccountId,
    ) {
        require_send(async move { state.list_accounts().await });
        require_send(async move { state.load_account(account_id).await });
        require_send(async move {
            weekly_floor
                .set_weekly_quota_floor_by_account_id(account_id, None)
                .await
        });
        require_send(async move { AsyncSqliteStateStore::open(Path::new("unused")).await });
        require_send(
            async move { AsyncWeeklyQuotaFloorMutationStore::open(Path::new("unused")).await },
        );
        require_send(
            async move { AsyncSqliteStateStore::open_read_only(Path::new("unused")).await },
        );
    }

    let _ = check_operations;
}
