use super::*;

#[cfg(test)]
#[test]
fn selector_inputs_mark_windows_stale_at_persisted_300_second_boundary() {
    let account_id = AccountId::new("acct_runtime_boundary")
        .unwrap_or_else(|error| panic!("test account id should be valid: {error}"));
    let windows = vec![
        PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            DEFAULT_SELECTOR_LIMIT_WINDOW_SECONDS,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(50)
        .with_observed_unix_seconds(1_000),
    ];
    let refresh_status = QuotaRefreshStatusView::recorded(
        account_id,
        "responses",
        Some(1_000),
        Some(1_000),
        None,
        Some(1_300),
    );

    assert!(
        !selector_windows_are_stale(&windows, Some(&refresh_status), 1_299),
        "persisted selector windows should still be routing-eligible at 299s"
    );
    assert!(
        selector_windows_are_stale(&windows, Some(&refresh_status), 1_300),
        "persisted selector windows should become stale at the 300s boundary"
    );
    assert!(
        selector_windows_are_stale(&windows, Some(&refresh_status), 1_301),
        "persisted selector windows should remain stale after the 300s boundary"
    );
}
