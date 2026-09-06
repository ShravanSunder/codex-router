//! Bounded filesystem collection of a completed private native schema export.
use crate::{NativeSchemaBundle, NativeSchemaError};
use std::{collections::BTreeMap, io::Read, path::Path};

const MAX_DOCUMENTS: usize = 4096;
const MAX_EXPORT_BYTES: usize = 64 * 1024 * 1024;

impl NativeSchemaBundle {
    /// Collects a completed export. The export process must have exited successfully first.
    /// All paths remain below the owner-controlled root; symlinks are never admitted.
    pub fn from_export_directory(root: &Path) -> Result<Self, NativeSchemaError> {
        let metadata = std::fs::symlink_metadata(root).map_err(NativeSchemaError::Filesystem)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(NativeSchemaError::InvalidDocuments);
        }
        let mut pending = vec![root.to_path_buf()];
        let mut documents = BTreeMap::new();
        let mut total_bytes = 0;
        let mut entries = 0;
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(directory).map_err(NativeSchemaError::Filesystem)? {
                let entry = entry.map_err(NativeSchemaError::Filesystem)?;
                entries += 1;
                if entries > MAX_DOCUMENTS {
                    return Err(NativeSchemaError::Capacity);
                }
                let metadata = entry.metadata().map_err(NativeSchemaError::Filesystem)?;
                if metadata.file_type().is_symlink() {
                    return Err(NativeSchemaError::InvalidDocuments);
                }
                if metadata.is_dir() {
                    pending.push(entry.path());
                    continue;
                }
                if !metadata.is_file() {
                    return Err(NativeSchemaError::InvalidDocuments);
                }
                let path = entry.path();
                let name = path
                    .strip_prefix(root)
                    .ok()
                    .and_then(Path::to_str)
                    .ok_or(NativeSchemaError::InvalidDocuments)?
                    .to_owned();
                if !name.ends_with(".json") {
                    return Err(NativeSchemaError::InvalidDocuments);
                }
                let mut bytes = Vec::new();
                let remaining = MAX_EXPORT_BYTES - total_bytes;
                std::fs::File::open(path)
                    .map_err(NativeSchemaError::Filesystem)?
                    .take(u64::try_from(remaining + 1).map_err(|_| NativeSchemaError::Capacity)?)
                    .read_to_end(&mut bytes)
                    .map_err(NativeSchemaError::Filesystem)?;
                if bytes.len() > remaining {
                    return Err(NativeSchemaError::Capacity);
                }
                total_bytes += bytes.len();
                if documents.insert(name, bytes).is_some() {
                    return Err(NativeSchemaError::InvalidDocuments);
                }
            }
        }
        Self::from_documents(documents)
    }
}
