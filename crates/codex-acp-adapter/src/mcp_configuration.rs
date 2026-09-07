//! Session-scoped stdio MCP settings; no global configuration writes or tool execution.
use crate::AcpSchemaCatalog;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

#[derive(Debug, thiserror::Error)]
pub enum McpConfigurationError {
    #[error("invalid ACP MCP server configuration")]
    InvalidParameters,
    #[error("MCP transport is not supported")]
    UnsupportedTransport,
    #[error("duplicate MCP server or environment name")]
    DuplicateName,
    #[error("ACP schema unavailable")]
    SchemaUnavailable,
}
/// Intentionally lacks Debug: environment values may contain credentials.
#[derive(Clone, PartialEq, Eq)]
pub struct McpConfiguration {
    servers: BTreeMap<String, StdioServer>,
}
#[derive(Clone, PartialEq, Eq)]
struct StdioServer {
    command: String,
    arguments: Vec<String>,
    environment: BTreeMap<String, String>,
}
impl McpConfiguration {
    pub fn parse(
        catalog: &mut AcpSchemaCatalog,
        servers: &[Value],
    ) -> Result<Self, McpConfigurationError> {
        let mut parsed = BTreeMap::new();
        for server in servers {
            if server.get("type").is_some() {
                return Err(McpConfigurationError::UnsupportedTransport);
            }
            if !catalog
                .validate("McpServerStdio", server)
                .map_err(|_| McpConfigurationError::SchemaUnavailable)?
            {
                return Err(McpConfigurationError::InvalidParameters);
            }
            let name = string_field(server, "name")?;
            let command = string_field(server, "command")?;
            if name.is_empty() || !Path::new(command).is_absolute() || command.contains('\0') {
                return Err(McpConfigurationError::InvalidParameters);
            }
            let arguments = server
                .get("args")
                .and_then(Value::as_array)
                .ok_or(McpConfigurationError::InvalidParameters)?
                .iter()
                .map(|argument| {
                    argument
                        .as_str()
                        .filter(|value| !value.contains('\0'))
                        .map(str::to_owned)
                        .ok_or(McpConfigurationError::InvalidParameters)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let mut environment = BTreeMap::new();
            for entry in server
                .get("env")
                .and_then(Value::as_array)
                .ok_or(McpConfigurationError::InvalidParameters)?
            {
                let key = string_field(entry, "name")?;
                let value = string_field(entry, "value")?;
                if key.is_empty() || key.contains(['=', '\0']) || value.contains('\0') {
                    return Err(McpConfigurationError::InvalidParameters);
                }
                if environment
                    .insert(key.to_owned(), value.to_owned())
                    .is_some()
                {
                    return Err(McpConfigurationError::DuplicateName);
                }
            }
            if parsed
                .insert(
                    name.to_owned(),
                    StdioServer {
                        command: command.to_owned(),
                        arguments,
                        environment,
                    },
                )
                .is_some()
            {
                return Err(McpConfigurationError::DuplicateName);
            }
        }
        Ok(Self { servers: parsed })
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }
    /// Insert into native thread/start's `config`; empty input leaves native preconfiguration alone.
    #[must_use]
    pub fn native_overrides(&self) -> Option<Value> {
        if self.is_empty() {
            return None;
        }
        let servers = self.servers.iter().map(|(name, server)| (name.clone(), json!({"command":server.command,"args":server.arguments,"env":server.environment}))).collect::<serde_json::Map<_, _>>();
        Some(json!({"mcp_servers":servers}))
    }
}
fn string_field<'a>(value: &'a Value, field: &str) -> Result<&'a str, McpConfigurationError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or(McpConfigurationError::InvalidParameters)
}
