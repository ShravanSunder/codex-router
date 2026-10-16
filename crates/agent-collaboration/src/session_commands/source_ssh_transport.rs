//! Strict, one-shot SSH process transport for the fixed source-side entrypoint.
use super::SessionsCommandError;
use super::router_connection_registry::{
    RouterConnectionProfile, RouterConnectionTransport, SourceSshConnection,
};
use super::source_ssh_protocol::{
    MAX_SOURCE_REPLY_BYTES, MAX_SOURCE_REQUEST_BYTES, PreparedSourceLaunch, SOURCE_REQUEST_SWITCH,
    SourceNativeOptions, SourceOperation, SourceReadError, SourceRejection, SourceReply,
    SourceRequest, SourceResponse, SourceSessionAction,
};
use std::{ffi::OsString, process::Stdio, time::Duration};
use tokio::io::AsyncReadExt;

const SOURCE_READ_TIMEOUT: Duration = Duration::from_secs(30);

fn request_for_profile(
    profile: &RouterConnectionProfile,
    operation: SourceOperation,
) -> Result<SourceRequest, SourceRejection> {
    let RouterConnectionTransport::Ssh(connection) = &profile.transport else {
        return Err(SourceRejection::UnsupportedConnection);
    };
    Ok(SourceRequest {
        service_directory: connection.service_directory.clone(),
        expected_service: profile.service_id.clone(),
        operation,
    })
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn ssh_arguments(
    connection: &SourceSshConnection,
    request: &SourceRequest,
    interactive: bool,
) -> Result<Vec<OsString>, SourceRejection> {
    let json = serde_json::to_string(request).map_err(|_| SourceRejection::InvalidRequest)?;
    if json.len() > MAX_SOURCE_REQUEST_BYTES {
        return Err(SourceRejection::InvalidRequest);
    }
    let command = format!(
        "exec {} {} {}",
        shell_quote(connection.executable.as_str()),
        SOURCE_REQUEST_SWITCH,
        shell_quote(&json)
    );
    // Bound the shell-escaped argv too; quoting can expand an otherwise bounded request.
    if command.len() > 2 * MAX_SOURCE_REQUEST_BYTES {
        return Err(SourceRejection::InvalidRequest);
    }
    Ok([
        if interactive { "-tt" } else { "-T" },
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "ConnectTimeout=5",
        "--",
        connection.destination.as_str(),
        &command,
    ]
    .into_iter()
    .map(OsString::from)
    .collect())
}

pub(crate) async fn read_source(
    profile: &RouterConnectionProfile,
    operation: SourceOperation,
) -> Result<SourceReply, SourceReadError> {
    if matches!(&operation, SourceOperation::Launch { .. }) {
        return Err(SourceReadError::Rejected(SourceRejection::InvalidRequest));
    }
    let request = request_for_profile(profile, operation).map_err(SourceReadError::Rejected)?;
    let RouterConnectionTransport::Ssh(connection) = &profile.transport else {
        return Err(SourceReadError::Rejected(
            SourceRejection::UnsupportedConnection,
        ));
    };
    let arguments =
        ssh_arguments(connection, &request, false).map_err(SourceReadError::Rejected)?;
    let mut child = tokio::process::Command::new("ssh")
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| SourceReadError::ReadUnavailable)?;
    let Some(stdout) = child.stdout.take() else {
        let _settled = child.kill().await;
        return Err(SourceReadError::ReadUnavailable);
    };
    let exchange = async {
        let mut bytes = Vec::new();
        stdout
            .take((MAX_SOURCE_REPLY_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| SourceReadError::ReadUnavailable)?;
        if bytes.len() > MAX_SOURCE_REPLY_BYTES {
            return Err(SourceReadError::InvalidReply);
        }
        let status = child
            .wait()
            .await
            .map_err(|_| SourceReadError::ReadUnavailable)?;
        let response: SourceResponse = serde_json::from_slice(&bytes).map_err(|_| {
            if status.success() {
                SourceReadError::InvalidReply
            } else {
                SourceReadError::ReadUnavailable
            }
        })?;
        match response {
            SourceResponse::Ready { reply } if status.success() => Ok(reply),
            SourceResponse::Rejected { reason } if status.code() == Some(2) => {
                Err(SourceReadError::Rejected(reason))
            }
            _ => Err(SourceReadError::InvalidReply),
        }
    };
    let result = tokio::time::timeout(SOURCE_READ_TIMEOUT, exchange)
        .await
        .unwrap_or(Err(SourceReadError::ReadUnavailable));
    // On every read failure, settle our own process; a read never creates a session.
    if result.is_err() {
        let _settled = child.kill().await;
    }
    result
}

pub(crate) async fn prepare_configured_source(
    profile: &RouterConnectionProfile,
    action: SourceSessionAction,
) -> Result<PreparedSourceLaunch, SourceReadError> {
    action
        .validate(&profile.service_id)
        .map_err(SourceReadError::Rejected)?;
    let RouterConnectionTransport::Ssh(connection) = &profile.transport else {
        return Err(SourceReadError::Rejected(
            SourceRejection::UnsupportedConnection,
        ));
    };
    let reply = read_source(
        profile,
        SourceOperation::Prepare {
            action: action.clone(),
            client_profile: connection.client_profile,
        },
    )
    .await?;
    let SourceReply::Prepared { launch } = reply else {
        return Err(SourceReadError::InvalidReply);
    };
    if launch.action != action
        || launch.client_profile != connection.client_profile
        || launch.endpoint.service_id != profile.service_id
    {
        return Err(SourceReadError::InvalidReply);
    }
    match action.target() {
        Some(target) if target.endpoint != launch.endpoint => {
            return Err(SourceReadError::InvalidReply);
        }
        None if String::from(launch.endpoint.endpoint_id.clone()) != "codex-local" => {
            return Err(SourceReadError::InvalidReply);
        }
        _ => {}
    }
    Ok(*launch)
}

pub(super) fn handoff_prepared_source(
    profile: &RouterConnectionProfile,
    launch: &PreparedSourceLaunch,
    native_arguments: &[OsString],
) -> Result<(), SessionsCommandError> {
    let RouterConnectionTransport::Ssh(connection) = &profile.transport else {
        return Err(SessionsCommandError::SessionSourceUnqualified);
    };
    let options = SourceNativeOptions::parse(native_arguments)
        .map_err(|_| SessionsCommandError::ConfiguredArgumentsUnsupported)?;
    if launch.endpoint.service_id != profile.service_id
        || launch.client_profile != connection.client_profile
        || launch.action.validate(&profile.service_id).is_err()
    {
        return Err(SessionsCommandError::SessionSourceUnqualified);
    }
    let request = request_for_profile(
        profile,
        SourceOperation::Launch {
            prepared: Box::new(launch.clone()),
            options,
        },
    )
    .map_err(|_| SessionsCommandError::SessionSourceUnqualified)?;
    let arguments = ssh_arguments(connection, &request, true)
        .map_err(|_| SessionsCommandError::SessionSourceUnqualified)?;
    let status = std::process::Command::new("ssh")
        .args(arguments)
        .status()
        .map_err(SessionsCommandError::SourceSshSpawn)?;
    if status.success() {
        Ok(())
    } else {
        // SSH start is the handoff boundary, including a possibly hidden source effect.
        Err(SessionsCommandError::SourceHandoffUnknown {
            status: status.to_string(),
        })
    }
}

#[cfg(test)]
#[path = "source_ssh_transport_tests.rs"]
mod tests;
