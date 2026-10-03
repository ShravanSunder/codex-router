//! SQLite sync affinity responsibilities.
use super::affinity_store::parse_previous_response_owner_row;
use super::*;

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
impl AffinityRepository for SqliteStateStore {
    fn pin_account(
        &self,
        affinity_key: &AffinityKey,
        account_id: &AccountId,
    ) -> Result<(), StateStoreError> {
        self.connection
            .execute(
                "INSERT INTO affinity_pins (affinity_key, account_id)
                 VALUES (?1, ?2)
                 ON CONFLICT(affinity_key) DO UPDATE SET
                   account_id = excluded.account_id",
                params![affinity_key.as_str(), account_id.as_str()],
            )
            .map_err(sqlite_error)?;

        Ok(())
    }

    fn load_pin(&self, affinity_key: &AffinityKey) -> Result<Option<AccountId>, StateStoreError> {
        let account_id = self
            .connection
            .query_row(
                "SELECT account_id FROM affinity_pins WHERE affinity_key = ?1",
                params![affinity_key.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(sqlite_error)?;

        account_id
            .map(AccountId::new)
            .transpose()
            .map_err(|_| StateStoreError::CorruptAccount {
                account_id: "<affinity-pin>".to_owned(),
                field: "account_id",
            })
    }

    fn write_previous_response_owner(
        &self,
        owner: &PreviousResponseAffinityOwnerRecord,
    ) -> Result<(), StateStoreError> {
        self.connection
            .execute(
                "INSERT INTO previous_response_affinity_owners (
                   affinity_key_hash, route_band, account_id, credential_generation,
                   source_transport, created_unix_seconds
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(affinity_key_hash, route_band, account_id) DO UPDATE SET
                   credential_generation = excluded.credential_generation,
                   source_transport = excluded.source_transport,
                   created_unix_seconds = excluded.created_unix_seconds",
                params![
                    owner.affinity_key_hash().as_str(),
                    owner.route_band().as_str(),
                    owner.account_id().as_str(),
                    u64_to_i64(owner.credential_generation())?,
                    owner.source_transport().as_str(),
                    u64_to_i64(owner.created_unix_seconds())?,
                ],
            )
            .map_err(sqlite_error)?;

        Ok(())
    }

    fn load_previous_response_owner(
        &self,
        affinity_key_hash: &AffinityKeyHash,
        route_band: &str,
    ) -> Result<PreviousResponseAffinityOwnerLookup, StateStoreError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT affinity_key_hash, route_band, account_id, credential_generation,
                        source_transport, created_unix_seconds
                   FROM previous_response_affinity_owners
                  WHERE affinity_key_hash = ?1 AND route_band = ?2
                  ORDER BY account_id
                  LIMIT 2",
            )
            .map_err(sqlite_error)?;
        let rows = statement
            .query_map(params![affinity_key_hash.as_str(), route_band], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(sqlite_error)?;

        let mut owners = Vec::new();
        for row in rows {
            let (
                hash_value,
                route_band_value,
                account_id_value,
                credential_generation,
                source_transport_value,
                created_unix_seconds,
            ) = row.map_err(sqlite_error)?;
            owners.push(parse_previous_response_owner_row(
                hash_value,
                route_band_value,
                account_id_value,
                credential_generation,
                source_transport_value,
                created_unix_seconds,
            )?);
        }

        match owners.len() {
            0 => Ok(PreviousResponseAffinityOwnerLookup::Missing),
            1 => Ok(PreviousResponseAffinityOwnerLookup::Found(owners.remove(0))),
            _ => Ok(PreviousResponseAffinityOwnerLookup::Ambiguous),
        }
    }

    fn purge_previous_response_owners(&self) -> Result<(), StateStoreError> {
        self.connection
            .execute("DELETE FROM previous_response_affinity_owners", [])
            .map_err(sqlite_error)?;

        Ok(())
    }
}
