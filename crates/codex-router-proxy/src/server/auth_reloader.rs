/// Thread-safe handle for replacing local auth without sharing the full runtime.
#[derive(Clone, Debug)]
pub struct LocalAuthReloader {
    auth_gate: crate::local_auth::ProxyLocalAuthGate,
    claude_edge_auth_gate: Option<crate::local_auth::ProxyLocalAuthGate>,
    codex_local_token_authentication_required: bool,
    websocket_revocations: WebSocketRevocationRegistry,
}

impl LocalAuthReloader {
    /// Replaces local auth from an already loaded auth snapshot.
    pub fn reload_auth(&self, auth: LocalRouterAuth) {
        let active_generation = auth.current_generation();
        if self.codex_local_token_authentication_required {
            self.auth_gate.replace(auth.clone());
            self.websocket_revocations
                .close_all_except(active_generation);
        }
        if let Some(claude_edge_auth_gate) = &self.claude_edge_auth_gate {
            claude_edge_auth_gate.replace(auth);
        }
    }

    /// Replaces local auth and closes WebSocket connections authenticated with old generations.
    pub fn reload_local_auth(
        &self,
        current: LocalRouterTokenRecord,
        previous: Vec<LocalRouterTokenRecord>,
    ) {
        self.reload_auth(LocalRouterAuth::new(current, previous));
    }
}

fn has_forbidden_websocket_subprotocol_auth_carrier(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("token") || value.contains("bearer") || value.contains("authorization")
}

fn websocket_handshake_from_hyper_headers(headers: &HeaderMap) -> WebSocketHandshakeRequest {
    let mut handshake = WebSocketHandshakeRequest::new();
    for (name, value) in headers {
        if let Ok(value) = value.to_str() {
            handshake = handshake.with_header(Header::new(name.as_str(), value));
        }
    }

    handshake
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(header_name, _value)| header_name.as_str().eq_ignore_ascii_case(name))
        .and_then(|(_header_name, value)| value.to_str().ok())
        .map(str::to_owned)
}

fn path_without_query(path: &str) -> &str {
    path.split_once('?').map_or(path, |(path, _query)| path)
}

const HTTP_REQUEST_METADATA_PREFIX_MAX_BYTES: usize = 16 * 1024;
const HTTP_REQUEST_REPLAY_MAX_BYTES: usize = 2 * 1024 * 1024;
const HTTP_RESPONSE_AFFINITY_SCAN_MAX_BYTES: usize = 64 * 1024;
const HTTP_RESPONSE_AFFINITY_SCAN_MAX_EVENTS: usize = 64;
