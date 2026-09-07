//! Immutable schema bytes are installed before their content identity is advertised.
use crate::{NativeSchemaBundle, NativeSchemaError};
use std::path::{Path, PathBuf};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

impl NativeSchemaBundle {
    pub fn publish(&self, directory: &Path) -> Result<PathBuf, NativeSchemaError> {
        let metadata =
            std::fs::symlink_metadata(directory).map_err(NativeSchemaError::Filesystem)?;
        if !directory.is_absolute()
            || !metadata.is_dir()
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(NativeSchemaError::InvalidDocuments);
        }
        let mut filename = String::new();
        for byte in self.digest() {
            use std::fmt::Write as _;
            write!(filename, "{byte:02x}").map_err(|_| NativeSchemaError::InvalidDocuments)?;
        }
        let destination = directory.join(format!("{filename}.json"));
        match verify_existing(&destination, self.canonical_bytes()) {
            Ok(()) => return Ok(destination),
            Err(NativeSchemaError::Filesystem(error))
                if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let temporary = directory.join(format!(
            ".schema-bundle-{}-{}.tmp",
            std::process::id(),
            NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(NativeSchemaError::Filesystem)?;
        let result = (|| {
            file.write_all(self.canonical_bytes())
                .map_err(NativeSchemaError::Filesystem)?;
            file.sync_all().map_err(NativeSchemaError::Filesystem)?;
            match std::fs::hard_link(&temporary, &destination) {
                Ok(()) => File::open(directory)
                    .and_then(|directory| directory.sync_all())
                    .map_err(NativeSchemaError::Filesystem)?,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    verify_existing(&destination, self.canonical_bytes())?;
                }
                Err(error) => return Err(NativeSchemaError::Filesystem(error)),
            }
            Ok(destination)
        })();
        let cleanup = std::fs::remove_file(temporary).map_err(NativeSchemaError::Filesystem);
        let published = result?;
        cleanup?;
        Ok(published)
    }
}

fn verify_existing(path: &Path, expected: &[u8]) -> Result<(), NativeSchemaError> {
    let metadata = std::fs::symlink_metadata(path).map_err(NativeSchemaError::Filesystem)?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.len()
            != u64::try_from(expected.len()).map_err(|_| NativeSchemaError::Capacity)?
    {
        return Err(NativeSchemaError::InvalidDocuments);
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(NativeSchemaError::Filesystem)?
        .take(metadata.len() + 1)
        .read_to_end(&mut bytes)
        .map_err(NativeSchemaError::Filesystem)?;
    if bytes != expected {
        return Err(NativeSchemaError::InvalidDocuments);
    }
    Ok(())
}
