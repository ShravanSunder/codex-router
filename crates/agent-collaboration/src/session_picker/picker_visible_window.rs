//! Pure visible-window positioning, shared with model snapshot tests.
pub(in super::super) fn visible_window_start(
    focused_index: usize,
    visible_len: usize,
    max_visible: usize,
) -> usize {
    if visible_len <= max_visible {
        return 0;
    }
    focused_index
        .saturating_add(1)
        .saturating_sub(max_visible)
        .min(visible_len.saturating_sub(max_visible))
}
