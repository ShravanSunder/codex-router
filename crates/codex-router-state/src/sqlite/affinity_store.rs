//! SQLite affinity store responsibilities.
use super::*;
impl AsyncSqliteStateStore {
    /// Writes a previous-response owner record through the async state pool.
    pub async fn write_previous_response_owner(
        &self,
        owner: &PreviousResponseAffinityOwnerRecord,
    ) -> Result<(), StateStoreError> {
        sqlx::query(
            "INSERT INTO previous_response_affinity_owners (
               affinity_key_hash, route_band, account_id, credential_generation,
               source_transport, created_unix_seconds
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(affinity_key_hash, route_band, account_id) DO UPDATE SET
               credential_generation = excluded.credential_generation,
               source_transport = excluded.source_transport,
               created_unix_seconds = excluded.created_unix_seconds",
        )
        .bind(owner.affinity_key_hash().as_str())
        .bind(owner.route_band().as_str())
        .bind(owner.account_id().as_str())
        .bind(u64_to_i64(owner.credential_generation())?)
        .bind(owner.source_transport().as_str())
        .bind(u64_to_i64(owner.created_unix_seconds())?)
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;

        Ok(())
    }

    /// Inserts or replaces the account affinity for one provider session.
    pub async fn upsert_session_account_affinity(
        &self,
        affinity: &SessionAccountAffinity,
    ) -> Result<(), StateStoreError> {
        sqlx::query(
            "INSERT INTO session_account_affinities (
               provider, session_id, account_id, last_seen_unix_seconds, pin_version
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(provider, session_id) DO UPDATE SET
               account_id = excluded.account_id,
               last_seen_unix_seconds = excluded.last_seen_unix_seconds,
               pin_version = excluded.pin_version",
        )
        .bind(affinity.provider().as_str())
        .bind(affinity.session_id())
        .bind(affinity.account_id().map(AccountId::as_str))
        .bind(u64_to_i64(affinity.last_seen_unix_seconds())?)
        .bind(u64_to_i64(affinity.pin_version())?)
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;
        Ok(())
    }

    /// Compare-and-sets one session pin from its observed account and version.
    ///
    /// `affinity.last_seen_unix_seconds()` is the publication time used to decide
    /// whether the observed pin is still active. An inactive observation may claim
    /// only a missing, released, or expired row; an active observation may renew
    /// only the same account while it remains active at publication time.
    pub async fn compare_and_set_session_account_affinity(
        &self,
        observation: &PinObservation,
        affinity: &SessionAccountAffinity,
        pin_ttl_seconds: u64,
    ) -> Result<bool, StateStoreError> {
        let expected_version = observation.version();
        let next_version = expected_version.checked_add(1);
        let desired_version = affinity.pin_version();
        let valid_transition = match (observation.active_account(), affinity.account_id()) {
            (None, Some(_)) => next_version == Some(desired_version),
            (Some(observed), Some(replacement)) => {
                observed == replacement && desired_version == expected_version
            }
            (Some(_), None) => next_version == Some(desired_version),
            (None, None) => false,
        };
        if !valid_transition {
            return Ok(false);
        }

        let expected_version = u64_to_i64(expected_version)?;
        let desired_version = u64_to_i64(desired_version)?;
        let publication_time = u64_to_i64(affinity.last_seen_unix_seconds())?;
        let pin_ttl_seconds = u64_to_i64(pin_ttl_seconds)?;
        let observed_account_id = observation.active_account().map(AccountId::as_str);
        let replacement_account_id = affinity.account_id().map(AccountId::as_str);
        let result = sqlx::query!(
            "INSERT INTO session_account_affinities (
                 provider, session_id, account_id, last_seen_unix_seconds, pin_version
             )
             SELECT ?1, ?2, ?3, ?4, ?5
              WHERE (
                    (?6 = 0 AND ?7 IS NULL
                        AND NOT EXISTS (
                            SELECT 1 FROM session_account_affinities
                             WHERE provider = ?1 AND session_id = ?2
                        ))
                    OR EXISTS (
                        SELECT 1 FROM session_account_affinities AS observed
                         WHERE observed.provider = ?1 AND observed.session_id = ?2
                           AND observed.pin_version = ?6
                           AND (
                               (?7 IS NULL AND (
                                   observed.account_id IS NULL
                                   OR (?4 >= ?8 AND observed.last_seen_unix_seconds <= ?4 - ?8)
                               ))
                               OR (?7 IS NOT NULL AND observed.account_id = ?7
                                   AND (?4 < ?8 OR observed.last_seen_unix_seconds > ?4 - ?8))
                           )
                    )
              )
             ON CONFLICT(provider, session_id) DO UPDATE SET
                 account_id = excluded.account_id,
                 last_seen_unix_seconds = MAX(
                     session_account_affinities.last_seen_unix_seconds,
                     excluded.last_seen_unix_seconds
                 ),
                 pin_version = excluded.pin_version
             WHERE session_account_affinities.pin_version = ?6
               AND (
                   (?7 IS NULL AND (
                       session_account_affinities.account_id IS NULL
                       OR (?4 >= ?8
                           AND session_account_affinities.last_seen_unix_seconds <= ?4 - ?8)
                   ))
                   OR (?7 IS NOT NULL
                       AND session_account_affinities.account_id = ?7
                       AND (?4 < ?8
                           OR session_account_affinities.last_seen_unix_seconds > ?4 - ?8))
               )",
            affinity.provider().as_str(),
            affinity.session_id(),
            replacement_account_id,
            publication_time,
            desired_version,
            expected_version,
            observed_account_id,
            pin_ttl_seconds,
        )
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;

        Ok(result.rows_affected() == 1)
    }

    /// Loads the account affinity for one provider session.
    pub async fn load_session_account_affinity(
        &self,
        provider: Provider,
        session_id: &str,
    ) -> Result<Option<SessionAccountAffinity>, StateStoreError> {
        let row = sqlx::query(
            "SELECT provider, session_id, account_id, last_seen_unix_seconds, pin_version
               FROM session_account_affinities
              WHERE provider = ?1 AND session_id = ?2",
        )
        .bind(provider.as_str())
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(sqlx_error)?;

        row.map(|row| {
            parse_session_account_affinity_row(
                row.get::<String, _>(0),
                row.get::<String, _>(1),
                row.get::<Option<String>, _>(2),
                row.get::<i64, _>(3),
                row.get::<i64, _>(4),
            )
        })
        .transpose()
    }

    /// Purges session-account affinities last observed before the cutoff.
    pub async fn purge_session_account_affinities_before(
        &self,
        cutoff_unix_seconds: u64,
    ) -> Result<(), StateStoreError> {
        sqlx::query(
            "DELETE FROM session_account_affinities
              WHERE last_seen_unix_seconds < ?1",
        )
        .bind(u64_to_i64(cutoff_unix_seconds)?)
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;
        Ok(())
    }

    /// Loads a hashed previous-response owner record for one route band.
    pub async fn load_previous_response_owner(
        &self,
        affinity_key_hash: &AffinityKeyHash,
        route_band: &str,
    ) -> Result<PreviousResponseAffinityOwnerLookup, StateStoreError> {
        let rows = sqlx::query(
            "SELECT affinity_key_hash, route_band, account_id, credential_generation,
                    source_transport, created_unix_seconds
               FROM previous_response_affinity_owners
              WHERE affinity_key_hash = ?1 AND route_band = ?2
              ORDER BY account_id
              LIMIT 2",
        )
        .bind(affinity_key_hash.as_str())
        .bind(route_band)
        .fetch_all(&self.pool)
        .await
        .map_err(sqlx_error)?;

        let mut owners = Vec::new();
        for row in rows {
            owners.push(parse_previous_response_owner_row(
                row.get::<String, _>(0),
                row.get::<String, _>(1),
                row.get::<String, _>(2),
                row.get::<i64, _>(3),
                row.get::<String, _>(4),
                row.get::<i64, _>(5),
            )?);
        }

        match owners.len() {
            0 => Ok(PreviousResponseAffinityOwnerLookup::Missing),
            1 => Ok(PreviousResponseAffinityOwnerLookup::Found(owners.remove(0))),
            _ => Ok(PreviousResponseAffinityOwnerLookup::Ambiguous),
        }
    }
}

fn parse_session_account_affinity_row(
    provider_value: String,
    session_id: String,
    account_id_value: Option<String>,
    last_seen_unix_seconds: i64,
    pin_version: i64,
) -> Result<SessionAccountAffinity, StateStoreError> {
    let corrupt_affinity = |field| StateStoreError::CorruptSessionAccountAffinity {
        session_id: session_id.clone(),
        field,
    };
    let provider = Provider::parse(&provider_value).ok_or_else(|| corrupt_affinity("provider"))?;
    let account_id = account_id_value
        .map(|account_id_value| {
            AccountId::new(account_id_value).map_err(|_| corrupt_affinity("account_id"))
        })
        .transpose()?;
    let last_seen_unix_seconds = u64::try_from(last_seen_unix_seconds)
        .map_err(|_| corrupt_affinity("last_seen_unix_seconds"))?;
    let pin_version = u64::try_from(pin_version).map_err(|_| corrupt_affinity("pin_version"))?;
    Ok(SessionAccountAffinity::with_pin_state(
        provider,
        session_id,
        account_id,
        pin_version,
        last_seen_unix_seconds,
    ))
}

pub(super) fn parse_previous_response_owner_row(
    hash_value: String,
    route_band_value: String,
    account_id_value: String,
    credential_generation: i64,
    source_transport_value: String,
    created_unix_seconds: i64,
) -> Result<PreviousResponseAffinityOwnerRecord, StateStoreError> {
    let affinity_key_hash =
        AffinityKeyHash::new(hash_value).map_err(|_| StateStoreError::CorruptQuotaSnapshot {
            account_id: account_id_value.clone(),
            field: "affinity_key_hash",
        })?;
    let route_band = RouteBand::parse(&route_band_value).ok_or_else(|| {
        StateStoreError::CorruptQuotaSnapshot {
            account_id: account_id_value.clone(),
            field: "route_band",
        }
    })?;
    let account_id =
        AccountId::new(account_id_value.clone()).map_err(|_| StateStoreError::CorruptAccount {
            account_id: account_id_value.clone(),
            field: "account_id",
        })?;
    let credential_generation = i64_to_u64_account_generation(
        credential_generation,
        &account_id_value,
        "credential_generation",
    )?;
    let source_transport =
        AffinitySourceTransport::parse(&source_transport_value).ok_or_else(|| {
            StateStoreError::CorruptQuotaSnapshot {
                account_id: account_id_value.clone(),
                field: "source_transport",
            }
        })?;
    let created_unix_seconds = i64_to_u64(
        created_unix_seconds,
        &account_id_value,
        "created_unix_seconds",
    )?;

    Ok(PreviousResponseAffinityOwnerRecord::new(
        affinity_key_hash,
        account_id,
        credential_generation,
        route_band,
        source_transport,
        created_unix_seconds,
    ))
}

#[cfg(test)]
#[path = "affinity_store_tests.rs"]
mod tests;
