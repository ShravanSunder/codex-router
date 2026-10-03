//! SQLite session rollups responsibilities.
use super::*;
impl AsyncSqliteStateStore {
    async fn active_session_intervals_for_route_band(
        &self,
        route_band: &str,
        interval_start_unix_seconds: u64,
        interval_end_unix_seconds: u64,
    ) -> Result<Vec<ActiveSessionInterval>, StateStoreError> {
        let terminal_rows = sqlx::query(
            "SELECT account_id, event_kind, session_started_unix_seconds,
                    COALESCE(session_ended_unix_seconds, event_unix_seconds)
               FROM active_session_events
              WHERE route_band = ?1
                AND event_kind IN ('released', 'retired', 'stale_purged')
                AND session_started_unix_seconds < ?2
                AND COALESCE(session_ended_unix_seconds, event_unix_seconds) > ?3
              ORDER BY session_started_unix_seconds, id",
        )
        .bind(route_band)
        .bind(u64_to_i64(interval_end_unix_seconds)?)
        .bind(u64_to_i64(interval_start_unix_seconds)?)
        .fetch_all(&self.pool)
        .await
        .map_err(sqlx_error)?;

        let mut intervals = Vec::with_capacity(terminal_rows.len());
        for row in terminal_rows {
            let account_id_value = row.get::<String, _>(0);
            let account_id = AccountId::new(account_id_value.clone()).map_err(|_| {
                StateStoreError::CorruptAccount {
                    account_id: account_id_value.clone(),
                    field: "account_id",
                }
            })?;
            intervals.push(ActiveSessionInterval {
                account_id,
                start_unix_seconds: i64_to_u64(
                    row.get::<i64, _>(2),
                    &account_id_value,
                    "session_started_unix_seconds",
                )?,
                end_unix_seconds: i64_to_u64(
                    row.get::<i64, _>(3),
                    &account_id_value,
                    "session_ended_unix_seconds",
                )?,
                terminal_event_kind: Some(ActiveSessionEventKind::parse(&row.get::<String, _>(1))?),
            });
        }

        let open_rows = sqlx::query(
            "SELECT acquired.account_id, acquired.session_started_unix_seconds
               FROM active_session_events acquired
              WHERE acquired.route_band = ?1
                AND acquired.event_kind = 'acquired'
                AND acquired.session_started_unix_seconds < ?2
                AND NOT EXISTS (
                    SELECT 1
                      FROM active_session_events terminal
                     WHERE terminal.route_band = acquired.route_band
                       AND terminal.process_run_id = acquired.process_run_id
                       AND terminal.reservation_id = acquired.reservation_id
                       AND terminal.event_kind IN ('released', 'retired', 'stale_purged')
                       AND terminal.event_unix_seconds >= acquired.event_unix_seconds
                )
              ORDER BY acquired.session_started_unix_seconds, acquired.id",
        )
        .bind(route_band)
        .bind(u64_to_i64(interval_end_unix_seconds)?)
        .fetch_all(&self.pool)
        .await
        .map_err(sqlx_error)?;

        intervals.reserve(open_rows.len());
        for row in open_rows {
            let account_id_value = row.get::<String, _>(0);
            let account_id = AccountId::new(account_id_value.clone()).map_err(|_| {
                StateStoreError::CorruptAccount {
                    account_id: account_id_value.clone(),
                    field: "account_id",
                }
            })?;
            intervals.push(ActiveSessionInterval {
                account_id,
                start_unix_seconds: i64_to_u64(
                    row.get::<i64, _>(1),
                    &account_id_value,
                    "session_started_unix_seconds",
                )?,
                end_unix_seconds: interval_end_unix_seconds,
                terminal_event_kind: None,
            });
        }

        Ok(intervals)
    }

    /// Refreshes persisted active-session rollups for one interval.
    pub async fn refresh_active_session_rollups_for_interval(
        &self,
        route_band: &str,
        interval_start_unix_seconds: u64,
        interval_end_unix_seconds: u64,
        bucket_seconds: u64,
    ) -> Result<(), StateStoreError> {
        if interval_end_unix_seconds <= interval_start_unix_seconds || bucket_seconds == 0 {
            return Ok(());
        }

        let intervals = self
            .active_session_intervals_for_route_band(
                route_band,
                interval_start_unix_seconds,
                interval_end_unix_seconds,
            )
            .await?;
        let rollups = compute_active_session_rollups_from_intervals(
            route_band,
            intervals,
            interval_start_unix_seconds,
            interval_end_unix_seconds,
            bucket_seconds,
        );

        let delete_start = interval_start_unix_seconds
            .saturating_sub(interval_start_unix_seconds % bucket_seconds);
        let delete_end = interval_end_unix_seconds.div_ceil(bucket_seconds) * bucket_seconds;
        sqlx::query(
            "DELETE FROM active_session_rollups
              WHERE route_band = ?1
                AND bucket_start_unix_seconds >= ?2
                AND bucket_start_unix_seconds < ?3",
        )
        .bind(route_band)
        .bind(u64_to_i64(delete_start)?)
        .bind(u64_to_i64(delete_end)?)
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;

        for rollup in rollups {
            sqlx::query(
                "INSERT INTO active_session_rollups (
                   account_id, route_band, bucket_start_unix_seconds,
                   bucket_end_unix_seconds, active_session_seconds,
                   max_concurrent_sessions, completed_sessions,
                   stale_purged_sessions
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(account_id, route_band, bucket_start_unix_seconds, bucket_end_unix_seconds)
                 DO UPDATE SET
                   active_session_seconds = excluded.active_session_seconds,
                   max_concurrent_sessions = excluded.max_concurrent_sessions,
                   completed_sessions = excluded.completed_sessions,
                   stale_purged_sessions = excluded.stale_purged_sessions",
            )
            .bind(rollup.account_id.as_str())
            .bind(&rollup.route_band)
            .bind(u64_to_i64(rollup.bucket_start_unix_seconds)?)
            .bind(u64_to_i64(rollup.bucket_end_unix_seconds)?)
            .bind(u64_to_i64(rollup.active_session_seconds)?)
            .bind(u32_to_i64(rollup.max_concurrent_sessions))
            .bind(u32_to_i64(rollup.completed_sessions))
            .bind(u32_to_i64(rollup.stale_purged_sessions))
            .execute(&self.pool)
            .await
            .map_err(sqlx_error)?;
        }

        Ok(())
    }

    /// Loads active-session rollups for one route band and interval.
    pub async fn active_session_rollups_for_route_band(
        &self,
        route_band: &str,
        interval_start_unix_seconds: u64,
        interval_end_unix_seconds: u64,
    ) -> Result<Vec<ActiveSessionRollup>, StateStoreError> {
        let rows = sqlx::query(
            "SELECT account_id, route_band, bucket_start_unix_seconds,
                    bucket_end_unix_seconds, active_session_seconds,
                    max_concurrent_sessions, completed_sessions,
                    stale_purged_sessions
               FROM active_session_rollups
              WHERE route_band = ?1
                AND bucket_end_unix_seconds > ?2
                AND bucket_start_unix_seconds < ?3
              ORDER BY account_id, bucket_start_unix_seconds",
        )
        .bind(route_band)
        .bind(u64_to_i64(interval_start_unix_seconds)?)
        .bind(u64_to_i64(interval_end_unix_seconds)?)
        .fetch_all(&self.pool)
        .await
        .map_err(sqlx_error)?;

        rows.into_iter().map(parse_active_session_rollup).collect()
    }

    /// Purges persisted active-session rollup buckets that end before the cutoff.
    pub async fn purge_active_session_rollups_before(
        &self,
        cutoff_unix_seconds: u64,
    ) -> Result<(), StateStoreError> {
        sqlx::query(
            "DELETE FROM active_session_rollups
              WHERE bucket_end_unix_seconds < ?1",
        )
        .bind(u64_to_i64(cutoff_unix_seconds)?)
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;

        Ok(())
    }
}

fn parse_active_session_rollup(row: SqliteRow) -> Result<ActiveSessionRollup, StateStoreError> {
    let account_id_value = row.get::<String, _>(0);
    let account_id =
        AccountId::new(account_id_value.clone()).map_err(|_| StateStoreError::CorruptAccount {
            account_id: account_id_value.clone(),
            field: "account_id",
        })?;

    Ok(ActiveSessionRollup::new(
        account_id,
        row.get::<String, _>(1),
        i64_to_u64(
            row.get::<i64, _>(2),
            &account_id_value,
            "bucket_start_unix_seconds",
        )?,
        i64_to_u64(
            row.get::<i64, _>(3),
            &account_id_value,
            "bucket_end_unix_seconds",
        )?,
        i64_to_u64(
            row.get::<i64, _>(4),
            &account_id_value,
            "active_session_seconds",
        )?,
        i64_to_u32(
            row.get::<i64, _>(5),
            &account_id_value,
            "max_concurrent_sessions",
        )?,
    )
    .with_terminal_counts(
        i64_to_u32(
            row.get::<i64, _>(6),
            &account_id_value,
            "completed_sessions",
        )?,
        i64_to_u32(
            row.get::<i64, _>(7),
            &account_id_value,
            "stale_purged_sessions",
        )?,
    ))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ActiveSessionInterval {
    account_id: AccountId,
    start_unix_seconds: u64,
    end_unix_seconds: u64,
    terminal_event_kind: Option<ActiveSessionEventKind>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RollupKey {
    account_id: AccountId,
    bucket_start_unix_seconds: u64,
    bucket_end_unix_seconds: u64,
}

fn compute_active_session_rollups_from_intervals(
    route_band: &str,
    intervals: Vec<ActiveSessionInterval>,
    interval_start_unix_seconds: u64,
    interval_end_unix_seconds: u64,
    bucket_seconds: u64,
) -> Vec<ActiveSessionRollup> {
    if interval_end_unix_seconds <= interval_start_unix_seconds || bucket_seconds == 0 {
        return Vec::new();
    }

    let delete_start =
        interval_start_unix_seconds.saturating_sub(interval_start_unix_seconds % bucket_seconds);
    let delete_end = interval_end_unix_seconds.div_ceil(bucket_seconds) * bucket_seconds;
    let mut bucket_segments = BTreeMap::<RollupKey, Vec<(u64, u64)>>::new();
    let mut bucket_terminal_events = BTreeMap::<RollupKey, Vec<ActiveSessionEventKind>>::new();
    for session in intervals {
        let clipped_start = session.start_unix_seconds.max(interval_start_unix_seconds);
        let clipped_end = session.end_unix_seconds.min(interval_end_unix_seconds);
        if clipped_end <= clipped_start {
            continue;
        }

        let mut bucket_start = clipped_start.saturating_sub(clipped_start % bucket_seconds);
        while bucket_start < clipped_end && bucket_start < delete_end {
            let bucket_end = bucket_start.saturating_add(bucket_seconds);
            let segment_start = clipped_start.max(bucket_start).max(delete_start);
            let segment_end = clipped_end.min(bucket_end).min(delete_end);
            if segment_end > segment_start {
                bucket_segments
                    .entry(RollupKey {
                        account_id: session.account_id.clone(),
                        bucket_start_unix_seconds: bucket_start,
                        bucket_end_unix_seconds: bucket_end,
                    })
                    .or_default()
                    .push((segment_start, segment_end));
            }
            bucket_start = bucket_end;
        }
        if let Some(terminal_event_kind) = session.terminal_event_kind {
            let terminal_time = session.end_unix_seconds;
            if terminal_time >= interval_start_unix_seconds
                && terminal_time < interval_end_unix_seconds
            {
                let bucket_start = terminal_time.saturating_sub(terminal_time % bucket_seconds);
                let bucket_end = bucket_start.saturating_add(bucket_seconds);
                bucket_terminal_events
                    .entry(RollupKey {
                        account_id: session.account_id,
                        bucket_start_unix_seconds: bucket_start,
                        bucket_end_unix_seconds: bucket_end,
                    })
                    .or_default()
                    .push(terminal_event_kind);
            }
        }
    }

    bucket_segments
        .into_iter()
        .map(|(key, segments)| {
            let active_session_seconds =
                segments.iter().map(|(start, end)| end - start).sum::<u64>();
            let terminal_events = bucket_terminal_events.remove(&key).unwrap_or_default();
            let completed_sessions = terminal_events
                .iter()
                .filter(|event_kind| {
                    matches!(
                        event_kind,
                        ActiveSessionEventKind::Released | ActiveSessionEventKind::Retired
                    )
                })
                .count();
            let stale_purged_sessions = terminal_events
                .iter()
                .filter(|event_kind| matches!(event_kind, ActiveSessionEventKind::StalePurged))
                .count();
            ActiveSessionRollup::new(
                key.account_id,
                route_band,
                key.bucket_start_unix_seconds,
                key.bucket_end_unix_seconds,
                active_session_seconds,
                max_concurrent_sessions(&segments),
            )
            .with_terminal_counts(
                u32::try_from(completed_sessions).unwrap_or(u32::MAX),
                u32::try_from(stale_purged_sessions).unwrap_or(u32::MAX),
            )
        })
        .collect()
}

fn max_concurrent_sessions(segments: &[(u64, u64)]) -> u32 {
    let mut edges = Vec::with_capacity(segments.len() * 2);
    for (start, end) in segments {
        edges.push((*start, 1_i32));
        edges.push((*end, -1_i32));
    }
    edges.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));

    let mut current = 0_i32;
    let mut maximum = 0_i32;
    for (_, delta) in edges {
        current += delta;
        maximum = maximum.max(current);
    }

    u32::try_from(maximum).unwrap_or_default()
}
