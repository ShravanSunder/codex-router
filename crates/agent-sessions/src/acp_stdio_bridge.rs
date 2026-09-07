//! ACP JSONL carrier bridge; protocol negotiation and permission decisions belong to its caller.
use clap::Parser;
use communication_client::{AcpTransportConnection, ClientError};
use std::{ffi::OsString, io, path::PathBuf};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

const FRAME_LIMIT: usize = 64 * 1024 * 1024;
#[derive(Parser)]
#[command(name = "agent-sessions acp", bin_name = "agent-sessions acp")]
struct AcpBridgeArguments {
    /// Select the endpoint's ACP carrier; this bridge does not initialize or approve requests.
    #[arg(long)]
    endpoint: String,
    #[arg(long)]
    service_directory: Option<PathBuf>,
}
pub fn run_acp_command(arguments: Vec<OsString>) -> i32 {
    let args = match AcpBridgeArguments::try_parse_from(arguments) {
        Ok(args) => args,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _printed = error.print();
            return code;
        }
    };
    let directory = match crate::endpoint_commands::resolve_directory(args.service_directory) {
        Ok(directory) => directory,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    let endpoint = match args.endpoint.try_into() {
        Ok(endpoint) => endpoint,
        Err(_) => {
            eprintln!("invalid ACP endpoint ID");
            return 2;
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            eprintln!("ACP client runtime unavailable");
            return 3;
        }
    };
    let code = runtime.block_on(async {
        let connection = match AcpTransportConnection::connect(&directory, endpoint).await {
            Ok(connection) => connection,
            Err(ClientError::UnsupportedCapability(_)) => {
                eprintln!("ACP channel unsupported");
                return 2;
            }
            Err(_) => {
                eprintln!("ACP service connection unavailable");
                return 3;
            }
        };
        let (reader, mut writer) = connection.stream.into_split();
        let mut input = BufReader::new(tokio::io::stdin());
        let mut reader = BufReader::new(reader);
        let mut output = tokio::io::stdout();
        let result = tokio::select! {
            result = forward_json_lines(&mut input, &mut writer) => result,
            result = forward_json_lines(&mut reader, &mut output) => result,
            _ = tokio::signal::ctrl_c() => return 130,
        };
        match result {
            Ok(()) => 0,
            Err(_) => {
                eprintln!("ACP carrier closed with an incomplete or failed exchange; no replay");
                3
            }
        }
    });
    // Backend EOF must terminate even when Tokio's blocking stdin read is still pending.
    runtime.shutdown_background();
    code
}
async fn forward_json_lines(
    reader: &mut (impl AsyncBufRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
) -> io::Result<()> {
    let mut frame = Vec::new();
    loop {
        let buffer = reader.fill_buf().await?;
        if buffer.is_empty() {
            return if frame.is_empty() {
                Ok(())
            } else {
                Err(io::Error::other("truncated ACP frame"))
            };
        }
        let end = buffer.iter().position(|byte| *byte == b'\n');
        let payload_count = end.unwrap_or(buffer.len());
        if payload_count > FRAME_LIMIT.saturating_sub(frame.len()) {
            return Err(io::Error::other("ACP frame exceeds carrier limit"));
        }
        frame.extend_from_slice(
            buffer
                .get(..payload_count)
                .ok_or_else(|| io::Error::other("ACP frame boundary"))?,
        );
        reader.consume(payload_count + usize::from(end.is_some()));
        if end.is_some() {
            // The bridge never parses/re-serializes IDs or substitutes protocol messages.
            frame.push(b'\n');
            writer.write_all(&frame).await?;
            writer.flush().await?;
            frame.clear();
        }
    }
}
