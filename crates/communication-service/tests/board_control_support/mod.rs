use communication_service::{ServiceIdentity, serve_control_connection};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub async fn send_raw_board_requests(
    identity: ServiceIdentity,
    requests: Vec<Value>,
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let (client, server) = tokio::net::UnixStream::pair()?;
    let task = tokio::spawn(serve_control_connection(server, identity));
    let (reader, mut writer) = client.into_split();
    let mut lines = BufReader::new(reader).lines();
    let initialize = json!({
        "jsonrpc":"2.0","id":"initialize","method":"control/initialize",
        "params":{"version":{"major":1,"minor":0},"client":{"name":"board-validation","version":"1"}}
    });
    writer
        .write_all(format!("{initialize}\n").as_bytes())
        .await?;
    let initialization: Value = serde_json::from_str(
        &lines
            .next_line()
            .await?
            .ok_or("initialization response missing")?,
    )?;
    if initialization.get("result").is_none() {
        return Err("Control initialization failed".into());
    }
    let mut responses = Vec::with_capacity(requests.len());
    for request in requests {
        writer.write_all(format!("{request}\n").as_bytes()).await?;
        let response = lines.next_line().await?.ok_or("board response missing")?;
        responses.push(serde_json::from_str(&response)?);
    }
    writer.shutdown().await?;
    task.await??;
    Ok(responses)
}
