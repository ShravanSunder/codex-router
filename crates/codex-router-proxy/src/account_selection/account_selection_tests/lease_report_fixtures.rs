use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum LeaseEvent {
    Acquired {
        route_band: String,
        reservation_id: String,
        account_id: String,
        acquired_unix_seconds: u64,
        active_pressure: u32,
    },
    Released {
        route_band: String,
        reservation_id: String,
    },
}

#[derive(Debug, Default)]
pub(super) struct RecordingLeaseReporter {
    events: Mutex<Vec<LeaseEvent>>,
}

impl RecordingLeaseReporter {
    pub(super) fn events(&self) -> Vec<LeaseEvent> {
        self.events
            .lock()
            .unwrap_or_else(|error| panic!("events lock should be available: {error}"))
            .clone()
    }
}

impl ActiveClientLeaseReporter for RecordingLeaseReporter {
    fn record_acquired(
        &self,
        route_band: &str,
        reservation_handle: &ReservationHandle,
        acquired_unix_seconds: u64,
        active_pressure: u32,
    ) {
        self.events
            .lock()
            .unwrap_or_else(|error| panic!("events lock should be available: {error}"))
            .push(LeaseEvent::Acquired {
                route_band: route_band.to_owned(),
                reservation_id: reservation_handle.reservation_id().as_str().to_owned(),
                account_id: reservation_handle.account_id().as_str().to_owned(),
                acquired_unix_seconds,
                active_pressure,
            });
    }

    fn record_released(&self, route_band: &str, reservation_handle: &ReservationHandle) {
        self.events
            .lock()
            .unwrap_or_else(|error| panic!("events lock should be available: {error}"))
            .push(LeaseEvent::Released {
                route_band: route_band.to_owned(),
                reservation_id: reservation_handle.reservation_id().as_str().to_owned(),
            });
    }
}
