//! Descriptive stored/loaded/active inventory through the public Control client.
use clap::{Parser, Subcommand, ValueEnum};
use collaboration_client::protocol::{
    ChannelDescription, EndpointDescription, EndpointRef, NativeSessionListParams,
    NativeSessionScope, NativeSessionSource, NativeSessionView, ProviderSessionListParams,
};
use collaboration_client::{ClientError, ControlClient};
use serde_json::json;
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
};

#[derive(Parser)]
#[command(
    name = "agent-collaboration sessions",
    bin_name = "agent-collaboration sessions"
)]
struct InventoryArguments {
    #[command(subcommand)]
    command: InventoryCommand,
}
#[derive(Clone, Copy, ValueEnum)]
enum InventoryView {
    Stored,
    Loaded,
    Active,
}
#[derive(Clone, Copy, ValueEnum)]
enum InventorySource {
    Interactive,
    Subagents,
    All,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InventoryEndpointKind {
    Codex,
    Provider,
    Unsupported,
}

fn classify_endpoint_for_session_list(endpoint: &EndpointDescription) -> InventoryEndpointKind {
    if String::from(endpoint.endpoint.endpoint_id.clone()) == "claude-local" {
        return InventoryEndpointKind::Provider;
    }
    if endpoint
        .channels
        .iter()
        .any(|channel| matches!(channel, ChannelDescription::NativeCodex { .. }))
    {
        InventoryEndpointKind::Codex
    } else if endpoint
        .channels
        .iter()
        .any(|channel| matches!(channel, ChannelDescription::ExternalProvider { .. }))
    {
        InventoryEndpointKind::Provider
    } else {
        InventoryEndpointKind::Unsupported
    }
}
#[derive(Subcommand)]
enum InventoryCommand {
    /// Read stored metadata or live sessions from the selected endpoint; never resumes sessions.
    List {
        #[arg(long)]
        endpoint: String,
        #[arg(long, value_enum)]
        view: InventoryView,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long)]
        checkout: Option<PathBuf>,
        #[arg(long)]
        repo: Option<PathBuf>,
        #[arg(long)]
        any: bool,
        #[arg(long, value_enum)]
        source: Option<InventorySource>,
        #[arg(long)]
        query: Option<String>,
        #[arg(long,default_value_t=100,value_parser=clap::value_parser!(u32).range(1..=100))]
        page_size: u32,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        service_directory: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}
pub fn run_session_inventory_command(arguments: Vec<OsString>) -> i32 {
    let parsed =
        match crate::automation_argument_feedback::parse_arguments::<InventoryArguments>(arguments)
        {
            Ok(v) => v,
            Err(code) => return code,
        };
    let InventoryCommand::List {
        endpoint,
        view,
        cwd,
        checkout,
        repo,
        any,
        source,
        query,
        page_size,
        cursor,
        service_directory,
        json: machine,
    } = parsed.command;
    let scope_count = usize::from(cwd.is_some())
        + usize::from(checkout.is_some())
        + usize::from(repo.is_some())
        + usize::from(any);
    if scope_count != 1 {
        return crate::endpoint_commands::report_failure(
            "invalidUsage",
            "Choose exactly one of --cwd, --checkout, --repo, or --any",
            2,
            machine,
        );
    }
    let directory = match crate::endpoint_commands::resolve_directory(service_directory) {
        Ok(v) => v,
        Err(e) => return crate::endpoint_commands::report_failure("invalidField", &e, 2, machine),
    };
    let endpoint = match endpoint.try_into() {
        Ok(v) => v,
        Err(_) => {
            return crate::endpoint_commands::report_failure(
                "invalidUsage",
                "Invalid endpoint ID",
                2,
                machine,
            );
        }
    };
    if cursor
        .as_ref()
        .is_some_and(|v| v.is_empty() || v.len() > 1024)
    {
        return crate::endpoint_commands::report_failure(
            "invalidUsage",
            "Invalid inventory cursor",
            2,
            machine,
        );
    }
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(v) => v,
        Err(_) => return 3,
    };
    let result = runtime.block_on(async {
        let mut client =
            ControlClient::connect(&directory, "sessions-inventory", env!("CARGO_PKG_VERSION"))
                .await?;
        let endpoint = EndpointRef {
            service_id: client.identity().service_id.clone(),
            endpoint_id: endpoint,
        };
        let view = match view {
                InventoryView::Stored => NativeSessionView::Stored,
                InventoryView::Loaded => NativeSessionView::Loaded,
                InventoryView::Active => NativeSessionView::Active,
            };
        let scope = if let Some(path) = cwd {
                NativeSessionScope::Cwd {
                    path: collaboration_client::session_catalog::normalize_path(&path),
                }
            } else if let Some(path) = checkout {
                NativeSessionScope::Checkout {
                    root: collaboration_client::session_catalog::checkout_root(&path),
                }
            } else if let Some(path) = repo {
                let identity =
                    collaboration_client::session_catalog::discover_repository_identity(&path);
                NativeSessionScope::Repo {
                    live_roots: identity.live_roots,
                    normalized_origin: identity.normalized_origin,
                    basename: identity.repository_basename,
                    fallback_cwd: identity.fallback_cwd,
                }
            } else {
                NativeSessionScope::Any
            };
        let source = source.map(|source| match source {
                InventorySource::Interactive => NativeSessionSource::Interactive,
                InventorySource::Subagents => NativeSessionSource::Subagents,
                InventorySource::All => NativeSessionSource::All,
            });
        let inventory = client.list_endpoints().await?;
        let endpoint_kind = inventory.endpoints.iter()
            .find(|entry| entry.endpoint == endpoint)
            .map(classify_endpoint_for_session_list)
            .ok_or_else(|| ClientError::Rejected {
                code: -32050,
                data: Some(json!({"kind":"endpointNotFound","stage":"discovery","message":"Endpoint not found"})),
            })?;
        let result = match endpoint_kind {
            InventoryEndpointKind::Codex => {
                let source = source.ok_or(ClientError::InvalidRequest("--source is required for Codex sessions"))?;
                serde_json::to_value(client.list_sessions(NativeSessionListParams {
                    endpoint, view, scope, source, query, page_size, cursor,
                }).await?).map_err(|_| ClientError::Protocol("session inventory encoding failed"))
            }
            InventoryEndpointKind::Provider => serde_json::to_value(client.list_provider_sessions(ProviderSessionListParams {
                endpoint, view, scope, source: source.unwrap_or(NativeSessionSource::All),
                query, page_size, cursor,
            }).await?).map_err(|_| ClientError::Protocol("provider session inventory encoding failed")),
            InventoryEndpointKind::Unsupported => Err(ClientError::Rejected {
                code: -32050,
                data: Some(json!({"kind":"unsupportedCapability","stage":"discovery","message":"Session inventory unsupported on this endpoint"})),
            }),
        };
        let _closed = client.close().await;
        result
    });
    match result {
        Ok(result) => {
            let record = crate::endpoint_commands::result_envelope(result);
            let text = if machine {
                record.to_string()
            } else {
                serde_json::to_string_pretty(&record).unwrap_or_default()
            };
            if writeln!(io::stdout(), "{text}").is_ok() {
                0
            } else {
                3
            }
        }
        Err(ClientError::Rejected { code, data }) => {
            let exit = if code == -32602 {
                2
            } else if data
                .as_ref()
                .and_then(|v| v.get("kind"))
                .and_then(serde_json::Value::as_str)
                == Some("unavailable")
            {
                3
            } else {
                4
            };
            if machine {
                let _printed = writeln!(
                    io::stdout(),
                    "{}",
                    json!({"kind":"error","error":{"code":code,"data":data}})
                );
            } else {
                let _printed = writeln!(io::stderr(), "Session inventory rejected");
            }
            exit
        }
        Err(ClientError::InvalidRequest(message)) => {
            crate::endpoint_commands::report_failure("invalidUsage", message, 2, machine)
        }
        Err(error) => crate::permission_diagnostic_reporting::report_permission_error(
            &error,
            crate::permission_diagnostic_reporting::PermissionDiagnosticRendering::Command,
            machine,
        )
        .unwrap_or_else(|| {
            crate::endpoint_commands::report_failure(
                "unavailable",
                "Session inventory unavailable",
                3,
                machine,
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{InventoryArguments, InventoryEndpointKind, classify_endpoint_for_session_list};
    use clap::Parser;
    use collaboration_client::protocol::{EndpointDescription, NativeSessionListResult};
    use serde_json::json;

    #[test]
    fn provider_source_defaults_to_all_and_endpoint_kind_drives_dispatch() {
        let parsed = InventoryArguments::try_parse_from([
            "sessions",
            "list",
            "--endpoint",
            "claude-local",
            "--view",
            "stored",
            "--any",
        ]);
        assert!(parsed.is_ok(), "provider source is optional");
        let provider: EndpointDescription = serde_json::from_value(json!({
            "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},
            "label":"Provider", "availability":{"state":"available","observedAt":"2026-09-26T00:00:00Z"},
            "channels":[{"kind":"externalProvider","transport":"stdioAcp","bindingId":"fixture-binding",
                "bindingGeneration":7,"runtime":{"provider":"claudeCode","runtimeName":"fixture"},
                "capabilities":[{"name":"create","status":"supported","evidence":"advertised"}]}]
        })).expect("provider endpoint");
        let native: EndpointDescription = serde_json::from_value(json!({
            "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
            "label":"Codex", "availability":{"state":"available","observedAt":"2026-09-26T00:00:00Z"},
            "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock",
                "schemaDigest":null,"generation":null}]
        })).expect("native endpoint");
        assert_eq!(
            classify_endpoint_for_session_list(&provider),
            InventoryEndpointKind::Provider
        );
        let unavailable_claude: EndpointDescription = serde_json::from_value(json!({
            "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},
            "label":"Claude", "availability":{"state":"unavailable","observedAt":"2026-09-26T00:00:00Z","reason":"ACP unavailable"},
            "channels":[]
        })).expect("unavailable Claude endpoint");
        assert_eq!(
            classify_endpoint_for_session_list(&unavailable_claude),
            InventoryEndpointKind::Provider
        );
        assert_eq!(
            classify_endpoint_for_session_list(&native),
            InventoryEndpointKind::Codex
        );
    }

    #[test]
    fn native_list_keeps_the_frozen_codex_page_shape() {
        let frozen_native_result = json!({
            "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
            "observedAt":"2026-09-26T00:00:00Z","generation":null,
            "sessions":[{
                "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"thread-1"},
                "name":null,"title":"Existing thread","source":"interactive",
                "workingDirectory":"/tmp/project",
                "observation":{"kind":"stored","updatedAt":"2026-09-26T00:00:00Z"},
                "gitBranch":null,"idleSeconds":0
            }]
        });
        let decoded: NativeSessionListResult =
            serde_json::from_value(frozen_native_result.clone()).expect("0.1.38 native result");
        let rendered = serde_json::to_value(decoded).expect("native result serialization");
        assert_eq!(rendered, frozen_native_result);
        let output = crate::endpoint_commands::result_envelope(rendered);
        let frozen_codex_page = json!({
            "page":{
                "endpoint":frozen_native_result["endpoint"],
                "observedAt":"2026-09-26T00:00:00Z",
                "generation":null,
                "records":frozen_native_result["sessions"],
                "nextCursor":null
            }
        });
        assert_eq!(
            serde_json::to_vec(&output["result"]).expect("CLI result bytes"),
            serde_json::to_vec(&frozen_codex_page).expect("frozen 0.1.38 page bytes")
        );
    }
}
