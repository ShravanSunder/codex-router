//! Install complete immutable Control schema bytes before advertising their digest.
use communication_protocol::ControlSchema;
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub fn publish_control_schema(directory: &Path, schema: &ControlSchema) -> io::Result<PathBuf> {
    let metadata = std::fs::symlink_metadata(directory)?;
    if !directory.is_absolute() || !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(io::Error::other(
            "schema directory must be private and absolute",
        ));
    }
    let digest = String::from(schema.digest().clone());
    let hex = digest
        .strip_prefix("sha256:")
        .ok_or_else(|| io::Error::other("invalid schema digest"))?;
    let path = directory.join(format!("control-schema-{hex}.json"));
    match verify_schema(&path, schema.bytes()) {
        Ok(()) => return Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let suffix = String::from(crate::new_service_uuid()?);
    let temporary = directory.join(format!(".control-schema-{suffix}.tmp"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(schema.bytes())?;
        file.sync_all()?;
        match std::fs::hard_link(&temporary, &path) {
            Ok(()) => File::open(directory)?.sync_all()?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                verify_schema(&path, schema.bytes())?
            }
            Err(error) => return Err(error),
        }
        Ok(path)
    })();
    let cleanup = std::fs::remove_file(temporary);
    let path = result?;
    cleanup?;
    Ok(path)
}
fn verify_schema(path: &Path, expected: &[u8]) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.len() != expected.len() as u64
    {
        return Err(io::Error::other("invalid immutable Control schema file"));
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(metadata.len() + 1)
        .read_to_end(&mut bytes)?;
    if bytes != expected {
        return Err(io::Error::other(
            "immutable Control schema content mismatch",
        ));
    }
    Ok(())
}
