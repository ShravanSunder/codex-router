//! SQLite active leases responsibilities.
use super::*;
struct ActiveSessionEventInsert<'a> {
    account_id: &'a AccountId,
    route_band: &'a str,
    process_run_id: &'a str,
    logical_session_id: &'a str,
    reservation_id: &'a ReservationId,
    event_kind: ActiveSessionEventKind,
    event_unix_seconds: u64,
    session_started_unix_seconds: u64,
    session_ended_unix_seconds: Option<u64>,
    transport_kind: &'a str,
}

impl AsyncSqliteStateStore {
    /// Records one active client lease for status/proof surfaces.
    pub async fn record_active_client_acquired(
        &self,
        route_band: &str,
        process_run_id: &str,
        reservation_id: &ReservationId,
        account_id: &AccountId,
        acquired_unix_seconds: u64,
        active_pressure: u32,
    ) -> Result<(), StateStoreError> {
        sqlx::query(
            "INSERT INTO active_client_leases (
               route_band, process_run_id, reservation_id, account_id,
               acquired_unix_seconds, active_pressure
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(route_band, process_run_id, reservation_id) DO UPDATE SET
               account_id = excluded.account_id,
               acquired_unix_seconds = excluded.acquired_unix_seconds,
               active_pressure = excluded.active_pressure",
        )
        .bind(route_band)
        .bind(process_run_id)
        .bind(reservation_id.as_str())
        .bind(account_id.as_str())
        .bind(u64_to_i64(acquired_unix_seconds)?)
        .bind(u32_to_i64(active_pressure))
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;

        self.append_active_session_event(ActiveSessionEventInsert {
            account_id,
            route_band,
            process_run_id,
            logical_session_id: reservation_id.as_str(),
            reservation_id,
            event_kind: ActiveSessionEventKind::Acquired,
            event_unix_seconds: acquired_unix_seconds,
            session_started_unix_seconds: acquired_unix_seconds,
            session_ended_unix_seconds: None,
            transport_kind: "unknown",
        })
        .await?;

        Ok(())
    }

    /// Releases one active client lease.
    pub async fn record_active_client_released(
        &self,
        route_band: &str,
        process_run_id: &str,
        reservation_id: &ReservationId,
        released_unix_seconds: u64,
    ) -> Result<(), StateStoreError> {
        let lease = sqlx::query(
            "SELECT account_id, acquired_unix_seconds
               FROM active_client_leases
              WHERE route_band = ?1 AND process_run_id = ?2 AND reservation_id = ?3",
        )
        .bind(route_band)
        .bind(process_run_id)
        .bind(reservation_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(sqlx_error)?;

        if let Some(row) = lease {
            let account_id_value = row.get::<String, _>(0);
            let account_id = AccountId::new(account_id_value.clone()).map_err(|_| {
                StateStoreError::CorruptAccount {
                    account_id: account_id_value.clone(),
                    field: "account_id",
                }
            })?;
            self.append_active_session_event(ActiveSessionEventInsert {
                account_id: &account_id,
                route_band,
                process_run_id,
                logical_session_id: reservation_id.as_str(),
                reservation_id,
                event_kind: ActiveSessionEventKind::Released,
                event_unix_seconds: released_unix_seconds,
                session_started_unix_seconds: i64_to_u64(
                    row.get::<i64, _>(1),
                    &account_id_value,
                    "acquired_unix_seconds",
                )?,
                session_ended_unix_seconds: Some(released_unix_seconds),
                transport_kind: "unknown",
            })
            .await?;
        }

        sqlx::query(
            "DELETE FROM active_client_leases
              WHERE route_band = ?1 AND process_run_id = ?2 AND reservation_id = ?3",
        )
        .bind(route_band)
        .bind(process_run_id)
        .bind(reservation_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;

        Ok(())
    }

    async fn append_active_session_event(
        &self,
        event: ActiveSessionEventInsert<'_>,
    ) -> Result<(), StateStoreError> {
        sqlx::query(
            "INSERT INTO active_session_events (
               account_id, route_band, process_run_id, logical_session_id, reservation_id,
               event_kind, event_unix_seconds, session_started_unix_seconds,
               session_ended_unix_seconds, transport_kind
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )
        .bind(event.account_id.as_str())
        .bind(event.route_band)
        .bind(event.process_run_id)
        .bind(event.logical_session_id)
        .bind(event.reservation_id.as_str())
        .bind(event.event_kind.as_str())
        .bind(u64_to_i64(event.event_unix_seconds)?)
        .bind(u64_to_i64(event.session_started_unix_seconds)?)
        .bind(
            event
                .session_ended_unix_seconds
                .map(u64_to_i64)
                .transpose()?,
        )
        .bind(event.transport_kind)
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;

        Ok(())
    }

    /// Loads active client counts and prunes stale leases.
    pub async fn active_client_counts_for_route_band(
        &self,
        route_band: &str,
        now_unix_seconds: u64,
        max_age_seconds: u64,
    ) -> Result<Vec<ActiveClientCount>, StateStoreError> {
        let oldest_allowed = now_unix_seconds.saturating_sub(max_age_seconds);
        let stale_rows = sqlx::query(
            "SELECT account_id, process_run_id, reservation_id, acquired_unix_seconds
               FROM active_client_leases
              WHERE route_band = ?1 AND acquired_unix_seconds < ?2
              ORDER BY acquired_unix_seconds, account_id, process_run_id, reservation_id",
        )
        .bind(route_band)
        .bind(u64_to_i64(oldest_allowed)?)
        .fetch_all(&self.pool)
        .await
        .map_err(sqlx_error)?;

        for row in stale_rows {
            let account_id_value = row.get::<String, _>(0);
            let account_id = AccountId::new(account_id_value.clone()).map_err(|_| {
                StateStoreError::CorruptAccount {
                    account_id: account_id_value.clone(),
                    field: "account_id",
                }
            })?;
            let reservation_id = ReservationId::new(row.get::<String, _>(2));
            let logical_session_id = reservation_id.as_str().to_owned();
            self.append_active_session_event(ActiveSessionEventInsert {
                account_id: &account_id,
                route_band,
                process_run_id: &row.get::<String, _>(1),
                logical_session_id: &logical_session_id,
                reservation_id: &reservation_id,
                event_kind: ActiveSessionEventKind::StalePurged,
                event_unix_seconds: now_unix_seconds,
                session_started_unix_seconds: i64_to_u64(
                    row.get::<i64, _>(3),
                    &account_id_value,
                    "acquired_unix_seconds",
                )?,
                session_ended_unix_seconds: Some(now_unix_seconds),
                transport_kind: "unknown",
            })
            .await?;
        }

        sqlx::query(
            "DELETE FROM active_client_leases
              WHERE route_band = ?1 AND acquired_unix_seconds < ?2",
        )
        .bind(route_band)
        .bind(u64_to_i64(oldest_allowed)?)
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;

        let rows = sqlx::query(
            "SELECT account_id,
                    COUNT(*) AS active_clients,
                    SUM(active_pressure) AS active_pressure
               FROM active_client_leases
              WHERE route_band = ?1
              GROUP BY account_id
              ORDER BY account_id",
        )
        .bind(route_band)
        .fetch_all(&self.pool)
        .await
        .map_err(sqlx_error)?;

        let mut counts = Vec::new();
        for row in rows {
            let account_id_value = row.get::<String, _>(0);
            let active_clients =
                i64_to_u32(row.get::<i64, _>(1), &account_id_value, "active_clients")?;
            let active_pressure =
                i64_to_u32(row.get::<i64, _>(2), &account_id_value, "active_pressure")?;
            let account_id = AccountId::new(account_id_value.clone()).map_err(|_| {
                StateStoreError::CorruptAccount {
                    account_id: account_id_value,
                    field: "account_id",
                }
            })?;
            counts.push(ActiveClientCount::new(
                account_id,
                active_clients,
                active_pressure,
            ));
        }

        Ok(counts)
    }

    /// Loads active client counts without pruning stale leases.
    pub async fn active_client_counts_for_route_band_read_only(
        &self,
        route_band: &str,
        now_unix_seconds: u64,
        max_age_seconds: u64,
    ) -> Result<Vec<ActiveClientCount>, StateStoreError> {
        let oldest_allowed = now_unix_seconds.saturating_sub(max_age_seconds);
        let rows = sqlx::query(
            "SELECT account_id,
                    COUNT(*) AS active_clients,
                    SUM(active_pressure) AS active_pressure
               FROM active_client_leases
              WHERE route_band = ?1 AND acquired_unix_seconds >= ?2
              GROUP BY account_id
              ORDER BY account_id",
        )
        .bind(route_band)
        .bind(u64_to_i64(oldest_allowed)?)
        .fetch_all(&self.pool)
        .await
        .map_err(sqlx_error)?;

        let mut counts = Vec::new();
        for row in rows {
            let account_id_value = row.get::<String, _>(0);
            let active_clients =
                i64_to_u32(row.get::<i64, _>(1), &account_id_value, "active_clients")?;
            let active_pressure =
                i64_to_u32(row.get::<i64, _>(2), &account_id_value, "active_pressure")?;
            let account_id = AccountId::new(account_id_value.clone()).map_err(|_| {
                StateStoreError::CorruptAccount {
                    account_id: account_id_value,
                    field: "account_id",
                }
            })?;
            counts.push(ActiveClientCount::new(
                account_id,
                active_clients,
                active_pressure,
            ));
        }

        Ok(counts)
    }
}
