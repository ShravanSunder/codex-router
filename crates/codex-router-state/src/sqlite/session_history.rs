//! SQLite session history responsibilities.
use super::*;
impl AsyncSqliteStateStore {
    /// Loads active-session events for one route band.
    pub async fn active_session_events_for_route_band(
        &self,
        route_band: &str,
    ) -> Result<Vec<ActiveSessionEvent>, StateStoreError> {
        let rows = sqlx::query(
            "SELECT account_id, route_band, process_run_id, logical_session_id, reservation_id,
                    event_kind, event_unix_seconds, session_started_unix_seconds,
                    session_ended_unix_seconds, transport_kind
               FROM active_session_events
              WHERE route_band = ?1
              ORDER BY event_unix_seconds, id",
        )
        .bind(route_band)
        .fetch_all(&self.pool)
        .await
        .map_err(sqlx_error)?;

        rows.into_iter().map(parse_active_session_event).collect()
    }

    /// Returns the newest recorded activity event for one account and route band.
    pub async fn latest_active_session_activity_unix_seconds(
        &self,
        account_id: &AccountId,
        route_band: &str,
    ) -> Result<Option<u64>, StateStoreError> {
        let event_unix_seconds = sqlx::query_scalar!(
            "SELECT MAX(event_unix_seconds) AS \"event_unix_seconds?\"
               FROM active_session_events
              WHERE account_id = ?1 AND route_band = ?2",
            account_id.as_str(),
            route_band
        )
        .fetch_one(&self.pool)
        .await
        .map_err(sqlx_error)?;
        event_unix_seconds
            .map(|value| i64_to_u64(value, account_id.as_str(), "last_activity_unix_seconds"))
            .transpose()
    }

    /// Compacts completed active-session events whose terminal event predates the cutoff.
    ///
    /// An acquired event is retained until its matching released, retired, or stale-purged
    /// event is also eligible. This keeps a long-lived or not-yet-reconciled session available
    /// to interval reconstruction.
    pub async fn compact_completed_active_session_events_before(
        &self,
        route_band: &str,
        completed_before_unix_seconds: u64,
    ) -> Result<(), StateStoreError> {
        sqlx::query(
            "DELETE FROM active_session_events AS event
              WHERE event.route_band = ?1
                AND (
                    (event.event_kind IN ('released', 'retired', 'stale_purged')
                     AND event.event_unix_seconds < ?2)
                    OR (
                        event.event_kind = 'acquired'
                        AND EXISTS (
                            SELECT 1
                              FROM active_session_events AS terminal
                             WHERE terminal.route_band = event.route_band
                               AND terminal.process_run_id = event.process_run_id
                               AND terminal.reservation_id = event.reservation_id
                               AND terminal.event_kind IN ('released', 'retired', 'stale_purged')
                               AND terminal.event_unix_seconds >= event.event_unix_seconds
                               AND terminal.event_unix_seconds < ?2
                        )
                    )
                )",
        )
        .bind(route_band)
        .bind(u64_to_i64(completed_before_unix_seconds)?)
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;

        Ok(())
    }
}

fn parse_active_session_event(row: SqliteRow) -> Result<ActiveSessionEvent, StateStoreError> {
    let account_id_value = row.get::<String, _>(0);
    let account_id =
        AccountId::new(account_id_value.clone()).map_err(|_| StateStoreError::CorruptAccount {
            account_id: account_id_value.clone(),
            field: "account_id",
        })?;
    let event_kind_value = row.get::<String, _>(5);
    let event_kind = ActiveSessionEventKind::parse(&event_kind_value)?;

    Ok(ActiveSessionEvent::new(
        account_id,
        row.get::<String, _>(1),
        row.get::<String, _>(2),
        ReservationId::new(row.get::<String, _>(4)),
        event_kind,
        i64_to_u64(
            row.get::<i64, _>(6),
            &account_id_value,
            "event_unix_seconds",
        )?,
    )
    .with_logical_session_id(row.get::<String, _>(3))
    .with_session_interval(
        i64_to_u64(
            row.get::<i64, _>(7),
            &account_id_value,
            "session_started_unix_seconds",
        )?,
        row.get::<Option<i64>, _>(8)
            .map(|value| i64_to_u64(value, &account_id_value, "session_ended_unix_seconds"))
            .transpose()?,
        row.get::<String, _>(9),
    ))
}
