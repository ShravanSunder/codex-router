//! Source-local API validation and fixed request execution; SSH never selects a local fallback.
use super::router_connection_registry::{AbsoluteRemoteCwd, SourceClientProfile};
use super::source_ssh_protocol::{
    MAX_SOURCE_REPLY_BYTES, MAX_SOURCE_REQUEST_BYTES, PreparedSourceLaunch, SourceOperation,
    SourceRejection, SourceReply, SourceRequest, SourceResponse, SourceSessionAction,
};
use crate::CliContext;
use collaboration_client::protocol::{
    ChannelDescription, EndpointAvailability, EndpointRef, UuidIdentity,
};
use collaboration_client::{CollaborationClient, NativeTransportConnection};
use std::{
    ffi::OsString,
    io::{IsTerminal, Write},
    path::Path,
    time::Duration,
};
use tokio::io::AsyncReadExt;

#[derive(Debug, thiserror::Error)]
pub(crate) enum SourceEntryFailure {
    #[error("{0}")]
    Rejected(SourceRejection),
    #[error("source native client exited with code {0}; session effect may be unknown")]
    NativeExit(i32),
}

impl SourceEntryFailure {
    pub(crate) fn exit_code(&self) -> i32 {
        match self {
            Self::Rejected(_) => 2,
            Self::NativeExit(code) => *code,
        }
    }
}

pub(crate) fn run_source_entry(
    arguments: &[OsString],
    context: &CliContext,
) -> Result<(), SourceEntryFailure> {
    let request = arguments
        .get(1)
        .filter(|_| arguments.len() == 2)
        .and_then(|value| value.to_str())
        .filter(|text| text.len() <= MAX_SOURCE_REQUEST_BYTES)
        .and_then(|text| serde_json::from_str::<SourceRequest>(text).ok());
    let Some(request) = request else {
        write_response(Err(SourceRejection::InvalidRequest))?;
        return Err(SourceEntryFailure::Rejected(
            SourceRejection::InvalidRequest,
        ));
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| SourceEntryFailure::Rejected(SourceRejection::SourceUnavailable))?;
    if let SourceOperation::Launch { prepared, options } = &request.operation {
        if !std::io::stdin().is_terminal() {
            return Err(SourceEntryFailure::Rejected(
                SourceRejection::InvalidRequest,
            ));
        }
        let status = runtime
            .block_on(async {
                options.validate()?;
                let connection = establish_prepared_connection(&request, prepared, context).await?;
                super::source_native_carrier::run_source_client(prepared, options, connection).await
            })
            .map_err(SourceEntryFailure::Rejected)?;
        if status == 0 {
            Ok(())
        } else {
            Err(SourceEntryFailure::NativeExit(status))
        }
    } else {
        let result = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(30), execute_read(&request, context))
                .await
                .unwrap_or(Err(SourceRejection::SourceUnavailable))
        });
        let rejection = result.as_ref().err().copied();
        write_response(result)?;
        match rejection {
            Some(reason) => Err(SourceEntryFailure::Rejected(reason)),
            None => Ok(()),
        }
    }
}

fn write_response(result: Result<SourceReply, SourceRejection>) -> Result<(), SourceEntryFailure> {
    let response = match result {
        Ok(reply) => SourceResponse::Ready { reply },
        Err(reason) => SourceResponse::Rejected { reason },
    };
    let mut bytes = serde_json::to_vec(&response)
        .map_err(|_| SourceEntryFailure::Rejected(SourceRejection::SourceUnavailable))?;
    if bytes.len() >= MAX_SOURCE_REPLY_BYTES {
        let rejection = SourceResponse::Rejected {
            reason: SourceRejection::SourceUnavailable,
        };
        let mut rejected = serde_json::to_vec(&rejection)
            .map_err(|_| SourceEntryFailure::Rejected(SourceRejection::SourceUnavailable))?;
        rejected.push(b'\n');
        std::io::stdout()
            .write_all(&rejected)
            .map_err(|_| SourceEntryFailure::Rejected(SourceRejection::SourceUnavailable))?;
        return Err(SourceEntryFailure::Rejected(
            SourceRejection::SourceUnavailable,
        ));
    }
    bytes.push(b'\n');
    std::io::stdout()
        .write_all(&bytes)
        .map_err(|_| SourceEntryFailure::Rejected(SourceRejection::SourceUnavailable))
}

async fn connect_expected(request: &SourceRequest) -> Result<CollaborationClient, SourceRejection> {
    let client = CollaborationClient::connect(
        Path::new(request.service_directory.as_str()),
        "agent-sessions-source",
        env!("CARGO_PKG_VERSION"),
    )
    .await
    .map_err(|_| SourceRejection::SourceUnavailable)?;
    if client.identity().service_id != request.expected_service {
        return Err(SourceRejection::WrongService);
    }
    Ok(client)
}

async fn execute_read(
    request: &SourceRequest,
    context: &CliContext,
) -> Result<SourceReply, SourceRejection> {
    let client = connect_expected(request).await?;
    match &request.operation {
        SourceOperation::Endpoints => Ok(SourceReply::Endpoints {
            inventory: client
                .list_endpoints()
                .await
                .map_err(|_| SourceRejection::SourceUnavailable)?,
        }),
        SourceOperation::NativeSessions { params } => {
            verify_service(&params.endpoint, &request.expected_service)?;
            Ok(SourceReply::NativeSessions {
                page: client
                    .list_sessions(params.clone())
                    .await
                    .map_err(|_| SourceRejection::SourceUnavailable)?,
            })
        }
        SourceOperation::ProviderSessions { params } => {
            verify_service(&params.endpoint, &request.expected_service)?;
            Ok(SourceReply::ProviderSessions {
                page: client
                    .list_provider_sessions(params.clone())
                    .await
                    .map_err(|_| SourceRejection::SourceUnavailable)?,
            })
        }
        SourceOperation::Prepare {
            action,
            client_profile,
        } => {
            let prepared = prepare_source(
                &client,
                action,
                *client_profile,
                &request.expected_service,
                context,
            )
            .await?;
            Ok(SourceReply::Prepared {
                launch: Box::new(prepared),
            })
        }
        SourceOperation::Launch { .. } => Err(SourceRejection::InvalidRequest),
    }
}

fn verify_service(endpoint: &EndpointRef, service: &UuidIdentity) -> Result<(), SourceRejection> {
    if endpoint.service_id == *service {
        Ok(())
    } else {
        Err(SourceRejection::WrongService)
    }
}

async fn prepare_source(
    client: &CollaborationClient,
    action: &SourceSessionAction,
    profile: SourceClientProfile,
    service: &UuidIdentity,
    context: &CliContext,
) -> Result<PreparedSourceLaunch, SourceRejection> {
    action.validate(service)?;
    let inventory = client
        .list_endpoints()
        .await
        .map_err(|_| SourceRejection::SourceUnavailable)?;
    crate::presentation::session_picker::SourceInventoryBinding::from_inventory(
        service, &inventory,
    )
    .map_err(|_| SourceRejection::InvalidNativeChannel)?;
    let endpoint = inventory
        .endpoints
        .iter()
        .find(|description| {
            action.target().map_or_else(
                || String::from(description.endpoint.endpoint_id.clone()) == "codex-local",
                |target| target.endpoint == description.endpoint,
            )
        })
        .ok_or(SourceRejection::UnavailableEndpoint)?;
    if !matches!(
        endpoint.availability,
        EndpointAvailability::Available { .. }
    ) {
        return Err(SourceRejection::UnavailableEndpoint);
    }
    let mut channels = endpoint
        .channels
        .iter()
        .filter_map(|channel| match channel {
            ChannelDescription::NativeCodex {
                path,
                generation: Some(generation),
                schema_digest: Some(schema),
                ..
            } => Some((path, generation, schema)),
            _ => None,
        });
    let (path, generation, schema) = channels
        .next()
        .ok_or(SourceRejection::InvalidNativeChannel)?;
    if channels.next().is_some()
        || generation.service_epoch != inventory.service_epoch
        || client.identity().native_schema_digest.as_ref() != Some(schema)
    {
        return Err(SourceRejection::StaleBinding);
    }
    validate_native_path(client.directory(), &String::from(path.clone())).await?;
    let directory = match action {
        SourceSessionAction::New { directory } => directory.clone(),
        SourceSessionAction::Resume { target, .. } | SourceSessionAction::Fork { target, .. } => {
            let inspection = client
                .inspect_session(target)
                .await
                .map_err(|_| SourceRejection::SessionUnavailable)?;
            if inspection.generation != *generation {
                return Err(SourceRejection::StaleBinding);
            }
            inspection
                .thread
                .get("cwd")
                .and_then(serde_json::Value::as_str)
                .ok_or(SourceRejection::DirectoryUnavailable)?
                .to_owned()
                .try_into()
                .map_err(|_| SourceRejection::DirectoryUnavailable)?
        }
    };
    let working_directory = validate_directory(&directory).await?;
    validate_profile(context, profile, action.target().is_some()).await?;
    Ok(PreparedSourceLaunch {
        action: action.clone(),
        endpoint: endpoint.endpoint.clone(),
        generation: generation.clone(),
        schema_digest: schema.clone(),
        working_directory,
        client_profile: profile,
    })
}

async fn validate_native_path(directory: &Path, channel_path: &str) -> Result<(), SourceRejection> {
    let relative = Path::new(channel_path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(SourceRejection::InvalidNativeChannel);
    }
    let root = tokio::fs::canonicalize(directory)
        .await
        .map_err(|_| SourceRejection::InvalidNativeChannel)?;
    let path = tokio::fs::canonicalize(root.join(relative))
        .await
        .map_err(|_| SourceRejection::InvalidNativeChannel)?;
    if path.parent() != Some(root.as_path()) {
        return Err(SourceRejection::InvalidNativeChannel);
    }
    Ok(())
}

async fn validate_directory(
    directory: &AbsoluteRemoteCwd,
) -> Result<AbsoluteRemoteCwd, SourceRejection> {
    let path = Path::new(directory.as_str());
    if !tokio::fs::metadata(path)
        .await
        .map_err(|_| SourceRejection::DirectoryUnavailable)?
        .is_dir()
    {
        return Err(SourceRejection::DirectoryUnavailable);
    }
    tokio::fs::canonicalize(path)
        .await
        .map_err(|_| SourceRejection::DirectoryUnavailable)?
        .to_str()
        .ok_or(SourceRejection::DirectoryUnavailable)?
        .to_owned()
        .try_into()
        .map_err(|_| SourceRejection::DirectoryUnavailable)
}

async fn validate_profile(
    context: &CliContext,
    profile: SourceClientProfile,
    reopening: bool,
) -> Result<(), SourceRejection> {
    let home = super::codex_home(context).map_err(|_| SourceRejection::SourceUnavailable)?;
    let path = home.join(profile.native_profile().file_name());
    let mut bytes = Vec::new();
    tokio::fs::File::open(path)
        .await
        .map_err(|_| SourceRejection::SourceUnavailable)?
        .take(65537)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| SourceRejection::SourceUnavailable)?;
    if bytes.len() > 65536 {
        return Err(SourceRejection::SourceUnavailable);
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| SourceRejection::SourceUnavailable)?;
    let keys = codex_native_integration::profile_remote_resume_permission_keys(text)
        .map_err(|_| SourceRejection::SourceUnavailable)?;
    if reopening && !keys.is_empty() {
        return Err(SourceRejection::ProfilePermissionConflict);
    }
    Ok(())
}

async fn establish_prepared_connection(
    request: &SourceRequest,
    prepared: &PreparedSourceLaunch,
    context: &CliContext,
) -> Result<NativeTransportConnection, SourceRejection> {
    establish_connection_at_boundary(request, prepared, context, std::future::ready(())).await
}

// The source preflight and native admission are separate IO observations. Keeping
// their boundary explicit lets integration tests hold a real replacement there;
// production continues immediately, without a second connection or test mode.
async fn establish_connection_at_boundary(
    request: &SourceRequest,
    prepared: &PreparedSourceLaunch,
    context: &CliContext,
    before_native_upgrade: impl std::future::Future<Output = ()>,
) -> Result<NativeTransportConnection, SourceRejection> {
    let establish = async {
        let before = connect_expected(request).await?;
        let current = prepare_source(
            &before,
            &prepared.action,
            prepared.client_profile,
            &request.expected_service,
            context,
        )
        .await?;
        if current != *prepared {
            return Err(SourceRejection::StaleBinding);
        }
        before_native_upgrade.await;
        let access = collaboration_client::CollaborationAccess::api(Path::new(
            request.service_directory.as_str(),
        ));
        let connection =
            NativeTransportConnection::connect(&access, prepared.endpoint.endpoint_id.clone())
                .await
                .map_err(|_| SourceRejection::InvalidNativeChannel)?;
        // Only now is a generation actually admitted. Re-read identity after upgrade;
        // never reopen the native path after this comparison.
        let after = connect_expected(request).await?;
        let current = prepare_source(
            &after,
            &prepared.action,
            prepared.client_profile,
            &request.expected_service,
            context,
        )
        .await?;
        if connection.endpoint != prepared.endpoint || current != *prepared {
            return Err(SourceRejection::StaleBinding);
        }
        Ok(connection)
    };
    tokio::time::timeout(Duration::from_secs(30), establish)
        .await
        .unwrap_or(Err(SourceRejection::SourceUnavailable))
}

#[cfg(test)]
#[path = "source_ssh_executor_tests.rs"]
mod tests;
