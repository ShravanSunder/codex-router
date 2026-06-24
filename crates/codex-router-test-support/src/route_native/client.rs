use std::io::Read;
use std::io::Write;
use std::net::SocketAddr;
use std::net::TcpStream;
use std::time::Duration;

use tungstenite::Message;
use tungstenite::client::IntoClientRequest;
use tungstenite::connect;

use super::fixture::LOCAL_TOKEN;
use super::fixture::contains_route_native_upstream_token;

pub(super) fn send_http_request(
    address: SocketAddr,
    method: &str,
    path: &str,
    body: &[u8],
) -> Result<String, String> {
    let mut stream = TcpStream::connect(address)
        .map_err(|error| format!("route-native client failed to connect: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| format!("route-native client failed to set timeout: {error}"))?;
    let request = format!(
        "{method} {path} HTTP/1.1\r\nhost: 127.0.0.1\r\nauthorization: Bearer {LOCAL_TOKEN}\r\naccept: */*\r\ncontent-length: {}\r\n\r\n",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .and_then(|()| stream.write_all(body))
        .map_err(|error| format!("route-native client failed to write request: {error}"))?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| format!("route-native client failed to read response: {error}"))?;
    Ok(response)
}

pub(super) fn send_websocket_request(
    address: SocketAddr,
    path: &str,
    local_token: Option<&str>,
) -> Result<String, String> {
    let mut request = format!("ws://{address}{path}")
        .into_client_request()
        .map_err(|error| format!("route-native websocket request failed to build: {error}"))?;
    if let Some(local_token) = local_token {
        let value = format!("Bearer {local_token}")
            .parse()
            .map_err(|error| format!("route-native websocket auth header failed: {error}"))?;
        request.headers_mut().insert("Authorization", value);
    }
    let (mut client, _response) = connect(request)
        .map_err(|error| format!("route-native websocket connect failed: {error}"))?;
    client
        .send(Message::text(
            r#"{"type":"response.create","model":"gpt-route-native","input":[{"role":"user","content":[{"type":"input_text","text":"route-native websocket canary"}]}],"stream":true}"#,
        ))
        .map_err(|error| format!("route-native websocket send failed: {error}"))?;
    client
        .read()
        .map(|message| message.to_string())
        .map_err(|error| format!("route-native websocket read failed: {error}"))
}

pub(super) fn assert_success_response(response: &str, label: &str) -> Result<(), String> {
    if !response.starts_with("HTTP/1.1 200 OK") {
        return Err(format!("{label} did not return 200 OK: {response}"));
    }
    Ok(())
}

pub(super) fn assert_rejected_locally(response: &str, label: &str) -> Result<(), String> {
    if response.starts_with("HTTP/1.1 200 OK") {
        return Err(format!("{label} unexpectedly returned 200 OK"));
    }
    if contains_route_native_upstream_token(response) {
        return Err(format!("{label} leaked upstream token in local response"));
    }
    Ok(())
}

pub(super) fn require_websocket_rejection(
    result: Result<String, String>,
    label: &str,
) -> Result<String, String> {
    match result {
        Ok(message) => Err(format!(
            "{label} unexpectedly completed websocket: {message}"
        )),
        Err(error) => Ok(error),
    }
}

pub(super) fn is_local_websocket_rejection(error: &str) -> bool {
    error.contains("HTTP error")
        || error.contains("Connection reset")
        || error.contains("Handshake not finished")
        || error.contains("Protocol")
}
