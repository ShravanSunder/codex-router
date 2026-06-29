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

    /// Returns active client count for an account and reservation cost class.
    #[must_use]
    pub fn active_client_count(&self, account_id: &AccountId, headroom_cost: u32) -> u64 {
        self.reservations
            .values()
            .filter(|reservation| {
                &reservation.account_id == account_id && reservation.headroom_cost == headroom_cost
            })
            .count() as u64
    }

    /// Returns active session count for an account across reservation cost classes.
    #[must_use]
    pub fn active_session_count(&self, account_id: &AccountId) -> u32 {
        self.reservations
            .values()
            .filter(|reservation| &reservation.account_id == account_id)
            .count()
            .try_into()
            .unwrap_or(u32::MAX)
    }

    /// Returns account ids with active reservations.
    #[must_use]
    pub fn account_ids(&self) -> Vec<&AccountId> {
        let mut account_ids = self
            .reservations
            .values()
            .map(|reservation| &reservation.account_id)
            .collect::<Vec<_>>();
        account_ids.sort();
        account_ids.dedup();
        account_ids
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

#[cfg(test)]
mod tests {
    use codex_router_core::ids::AccountId;

    use super::ReservationBook;

    #[test]
    fn active_client_count_tracks_account_and_cost_class() {
        let account_id = AccountId::new("acct_selected")
            .unwrap_or_else(|error| panic!("test account id should parse: {error}"));
        let other_account_id = AccountId::new("acct_other")
            .unwrap_or_else(|error| panic!("test account id should parse: {error}"));
        let mut book = ReservationBook::default();

        let first = book.reserve_next_at(account_id.clone(), 8, 1);
        assert_eq!(book.active_client_count(&account_id, 8), 1);

        let second = book.reserve_next_at(account_id.clone(), 8, 2);
        assert_eq!(book.active_client_count(&account_id, 8), 2);

        let _http = book.reserve_next_at(account_id.clone(), 1, 3);
        let _other = book.reserve_next_at(other_account_id, 8, 4);
        assert_eq!(book.active_client_count(&account_id, 8), 2);
        assert_eq!(book.active_client_count(&account_id, 1), 1);

        book.release_handle(&first);
        assert_eq!(book.active_client_count(&account_id, 8), 1);

        book.release_handle(&second);
        assert_eq!(book.active_client_count(&account_id, 8), 0);
    }
}
