//! One source-owned Codex child adapted to one already-qualified native connection.
use super::source_ssh_protocol::{
    PreparedSourceLaunch, SourceNativeOptions, SourceRejection, SourceSessionAction,
};
use codex_native_integration::SessionLaunch;
use collaboration_client::{NATIVE_WEBSOCKET_FRAME_LIMIT, NativeTransportConnection};
use futures_util::{SinkExt, StreamExt};
use std::{
    ffi::OsString,
    io,
    os::unix::{fs::PermissionsExt, process::ExitStatusExt},
    path::Path,
    process::{ExitStatus, Stdio},
    time::Duration,
};
use tokio::{
    net::{UnixListener, UnixStream},
    process::Child,
    signal::unix::{SignalKind, signal},
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::WebSocketConfig},
};

pub(super) async fn run_source_client(
    prepared: &PreparedSourceLaunch,
    options: &SourceNativeOptions,
    connection: NativeTransportConnection,
) -> Result<i32, SourceRejection> {
    run_source_client_with_command(prepared, options, connection, |arguments| {
        let mut command = tokio::process::Command::new("codex");
        command.args(arguments);
        command
    })
    .await
}

// Process construction is the external native-client boundary. The supervisor,
// private socket, stream forwarding and cleanup remain identical for every caller.
async fn run_source_client_with_command(
    prepared: &PreparedSourceLaunch,
    options: &SourceNativeOptions,
    connection: NativeTransportConnection,
    command_for_arguments: impl FnOnce(&[OsString]) -> tokio::process::Command,
) -> Result<i32, SourceRejection> {
    options.validate()?;
    let directory = tokio::task::spawn_blocking(|| {
        tempfile::Builder::new()
            .prefix("as-native-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
    })
    .await
    .map_err(|_| SourceRejection::SourceUnavailable)?
    .map_err(|_| SourceRejection::SourceUnavailable)?;
    let socket_path = directory.path().join("native.sock");
    let listener =
        UnixListener::bind(&socket_path).map_err(|_| SourceRejection::SourceUnavailable)?;
    tokio::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
        .await
        .map_err(|_| SourceRejection::SourceUnavailable)?;
    let arguments = native_arguments(prepared, options, &socket_path);
    let mut hangup =
        signal(SignalKind::hangup()).map_err(|_| SourceRejection::SourceUnavailable)?;
    let mut terminate =
        signal(SignalKind::terminate()).map_err(|_| SourceRejection::SourceUnavailable)?;
    let mut interrupt =
        signal(SignalKind::interrupt()).map_err(|_| SourceRejection::SourceUnavailable)?;
    let mut child = command_for_arguments(&arguments)
        .current_dir(prepared.working_directory.as_str())
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| SourceRejection::SourceUnavailable)?;
    let mut forwarding = Box::pin(async move {
        let (frontend, _) = tokio::time::timeout(Duration::from_secs(30), listener.accept())
            .await
            .map_err(|_| io::Error::other("source frontend did not connect"))??;
        // This invocation has one frontend. Never accept/reconnect another client.
        drop(listener);
        let frontend = tokio::time::timeout(
            Duration::from_secs(30),
            tokio_tungstenite::accept_async_with_config(
                frontend,
                Some(
                    WebSocketConfig::default()
                        .read_buffer_size(8192)
                        .max_message_size(Some(NATIVE_WEBSOCKET_FRAME_LIMIT))
                        .max_frame_size(Some(NATIVE_WEBSOCKET_FRAME_LIMIT)),
                ),
            ),
        )
        .await
        .map_err(|_| io::Error::other("source frontend upgrade timed out"))?
        .map_err(|_| io::Error::other("source frontend upgrade failed"))?;
        forward_native_streams(frontend, connection.stream).await
    });
    let result = tokio::select! {
        status = child.wait() => status.map(exit_code).map_err(|_| SourceRejection::SourceUnavailable),
        result = &mut forwarding => {
            // A normal TUI close precedes process teardown. Let that actual exit
            // settle before forced cleanup; do not sleep or reopen its carrier.
            match settle_child(&mut child).await {
                Some(status) if result.is_ok() => Ok(exit_code(status)),
                _ => Err(SourceRejection::SourceUnavailable),
            }
        },
        _ = hangup.recv() => { stop_child(&mut child).await; Ok(129) },
        _ = terminate.recv() => { stop_child(&mut child).await; Ok(143) },
        _ = interrupt.recv() => { stop_child(&mut child).await; Ok(130) },
    };
    // Drop forwarding (both sockets) before the task-owned directory is removed.
    drop(forwarding);
    drop(directory);
    result
}

#[cfg(test)]
#[path = "source_native_carrier_tests.rs"]
mod tests;

fn native_arguments(
    prepared: &PreparedSourceLaunch,
    options: &SourceNativeOptions,
    frontend: &Path,
) -> Vec<OsString> {
    let mut arguments = Vec::new();
    if let Some(model) = &options.model {
        arguments.push(OsString::from(format!("--model={model}")));
    }
    if let Some(effort) = &options.reasoning_effort {
        arguments.extend([
            OsString::from("-c"),
            OsString::from(format!("model_reasoning_effort=\"{effort}\"")),
        ]);
    }
    if options.no_alt_screen {
        arguments.push(OsString::from("--no-alt-screen"));
    }
    let cwd = Path::new(prepared.working_directory.as_str());
    let launch = match &prepared.action {
        SourceSessionAction::New { .. } => SessionLaunch::new(frontend, cwd, &arguments),
        SourceSessionAction::Resume {
            target,
            stored_model,
        } => SessionLaunch::resume(
            frontend,
            cwd,
            &arguments,
            &String::from(target.session_id.clone()),
            &stored_model.native_choice(),
        ),
        SourceSessionAction::Fork {
            target,
            stored_model,
        } => SessionLaunch::fork(
            frontend,
            cwd,
            &arguments,
            &String::from(target.session_id.clone()),
            &stored_model.native_choice(),
        ),
    };
    launch
        .with_profile(prepared.client_profile.native_profile())
        .arguments()
}

fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1))
}

async fn settle_child(child: &mut Child) -> Option<ExitStatus> {
    match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
        Ok(Ok(status)) => Some(status),
        _ => {
            stop_child(child).await;
            None
        }
    }
}

async fn stop_child(child: &mut Child) {
    // Child owns this process handle; never signal a discovered or replacement PID.
    let _settled = child.kill().await;
}

async fn forward_native_streams(
    frontend: WebSocketStream<UnixStream>,
    backend: WebSocketStream<UnixStream>,
) -> io::Result<()> {
    let (mut frontend_write, mut frontend_read) = frontend.split();
    let (mut backend_write, mut backend_read) = backend.split();
    let outbound = async {
        while let Some(frame) = frontend_read.next().await {
            let message = frame.map_err(|_| io::Error::other("source frontend read failed"))?;
            if message.len() > NATIVE_WEBSOCKET_FRAME_LIMIT {
                return Err(io::Error::other("source native frame exceeded limit"));
            }
            let closed = matches!(message, Message::Close(_));
            backend_write
                .send(message)
                .await
                .map_err(|_| io::Error::other("source backend write failed"))?;
            if closed {
                break;
            }
        }
        Ok(())
    };
    let inbound = async {
        while let Some(frame) = backend_read.next().await {
            let message = frame.map_err(|_| io::Error::other("source backend read failed"))?;
            if message.len() > NATIVE_WEBSOCKET_FRAME_LIMIT {
                return Err(io::Error::other("source native frame exceeded limit"));
            }
            let closed = matches!(message, Message::Close(_));
            frontend_write
                .send(message)
                .await
                .map_err(|_| io::Error::other("source frontend write failed"))?;
            if closed {
                break;
            }
        }
        Ok(())
    };
    // Cancelling the losing future drops its one pending message and both halves.
    tokio::select! { result = outbound => result, result = inbound => result }
}
