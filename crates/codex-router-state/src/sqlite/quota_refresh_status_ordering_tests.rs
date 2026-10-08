use super::*;

#[tokio::test]
async fn older_success_does_not_overwrite_newer_failure_or_quota_evidence() {
    let (_directory, state) = open_quota_status_state().await;
    let account_id = seed_protected_state(&state).await;
    let status_before = read_claude_refresh_status(&state, &account_id)
        .await
        .expect("newer failure should persist");
    assert_eq!(status_before.last_success_unix_seconds(), None);
    assert_eq!(status_before.last_attempt_unix_seconds(), Some(1_200));
    assert_eq!(
        status_before.last_error_class(),
        Some(QuotaRefreshErrorClass::ParseError)
    );
    assert_eq!(status_before.stale_after_unix_seconds(), Some(1_200));
    let evidence_before = read_preserved_account_state(&state, &account_id).await;

    state
        .record_refresh_success_status(
            &account_id,
            RouteBand::ClaudeMessages.as_str(),
            1_100,
            2_100,
        )
        .await
        .expect("older status write is safely ignored");

    let status_after = read_claude_refresh_status(&state, &account_id)
        .await
        .expect("late successful observation should record its success time");
    assert_eq!(status_after.last_success_unix_seconds(), Some(1_100));
    assert_eq!(status_after.last_attempt_unix_seconds(), Some(1_200));
    assert_eq!(
        status_after.last_error_class(),
        Some(QuotaRefreshErrorClass::ParseError)
    );
    assert_eq!(status_after.stale_after_unix_seconds(), Some(1_200));
    assert_eq!(
        read_preserved_account_state(&state, &account_id).await,
        evidence_before
    );
    state.close().await.expect("close state");
}

#[tokio::test]
async fn late_success_advances_known_success_but_keeps_newer_failure_fields() {
    let (_directory, state) = open_quota_status_state().await;
    let account_id =
        create_claude_account_without_refresh_status(&state, "claude-late-success-after-failure")
            .await;
    state
        .record_refresh_success_status(
            &account_id,
            RouteBand::ClaudeMessages.as_str(),
            1_000,
            2_000,
        )
        .await
        .expect("first successful attempt should persist");
    state
        .record_refresh_failure_preserving_selector_windows(
            &account_id,
            RouteBand::ClaudeMessages.as_str(),
            1_200,
            QuotaRefreshErrorClass::ParseError,
        )
        .await
        .expect("newer failed attempt should persist");
    let failure_status = read_claude_refresh_status(&state, &account_id)
        .await
        .expect("failure status should load");
    assert_eq!(failure_status.last_success_unix_seconds(), Some(1_000));
    assert_eq!(failure_status.last_attempt_unix_seconds(), Some(1_200));
    assert_eq!(
        failure_status.last_error_class(),
        Some(QuotaRefreshErrorClass::ParseError)
    );
    assert_eq!(failure_status.stale_after_unix_seconds(), Some(1_200));
    let evidence_before = read_preserved_account_state(&state, &account_id).await;

    state
        .record_refresh_success_status(
            &account_id,
            RouteBand::ClaudeMessages.as_str(),
            1_100,
            2_100,
        )
        .await
        .expect("late successful result should update the known success time");

    let status = read_claude_refresh_status(&state, &account_id)
        .await
        .expect("late success status should load");
    assert_eq!(status.last_success_unix_seconds(), Some(1_100));
    assert_eq!(status.last_attempt_unix_seconds(), Some(1_200));
    assert_eq!(
        status.last_error_class(),
        Some(QuotaRefreshErrorClass::ParseError)
    );
    assert_eq!(status.stale_after_unix_seconds(), Some(1_200));
    assert_eq!(
        read_preserved_account_state(&state, &account_id).await,
        evidence_before
    );
    state.close().await.expect("close state");
}

#[tokio::test]
async fn older_success_does_not_overwrite_newer_success_or_quota_evidence() {
    let (_directory, state) = open_quota_status_state().await;
    let account_id = seed_protected_state(&state).await;
    state
        .record_refresh_success_status(
            &account_id,
            RouteBand::ClaudeMessages.as_str(),
            1_400,
            2_400,
        )
        .await
        .expect("newer success should persist");
    let status_before = read_claude_refresh_status(&state, &account_id)
        .await
        .expect("newer success status");
    let evidence_before = read_preserved_account_state(&state, &account_id).await;

    state
        .record_refresh_success_status(
            &account_id,
            RouteBand::ClaudeMessages.as_str(),
            1_300,
            2_300,
        )
        .await
        .expect("older success write is safely ignored");

    assert_eq!(
        read_claude_refresh_status(&state, &account_id).await,
        Some(status_before)
    );
    assert_eq!(
        read_preserved_account_state(&state, &account_id).await,
        evidence_before
    );
    state.close().await.expect("close state");
}

#[tokio::test]
async fn older_failure_does_not_overwrite_newer_success_or_quota_evidence() {
    let (_directory, state) = open_quota_status_state().await;
    let account_id = seed_protected_state(&state).await;
    state
        .record_refresh_success_status(
            &account_id,
            RouteBand::ClaudeMessages.as_str(),
            1_400,
            2_400,
        )
        .await
        .expect("newer success should persist");
    let status_before = read_claude_refresh_status(&state, &account_id)
        .await
        .expect("newer success status");
    let evidence_before = read_preserved_account_state(&state, &account_id).await;

    state
        .record_refresh_failure_preserving_selector_windows(
            &account_id,
            RouteBand::ClaudeMessages.as_str(),
            1_300,
            QuotaRefreshErrorClass::NetworkError,
        )
        .await
        .expect("older failure write is safely ignored");

    assert_eq!(
        read_claude_refresh_status(&state, &account_id).await,
        Some(status_before)
    );
    assert_eq!(
        read_preserved_account_state(&state, &account_id).await,
        evidence_before
    );
    state.close().await.expect("close state");
}

#[tokio::test]
async fn absent_status_row_accepts_first_success_and_failure() {
    let (_directory, state) = open_quota_status_state().await;
    let success_account =
        create_claude_account_without_refresh_status(&state, "claude-first-success-status").await;
    assert_eq!(
        read_claude_refresh_status(&state, &success_account).await,
        None
    );
    state
        .record_refresh_success_status(
            &success_account,
            RouteBand::ClaudeMessages.as_str(),
            1_000,
            2_000,
        )
        .await
        .expect("absent status row accepts first success");
    let status = read_claude_refresh_status(&state, &success_account)
        .await
        .expect("first success status");
    assert_eq!(status.last_success_unix_seconds(), Some(1_000));
    assert_eq!(status.last_attempt_unix_seconds(), Some(1_000));
    assert_eq!(status.last_error_class(), None);

    let failure_account =
        create_claude_account_without_refresh_status(&state, "claude-first-failure-status").await;
    assert_eq!(
        read_claude_refresh_status(&state, &failure_account).await,
        None
    );
    state
        .record_refresh_failure_preserving_selector_windows(
            &failure_account,
            RouteBand::ClaudeMessages.as_str(),
            1_000,
            QuotaRefreshErrorClass::ParseError,
        )
        .await
        .expect("absent status row accepts first failure");
    let status = read_claude_refresh_status(&state, &failure_account)
        .await
        .expect("first failure status");
    assert_eq!(status.last_success_unix_seconds(), None);
    assert_eq!(status.last_attempt_unix_seconds(), Some(1_000));
    assert_eq!(
        status.last_error_class(),
        Some(QuotaRefreshErrorClass::ParseError)
    );
    assert_eq!(status.stale_after_unix_seconds(), Some(1_000));
    state.close().await.expect("close state");
}

#[tokio::test]
async fn existing_status_with_null_attempt_timestamp_accepts_success_and_failure() {
    let (_directory, state) = open_quota_status_state().await;
    let success_account =
        create_claude_account_without_refresh_status(&state, "claude-null-attempt-success").await;
    sqlx::query(
        "INSERT INTO quota_refresh_status (
             account_id, route_band, last_success_unix_seconds,
             last_attempt_unix_seconds, last_error_class, stale_after_unix_seconds
         ) VALUES (?1, ?2, ?3, NULL, ?4, ?5)",
    )
    .bind(success_account.as_str())
    .bind(RouteBand::ClaudeMessages.as_str())
    .bind(800_i64)
    .bind(QuotaRefreshErrorClass::ParseError.as_str())
    .bind(850_i64)
    .execute(&state.pool)
    .await
    .expect("existing success row with null attempt timestamp");

    state
        .record_refresh_success_status(
            &success_account,
            RouteBand::ClaudeMessages.as_str(),
            900,
            1_900,
        )
        .await
        .expect("null attempt timestamp admits success update");
    let status = read_claude_refresh_status(&state, &success_account)
        .await
        .expect("updated success status");
    assert_eq!(status.last_success_unix_seconds(), Some(900));
    assert_eq!(status.last_attempt_unix_seconds(), Some(900));
    assert_eq!(status.last_error_class(), None);
    assert_eq!(status.stale_after_unix_seconds(), Some(1_900));

    let failure_account =
        create_claude_account_without_refresh_status(&state, "claude-null-attempt-failure").await;
    sqlx::query(
        "INSERT INTO quota_refresh_status (
             account_id, route_band, last_success_unix_seconds,
             last_attempt_unix_seconds, last_error_class, stale_after_unix_seconds
         ) VALUES (?1, ?2, ?3, NULL, ?4, ?5)",
    )
    .bind(failure_account.as_str())
    .bind(RouteBand::ClaudeMessages.as_str())
    .bind(700_i64)
    .bind(QuotaRefreshErrorClass::NetworkError.as_str())
    .bind(800_i64)
    .execute(&state.pool)
    .await
    .expect("existing failure row with null attempt timestamp");

    state
        .record_refresh_failure_preserving_selector_windows(
            &failure_account,
            RouteBand::ClaudeMessages.as_str(),
            1_000,
            QuotaRefreshErrorClass::ParseError,
        )
        .await
        .expect("null attempt timestamp admits failure update");
    let status = read_claude_refresh_status(&state, &failure_account)
        .await
        .expect("updated failure status");
    assert_eq!(status.last_success_unix_seconds(), Some(700));
    assert_eq!(status.last_attempt_unix_seconds(), Some(1_000));
    assert_eq!(
        status.last_error_class(),
        Some(QuotaRefreshErrorClass::ParseError)
    );
    assert_eq!(status.stale_after_unix_seconds(), Some(800));
    state.close().await.expect("close state");
}

#[tokio::test]
async fn equal_and_newer_attempt_timestamps_keep_existing_upsert_behavior() {
    let (_directory, state) = open_quota_status_state().await;
    let account_id = seed_protected_state(&state).await;
    state
        .record_refresh_success_status(
            &account_id,
            RouteBand::ClaudeMessages.as_str(),
            1_200,
            2_400,
        )
        .await
        .expect("equal success attempt is accepted");
    state
        .record_refresh_success_status(
            &account_id,
            RouteBand::ClaudeMessages.as_str(),
            1_300,
            2_500,
        )
        .await
        .expect("newer success attempt is accepted");
    state
        .record_refresh_failure_preserving_selector_windows(
            &account_id,
            RouteBand::ClaudeMessages.as_str(),
            1_300,
            QuotaRefreshErrorClass::NetworkError,
        )
        .await
        .expect("equal failure attempt is accepted");
    state
        .record_refresh_failure_preserving_selector_windows(
            &account_id,
            RouteBand::ClaudeMessages.as_str(),
            1_400,
            QuotaRefreshErrorClass::RateLimited,
        )
        .await
        .expect("newer failure attempt is accepted");

    let status = read_claude_refresh_status(&state, &account_id)
        .await
        .expect("final equal/newer status");
    assert_eq!(status.last_success_unix_seconds(), Some(1_300));
    assert_eq!(status.last_attempt_unix_seconds(), Some(1_400));
    assert_eq!(
        status.last_error_class(),
        Some(QuotaRefreshErrorClass::RateLimited)
    );
    assert_eq!(status.stale_after_unix_seconds(), Some(1_300));
    state.close().await.expect("close state");
}

#[tokio::test]
async fn older_combined_success_keeps_status_but_still_replaces_quota_state() {
    let (_directory, state) = open_quota_status_state().await;
    let account_id = seed_protected_state(&state).await;
    state
        .record_refresh_success_status(
            &account_id,
            RouteBand::ClaudeMessages.as_str(),
            1_400,
            2_400,
        )
        .await
        .expect("newer status-only success");
    let prior_status = read_claude_refresh_status(&state, &account_id)
        .await
        .expect("newer status-only success should persist");
    let before = read_preserved_account_state(&state, &account_id).await;
    let replacement = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        RouteBand::ClaudeMessages.as_str(),
        604_800,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(45)
    .with_observed_unix_seconds(1_300);

    state
        .record_refresh_success_and_replace_selector_windows(
            &account_id,
            RouteBand::ClaudeMessages.as_str(),
            &[replacement],
            1_300,
            2_300,
        )
        .await
        .expect("older combined refresh still performs quota replacement");

    let after = read_preserved_account_state(&state, &account_id).await;
    assert_eq!(after.selector_windows.len(), 1);
    assert_eq!(after.selector_windows[0].limit_window_seconds, 604_800);
    assert_eq!(after.selector_windows[0].remaining_headroom, 45);
    assert!(after.route_states.is_empty());
    assert_eq!(after.observations, before.observations);
    assert_eq!(after.rejections, before.rejections);
    assert_eq!(after.account, before.account);
    assert_eq!(
        read_claude_refresh_status(&state, &account_id).await,
        Some(prior_status)
    );
    state.close().await.expect("close state");
}
