//! The published service manifest: how a client on this machine finds the collaboration API.
//!
//! Version 3 is the only version read. There is no connection handshake: a client takes the
//! Router's identity and version from here, then calls tools on the API socket it names.
use crate::{MachineLabel, NonEmptyText, SchemaDigest, UuidIdentity};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

/// The only manifest version a client reads.
pub const SERVICE_MANIFEST_VERSION: u8 = 3;

/// MCP over HTTP on the owner-only Unix socket in the service directory.
#[derive(schemars::JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ApiTransport {
    #[serde(rename = "streamableHttpUnix")]
    StreamableHttpUnix,
}

/// The API socket's file name inside the service directory.
#[derive(schemars::JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ApiSocketPath {
    #[serde(rename = "control.sock")]
    ServiceSocket,
}

impl ApiSocketPath {
    #[must_use]
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::ServiceSocket => "control.sock",
        }
    }
}

/// Where the CLIs reach the collaboration API.
#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiSelector {
    pub transport: ApiTransport,
    pub path: ApiSocketPath,
}

#[derive(schemars::JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum McpTransport {
    #[serde(rename = "streamableHttp")]
    StreamableHttp,
}

/// Where models reach the same API over localhost TCP.
#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpSelector {
    pub transport: McpTransport,
    pub url: String,
}

#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceManifest {
    #[serde(deserialize_with = "read_version")]
    #[schemars(range(min = 3, max = 3))]
    pub version: u8,
    pub service_id: UuidIdentity,
    pub service_epoch: UuidIdentity,
    pub machine_label: MachineLabel,
    /// The running Host's release version; clients warn when theirs differs.
    pub service_version: NonEmptyText,
    pub api: ApiSelector,
    pub mcp: McpSelector,
    /// The Codex native schema bundle the Host published, when it started with one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_schema_digest: Option<SchemaDigest>,
    /// Router model-proxy listener used for Claude Code routed launches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router_proxy_endpoint: Option<SocketAddr>,
}

fn read_version<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u8, D::Error> {
    let version = u8::deserialize(deserializer)?;
    if version != SERVICE_MANIFEST_VERSION {
        return Err(serde::de::Error::custom("unsupported manifest version"));
    }
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::ServiceManifest;
    use serde_json::json;

    fn manifest(version: u8, router_proxy_endpoint: Option<&str>) -> serde_json::Value {
        let mut value = json!({
            "version":version,
            "serviceId":"00000000-0000-4000-8000-000000000001",
            "serviceEpoch":"00000000-0000-4000-8000-000000000002",
            "machineLabel":"test-machine",
            "serviceVersion":"0.1.40",
            "api":{"transport":"streamableHttpUnix","path":"control.sock"},
            "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:8788/mcp"},
            "nativeSchemaDigest":format!("sha256:{}", "b".repeat(64))
        });
        if let Some(endpoint) = router_proxy_endpoint {
            value["routerProxyEndpoint"] = json!(endpoint);
        }
        value
    }

    #[test]
    fn only_version_three_is_read() {
        assert!(serde_json::from_value::<ServiceManifest>(manifest(3, None)).is_ok());
        for version in [1, 2, 4] {
            let error = serde_json::from_value::<ServiceManifest>(manifest(version, None))
                .expect_err("only version three is read")
                .to_string();
            assert!(error.contains("unsupported manifest version"), "{error}");
        }
    }

    #[test]
    fn a_version_two_manifest_is_refused_by_its_shape_too() {
        let version_two = json!({
            "version":2,
            "serviceId":"00000000-0000-4000-8000-000000000001",
            "machineLabel":"test-machine",
            "serviceEpoch":"00000000-0000-4000-8000-000000000002",
            "control":{"transport":"unixJsonLines","path":"control.sock"},
            "controlSchemaDigest":format!("sha256:{}", "a".repeat(64)),
            "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:8788/mcp"}
        });
        assert!(serde_json::from_value::<ServiceManifest>(version_two).is_err());
    }

    #[test]
    fn optional_fields_round_trip() {
        for value in [manifest(3, Some("127.0.0.1:18787")), manifest(3, None), {
            let mut value = manifest(3, None);
            value
                .as_object_mut()
                .expect("manifest object")
                .remove("nativeSchemaDigest");
            value
        }] {
            let parsed: ServiceManifest =
                serde_json::from_value(value.clone()).expect("version three manifest");
            assert_eq!(
                serde_json::to_value(parsed).expect("serialize manifest"),
                value
            );
        }
    }
}
