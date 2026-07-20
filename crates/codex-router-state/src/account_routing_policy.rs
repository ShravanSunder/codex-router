//! Per-account routing policy persisted outside account credentials.

use codex_router_core::ids::AccountId;
use thiserror::Error;

/// Invalid configured weekly quota floor.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("weekly quota floor must be an integer percent from 1 through 15")]
pub struct InvalidWeeklyQuotaFloor;

/// Enabled weekly quota floor stored as basis points.
///
/// Absence of a policy row represents the disabled `0%` state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WeeklyQuotaFloorBasisPoints(u16);

impl WeeklyQuotaFloorBasisPoints {
    /// Creates an enabled floor from integer-percent-derived basis points.
    pub fn new(basis_points: u16) -> Result<Self, InvalidWeeklyQuotaFloor> {
        if (100..=1_500).contains(&basis_points) && basis_points.is_multiple_of(100) {
            Ok(Self(basis_points))
        } else {
            Err(InvalidWeeklyQuotaFloor)
        }
    }

    /// Returns the stored basis points.
    #[must_use]
    pub const fn basis_points(self) -> u16 {
        self.0
    }

    /// Returns the configured integer percent.
    #[must_use]
    pub const fn percent(self) -> u16 {
        self.0 / 100
    }
}

/// Persisted per-account routing policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountRoutingPolicy {
    account_id: AccountId,
    weekly_quota_floor_basis_points: WeeklyQuotaFloorBasisPoints,
}

impl AccountRoutingPolicy {
    /// Creates one enabled account policy.
    #[must_use]
    pub const fn new(
        account_id: AccountId,
        weekly_quota_floor_basis_points: WeeklyQuotaFloorBasisPoints,
    ) -> Self {
        Self {
            account_id,
            weekly_quota_floor_basis_points,
        }
    }

    /// Returns the policy account.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the enabled weekly quota floor.
    #[must_use]
    pub const fn weekly_quota_floor_basis_points(&self) -> WeeklyQuotaFloorBasisPoints {
        self.weekly_quota_floor_basis_points
    }
}
