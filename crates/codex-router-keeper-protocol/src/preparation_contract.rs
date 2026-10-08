use serde::{Deserialize, Serialize, ser::Error as _};
use std::num::NonZeroI64;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum MigrationVersionError {
    #[error("migration version must be positive")]
    NotPositive,
    #[error("migration version exceeds i64")]
    Overflow,
}

/// An image-local migration number; membership and ordering belong to the store owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct MigrationVersion(NonZeroI64);

impl MigrationVersion {
    pub fn get(self) -> i64 {
        self.0.get()
    }
}

impl TryFrom<i128> for MigrationVersion {
    type Error = MigrationVersionError;

    fn try_from(value: i128) -> Result<Self, Self::Error> {
        let value = i64::try_from(value).map_err(|_| MigrationVersionError::Overflow)?;
        if value <= 0 {
            return Err(MigrationVersionError::NotPositive);
        }
        NonZeroI64::new(value)
            .map(Self)
            .ok_or(MigrationVersionError::NotPositive)
    }
}

impl TryFrom<i64> for MigrationVersion {
    type Error = MigrationVersionError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::try_from(i128::from(value))
    }
}

impl From<MigrationVersion> for i64 {
    fn from(value: MigrationVersion) -> Self {
        value.get()
    }
}

impl From<MigrationVersion> for i128 {
    fn from(value: MigrationVersion) -> Self {
        i128::from(value.get())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(try_from = "PreparedStoreSchemaWire")]
pub enum PreparedStoreSchema {
    Current,
    Pending { migrations: Vec<MigrationVersion> },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PreparedStoreSchemaError {
    #[error("pending prepared schema requires at least one migration")]
    EmptyPendingMigrations,
}

#[derive(Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum PreparedStoreSchemaWire {
    Current {},
    Pending { migrations: Vec<MigrationVersion> },
}

impl PreparedStoreSchema {
    pub fn pending(migrations: Vec<MigrationVersion>) -> Result<Self, PreparedStoreSchemaError> {
        let schema = Self::Pending { migrations };
        schema.validate()?;
        Ok(schema)
    }

    fn validate(&self) -> Result<(), PreparedStoreSchemaError> {
        if matches!(self, Self::Pending { migrations } if migrations.is_empty()) {
            return Err(PreparedStoreSchemaError::EmptyPendingMigrations);
        }
        Ok(())
    }
}

impl TryFrom<PreparedStoreSchemaWire> for PreparedStoreSchema {
    type Error = PreparedStoreSchemaError;

    fn try_from(wire: PreparedStoreSchemaWire) -> Result<Self, Self::Error> {
        match wire {
            PreparedStoreSchemaWire::Current {} => Ok(Self::Current),
            PreparedStoreSchemaWire::Pending { migrations } => Self::pending(migrations),
        }
    }
}

impl From<PreparedStoreSchema> for PreparedStoreSchemaWire {
    fn from(value: PreparedStoreSchema) -> Self {
        match value {
            PreparedStoreSchema::Current => Self::Current {},
            PreparedStoreSchema::Pending { migrations } => Self::Pending { migrations },
        }
    }
}

impl Serialize for PreparedStoreSchema {
    fn serialize<TSerializer>(
        &self,
        serializer: TSerializer,
    ) -> Result<TSerializer::Ok, TSerializer::Error>
    where
        TSerializer: serde::Serializer,
    {
        self.validate().map_err(TSerializer::Error::custom)?;
        PreparedStoreSchemaWire::from(self.clone()).serialize(serializer)
    }
}
