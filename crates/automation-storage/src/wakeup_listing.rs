//! Snapshot membership and stable creation ordering for bounded wake listings.
use crate::{AutomationStore, StorageError};
use agent_automation::{WakeRecord, WakeupId};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::{Connection, Row};
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WakeListPosition {
    pub upper_created_at_ms: i64,
    pub upper_wakeup_id: WakeupId,
    pub created_at_ms: i64,
    pub wakeup_id: WakeupId,
}
pub struct WakeListPage<TMessage> {
    pub records: Vec<WakeRecord<TMessage>>,
    pub next: Option<WakeListPosition>,
}
impl AutomationStore {
    pub async fn list_wakeups<TMessage: DeserializeOwned>(
        &mut self,
        after: Option<WakeListPosition>,
        limit: u32,
    ) -> Result<WakeListPage<TMessage>, StorageError> {
        if !(1..=100).contains(&limit) {
            return Err(StorageError::InvalidRecord);
        }
        let mut transaction = self.connection.begin().await?;
        let upper = if let Some(position) = &after {
            Some((
                position.upper_created_at_ms,
                position.upper_wakeup_id.clone(),
            ))
        } else {
            let row=sqlx::query("SELECT created_at_ms,wakeup_id FROM wakeup_definitions ORDER BY created_at_ms DESC,wakeup_id DESC LIMIT 1").fetch_optional(&mut *transaction).await?;
            row.map(|row| {
                Ok::<_, StorageError>((
                    row.try_get::<i64, _>("created_at_ms")?,
                    row.try_get::<String, _>("wakeup_id")?
                        .try_into()
                        .map_err(|_| StorageError::InvalidRecord)?,
                ))
            })
            .transpose()?
        };
        let Some((upper_created_at_ms, upper_wakeup_id)) = upper else {
            transaction.commit().await?;
            return Ok(WakeListPage {
                records: Vec::new(),
                next: None,
            });
        };
        if upper_created_at_ms < 0
            || after
                .as_ref()
                .is_some_and(|position| position.created_at_ms < 0)
        {
            return Err(StorageError::InvalidRecord);
        }
        let rows=sqlx::query("SELECT wakeup_id,created_at_ms FROM wakeup_definitions WHERE (created_at_ms,wakeup_id)<=(?,?) AND (? IS NULL OR (created_at_ms,wakeup_id)>(?,?)) ORDER BY created_at_ms,wakeup_id LIMIT ?")
            .bind(upper_created_at_ms).bind(upper_wakeup_id.as_str()).bind(after.as_ref().map(|position|position.created_at_ms)).bind(after.as_ref().map(|position|position.created_at_ms)).bind(after.as_ref().map(|position|position.wakeup_id.as_str())).bind(i64::from(limit)+1).fetch_all(&mut *transaction).await?;
        let has_more =
            rows.len() > usize::try_from(limit).map_err(|_| StorageError::InvalidRecord)?;
        let mut records = Vec::new();
        let mut next = None;
        for row in rows
            .into_iter()
            .take(usize::try_from(limit).map_err(|_| StorageError::InvalidRecord)?)
        {
            let id: WakeupId = row
                .try_get::<String, _>("wakeup_id")?
                .try_into()
                .map_err(|_| StorageError::InvalidRecord)?;
            let created_at_ms = row.try_get("created_at_ms")?;
            records.push(crate::wakeup_repository::read_current(&mut transaction, &id).await?);
            next = Some(WakeListPosition {
                upper_created_at_ms,
                upper_wakeup_id: upper_wakeup_id.clone(),
                created_at_ms,
                wakeup_id: id,
            });
        }
        transaction.commit().await?;
        Ok(WakeListPage {
            records,
            next: if has_more { next } else { None },
        })
    }
}
