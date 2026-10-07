use codex_native_integration::{NativeSchemaDigest, RecordedExecutableIdentity};
use serde::{Deserialize, Serialize};
use std::{
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum GenerationEvidenceError {
    #[error("schema bundle directory path must be absolute and structurally valid")]
    InvalidSchemaBundleDirectoryPath,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "availability", rename_all = "camelCase", deny_unknown_fields)]
pub enum GenerationSchemaAvailability {
    Ready {
        schema_digest: NativeSchemaDigest,
        schema_bundle_dir: PathBuf,
    },
    Unavailable {
        reason: SchemaUnavailableReason,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SchemaUnavailableReason {
    ExportFailed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "GenerationEvidenceWire", into = "GenerationEvidenceWire")]
pub struct GenerationEvidence {
    executable: RecordedExecutableIdentity,
    schema: GenerationSchemaAvailability,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GenerationEvidenceWire {
    executable: RecordedExecutableIdentity,
    schema: GenerationSchemaAvailability,
}

impl GenerationEvidence {
    /// Composes captured evidence without observing the executable or schema bundle again.
    pub fn new(
        executable: RecordedExecutableIdentity,
        schema: GenerationSchemaAvailability,
    ) -> Result<Self, GenerationEvidenceError> {
        if let GenerationSchemaAvailability::Ready {
            schema_bundle_dir, ..
        } = &schema
            && !is_absolute_named_directory_path(schema_bundle_dir)
        {
            return Err(GenerationEvidenceError::InvalidSchemaBundleDirectoryPath);
        }
        Ok(Self { executable, schema })
    }

    pub fn executable(&self) -> &RecordedExecutableIdentity {
        &self.executable
    }

    pub fn schema(&self) -> &GenerationSchemaAvailability {
        &self.schema
    }
}

impl TryFrom<GenerationEvidenceWire> for GenerationEvidence {
    type Error = GenerationEvidenceError;

    fn try_from(value: GenerationEvidenceWire) -> Result<Self, Self::Error> {
        Self::new(value.executable, value.schema)
    }
}

impl From<GenerationEvidence> for GenerationEvidenceWire {
    fn from(value: GenerationEvidence) -> Self {
        Self {
            executable: value.executable,
            schema: value.schema,
        }
    }
}

fn is_absolute_named_directory_path(path: &Path) -> bool {
    let bytes = path.as_os_str().as_bytes();
    path.is_absolute()
        && path.file_name().is_some()
        && !bytes.contains(&0)
        && !bytes
            .split(|byte| *byte == b'/')
            .skip(1)
            .any(|component| component.is_empty() || component == b"." || component == b"..")
}
