use super::*;

/// Mirrors active client leases into a process-external status source.
pub trait ActiveClientLeaseReporter: Send + Sync {
    /// Records one acquired client lease.
    fn record_acquired(
        &self,
        route_band: &str,
        reservation_handle: &ReservationHandle,
        acquired_unix_seconds: u64,
        active_pressure: u32,
    );

    /// Records one released client lease.
    fn record_released(&self, route_band: &str, reservation_handle: &ReservationHandle);
}

/// Actor-backed active client lease reporter.
#[derive(Clone)]
pub struct SqliteActiveClientLeaseReporter {
    db_write_actor: DbWriteActor,
    process_run_id: String,
    clock: UnixClock,
}

impl SqliteActiveClientLeaseReporter {
    /// Creates an actor-backed active client lease reporter.
    #[must_use]
    pub fn new(db_write_actor: DbWriteActor, clock: UnixClock) -> Self {
        Self {
            db_write_actor,
            process_run_id: new_process_run_id(),
            clock,
        }
    }
}

impl ActiveClientLeaseReporter for SqliteActiveClientLeaseReporter {
    fn record_acquired(
        &self,
        route_band: &str,
        reservation_handle: &ReservationHandle,
        acquired_unix_seconds: u64,
        active_pressure: u32,
    ) {
        let Some(route_band) = RouteBand::parse(route_band) else {
            tracing::warn!(target: "codex_router_proxy::account_selection",
                route_band = "unknown",
                error.class = "invalid_route_band",
                "codex_router.active_client_mirror_failed"
            );
            return;
        };
        let _enqueue_result =
            self.db_write_actor
                .try_enqueue(DbWriteCommand::active_client_acquired(
                    route_band,
                    self.process_run_id.clone(),
                    reservation_handle.reservation_id().clone(),
                    reservation_handle.account_id().clone(),
                    acquired_unix_seconds,
                    active_pressure,
                ));
    }

    fn record_released(&self, route_band: &str, reservation_handle: &ReservationHandle) {
        let Some(route_band) = RouteBand::parse(route_band) else {
            tracing::warn!(target: "codex_router_proxy::account_selection",
                route_band = "unknown",
                error.class = "invalid_route_band",
                "codex_router.active_client_mirror_failed"
            );
            return;
        };
        let released_unix_seconds = (self.clock)();
        let _enqueue_result =
            self.db_write_actor
                .try_enqueue(DbWriteCommand::active_client_released(
                    route_band,
                    self.process_run_id.clone(),
                    reservation_handle.reservation_id().clone(),
                    released_unix_seconds,
                ));
    }
}

impl ActiveReservationGuard {
    #[cfg(test)]
    pub(crate) fn new(
        active_reservations: RouteBandReservationBooks,
        route_band: String,
        reservation_handle: ReservationHandle,
    ) -> Self {
        Self::new_with_active_client_leases(
            active_reservations,
            route_band,
            reservation_handle,
            None,
        )
    }

    #[cfg(test)]
    pub(crate) fn new_with_active_client_leases(
        active_reservations: RouteBandReservationBooks,
        route_band: String,
        reservation_handle: ReservationHandle,
        active_client_leases: Option<Arc<dyn ActiveClientLeaseReporter>>,
    ) -> Self {
        Self::new_with_transport_and_active_client_leases(
            active_reservations,
            route_band,
            reservation_handle,
            "session",
            active_client_leases,
        )
    }

    pub(crate) fn new_with_transport_and_active_client_leases(
        active_reservations: RouteBandReservationBooks,
        route_band: String,
        reservation_handle: ReservationHandle,
        transport_label: &'static str,
        active_client_leases: Option<Arc<dyn ActiveClientLeaseReporter>>,
    ) -> Self {
        Self {
            inner: Arc::new(ActiveReservationGuardInner {
                active_reservations,
                route_band,
                reservation_handle,
                transport_label,
                active_client_leases,
                released: AtomicBool::new(false),
            }),
        }
    }

    /// Returns the reservation handle.
    #[must_use]
    pub fn reservation_handle(&self) -> &ReservationHandle {
        &self.inner.reservation_handle
    }

    /// Releases the reservation before the stream object itself closes.
    pub fn release(&self) {
        self.inner.release_once();
    }

    /// Reserves the same account/route/cost again after a completed turn.
    pub fn reserve_again_at(&self, reserved_unix_seconds: u64) -> Option<Self> {
        let mut active_reservations = self.inner.active_reservations.lock().ok()?;
        let reservation_handle = active_reservations
            .entry(self.inner.route_band.clone())
            .or_insert_with(ReservationBook::default)
            .reserve_next_at(
                self.inner.reservation_handle.account_id().clone(),
                ACTIVE_SESSION_RESERVATION_UNITS,
                reserved_unix_seconds,
            );
        if let Some(active_client_leases) = self.inner.active_client_leases.as_ref() {
            active_client_leases.record_acquired(
                &self.inner.route_band,
                &reservation_handle,
                reserved_unix_seconds,
                ACTIVE_SESSION_RESERVATION_UNITS,
            );
        }
        let span = tracing::info_span!(target: "codex_router_proxy::account_selection",
            "codex_router.account_rereserved",
            route_band = self.inner.route_band.as_str(),
            account.hash = telemetry_hash(reservation_handle.account_id().as_str()),
            reservation.hash = telemetry_hash(reservation_handle.reservation_id().as_str()),
        );
        let _span_guard = span.enter();
        tracing::info!(target: "codex_router_proxy::account_selection","codex_router.account_rereserved");
        Some(Self::new_with_transport_and_active_client_leases(
            Arc::clone(&self.inner.active_reservations),
            self.inner.route_band.clone(),
            reservation_handle,
            self.inner.transport_label,
            self.inner.active_client_leases.clone(),
        ))
    }
}

impl std::fmt::Debug for ActiveReservationGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActiveReservationGuard")
            .field("route_band", &self.inner.route_band)
            .field("reservation_handle", &self.inner.reservation_handle)
            .finish()
    }
}

impl PartialEq for ActiveReservationGuard {
    fn eq(&self, other: &Self) -> bool {
        self.inner.route_band == other.inner.route_band
            && self.inner.reservation_handle == other.inner.reservation_handle
    }
}

impl Eq for ActiveReservationGuard {}

pub(super) struct ActiveReservationGuardInner {
    active_reservations: RouteBandReservationBooks,
    route_band: String,
    reservation_handle: ReservationHandle,
    transport_label: &'static str,
    active_client_leases: Option<Arc<dyn ActiveClientLeaseReporter>>,
    released: AtomicBool,
}

impl ActiveReservationGuardInner {
    fn release_once(&self) {
        if self
            .released
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            release_account_reservation(
                &self.active_reservations,
                &self.route_band,
                &self.reservation_handle,
            );
            let active_client_count = active_client_count_for_handle(
                &self.active_reservations,
                &self.route_band,
                &self.reservation_handle,
            )
            .unwrap_or(0);
            if let Some(active_client_leases) = self.active_client_leases.as_ref() {
                active_client_leases.record_released(&self.route_band, &self.reservation_handle);
            }
            let account_hash = telemetry_hash(self.reservation_handle.account_id().as_str());
            crate::telemetry::record_active_clients(
                account_hash.clone(),
                &self.route_band,
                self.transport_label,
                active_client_count,
            );
            let span = tracing::info_span!(target: "codex_router_proxy::account_selection",
                "codex_router.account_reservation_released",
                route_band = self.route_band.as_str(),
                account.hash = account_hash,
                reservation.hash =
                    telemetry_hash(self.reservation_handle.reservation_id().as_str()),
            );
            let _span_guard = span.enter();
            tracing::info!(target: "codex_router_proxy::account_selection","codex_router.account_reservation_released");
        }
    }
}

impl Drop for ActiveReservationGuardInner {
    fn drop(&mut self) {
        self.release_once();
    }
}

pub(super) fn active_session_counts_by_account(book: &ReservationBook) -> HashMap<AccountId, u32> {
    // The selector only needs counts for accounts already represented in the book.
    // Probe by walking reservations through the public account-specific counter below.
    let mut counts = HashMap::new();
    for account_id in book.account_ids() {
        counts.insert(account_id.clone(), book.active_session_count(account_id));
    }

    counts
}

pub(super) fn reserve_selected_account(
    selected: SelectedAccountDecision,
    active_reservations: &RouteBandReservationBooks,
    active_client_leases: Option<&Arc<dyn ActiveClientLeaseReporter>>,
    route_band: &str,
    transport_label: &'static str,
    now_unix_seconds: u64,
) -> Result<SelectedAccountDecision, HttpProxyError> {
    let active_reservations_guard_source = Arc::clone(active_reservations);
    let mut active_reservations =
        active_reservations
            .lock()
            .map_err(|_error| HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::SelectorStateUnavailable,
            })?;
    let reservation_handle = active_reservations
        .entry(route_band.to_owned())
        .or_insert_with(ReservationBook::default)
        .reserve_next_at(
            selected.account_id().clone(),
            ACTIVE_SESSION_RESERVATION_UNITS,
            now_unix_seconds,
        );
    let active_client_count = active_reservations.get(route_band).map_or(0, |book| {
        u64::from(book.active_session_count(selected.account_id()))
    });
    if let Some(active_client_leases) = active_client_leases {
        active_client_leases.record_acquired(
            route_band,
            &reservation_handle,
            now_unix_seconds,
            ACTIVE_SESSION_RESERVATION_UNITS,
        );
    }
    let account_hash = telemetry_hash(selected.account_id().as_str());
    crate::telemetry::record_account_selected(
        account_hash.clone(),
        route_band,
        transport_label,
        selected.selection_reason(),
    );
    crate::telemetry::record_active_clients(
        account_hash.clone(),
        route_band,
        transport_label,
        active_client_count,
    );
    let span = tracing::info_span!(target: "codex_router_proxy::account_selection",
        "codex_router.account_reserved",
        route_band,
        account.hash = account_hash,
        selection.reason = selected.selection_reason(),
        active.sessions = active_client_count,
        reservation.hash = telemetry_hash(reservation_handle.reservation_id().as_str()),
    );
    let _span_guard = span.enter();
    tracing::info!(target: "codex_router_proxy::account_selection","codex_router.account_reserved");
    Ok(selected.with_active_reservation_guard(
        ActiveReservationGuard::new_with_transport_and_active_client_leases(
            active_reservations_guard_source,
            route_band.to_owned(),
            reservation_handle,
            transport_label,
            active_client_leases.cloned(),
        ),
    ))
}

/// Releases a selection reservation from route-band active load accounting.
pub fn release_account_reservation(
    active_reservations: &RouteBandReservationBooks,
    route_band: &str,
    reservation_handle: &ReservationHandle,
) {
    let Ok(mut active_reservations) = active_reservations.lock() else {
        return;
    };
    if let Some(book) = active_reservations.get_mut(route_band) {
        book.release_handle(reservation_handle);
    }
}

fn active_client_count_for_handle(
    active_reservations: &RouteBandReservationBooks,
    route_band: &str,
    reservation_handle: &ReservationHandle,
) -> Option<u64> {
    let active_reservations = active_reservations.lock().ok()?;
    active_reservations
        .get(route_band)
        .map(|book| u64::from(book.active_session_count(reservation_handle.account_id())))
}

pub(super) fn telemetry_hash(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn new_process_run_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("pid{}-{nanos}", std::process::id())
}
