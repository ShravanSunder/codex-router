use super::*;

struct CorruptProviderFactCase {
    name: &'static str,
    update_statement: &'static str,
    value: &'static str,
    expected_field: &'static str,
}

#[tokio::test]
async fn stored_credit_observation_rejects_corrupt_provider_facts_without_coercion() {
    let cases = [
        CorruptProviderFactCase {
            name: "availability tag",
            update_statement: "UPDATE account_credit_observations SET availability = ?1 WHERE account_id = ?2",
            value: "corrupt_availability",
            expected_field: "credit_availability",
        },
        CorruptProviderFactCase {
            name: "balance spelling",
            update_statement: "UPDATE account_credit_observations SET balance = ?1 WHERE account_id = ?2",
            value: "not-a-decimal-balance",
            expected_field: "credit_balance",
        },
        CorruptProviderFactCase {
            name: "spend-control tag",
            update_statement: "UPDATE account_credit_observations SET spend_control_state = ?1 WHERE account_id = ?2",
            value: "corrupt_spend_control",
            expected_field: "credit_spend_control",
        },
    ];

    for case in cases {
        let temporary_directory = CreditStoreTempDir::new();
        let (state, account_id) =
            openai_account(&temporary_directory.database_path(), case.name, 1).await;
        let attempt = state
            .begin_credit_refresh_attempt(&account_id, 1)
            .await
            .expect("valid observation should allocate an attempt");
        assert!(
            state
                .record_responses_refresh_success(ResponsesRefreshSuccessCommit {
                    attempt: &attempt,
                    selector_windows: &[selector_window(&account_id, 75, 100)],
                    observed_unix_seconds: 100,
                    stale_after_unix_seconds: 400,
                    provider_observation: &provider_observation(
                        "2.75",
                        CreditSpendControl::Clear,
                        Some(CreditProviderLimitReason::RateLimitReached),
                    ),
                    history_observations: &[successful_history_observation(&account_id, 75, 100)],
                    snapshot: &quota_snapshot(&account_id, 75, 100),
                })
                .await
                .expect("valid observation should commit")
        );

        sqlx::query(case.update_statement)
            .bind(case.value)
            .bind(account_id.as_str())
            .execute(&state.pool)
            .await
            .unwrap_or_else(|error| panic!("test should inject corrupt {}: {error}", case.name));

        assert_eq!(
            state.load_account_credit_observation(&account_id).await,
            Err(StateStoreError::CorruptAccount {
                account_id: account_id.as_str().to_owned(),
                field: case.expected_field,
            }),
            "corrupt {} must fail explicitly, never decode as Unknown or authorize credit use",
            case.name
        );
        state
            .close()
            .await
            .unwrap_or_else(|error| panic!("corrupt observation state should close: {error}"));
    }
}
