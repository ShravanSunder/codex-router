//! Native JSONL/WebSocket carrier bridge; no protocol initialization or replay is invented.
use clap::Parser;
use collaboration_client::{
    NATIVE_WEBSOCKET_FRAME_LIMIT, NativeTransportConnection, NativeTransportError,
    protocol::EndpointId,
};
use futures_util::{SinkExt, StreamExt};
use std::{ffi::OsString, io, path::PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

const FRAME_LIMIT: usize = NATIVE_WEBSOCKET_FRAME_LIMIT;
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
        Err(NativeBridgeError::Transport(error)) => match error.permission_diagnostic() {
            Some(diagnostic) => crate::permission_diagnostic_reporting::report_diagnostic(
                diagnostic,
                crate::permission_diagnostic_reporting::PermissionDiagnosticRendering::Command,
                false,
            ),
            None => {
                eprintln!("{error}");
                3
            }
        },
        Err(error) => {
            eprintln!("{error}");
            3
        }
    }
}
#[derive(Debug, thiserror::Error)]
enum NativeBridgeError {
    #[error(transparent)]
    Transport(#[from] NativeTransportError),
    #[error(transparent)]
    Io(#[from] io::Error),
}

async fn bridge(directory: PathBuf, endpoint: EndpointId) -> Result<(), NativeBridgeError> {
    let connection = NativeTransportConnection::connect(&directory, endpoint).await?;
    let socket = connection.stream;
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
    tokio::select! {result=input=>result.map_err(Into::into),result=output=>result.map_err(Into::into)}
}
