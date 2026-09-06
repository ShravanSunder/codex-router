//! Native JSONL/WebSocket carrier bridge; no protocol initialization or replay is invented.
use clap::Parser;
use communication_client::ControlClient;
use communication_protocol::{ChannelDescription, EndpointAvailability, EndpointId};
use futures_util::{SinkExt, StreamExt};
use std::{ffi::OsString, io, path::PathBuf, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::{Message, protocol::WebSocketConfig};

const FRAME_LIMIT: usize = 64 * 1024 * 1024;
#[derive(Parser)]
#[command(name = "native")]
struct NativeArguments {
    #[arg(long)]
    endpoint: String,
    #[arg(long)]
    service_directory: Option<PathBuf>,
}
pub fn run_native_command(arguments: Vec<OsString>) -> i32 {
    let args = match NativeArguments::try_parse_from(arguments) {
        Ok(value) => value,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _printed = error.print();
            return code;
        }
    };
    let directory = match crate::endpoint_commands::resolve_directory(args.service_directory) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    let endpoint = match EndpointId::try_from(args.endpoint) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(value) => value,
        Err(_) => {
            eprintln!("client runtime unavailable");
            return 3;
        }
    };
    let result = runtime.block_on(bridge(directory, endpoint));
    // Tokio stdin may retain a blocking read after backend EOF; do not hang process exit.
    runtime.shutdown_background();
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            3
        }
    }
}
async fn bridge(directory: PathBuf, endpoint: EndpointId) -> io::Result<()> {
    let mut control = ControlClient::connect(
        &directory,
        "agent-sessions-native",
        env!("CARGO_PKG_VERSION"),
    )
    .await
    .map_err(|_| io::Error::other("service unavailable"))?;
    let inventory = control
        .list_endpoints()
        .await
        .map_err(|_| io::Error::other("endpoint discovery unavailable"))?;
    let description = inventory
        .endpoints
        .into_iter()
        .find(|item| {
            item.endpoint.endpoint_id == endpoint
                && item.endpoint.service_id == control.identity().service_id
        })
        .ok_or_else(|| io::Error::other("endpoint not found"))?;
    if !matches!(
        description.availability,
        EndpointAvailability::Available { .. }
    ) {
        return Err(io::Error::other("native endpoint unavailable"));
    }
    let path = description
        .channels
        .into_iter()
        .find_map(|channel| match channel {
            ChannelDescription::NativeCodex { path, .. } => Some(String::from(path)),
            _ => None,
        })
        .ok_or_else(|| io::Error::other("native channel unsupported"))?;
    let relative = std::path::Path::new(&path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(io::Error::other("invalid native channel path"));
    }
    let root = std::fs::canonicalize(directory)?;
    let path = std::fs::canonicalize(root.join(relative))?;
    if !path.starts_with(&root) {
        return Err(io::Error::other("native channel escapes service directory"));
    }
    let establish = async {
        let stream = tokio::net::UnixStream::connect(path).await?;
        tokio_tungstenite::client_async_with_config(
            "ws://localhost/",
            stream,
            Some(
                WebSocketConfig::default()
                    .max_message_size(Some(FRAME_LIMIT))
                    .max_frame_size(Some(FRAME_LIMIT)),
            ),
        )
        .await
        .map_err(|_| io::Error::other("native channel upgrade failed"))
    };
    let (socket, _) = tokio::time::timeout(Duration::from_secs(30), establish)
        .await
        .map_err(|_| io::Error::other("native connect timed out"))??;
    control
        .close()
        .await
        .map_err(|_| io::Error::other("discovery close failed"))?;
    let (mut writer, mut reader) = socket.split();
    let input = async {
        let mut stdin = tokio::io::BufReader::new(tokio::io::stdin());
        let mut frame = Vec::new();
        loop {
            let buffer = stdin.fill_buf().await?;
            if buffer.is_empty() {
                return if frame.is_empty() {
                    Ok(())
                } else {
                    Err(io::Error::other("truncated native input"))
                };
            }
            let end = buffer.iter().position(|byte| *byte == b'\n');
            let count = end.map_or(buffer.len(), |index| index + 1);
            if count > FRAME_LIMIT.saturating_sub(frame.len()) {
                return Err(io::Error::other("native input too large"));
            }
            frame.extend_from_slice(
                buffer
                    .get(..count)
                    .ok_or_else(|| io::Error::other("input boundary"))?,
            );
            stdin.consume(count);
            if end.is_some() {
                frame.pop();
                let text = String::from_utf8(std::mem::take(&mut frame))
                    .map_err(|_| io::Error::other("invalid native UTF-8"))?;
                // Validate syntax without reserializing values or rounding numeric IDs.
                let _: serde_json::Value = serde_json::from_str(&text)
                    .map_err(|_| io::Error::other("invalid native JSON"))?;
                writer
                    .send(Message::Text(text.into()))
                    .await
                    .map_err(|_| io::Error::other("native write failed; outcome may be unknown"))?;
            }
        }
    };
    let output = async {
        let mut stdout = tokio::io::stdout();
        while let Some(message) = reader.next().await {
            match message
                .map_err(|_| io::Error::other("native read failed; outcomes may be unknown"))?
            {
                Message::Text(text) => {
                    if text.contains('\n') {
                        return Err(io::Error::other(
                            "native frame cannot be represented as one JSON line",
                        ));
                    }
                    stdout.write_all(text.as_bytes()).await?;
                    stdout.write_all(b"\n").await?;
                    stdout.flush().await?;
                }
                Message::Close(_) => return Ok(()),
                Message::Ping(_) | Message::Pong(_) => {}
                _ => {
                    return Err(io::Error::other(
                        "binary native frames require a WebSocket client",
                    ));
                }
            }
        }
        Ok(())
    };
    tokio::select! {result=input=>result,result=output=>result}
}
