//! Atomic creation of stable logical Host identity; corruption never creates a replacement.
use communication_protocol::UuidIdentity;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IdentityRecord {
    version: u8,
    service_id: UuidIdentity,
}

/// Generates an independent epoch/identity from OS randomness.
pub fn new_service_uuid() -> io::Result<UuidIdentity> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| io::Error::other("identity randomness unavailable"))?;
    if let Some(value) = bytes.get_mut(6) {
        *value = (*value & 0x0f) | 0x40;
    }
    if let Some(value) = bytes.get_mut(8) {
        *value = (*value & 0x3f) | 0x80;
    }
    let mut encoded = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            encoded.push('-');
        }
        use std::fmt::Write as _;
        write!(encoded, "{byte:02x}").map_err(|_| io::Error::other("identity encoding"))?;
    }
    UuidIdentity::try_from(encoded).map_err(io::Error::other)
}

pub fn load_service_identity(directory: &Path) -> io::Result<UuidIdentity> {
    let metadata = std::fs::symlink_metadata(directory)?;
    if !directory.is_absolute() || !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(io::Error::other(
            "identity directory must be private and absolute",
        ));
    }
    let path = directory.join("service-identity.json");
    match read_identity(&path) {
        Ok(identity) => return Ok(identity),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let identity = new_service_uuid()?;
    let suffix: String = new_service_uuid()?.into();
    let temporary = directory.join(format!(".service-identity-{suffix}.tmp"));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        let bytes = serde_json::to_vec(&IdentityRecord {
            version: 1,
            service_id: identity.clone(),
        })
        .map_err(io::Error::other)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        // Link installs the complete file without ever replacing a concurrent creator.
        match std::fs::hard_link(&temporary, &path) {
            Ok(()) => {
                File::open(directory)?.sync_all()?;
                Ok(identity)
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => read_identity(&path),
            Err(error) => Err(error),
        }
    })();
    let cleanup = std::fs::remove_file(&temporary);
    match result {
        Ok(identity) => {
            cleanup?;
            Ok(identity)
        }
        Err(error) => Err(error),
    }
}
fn read_identity(path: &Path) -> io::Result<UuidIdentity> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 || metadata.len() > 1024 {
        return Err(io::Error::other("invalid identity file"));
    }
    let mut bytes = Vec::new();
    File::open(path)?.take(1025).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 {
        return Err(io::Error::other("identity file too large"));
    }
    let record: IdentityRecord =
        serde_json::from_slice(&bytes).map_err(|_| io::Error::other("invalid identity content"))?;
    if record.version != 1 {
        return Err(io::Error::other("unsupported identity version"));
    }
    Ok(record.service_id)
}
