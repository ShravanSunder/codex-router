//! A native app-server's answer to a Router permission profile request.
use serde_json::{Value, json};

/// Returns the `sandbox` native reports for the profile a request selected: the
/// requested write grants become writable roots, and Router profiles carry
/// direct network. Requests that select no profile get no sandbox.
pub fn applied_router_sandbox(request: &Value) -> Value {
    let Some(profile) = request
        .pointer("/params/permissions")
        .and_then(Value::as_str)
    else {
        return Value::Null;
    };
    let roots = request
        .pointer("/params/config")
        .and_then(|config| config.get(format!("permissions.{profile}.filesystem")))
        .and_then(Value::as_object)
        .map(|filesystem| {
            // Native reports write grants as roots and omits read-only exceptions.
            filesystem
                .iter()
                .filter(|(_, access)| access.as_str() == Some("write"))
                .map(|(path, _)| path.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "type":"workspaceWrite",
        "writableRoots":roots,
        "networkAccess":true,
        "excludeTmpdirEnvVar":false,
        "excludeSlashTmp":false
    })
}
