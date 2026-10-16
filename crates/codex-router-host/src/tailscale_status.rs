//! Decode only the Tailscale status and Serve fields this Host owns or validates.

use serde::Deserialize;
use std::{collections::BTreeMap, io, num::NonZeroU16};

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct NodeStatus {
    backend_state: BackendState,
    #[serde(rename = "Self")]
    own_node: Option<NodeIdentity>,
    cert_domains: Option<Vec<String>>,
}

#[derive(Deserialize)]
enum BackendState {
    Running,
    #[serde(other)]
    Unavailable,
}

#[derive(Deserialize)]
struct NodeIdentity {
    #[serde(rename = "DNSName")]
    dns_name: String,
}

pub(super) fn https_node_name(encoded: &[u8]) -> io::Result<String> {
    let status: NodeStatus = serde_json::from_slice(encoded)
        .map_err(|_| io::Error::other("Tailscale status was malformed"))?;
    if !matches!(status.backend_state, BackendState::Running) {
        return Err(io::Error::other(
            "Tailscale is not running; connect it before enabling remote access",
        ));
    }
    let node = status
        .own_node
        .ok_or_else(|| io::Error::other("Tailscale status has no local node"))?;
    let name = node.dns_name.strip_suffix('.').unwrap_or(&node.dns_name);
    if name.len() > 253
        || !name.contains('.')
        || name.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err(io::Error::other(
            "Tailscale local DNS name is invalid; configure MagicDNS first",
        ));
    }
    if !status
        .cert_domains
        .as_ref()
        .is_some_and(|domains| domains.iter().any(|domain| domain == name))
    {
        return Err(io::Error::other(
            "Tailscale HTTPS is not enabled for this node; enable HTTPS in the tailnet administration console first",
        ));
    }
    Ok(name.to_owned())
}

/// The opaque nonempty IPN watcher identity that owns one foreground Serve lease.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ServeSessionId(String);

impl TryFrom<String> for ServeSessionId {
    type Error = io::Error;
    fn try_from(value: String) -> io::Result<Self> {
        if value.is_empty() {
            return Err(io::Error::other("Tailscale foreground session ID is empty"));
        }
        Ok(Self(value))
    }
}

impl ServeSessionId {
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct ServeStatus {
    #[serde(default, rename = "TCP")]
    tcp: BTreeMap<NonZeroU16, TcpHandler>,
    #[serde(default)]
    web: BTreeMap<String, WebHandler>,
    #[serde(default)]
    allow_funnel: BTreeMap<String, bool>,
    #[serde(default)]
    foreground: BTreeMap<String, Self>,
}

#[derive(Deserialize)]
struct TcpHandler {
    #[serde(default, rename = "HTTPS")]
    https: bool,
}

#[derive(Deserialize)]
struct WebHandler {
    #[serde(rename = "Handlers")]
    handlers: BTreeMap<String, ProxyHandler>,
}

#[derive(Deserialize)]
struct ProxyHandler {
    #[serde(rename = "Proxy")]
    proxy: Option<String>,
}

impl ServeStatus {
    pub(super) fn parse(encoded: &[u8]) -> io::Result<Self> {
        serde_json::from_slice::<Option<Self>>(encoded)
            .map(Option::unwrap_or_default)
            .map_err(|_| io::Error::other("Tailscale Serve status was malformed"))
    }

    pub(super) fn port_in_use(&self, selected: NonZeroU16) -> io::Result<bool> {
        let mut configs = vec![self];
        while let Some(config) = configs.pop() {
            if config.tcp.contains_key(&selected) {
                return Ok(true);
            }
            for host_port in config.web.keys().chain(config.allow_funnel.keys()) {
                let (_, port) = host_port
                    .rsplit_once(':')
                    .ok_or_else(|| io::Error::other("invalid Tailscale Serve host/port"))?;
                let port: NonZeroU16 = port
                    .parse()
                    .map_err(|_| io::Error::other("invalid Tailscale Serve port"))?;
                if port == selected {
                    return Ok(true);
                }
            }
            configs.extend(config.foreground.values());
        }
        Ok(false)
    }

    pub(super) fn owned_session(
        &self,
        node_name: &str,
        port: NonZeroU16,
        backend: &str,
    ) -> io::Result<ServeSessionId> {
        let host_port = format!("{node_name}:{port}");
        let mut matches = self
            .foreground
            .iter()
            .filter(|(_, config)| {
                config.tcp.get(&port).is_some_and(|handler| handler.https)
                    && config
                        .web
                        .get(&host_port)
                        .and_then(|handler| handler.handlers.get("/"))
                        .and_then(|handler| handler.proxy.as_deref())
                        == Some(backend)
                    && !config
                        .allow_funnel
                        .get(&host_port)
                        .copied()
                        .unwrap_or(false)
            })
            .map(|(session_id, _)| session_id);
        match (matches.next(), matches.next()) {
            (Some(session_id), None) => ServeSessionId::try_from(session_id.clone()),
            _ => Err(io::Error::other(
                "Tailscale did not publish exactly one owned private foreground Serve session",
            )),
        }
    }

    pub(super) fn has_session(&self, session_id: &ServeSessionId) -> bool {
        self.foreground.contains_key(session_id.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const PORT: NonZeroU16 = NonZeroU16::new(8443).expect("port");

    #[test]
    fn node_status_requires_running_https_and_a_valid_local_dns_name() {
        assert_eq!(https_node_name(br#"{"BackendState":"Running","Self":{"DNSName":"node.example.ts.net."},"CertDomains":["node.example.ts.net"]}"#).expect("valid node"), "node.example.ts.net");
        for invalid in [
            &b"not-json"[..],
            br#"{"BackendState":"Stopped","Self":null,"CertDomains":null}"#,
            br#"{"BackendState":"Running","Self":{"DNSName":"node.example.ts.net."},"CertDomains":[]}"#,
            br#"{"BackendState":"Running","Self":{"DNSName":"node/example.ts.net"},"CertDomains":["node/example.ts.net"]}"#,
        ] {
            assert!(https_node_name(invalid).is_err());
        }
    }

    #[test]
    fn collisions_include_background_foreground_web_and_funnel_ports() {
        for config in [
            r#"{"TCP":{"8443":{"HTTPS":true}}}"#,
            r#"{"Web":{"node.example.ts.net:8443":{"Handlers":{}}}}"#,
            r#"{"AllowFunnel":{"node.example.ts.net:8443":true}}"#,
            r#"{"Foreground":{"foreign":{"TCP":{"8443":{"HTTPS":true}}}}}"#,
        ] {
            assert!(
                ServeStatus::parse(config.as_bytes())
                    .expect("config")
                    .port_in_use(PORT)
                    .expect("port")
            );
        }
        for empty in ["null", "{}", r#"{"TCP":{"443":{"HTTPS":true}}}"#] {
            assert!(
                !ServeStatus::parse(empty.as_bytes())
                    .expect("config")
                    .port_in_use(PORT)
                    .expect("port")
            );
        }
        assert!(ServeStatus::parse(b"broken").is_err());
    }

    #[test]
    fn owned_lease_requires_the_exact_private_http_proxy() {
        let config = ServeStatus::parse(br#"{"Foreground":{"own-session":{"TCP":{"8443":{"HTTPS":true}},"Web":{"node.example.ts.net:8443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:19088"}}}}}}}"#).expect("config");
        assert_eq!(
            config
                .owned_session("node.example.ts.net", PORT, "http://127.0.0.1:19088")
                .expect("own lease")
                .as_str(),
            "own-session"
        );
        assert!(
            config
                .owned_session("node.example.ts.net", PORT, "http://127.0.0.1:29088")
                .is_err()
        );
        assert!(
            config.has_session(
                &ServeSessionId::try_from("own-session".to_owned()).expect("session ID")
            )
        );
    }
}
