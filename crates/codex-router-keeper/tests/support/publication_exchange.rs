//! Literal fixture protocol client, outside the production keeper filesystem mechanism.
use codex_router_descriptor_boundary::{DescriptorGate, OwnedSocket, UnixReceipt};
use std::path::Path;
use tokio::time::{Duration, timeout};
pub async fn connect(path: &Path) -> Result<OwnedSocket, Box<dyn std::error::Error + Send + Sync>> {
    Ok(timeout(
        Duration::from_secs(2),
        OwnedSocket::connect_unix(path, DescriptorGate::global()),
    )
    .await??)
}
pub async fn request_reply(
    connection: &OwnedSocket,
) -> Result<[u8; 2], Box<dyn std::error::Error + Send + Sync>> {
    let gate = DescriptorGate::global();
    let reader = UnixReceipt::new(connection.duplicate(gate).await?);
    let writer = connection.duplicate(gate).await?.into_writer();
    timeout(Duration::from_secs(1), writer.write_all(b"PING")).await??;
    let mut reply = [0; 2];
    let mut offset = 0;
    while let Some(remaining) = reply.get_mut(offset..).filter(|part| !part.is_empty()) {
        let count = timeout(Duration::from_secs(1), reader.read(remaining, false, gate))
            .await??
            .bytes;
        if count == 0 {
            return Err("fixture closed before its literal reply".into());
        }
        offset += count;
    }
    Ok(reply)
}
