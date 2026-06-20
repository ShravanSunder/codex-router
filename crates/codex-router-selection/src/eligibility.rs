//! Account eligibility classification.

use codex_router_core::ids::AccountId;
use codex_router_quota::snapshot::SnapshotFreshness;

/// Candidate account presented to selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectionCandidate {
    account_id: AccountId,
    remaining_headroom: u32,
    freshness: SnapshotFreshness,
}

impl SelectionCandidate {
    /// Creates a selection candidate.
    #[must_use]
    pub const fn new(
        account_id: AccountId,
        remaining_headroom: u32,
        freshness: SnapshotFreshness,
    ) -> Self {
        Self {
            account_id,
            remaining_headroom,
            freshness,
        }
    }

    /// Returns the account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Classifies eligibility.
    #[must_use]
    pub const fn eligibility(&self, known_fresh_account_exists: bool) -> Eligibility {
        if self.remaining_headroom == 0 {
            return Eligibility::Ineligible {
                reason: "no_headroom",
            };
        }

        match self.freshness {
            SnapshotFreshness::Fresh { .. } => Eligibility::Eligible {
                headroom: self.remaining_headroom,
            },
            SnapshotFreshness::StaleWithPenalty { .. } if known_fresh_account_exists => {
                Eligibility::Penalized {
                    headroom: self.remaining_headroom / 4,
                    reason: "stale_quota",
                }
            }
            SnapshotFreshness::Unknown if known_fresh_account_exists => Eligibility::Penalized {
                headroom: self.remaining_headroom / 8,
                reason: "unknown_quota",
            },
            SnapshotFreshness::StaleWithPenalty { .. } | SnapshotFreshness::Unknown => {
                Eligibility::Eligible {
                    headroom: self.remaining_headroom,
                }
            }
        }
    }
}

/// Selection eligibility result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Eligibility {
    /// Account is eligible with usable headroom.
    Eligible {
        /// Effective headroom.
        headroom: u32,
    },
    /// Account is usable but should lose against known-fresh accounts.
    Penalized {
        /// Penalized headroom.
        headroom: u32,
        /// Static reason for audit.
        reason: &'static str,
    },
    /// Account must not be selected.
    Ineligible {
        /// Static reason for audit.
        reason: &'static str,
    },
}
