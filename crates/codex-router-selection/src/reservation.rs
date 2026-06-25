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
    reserved_unix_seconds: u64,
}

/// Tracks transient reservations before upstream commit/finalization.
#[derive(Clone, Debug, Default)]
pub struct ReservationBook {
    reservations: HashMap<ReservationId, Reservation>,
    next_reservation_number: u64,
}

impl ReservationBook {
    /// Reserves headroom for an account and returns a release handle.
    pub fn reserve_next(&mut self, account_id: AccountId, headroom_cost: u32) -> ReservationHandle {
        self.reserve_next_at(account_id, headroom_cost, 0)
    }

    /// Reserves headroom at an observed time and returns a release handle.
    pub fn reserve_next_at(
        &mut self,
        account_id: AccountId,
        headroom_cost: u32,
        reserved_unix_seconds: u64,
    ) -> ReservationHandle {
        self.next_reservation_number = self.next_reservation_number.saturating_add(1);
        let reservation_id =
            ReservationId::new(format!("reservation_{}", self.next_reservation_number));
        self.reserve_at(
            reservation_id.clone(),
            account_id.clone(),
            headroom_cost,
            reserved_unix_seconds,
        );
        ReservationHandle::new(reservation_id, account_id, headroom_cost)
    }

    /// Reserves headroom for an account.
    pub fn reserve(
        &mut self,
        reservation_id: ReservationId,
        account_id: AccountId,
        headroom_cost: u32,
    ) {
        self.reserve_at(reservation_id, account_id, headroom_cost, 0);
    }

    /// Reserves headroom for an account at a known timestamp.
    pub fn reserve_at(
        &mut self,
        reservation_id: ReservationId,
        account_id: AccountId,
        headroom_cost: u32,
        reserved_unix_seconds: u64,
    ) {
        self.reservations.insert(
            reservation_id,
            Reservation {
                account_id,
                headroom_cost,
                reserved_unix_seconds,
            },
        );
    }

    /// Releases a reservation.
    pub fn release(&mut self, reservation_id: &ReservationId) {
        self.reservations.remove(reservation_id);
    }

    /// Releases a reservation handle.
    pub fn release_handle(&mut self, reservation_handle: &ReservationHandle) {
        self.release(reservation_handle.reservation_id());
    }

    /// Returns total active load pressure for an account.
    #[must_use]
    pub fn active_load_pressure(&self, account_id: &AccountId) -> u32 {
        self.reservations
            .values()
            .filter(|reservation| &reservation.account_id == account_id)
            .fold(0_u32, |total, reservation| {
                total.saturating_add(reservation.headroom_cost)
            })
            .min(100)
    }

    /// Removes reservations older than `max_age_seconds`.
    pub fn purge_stale(&mut self, now_unix_seconds: u64, max_age_seconds: u64) -> usize {
        let before_count = self.reservations.len();
        self.reservations.retain(|_reservation_id, reservation| {
            now_unix_seconds.saturating_sub(reservation.reserved_unix_seconds) <= max_age_seconds
        });
        before_count.saturating_sub(self.reservations.len())
    }

    /// Returns available headroom after active reservations.
    #[must_use]
    pub fn available_headroom(&self, account_id: &AccountId, raw_headroom: u32) -> u32 {
        raw_headroom.saturating_sub(self.active_load_pressure(account_id))
    }
}
