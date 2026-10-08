//! Immediate provider Session settings actions through the collaboration API.

use clap::{Args, Subcommand};
use collaboration_client::CollaborationClient;
use collaboration_client::protocol::{
    ProviderIdentity, ProviderSettingName, ProviderSettingsAcceptRequest, ProviderSettingsFailure,
    ProviderSettingsFailureKind, ProviderSettingsSetRequest, SessionRef,
};
use std::{
    io::{self, Write},
    path::PathBuf,
};

#[derive(Args)]
pub(crate) struct SettingsArguments {
    #[command(subcommand)]
    command: SettingsCommand,
}

#[derive(Subcommand)]
enum SettingsCommand {
    Set {
        #[arg(long)]
        target: String,
        /// Exact creator or Approver SessionRef JSON.
        #[arg(long)]
        actor: String,
        #[arg(long)]
        setting: String,
        #[arg(long)]
        value: String,
        #[arg(long)]
        service_directory: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Accept {
        #[arg(long)]
        target: String,
        /// Exact creator or Approver SessionRef JSON.
        #[arg(long)]
        actor: String,
        #[arg(long)]
        service_directory: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn run(arguments: SettingsArguments) -> i32 {
    let (target, actor, setting, directory, json_output) = match arguments.command {
        SettingsCommand::Set {
            target,
            actor,
            setting,
            value,
            service_directory,
            json,
        } => {
            let setting = match setting.as_str() {
                "mode" => ProviderSettingName::Mode,
                "model" => ProviderSettingName::Model,
                "effort" => ProviderSettingName::Effort,
                _ => {
                    return crate::endpoint_commands::report_failure(
                        "invalidField",
                        "--setting must be mode, model or effort",
                        2,
                        json,
                    );
                }
            };
            (
                target,
                actor,
                Some((setting, value)),
                service_directory,
                json,
            )
        }
        SettingsCommand::Accept {
            target,
            actor,
            service_directory,
            json,
        } => (target, actor, None, service_directory, json),
    };
    let target: SessionRef = match serde_json::from_str(&target) {
        Ok(value) => value,
        Err(_) => {
            return crate::endpoint_commands::report_failure(
                "invalidField",
                "--target must be a SessionRef JSON",
                2,
                json_output,
            );
        }
    };
    let actor: ProviderIdentity = match serde_json::from_str(&actor) {
        Ok(value) => value,
        Err(_) => {
            return crate::endpoint_commands::report_failure(
                "invalidField",
                "--actor must be a SessionRef or Human identity JSON",
                2,
                json_output,
            );
        }
    };
    let directory = match crate::endpoint_commands::resolve_directory(directory) {
        Ok(value) => value,
        Err(error) => {
            return crate::endpoint_commands::report_failure(
                "invalidField",
                &error,
                2,
                json_output,
            );
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(value) => value,
        Err(_) => return 3,
    };
    runtime.block_on(async move {
        let client = match CollaborationClient::connect(
            &directory,
            "agent-collaboration",
            env!("CARGO_PKG_VERSION"),
        )
        .await
        {
            Ok(value) => value,
            Err(error) => return report_client_failure(error, json_output),
        };
        let result = match setting {
            Some((setting, value)) => {
                client
                    .set_provider_conversation_setting(ProviderSettingsSetRequest {
                        target,
                        actor,
                        setting,
                        value,
                    })
                    .await
            }
            None => {
                client
                    .accept_provider_conversation_settings(ProviderSettingsAcceptRequest {
                        target,
                        actor,
                    })
                    .await
            }
        };
        match result {
            Ok(result) => {
                let rendered = if json_output {
                    serde_json::json!({"kind":"result","result":result}).to_string()
                } else {
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                };
                if writeln!(io::stdout(), "{rendered}").is_ok() {
                    0
                } else {
                    3
                }
            }
            Err(error) => report_client_failure(error, json_output),
        }
    })
}

fn report_client_failure(error: collaboration_client::ClientError, json_output: bool) -> i32 {
    let typed = match &error {
        collaboration_client::ClientError::Rejected {
            data: Some(data), ..
        } => serde_json::from_value::<ProviderSettingsFailure>(data.clone()).ok(),
        _ => None,
    };
    let exit = match typed.as_ref().map(|failure| failure.kind) {
        Some(ProviderSettingsFailureKind::InvalidSetting) => 2,
        Some(
            ProviderSettingsFailureKind::WrongActor
            | ProviderSettingsFailureKind::Busy
            | ProviderSettingsFailureKind::ProviderRejected,
        ) => 4,
        Some(ProviderSettingsFailureKind::OutcomeUnknown) => 5,
        _ => 3,
    };
    let rendered = typed.map_or_else(
        || serde_json::json!({"kind":"unavailable","message":error.to_string()}),
        |failure| serde_json::json!(failure),
    );
    if json_output {
        let _ = writeln!(
            io::stdout(),
            "{}",
            serde_json::json!({"kind":"error","error":rendered})
        );
    } else {
        let _ = writeln!(
            io::stderr(),
            "{}",
            rendered
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("provider settings action failed")
        );
    }
    exit
}
