use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use communication_client::ControlClient;
use communication_protocol::{AutomationPageRequest, OperationId, WakeSendRequest};
use communication_service::{ServiceIdentity, serve_control_connection};
use serde_json::{Value, json};
use std::sync::Arc;
#[tokio::test]
async fn wake_cursor_uses_closed_scoped_base64url_contract()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "wake-cursor-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let store = Arc::new(tokio::sync::Mutex::new(
        automation_storage::AutomationStore::open(&path).await?,
    ));
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .map_err(std::io::Error::other)?
    .with_automation_store(store.clone());
    let (socket, server) = tokio::net::UnixStream::pair()?;
    let task = tokio::spawn(serve_control_connection(server, identity));
    let mut client = ControlClient::initialize(socket, "cursor-test", "1").await?;
    for _ in 0..2 {
        let request: WakeSendRequest = serde_json::from_value(
            json!({"operationId":OperationId::generate(),"message":{"target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"fixture"},"content":{"kind":"humanUser","text":"Check"},"delivery":"auto","generationGuard":null},"timing":{"kind":"after","seconds":60},"expiry":{"kind":"none"}}),
        )?;
        client.send_wakeup(request).await?;
    }
    let first = client
        .list_wakeups(AutomationPageRequest {
            cursor: None,
            limit: 1.try_into()?,
        })
        .await?;
    let cursor = first.next_cursor.ok_or("missing cursor")?;
    let mut payload: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(&cursor)?)?;
    if payload.as_object().map(|object| object.len()) != Some(6)
        || payload
            .get("upperKey")
            .and_then(Value::as_array)
            .map(Vec::len)
            != Some(2)
        || payload
            .get("filterDigest")
            .and_then(Value::as_str)
            .map(str::len)
            != Some(64)
    {
        return Err("cursor did not match specified encoding".into());
    }
    let second = client
        .list_wakeups(AutomationPageRequest {
            cursor: Some(cursor),
            limit: 1.try_into()?,
        })
        .await?;
    if second.records.len() != 1 || second.next_cursor.is_some() {
        return Err("cursor did not continue listing".into());
    }
    payload.as_object_mut().ok_or("cursor not object")?.insert(
        "serviceId".into(),
        json!("00000000-0000-4000-8000-000000000003"),
    );
    let foreign = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload)?);
    if client
        .list_wakeups(AutomationPageRequest {
            cursor: Some(foreign),
            limit: 1.try_into()?,
        })
        .await
        .is_ok()
    {
        return Err("foreign service cursor was accepted".into());
    }
    client.close().await?;
    task.await??;
    drop(store);
    std::fs::remove_file(path)?;
    Ok(())
}
