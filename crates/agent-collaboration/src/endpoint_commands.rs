//! Descriptive endpoint discovery through the public Rust client.
use clap::{Parser, Subcommand};
use collaboration_client::{
    ClientError, ControlClient, ServiceDirectoryOptions, resolve_service_directory,
};
use serde_json::json;
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
};
pub(crate) fn result_envelope(result: serde_json::Value) -> serde_json::Value {
    let result = normalize_result(result);
    let mut envelope =
        serde_json::json!({"kind":"result","cliVersion":env!("CARGO_PKG_VERSION"),"result":result});
    if let (Some(fields), Some(service_version)) = (
        envelope.as_object_mut(),
        collaboration_client::observed_service_version(),
    ) {
        fields.insert(
            "serviceVersion".to_owned(),
            serde_json::json!(service_version),
        );
    }
    envelope
}
fn normalize_result(result: serde_json::Value) -> serde_json::Value {
    let Some(fields) = result.as_object() else {
        return serde_json::json!({"record":result});
    };
    if fields.contains_key("page") {
        return result;
    }
    if let (Some(operation_id), Some(record)) = (fields.get("operationId"), fields.get("result")) {
        if operation_id.is_null() {
            return normalize_result(record.clone());
        }
        return serde_json::json!({"record":record,"effects":{"operationId":operation_id}});
    }
    for collection in [
        "sessions",
        "endpoints",
        "entries",
        "records",
        "data",
        "projects",
        "boards",
        "topics",
        "messages",
        "threads",
    ] {
        if let Some(records) = fields.get(collection).and_then(serde_json::Value::as_array) {
            let mut page = fields.clone();
            page.remove(collection);
            page.insert("records".into(), serde_json::json!(records));
            page.entry("nextCursor").or_insert(serde_json::Value::Null);
            return serde_json::json!({"page":page});
        }
    }
    let mutation = fields.contains_key("outcome")
        || fields.contains_key("operationId")
        || fields.contains_key("changeId")
        || fields.contains_key("acceptance")
        || fields.contains_key("created")
        || fields.contains_key("updated")
        || fields.contains_key("attached")
        || fields.contains_key("archived");
    if mutation {
        let effects = fields
            .get("effects")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        serde_json::json!({"record":result,"effects":effects})
    } else {
        serde_json::json!({"record":result})
    }
}

#[derive(Parser)]
#[command(name = "endpoints")]
struct EndpointArguments {
    #[command(subcommand)]
    command: EndpointCommand,
}
#[derive(Subcommand)]
enum EndpointCommand {
    List {
        #[arg(long)]
        service_directory: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}
/// Runs discovery without loading a thread or launching a backend.
pub fn run_endpoint_command(arguments: Vec<OsString>) -> i32 {
    let arguments = match crate::automation_argument_feedback::parse_arguments::<EndpointArguments>(
        arguments,
    ) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let EndpointCommand::List {
        service_directory,
        json: machine_output,
    } = arguments.command;
    let directory = match resolve_directory(service_directory) {
        Ok(value) => value,
        Err(message) => return report_failure("invalidUsage", &message, 2, machine_output),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(value) => value,
        Err(_) => {
            return report_failure(
                "unavailable",
                "Client runtime unavailable",
                3,
                machine_output,
            );
        }
    };
    let result = runtime.block_on(async {
        let mut client =
            ControlClient::connect(&directory, "agent-collaboration", env!("CARGO_PKG_VERSION"))
                .await?;
        let inventory = client.list_endpoints().await?;
        client.close().await?;
        Ok::<_, ClientError>(inventory)
    });
    match result {
        Ok(inventory) => {
            let mut out = io::stdout().lock();
            if machine_output {
                if writeln!(
                    out,
                    "{}",
                    crate::endpoint_commands::result_envelope(json!(inventory))
                )
                .is_err()
                {
                    return 3;
                }
            } else {
                for entry in inventory.endpoints {
                    let id: String = entry.endpoint.endpoint_id.into();
                    let label: String = entry.label.into();
                    if writeln!(out, "{id}: {label} ({:?})", entry.availability).is_err() {
                        return 3;
                    }
                }
            }
            0
        }
        Err(ClientError::Rejected { .. }) => {
            report_failure("rejected", "Endpoint discovery rejected", 4, machine_output)
        }
        Err(error) => crate::permission_diagnostic_reporting::report_permission_error(
            &error,
            crate::permission_diagnostic_reporting::PermissionDiagnosticRendering::Command,
            machine_output,
        )
        .unwrap_or_else(|| {
            report_failure(
                "unavailable",
                "Endpoint discovery unavailable",
                3,
                machine_output,
            )
        }),
    }
}
pub(crate) fn resolve_directory(explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    resolve_service_directory(ServiceDirectoryOptions {
        explicit_directory: explicit,
        debug_defaults: cfg!(all(debug_assertions, not(test))),
        use_home_default: std::env::var_os("CODEX_ROUTER_USE_HOME_DEFAULT").is_some(),
        debug_router_root: std::env::var_os("CODEX_ROUTER_DEBUG_ROUTER_ROOT"),
        home_directory: std::env::var_os("HOME"),
    })
    .map_err(|error| error.to_string())
}
pub(crate) fn report_failure(kind: &str, message: &str, code: i32, machine_output: bool) -> i32 {
    if machine_output {
        let error = if code == 2 && matches!(kind, "invalidField" | "invalidUsage") {
            let field = message
                .split_whitespace()
                .find(|word| word.starts_with("--"))
                .unwrap_or("arguments");
            json!({"kind":kind,"message":message,"stage":"validation",
                "field":field,"constraint":message,
                "effects":{"kind":"local","mutation":"none"},
                "nextAction":"correctRequest"})
        } else {
            json!({"kind":kind,"message":message})
        };
        let _printed = writeln!(io::stdout(), "{}", json!({"kind":"error","error":error}));
    } else {
        let _printed = writeln!(io::stderr(), "{message}");
    }
    code
}

#[cfg(test)]
mod tests {
    use super::result_envelope;
    use serde_json::json;

    #[test]
    fn result_envelope_normalizes_list_read_and_mutation_shapes() {
        let list = result_envelope(json!({"records":[{"id":1}],"nextCursor":"next"}));
        let read = result_envelope(json!({"id":1}));
        let mutation = result_envelope(json!({"operationId":"op","result":{"id":1}}));

        for envelope in [&list, &read, &mutation] {
            assert_eq!(envelope["kind"], "result");
            assert!(
                envelope["cliVersion"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty())
            );
            // Unobserved before any handshake: omitted rather than empty.
            assert!(
                envelope
                    .get("serviceVersion")
                    .is_none_or(|version| version.as_str().is_some_and(|value| !value.is_empty()))
            );
        }
        assert_eq!(list.pointer("/result/page/records/0/id"), Some(&json!(1)));
        assert_eq!(
            list.pointer("/result/page/nextCursor"),
            Some(&json!("next"))
        );
        assert_eq!(read.pointer("/result/record/id"), Some(&json!(1)));
        assert_eq!(mutation.pointer("/result/record/id"), Some(&json!(1)));
        assert_eq!(
            mutation.pointer("/result/effects/operationId"),
            Some(&json!("op"))
        );
    }

    #[test]
    fn absent_operation_id_preserves_the_underlying_read_or_page_shape() {
        let list =
            result_envelope(json!({"operationId":null,"result":{"records":[],"nextCursor":null}}));
        let read = result_envelope(json!({"operationId":null,"result":{"id":1}}));
        assert!(list.pointer("/result/page/records").is_some());
        assert_eq!(read.pointer("/result/record/id"), Some(&json!(1)));
    }
}
