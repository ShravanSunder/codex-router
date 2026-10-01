//! Stored credit observation decoding and coherent selector reads.

use codex_router_core::credit_usage::CreditAvailability;
use codex_router_core::credit_usage::CreditBalance;
use codex_router_core::credit_usage::CreditProviderLimitReason;
use codex_router_core::credit_usage::CreditProviderObservation;
use codex_router_core::credit_usage::CreditSpendControl;
use codex_router_core::credit_usage::CreditUsagePolicy;
use codex_router_core::ids::AccountId;

use crate::sqlite::AsyncSqliteStateStore;
use crate::sqlite::StateStoreError;

use super::CreditUsageObservation;
use super::corrupt_credit_account;
use super::optional_stored_nonnegative_u64;
use super::stored_nonnegative_u64;

#[derive(Debug)]
struct StoredCreditUsageObservationRow {
    credential_generation: i64,
    latest_started_attempt: i64,
    committed_attempt: Option<i64>,
    observed_unix_seconds: Option<i64>,
    stale_after_unix_seconds: Option<i64>,
    availability: String,
    balance: Option<String>,
    spend_control_state: String,
    provider_limit_reason: Option<String>,
}

fn parse_stored_credit_observation(
    account_id: &AccountId,
    row: StoredCreditUsageObservationRow,
) -> Result<CreditUsageObservation, StateStoreError> {
    let credential_generation = stored_nonnegative_u64(
        row.credential_generation,
        account_id,
        "credit_credential_generation",
    )?;
    let latest_started_attempt = stored_nonnegative_u64(
        row.latest_started_attempt,
        account_id,
        "latest_started_credit_attempt",
    )?;
    let committed_attempt = optional_stored_nonnegative_u64(
        row.committed_attempt,
        account_id,
        "committed_credit_attempt",
    )?;
    let observed_unix_seconds = optional_stored_nonnegative_u64(
        row.observed_unix_seconds,
        account_id,
        "credit_observed_unix_seconds",
    )?;
    let stale_after_unix_seconds = optional_stored_nonnegative_u64(
        row.stale_after_unix_seconds,
        account_id,
        "credit_stale_after_unix_seconds",
    )?;

    if latest_started_attempt == 0
        || committed_attempt
            .is_some_and(|committed| committed == 0 || committed > latest_started_attempt)
        || observed_unix_seconds.is_some() != stale_after_unix_seconds.is_some()
        || committed_attempt.is_some() != observed_unix_seconds.is_some()
        || observed_unix_seconds
            .zip(stale_after_unix_seconds)
            .is_some_and(|(observed, stale_after)| stale_after < observed)
    {
        return Err(corrupt_credit_account(
            account_id,
            "credit_attempt_ordering",
        ));
    }

    let balance = row
        .balance
        .map(CreditBalance::new)
        .transpose()
        .map_err(|_| corrupt_credit_account(account_id, "credit_balance"))?;
    let availability = CreditAvailability::from_stored_parts(&row.availability, balance)
        .ok_or_else(|| corrupt_credit_account(account_id, "credit_availability"))?;
    let spend_control = CreditSpendControl::parse(&row.spend_control_state)
        .ok_or_else(|| corrupt_credit_account(account_id, "credit_spend_control"))?;
    let limit_reason =
        match row.provider_limit_reason.as_deref() {
            Some(value) => Some(CreditProviderLimitReason::parse(value).ok_or_else(|| {
                corrupt_credit_account(account_id, "credit_provider_limit_reason")
            })?),
            None => None,
        };
    if committed_attempt.is_none()
        && (availability != CreditAvailability::Unknown
            || spend_control != CreditSpendControl::Unreported
            || limit_reason.is_some())
    {
        return Err(corrupt_credit_account(
            account_id,
            "uncommitted_credit_facts",
        ));
    }

    Ok(CreditUsageObservation {
        account_id: account_id.clone(),
        credential_generation,
        latest_started_attempt,
        committed_attempt,
        observed_unix_seconds,
        stale_after_unix_seconds,
        provider_observation: CreditProviderObservation::new(
            availability,
            spend_control,
            limit_reason,
        ),
    })
}

impl AsyncSqliteStateStore {
    /// Loads one stored credit observation and validates every persisted domain field.
    pub async fn load_account_credit_observation(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<CreditUsageObservation>, StateStoreError> {
        let row = sqlx::query_as!(
            StoredCreditUsageObservationRow,
            "SELECT credential_generation, latest_started_attempt, committed_attempt,
                    observed_unix_seconds, stale_after_unix_seconds, availability, balance,
                    spend_control_state, provider_limit_reason
               FROM account_credit_observations
              WHERE account_id = ?1",
            account_id.as_str(),
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(crate::sqlite::sqlx_error)?;

        row.map(|row| parse_stored_credit_observation(account_id, row))
            .transpose()
    }
}

/// Reads credit policy and observation inside the caller's quota snapshot transaction.
pub(crate) async fn load_credit_usage_for_selector(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    account_id: &AccountId,
) -> Result<(CreditUsagePolicy, Option<CreditUsageObservation>), StateStoreError> {
    let policy_row = sqlx::query!(
        "SELECT allow_credits
           FROM account_credit_policies
          WHERE account_id = ?1",
        account_id.as_str(),
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(crate::sqlite::sqlx_error)?;
    let policy = match policy_row.map(|row| row.allow_credits) {
        None | Some(0) => CreditUsagePolicy::Disallow,
        Some(1) => CreditUsagePolicy::Allow,
        Some(_) => return Err(corrupt_credit_account(account_id, "allow_credits")),
    };

    let observation_row = sqlx::query_as!(
        StoredCreditUsageObservationRow,
        "SELECT credential_generation, latest_started_attempt, committed_attempt,
                observed_unix_seconds, stale_after_unix_seconds, availability, balance,
                spend_control_state, provider_limit_reason
           FROM account_credit_observations
          WHERE account_id = ?1",
        account_id.as_str(),
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(crate::sqlite::sqlx_error)?;
    let observation = observation_row
        .map(|row| parse_stored_credit_observation(account_id, row))
        .transpose()?;

    Ok((policy, observation))
}
