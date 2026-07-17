use codex_router_core::ids::AccountId;

use crate::quota_reset::reset_session_supervisor::ResetWorkflowSnapshot;
use crate::quota_reset::reset_session_supervisor::WorkflowPhase;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ResetPaneTarget {
    pub(super) account_id: AccountId,
    pub(super) active_credential_generation: u64,
    pub(super) account_label: String,
    pub(super) account_tag: String,
    pub(super) saved_reset_credits: String,
    pub(super) saved_weekly_window: String,
}

pub(super) fn reset_mode(snapshot: Option<&ResetWorkflowSnapshot>) -> bool {
    snapshot.is_some_and(|snapshot| snapshot.phase() != WorkflowPhase::Browse)
}

pub(super) fn credit_page_start(
    current_start: usize,
    credit_count: usize,
    page_size: usize,
    next_page: bool,
) -> usize {
    if credit_count == 0 || page_size == 0 {
        return 0;
    }
    let maximum_start = credit_count.saturating_sub(1) / page_size * page_size;
    if next_page {
        current_start.saturating_add(page_size).min(maximum_start)
    } else {
        current_start.saturating_sub(page_size)
    }
}

pub(super) fn reset_inventory_page_size(detail_height: usize) -> usize {
    detail_height.saturating_sub(15).clamp(1, 4)
}
