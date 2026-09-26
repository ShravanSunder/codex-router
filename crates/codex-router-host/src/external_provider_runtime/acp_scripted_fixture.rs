//! A scripted ACP agent subprocess for wire-level client tests.
//!
//! Scripts are ordered JSON-RPC exchanges. Request IDs are captured from the
//! client, so a test checks the wire contract without relying on SDK ID values.

use super::ExternalProviderLaunch;
use serde_json::{Value, json};
use std::path::PathBuf;

const AGENT_SCRIPT: &str = include_str!("acp_scripted_fixture.py");

#[derive(Default)]
pub(crate) struct AcpFixtureScript {
    steps: Vec<Value>,
    diagnostic_path: Option<PathBuf>,
}

impl AcpFixtureScript {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Read a client request, match its method and selected parameter fields,
    /// and remember its ID under `request_name` for a later response.
    pub(crate) fn expect_request(
        mut self,
        request_name: &str,
        method: &str,
        expected_params: Value,
    ) -> Self {
        self.steps.push(json!({
            "action": "expect_request",
            "requestName": request_name,
            "method": method,
            "params": expected_params,
        }));
        self
    }

    pub(crate) fn expect_exact_request(
        mut self,
        request_name: &str,
        method: &str,
        expected_params: Value,
    ) -> Self {
        self.steps.push(json!({
            "action": "expect_request",
            "requestName": request_name,
            "method": method,
            "params": expected_params,
            "exactParams": true,
        }));
        self
    }

    /// Read and check an entire client message. Use this for notifications or
    /// responses to requests sent by the fixture agent.
    pub(crate) fn expect_message(mut self, expected_message: Value) -> Self {
        self.steps.push(json!({
            "action": "expect_message",
            "message": expected_message,
        }));
        self
    }

    pub(crate) fn respond(mut self, request_name: &str, result: Value) -> Self {
        self.steps.push(json!({
            "action": "respond",
            "requestName": request_name,
            "result": result,
        }));
        self
    }

    pub(crate) fn respond_error(mut self, request_name: &str, code: i32) -> Self {
        self.steps.push(json!({
            "action": "respond_error",
            "requestName": request_name,
            "code": code,
        }));
        self
    }

    pub(crate) fn send(mut self, message: Value) -> Self {
        self.steps
            .push(json!({"action": "send", "message": message}));
        self
    }

    pub(crate) fn exit(mut self) -> Self {
        self.steps.push(json!({"action": "exit"}));
        self
    }

    pub(crate) fn wait_for_signal(mut self, process_id_path: &std::path::Path) -> Self {
        self.steps.push(json!({
            "action": "wait_for_signal",
            "processIdPath": process_id_path,
        }));
        self
    }

    pub(crate) fn record_diagnostics(mut self, path: PathBuf) -> Self {
        self.diagnostic_path = Some(path);
        self
    }

    pub(crate) fn write_marker(mut self, path: &std::path::Path) -> Self {
        self.steps
            .push(json!({"action": "write_marker", "path": path}));
        self
    }

    pub(crate) fn launch(self) -> ExternalProviderLaunch {
        let script = serde_json::to_string(&self.steps).expect("fixture script serializes");
        ExternalProviderLaunch {
            executable: PathBuf::from("/usr/bin/python3"),
            arguments: vec![
                "-u".to_owned(),
                "-c".to_owned(),
                AGENT_SCRIPT.to_owned(),
                script,
            ],
            environment: self
                .diagnostic_path
                .map(|path| {
                    vec![(
                        "ACP_FIXTURE_DIAGNOSTICS".to_owned(),
                        path.to_string_lossy().into_owned(),
                    )]
                })
                .unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};

    #[test]
    fn script_mismatch_reports_expected_and_actual_messages() {
        let fixture = AcpFixtureScript::new()
            .expect_request("initialize", "initialize", json!({"protocolVersion": 1}))
            .launch();
        let mut child = Command::new(&fixture.executable)
            .args(&fixture.arguments)
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start fixture agent");
        child
            .stdin
            .take()
            .expect("fixture stdin")
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session/new\",\"params\":{\"protocolVersion\":1}}\n")
            .expect("send mismatched request");
        let output = child.wait_with_output().expect("fixture exits");
        let diagnostic = String::from_utf8(output.stderr).expect("utf-8 diagnostic");
        assert!(!output.status.success());
        assert!(diagnostic.contains("expected"), "{diagnostic}");
        assert!(diagnostic.contains("initialize"), "{diagnostic}");
        assert!(diagnostic.contains("session/new"), "{diagnostic}");
    }

    #[test]
    fn script_exchanges_requests_notifications_and_responses() {
        let fixture = AcpFixtureScript::new()
            .expect_request("initialize", "initialize", json!({"protocolVersion": 1}))
            .respond("initialize", json!({"protocolVersion": 1}))
            .send(json!({"jsonrpc": "2.0", "id": 91, "method": "session/request_permission"}))
            .expect_message(json!({"jsonrpc": "2.0", "id": 91, "result": {"outcome": "cancelled"}}))
            .launch();
        let mut child = Command::new(&fixture.executable)
            .args(&fixture.arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start fixture agent");
        let mut stdin = child.stdin.take().expect("fixture stdin");
        let mut stdout = BufReader::new(child.stdout.take().expect("fixture stdout"));
        stdin
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1}}\n")
            .expect("send initialize");
        let mut response = String::new();
        stdout
            .read_line(&mut response)
            .expect("read initialize response");
        assert_eq!(
            serde_json::from_str::<Value>(&response).expect("response JSON"),
            json!({"jsonrpc": "2.0", "id": 7, "result": {"protocolVersion": 1}})
        );
        response.clear();
        stdout.read_line(&mut response).expect("read agent request");
        assert_eq!(
            serde_json::from_str::<Value>(&response).expect("request JSON"),
            json!({"jsonrpc": "2.0", "id": 91, "method": "session/request_permission"})
        );
        stdin
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":91,\"result\":{\"outcome\":\"cancelled\"}}\n")
            .expect("answer agent request");
        drop(stdin);
        let output = child.wait_with_output().expect("fixture exits");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
