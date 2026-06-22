//! In-flight account reservation accounting.

use std::collections::HashMap;

use codex_router_core::ids::AccountId;
use codex_router_core::ids::ReservationId;

/// Reservation handle returned to proxy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReservationHandle {
    reservation_id: ReservationId,
    account_id: AccountId,
    headroom_cost: u32,
}

impl ReservationHandle {
    /// Creates a reservation handle.
    #[must_use]
    pub const fn new(
        reservation_id: ReservationId,
        account_id: AccountId,
        headroom_cost: u32,
    ) -> Self {
        Self {
            reservation_id,
            account_id,
            headroom_cost,
        }
    }

    /// Returns reservation id.
    #[must_use]
    pub const fn reservation_id(&self) -> &ReservationId {
        &self.reservation_id
    }

    /// Returns selected account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns reserved headroom cost.
    #[must_use]
    pub const fn headroom_cost(&self) -> u32 {
        self.headroom_cost
    }
}

#[derive(Clone, Debug)]
struct Reservation {
    account_id: AccountId,
    headroom_cost: u32,
}

/// Tracks transient reservations before upstream commit/finalization.
#[derive(Clone, Debug, Default)]
pub struct ReservationBook {
    reservations: HashMap<ReservationId, Reservation>,
}

impl ReservationBook {
    /// Reserves headroom for an account.
    pub fn reserve(
        &mut self,
        reservation_id: ReservationId,
        account_id: AccountId,
        headroom_cost: u32,
    ) {
        self.reservations.insert(
            reservation_id,
            Reservation {
                account_id,
                headroom_cost,
            },
        );
    }

    /// Releases a reservation.
    pub fn release(&mut self, reservation_id: &ReservationId) {
        self.reservations.remove(reservation_id);
    }

    /// Returns available headroom after active reservations.
    #[must_use]
    pub fn available_headroom(&self, account_id: &AccountId, raw_headroom: u32) -> u32 {
        let reserved = self
            .reservations
            .values()
            .filter(|reservation| &reservation.account_id == account_id)
            .fold(0_u32, |total, reservation| {
                total.saturating_add(reservation.headroom_cost)
            });

        raw_headroom.saturating_sub(reserved)
    }
}
