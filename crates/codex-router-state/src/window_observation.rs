//! Durable Claude quota observations and per-window rejection barriers.

use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_core::route_profile::WindowKind;

use crate::sqlite::AsyncSqliteStateStore;
use crate::sqlite::StateStoreError;
use crate::sqlite::i64_to_u64_window_state;
use crate::sqlite::sqlx_error;
use crate::sqlite::u64_to_i64;

const MAX_REMAINING_BASIS_POINTS: u32 = 10_000;

/// Input values for one Claude quota observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowObservationProps {
    account_id: AccountId,
    window_kind: WindowKind,
    remaining_basis_points: u32,
    reset_unix_seconds: Option<u64>,
    observation_started_at: u64,
}

impl WindowObservationProps {
    /// Creates observation properties before optional reset metadata is attached.
    #[must_use]
    pub fn new(
        account_id: AccountId,
        window_kind: WindowKind,
        remaining_basis_points: u32,
    ) -> Self {
        Self {
            account_id,
            window_kind,
            remaining_basis_points,
            reset_unix_seconds: None,
            observation_started_at: 0,
        }
    }

    /// Sets the provider-reported reset time.
    #[must_use]
    pub const fn with_reset_unix_seconds(mut self, reset_unix_seconds: u64) -> Self {
        self.reset_unix_seconds = Some(reset_unix_seconds);
        self
    }

    /// Sets when the poll or request observation began.
    #[must_use]
    pub const fn with_observation_started_at(mut self, observation_started_at: u64) -> Self {
        self.observation_started_at = observation_started_at;
        self
    }
}

/// One observation for one account and one provider quota window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowObservation {
    account_id: AccountId,
    window_kind: WindowKind,
    remaining_basis_points: u32,
    reset_unix_seconds: Option<u64>,
    observation_started_at: u64,
}

impl WindowObservation {
    /// Creates a validated observation.
    pub fn new(props: WindowObservationProps) -> Result<Self, StateStoreError> {
        if props.remaining_basis_points > MAX_REMAINING_BASIS_POINTS {
            return Err(StateStoreError::InvalidAccountWindowState {
                field: "remaining_basis_points",
            });
        }

        Ok(Self {
            account_id: props.account_id,
            window_kind: props.window_kind,
            remaining_basis_points: props.remaining_basis_points,
            reset_unix_seconds: props.reset_unix_seconds,
            observation_started_at: props.observation_started_at,
        })
    }

    /// Returns the account identity.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the window identity.
    #[must_use]
    pub const fn window_kind(&self) -> WindowKind {
        self.window_kind
    }

    /// Returns remaining quota in basis points.
    #[must_use]
    pub const fn remaining_basis_points(&self) -> u32 {
        self.remaining_basis_points
    }

    /// Returns the provider-reported reset time.
    #[must_use]
    pub const fn reset_unix_seconds(&self) -> Option<u64> {
        self.reset_unix_seconds
    }

    /// Returns when the poll or request observation began.
    #[must_use]
    pub const fn observation_started_at(&self) -> u64 {
        self.observation_started_at
    }
}

/// Input values for one provider-reported window rejection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowRejectionProps {
    account_id: AccountId,
    window_kind: WindowKind,
    rejected_at: u64,
    reported_reset: Option<u64>,
}

impl WindowRejectionProps {
    /// Creates rejection properties before optional reset metadata is attached.
    #[must_use]
    pub fn new(account_id: AccountId, window_kind: WindowKind, rejected_at: u64) -> Self {
        Self {
            account_id,
            window_kind,
            rejected_at,
            reported_reset: None,
        }
    }

    /// Sets the reset time reported with this rejection.
    #[must_use]
    pub const fn with_reported_reset(mut self, reported_reset: u64) -> Self {
        self.reported_reset = Some(reported_reset);
        self
    }
}

/// One durable rejection of a shared provider quota window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowRejection {
    account_id: AccountId,
    window_kind: WindowKind,
    rejected_at: u64,
    reported_reset: Option<u64>,
}

impl WindowRejection {
    /// Creates a validated rejection.
    #[must_use]
    pub fn new(props: WindowRejectionProps) -> Self {
        Self {
            account_id: props.account_id,
            window_kind: props.window_kind,
            rejected_at: props.rejected_at,
            reported_reset: props.reported_reset,
        }
    }

    /// Returns the account identity.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the rejected window identity.
    #[must_use]
    pub const fn window_kind(&self) -> WindowKind {
        self.window_kind
    }

    /// Returns when the provider rejected the account for this window.
    #[must_use]
    pub const fn rejected_at(&self) -> u64 {
        self.rejected_at
    }

    /// Returns the reset reported with the latest rejection for this window.
    #[must_use]
    pub const fn reported_reset(&self) -> Option<u64> {
        self.reported_reset
    }
}

impl AsyncSqliteStateStore {
    /// Records a per-window observation and clears a matching rejection only when fresh,
    /// post-rejection headroom is positive at application time.
    pub async fn record_window_observation(
        &self,
        observation: &WindowObservation,
        applied_at_unix_seconds: u64,
        freshness_interval_seconds: u64,
    ) -> Result<bool, StateStoreError> {
        ensure_claude_account(&self.pool, observation.account_id()).await?;
        let account_id = observation.account_id().as_str();
        let window_kind = observation.window_kind().as_str();
        let remaining_basis_points = i64::from(observation.remaining_basis_points());
        let reset_unix_seconds = observation
            .reset_unix_seconds()
            .map(u64_to_i64)
            .transpose()?;
        let observation_started_at = u64_to_i64(observation.observation_started_at())?;
        let mut transaction = self.pool.begin().await.map_err(sqlx_error)?;
        let write_result = sqlx::query!(
            "INSERT INTO account_window_observations (
                account_id, window_kind, remaining_basis_points,
                reset_unix_seconds, observation_started_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(account_id, window_kind) DO UPDATE SET
                remaining_basis_points = excluded.remaining_basis_points,
                reset_unix_seconds = excluded.reset_unix_seconds,
                observation_started_at = excluded.observation_started_at
             WHERE excluded.observation_started_at
                   > account_window_observations.observation_started_at",
            account_id,
            window_kind,
            remaining_basis_points,
            reset_unix_seconds,
            observation_started_at,
        )
        .execute(&mut *transaction)
        .await
        .map_err(sqlx_error)?;
        let observation_was_newest = write_result.rows_affected() > 0;

        let observation_age_is_fresh = applied_at_unix_seconds
            >= observation.observation_started_at()
            && applied_at_unix_seconds.saturating_sub(observation.observation_started_at())
                <= freshness_interval_seconds;
        if observation_was_newest
            && observation.remaining_basis_points() > 0
            && observation_age_is_fresh
        {
            sqlx::query!(
                "DELETE FROM account_window_rejections
                  WHERE account_id = ?1
                    AND window_kind = ?2
                    AND rejected_at < ?3",
                account_id,
                window_kind,
                observation_started_at,
            )
            .execute(&mut *transaction)
            .await
            .map_err(sqlx_error)?;
        }

        transaction.commit().await.map_err(sqlx_error)?;
        Ok(observation_was_newest)
    }

    /// Upserts the latest rejection barrier for one account and window.
    pub async fn record_window_rejection(
        &self,
        rejection: &WindowRejection,
    ) -> Result<(), StateStoreError> {
        ensure_claude_account(&self.pool, rejection.account_id()).await?;
        let account_id = rejection.account_id().as_str();
        let window_kind = rejection.window_kind().as_str();
        let rejected_at = u64_to_i64(rejection.rejected_at())?;
        let reported_reset = rejection.reported_reset().map(u64_to_i64).transpose()?;
        let mut transaction = self.pool.begin().await.map_err(sqlx_error)?;
        sqlx::query!(
            "INSERT INTO account_window_rejections (
                account_id, window_kind, rejected_at, reported_reset
             )
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(account_id, window_kind) DO UPDATE SET
                rejected_at = excluded.rejected_at,
                reported_reset = excluded.reported_reset
             WHERE excluded.rejected_at > account_window_rejections.rejected_at",
            account_id,
            window_kind,
            rejected_at,
            reported_reset,
        )
        .execute(&mut *transaction)
        .await
        .map_err(sqlx_error)?;
        transaction.commit().await.map_err(sqlx_error)
    }

    /// Loads all newest observations for one account, ordered by window kind.
    pub async fn window_observations_for_account(
        &self,
        account_id: &AccountId,
    ) -> Result<Vec<WindowObservation>, StateStoreError> {
        ensure_claude_account(&self.pool, account_id).await?;
        let rows = sqlx::query!(
            "SELECT window_kind, remaining_basis_points,
                    reset_unix_seconds, observation_started_at
               FROM account_window_observations
              WHERE account_id = ?1
              ORDER BY window_kind",
            account_id.as_str(),
        )
        .fetch_all(&self.pool)
        .await
        .map_err(sqlx_error)?;
        let mut observations = Vec::with_capacity(rows.len());
        for row in rows {
            let window_kind = parse_window_kind(account_id.as_str(), &row.window_kind)?;
            let remaining_basis_points =
                u32::try_from(row.remaining_basis_points).map_err(|_| {
                    StateStoreError::CorruptAccountWindowState {
                        account_id: account_id.as_str().to_owned(),
                        field: "remaining_basis_points",
                    }
                })?;
            if remaining_basis_points > MAX_REMAINING_BASIS_POINTS {
                return Err(StateStoreError::CorruptAccountWindowState {
                    account_id: account_id.as_str().to_owned(),
                    field: "remaining_basis_points",
                });
            }
            let reset_unix_seconds = row
                .reset_unix_seconds
                .map(|value| {
                    i64_to_u64_window_state(value, account_id.as_str(), "reset_unix_seconds")
                })
                .transpose()?;
            let observation_started_at = i64_to_u64_window_state(
                row.observation_started_at,
                account_id.as_str(),
                "observation_started_at",
            )?;
            observations.push(WindowObservation::new(
                WindowObservationProps::new(
                    account_id.clone(),
                    window_kind,
                    remaining_basis_points,
                )
                .with_reset_unix_seconds_option(reset_unix_seconds)
                .with_observation_started_at(observation_started_at),
            )?);
        }
        Ok(observations)
    }

    /// Loads all outstanding rejected windows for one account.
    pub async fn window_rejections_for_account(
        &self,
        account_id: &AccountId,
    ) -> Result<Vec<WindowRejection>, StateStoreError> {
        ensure_claude_account(&self.pool, account_id).await?;
        let rows = sqlx::query!(
            "SELECT window_kind, rejected_at, reported_reset
               FROM account_window_rejections
              WHERE account_id = ?1
              ORDER BY window_kind",
            account_id.as_str(),
        )
        .fetch_all(&self.pool)
        .await
        .map_err(sqlx_error)?;
        rows.into_iter()
            .map(|row| {
                let window_kind = parse_window_kind(account_id.as_str(), &row.window_kind)?;
                let rejected_at =
                    i64_to_u64_window_state(row.rejected_at, account_id.as_str(), "rejected_at")?;
                let reported_reset = row
                    .reported_reset
                    .map(|value| {
                        i64_to_u64_window_state(value, account_id.as_str(), "reported_reset")
                    })
                    .transpose()?;
                Ok(WindowRejection::new(
                    WindowRejectionProps::new(account_id.clone(), window_kind, rejected_at)
                        .with_reported_reset_option(reported_reset),
                ))
            })
            .collect()
    }

    /// Returns whether any rejected quota window still keeps this account exhausted.
    pub async fn account_window_is_exhausted(
        &self,
        account_id: &AccountId,
    ) -> Result<bool, StateStoreError> {
        ensure_claude_account(&self.pool, account_id).await?;
        let row = sqlx::query!(
            "SELECT EXISTS(
                        SELECT 1 FROM account_window_rejections
                         WHERE account_id = ?1
                    ) AS is_exhausted",
            account_id.as_str(),
        )
        .fetch_one(&self.pool)
        .await
        .map_err(sqlx_error)?;
        match row.is_exhausted {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(StateStoreError::CorruptAccountWindowState {
                account_id: account_id.as_str().to_owned(),
                field: "rejection_exists",
            }),
        }
    }
}

impl WindowObservationProps {
    fn with_reset_unix_seconds_option(mut self, reset_unix_seconds: Option<u64>) -> Self {
        self.reset_unix_seconds = reset_unix_seconds;
        self
    }
}

impl WindowRejectionProps {
    fn with_reported_reset_option(mut self, reported_reset: Option<u64>) -> Self {
        self.reported_reset = reported_reset;
        self
    }
}

async fn ensure_claude_account(
    pool: &sqlx::SqlitePool,
    account_id: &AccountId,
) -> Result<(), StateStoreError> {
    let row = sqlx::query!(
        "SELECT provider FROM accounts WHERE account_id = ?1",
        account_id.as_str(),
    )
    .fetch_optional(pool)
    .await
    .map_err(sqlx_error)?
    .ok_or(StateStoreError::AccountWindowStateAccountNotFound)?;
    let provider =
        Provider::parse(&row.provider).ok_or_else(|| StateStoreError::CorruptAccount {
            account_id: account_id.as_str().to_owned(),
            field: "provider",
        })?;
    if provider != Provider::Claude {
        return Err(StateStoreError::AccountWindowStateRequiresClaudeAccount);
    }
    Ok(())
}

fn parse_window_kind(account_id: &str, value: &str) -> Result<WindowKind, StateStoreError> {
    WindowKind::parse(value).ok_or_else(|| StateStoreError::CorruptAccountWindowState {
        account_id: account_id.to_owned(),
        field: "window_kind",
    })
}

#[cfg(test)]
#[path = "window_observation_tests.rs"]
mod tests;
