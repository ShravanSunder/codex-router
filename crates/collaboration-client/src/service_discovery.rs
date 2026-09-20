//! Bounded owner-local discovery with manifest-to-connection identity verification.
use crate::{ClientError, ControlClient};
use collaboration_protocol::ServiceManifest;
use std::{
    io::{self, Read},
    path::Path,
    time::Duration,
};
use tokio::net::UnixStream;

impl ControlClient {
    pub async fn connect(directory: &Path, name: &str, version: &str) -> Result<Self, ClientError> {
        if !directory.is_absolute() {
            return Err(ClientError::Protocol("service directory must be absolute"));
        }
        let manifest = read_manifest(directory).map_err(|error| match error {
            ClientError::Transport(source) => ClientError::Discovery {
                stage: "manifest-read",
                source,
            },
            other => other,
        })?;
        let root = std::fs::canonicalize(directory).map_err(|source| ClientError::Discovery {
            stage: "directory-resolve",
            source,
        })?;
        let socket = std::fs::canonicalize(root.join("control.sock")).map_err(|source| {
            ClientError::Discovery {
                stage: "socket-resolve",
                source,
            }
        })?;
        if socket.parent() != Some(root.as_path()) {
            return Err(ClientError::Protocol(
                "Control socket escapes service directory",
            ));
        }
        let stream = tokio::time::timeout(Duration::from_secs(30), UnixStream::connect(socket))
            .await
            .map_err(|_| ClientError::Timeout)?
            .map_err(|source| ClientError::Discovery {
                stage: "socket-connect",
                source,
            })?;
        let client = Self::initialize(stream, name, version).await?;
        let identity = client.identity();
        if identity.service_id != manifest.service_id
            || identity.service_epoch != manifest.service_epoch
            || identity.control_schema_digest != manifest.control_schema_digest
        {
            return Err(ClientError::Protocol(
                "manifest and connected service disagree",
            ));
        }
        Ok(client)
    }
}
fn read_manifest(directory: &Path) -> Result<ServiceManifest, ClientError> {
    let path = directory.join("service.json");
    let metadata = std::fs::symlink_metadata(&path)?;
    if !metadata.is_file() || metadata.len() > 65536 {
        return Err(ClientError::Protocol("invalid service manifest file"));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(65537)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err(ClientError::Protocol("service manifest too large"));
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
        ClientError::Transport(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid service manifest",
        ))
    })?;
    if value.get("version").and_then(serde_json::Value::as_u64) != Some(2) {
        return Err(ClientError::Protocol(
            "unsupported service manifest version; expected version 2",
        ));
    }
    serde_json::from_value(value).map_err(|error| {
        ClientError::Transport(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid service manifest: {error}"),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::ControlClient;

    #[tokio::test]
    async fn client_discovery_reports_manifest_version_skew_before_socket_resolution() {
        let directory = std::env::temp_dir().join(format!(
            "collaboration-client-manifest-skew-{}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).expect("temporary service directory");
        let manifest = serde_json::json!({
            "version":1,
            "serviceId":"00000000-0000-4000-8000-000000000001",
            "serviceEpoch":"00000000-0000-4000-8000-000000000002",
            "control":{"transport":"unixJsonLines","path":"control.sock"},
            "controlSchemaDigest":format!("sha256:{}", "a".repeat(64))
        });
        std::fs::write(
            directory.join("service.json"),
            serde_json::to_vec(&manifest).expect("manifest JSON"),
        )
        .expect("manifest fixture");
        let error = match ControlClient::connect(&directory, "test-client", "1").await {
            Ok(_) => panic!("version one must fail discovery"),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "Control protocol violation: unsupported service manifest version; expected version 2"
        );
        std::fs::remove_file(directory.join("service.json")).expect("remove manifest fixture");
        std::fs::remove_dir(directory).expect("remove fixture directory");
    }
}
