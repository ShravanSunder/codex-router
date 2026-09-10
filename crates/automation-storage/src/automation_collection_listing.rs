//! Closed collection selection provides bounded stable keys without accepting SQL from callers.
use crate::{AutomationStore, StorageError};
use agent_automation::{InstructionId, ScheduleId, WakeupId};
use sqlx::{Connection, Row, SqliteConnection};

pub enum AutomationCollection {
    Instructions,
    Schedules,
    Runs(ScheduleId),
    Deliveries(Option<WakeupId>),
    Revisions(InstructionId),
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct AutomationListKey {
    pub created_at_ms: i64,
    pub resource_id: String,
}
#[derive(Clone, Debug)]
pub struct AutomationListPosition {
    pub upper: AutomationListKey,
    pub last: AutomationListKey,
}
impl AutomationListPosition {
    pub fn validate_for(&self, collection: &AutomationCollection) -> Result<(), StorageError> {
        if self.last >= self.upper || self.last.created_at_ms < 0 {
            return Err(StorageError::InvalidRecord);
        }
        if matches!(collection, AutomationCollection::Runs(_)) {
            for key in [&self.upper, &self.last] {
                if run_timestamp(&key.resource_id)? != key.created_at_ms {
                    return Err(StorageError::InvalidRecord);
                }
            }
        }
        Ok(())
    }
}
pub struct AutomationKeyPage {
    pub records: Vec<AutomationListKey>,
    pub upper: Option<AutomationListKey>,
    pub has_more: bool,
}
impl AutomationStore {
    pub async fn list_collection_keys(
        &mut self,
        collection: &AutomationCollection,
        position: Option<AutomationListPosition>,
        limit: u32,
    ) -> Result<AutomationKeyPage, StorageError> {
        if !(1..=100).contains(&limit) {
            return Err(StorageError::InvalidRecord);
        }
        if let Some(position) = &position {
            position.validate_for(collection)?;
        }
        let mut transaction = self.connection.begin().await?;
        validate_parent(&mut transaction, collection).await?;
        let (base, filter) = match collection {
            AutomationCollection::Instructions => (
                "SELECT instruction_id AS resource_id,(SELECT MIN(recorded_at_ms) FROM instruction_revisions r WHERE r.instruction_id=d.instruction_id) AS created_at_ms FROM instruction_documents d WHERE ? IS NULL",
                None,
            ),
            AutomationCollection::Schedules => (
                "SELECT schedule_id AS resource_id,created_at_ms FROM schedule_definitions WHERE ? IS NULL",
                None,
            ),
            AutomationCollection::Runs(id) => (
                "SELECT run_id AS resource_id,0 AS created_at_ms FROM workflow_runs WHERE schedule_id=?",
                Some(id.as_str()),
            ),
            AutomationCollection::Deliveries(id) => (
                "SELECT delivery_id AS resource_id,created_at_ms FROM mailbox_deliveries WHERE (? IS NULL OR wakeup_id=?)",
                id.as_ref().map(WakeupId::as_str),
            ),
            AutomationCollection::Revisions(id) => (
                "SELECT revision_id AS resource_id,recorded_at_ms AS created_at_ms FROM instruction_revisions WHERE instruction_id=?",
                Some(id.as_str()),
            ),
        };
        let run_order = matches!(collection, AutomationCollection::Runs(_));
        let upper = if let Some(position) = &position {
            Some(position.upper.clone())
        } else {
            let order = if run_order {
                "resource_id DESC"
            } else {
                "created_at_ms DESC,resource_id DESC"
            };
            let mut builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new("SELECT * FROM (");
            builder
                .push(base)
                .push(") ORDER BY ")
                .push(order)
                .push(" LIMIT 1");
            let mut query = builder.build().bind(filter);
            if matches!(collection, AutomationCollection::Deliveries(_)) {
                query = query.bind(filter);
            }
            query
                .fetch_optional(&mut *transaction)
                .await?
                .map(|row| decode_key(row, run_order))
                .transpose()?
        };
        let Some(upper) = upper else {
            transaction.commit().await?;
            return Ok(AutomationKeyPage {
                records: Vec::new(),
                upper: None,
                has_more: false,
            });
        };
        let predicate = if run_order {
            "resource_id <= ? AND (? IS NULL OR resource_id > ?) ORDER BY resource_id"
        } else {
            "(created_at_ms,resource_id) <= (?,?) AND (? IS NULL OR (created_at_ms,resource_id) > (?,?)) ORDER BY created_at_ms,resource_id"
        };
        let mut builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new("SELECT * FROM (");
        builder
            .push(base)
            .push(") WHERE ")
            .push(predicate)
            .push(" LIMIT ?");
        let mut query = builder.build().bind(filter);
        if matches!(collection, AutomationCollection::Deliveries(_)) {
            query = query.bind(filter);
        }
        let last = position.as_ref().map(|position| &position.last);
        query = if run_order {
            query
                .bind(&upper.resource_id)
                .bind(last.map(|key| key.resource_id.as_str()))
                .bind(last.map(|key| key.resource_id.as_str()))
        } else {
            query
                .bind(upper.created_at_ms)
                .bind(&upper.resource_id)
                .bind(last.map(|key| key.created_at_ms))
                .bind(last.map(|key| key.created_at_ms))
                .bind(last.map(|key| key.resource_id.as_str()))
        };
        let rows = query
            .bind(i64::from(limit) + 1)
            .fetch_all(&mut *transaction)
            .await?;
        let mut records = rows
            .into_iter()
            .map(|row| decode_key(row, run_order))
            .collect::<Result<Vec<_>, _>>()?;
        let has_more =
            records.len() > usize::try_from(limit).map_err(|_| StorageError::InvalidRecord)?;
        records.truncate(usize::try_from(limit).map_err(|_| StorageError::InvalidRecord)?);
        transaction.commit().await?;
        Ok(AutomationKeyPage {
            records,
            upper: Some(upper),
            has_more,
        })
    }
}
async fn validate_parent(
    connection: &mut SqliteConnection,
    collection: &AutomationCollection,
) -> Result<(), StorageError> {
    let (sql, id, error) = match collection {
        AutomationCollection::Runs(id) => (
            "SELECT COUNT(*) FROM schedule_definitions WHERE schedule_id=?",
            id.as_str(),
            StorageError::ScheduleNotFound,
        ),
        AutomationCollection::Revisions(id) => (
            "SELECT COUNT(*) FROM instruction_documents WHERE instruction_id=?",
            id.as_str(),
            StorageError::InstructionNotFound,
        ),
        AutomationCollection::Deliveries(Some(id)) => (
            "SELECT COUNT(*) FROM wakeup_definitions WHERE wakeup_id=?",
            id.as_str(),
            StorageError::WakeNotFound,
        ),
        _ => return Ok(()),
    };
    let count: i64 = sqlx::query_scalar(sql)
        .bind(id)
        .fetch_one(connection)
        .await?;
    if count == 1 { Ok(()) } else { Err(error) }
}
fn decode_key(
    row: sqlx::sqlite::SqliteRow,
    run_order: bool,
) -> Result<AutomationListKey, StorageError> {
    let resource_id: String = row.try_get("resource_id")?;
    let created_at_ms = if run_order {
        run_timestamp(&resource_id)?
    } else {
        row.try_get("created_at_ms")?
    };
    Ok(AutomationListKey {
        created_at_ms,
        resource_id,
    })
}
fn run_timestamp(value: &str) -> Result<i64, StorageError> {
    // Locally created Runs are never imported: UUIDv7 carries their creation timestamp, not due time.
    let id = agent_automation::RunId::try_from(value.to_owned())
        .map_err(|_| StorageError::InvalidRecord)?;
    let timestamp = uuid::Uuid::parse_str(id.as_str())
        .map_err(|_| StorageError::InvalidRecord)?
        .get_timestamp()
        .ok_or(StorageError::InvalidRecord)?;
    let (seconds, nanos) = timestamp.to_unix();
    let milliseconds = seconds
        .checked_mul(1000)
        .and_then(|seconds| seconds.checked_add(u64::from(nanos / 1_000_000)))
        .ok_or(StorageError::InvalidRecord)?;
    i64::try_from(milliseconds).map_err(|_| StorageError::InvalidRecord)
}
