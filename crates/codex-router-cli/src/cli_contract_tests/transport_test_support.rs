use super::*;

pub(super) fn reserve_loopback_port() -> u16 {
    let listener = must_ok(TcpListener::bind("127.0.0.1:0"));
    let address = must_ok(listener.local_addr());
    address.port()
}

pub(super) fn send_loopback_request_with_retry(port: u16, token: &str, body: &[u8]) -> String {
    let mut client = connect_with_retry(port);
    let request = format!(
        "POST /v1/responses HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nX-Codex-Router-Token: {token}\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        String::from_utf8_lossy(body)
    );
    if let Err(error) = client.write_all(request.as_bytes()) {
        panic!("client request write should succeed: {error}");
    }
    if let Err(error) = client.shutdown(Shutdown::Write) {
        panic!("client write shutdown should succeed: {error}");
    }
    let mut response = String::new();
    if let Err(error) = client.read_to_string(&mut response) {
        panic!("client response read should succeed: {error}");
    }

    response
}

pub(super) fn send_tokenless_loopback_request_with_retry(port: u16, body: &[u8]) -> String {
    let mut client = connect_with_retry(port);
    let request = format!(
        "POST /v1/responses HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        String::from_utf8_lossy(body)
    );
    if let Err(error) = client.write_all(request.as_bytes()) {
        panic!("client request write should succeed: {error}");
    }
    if let Err(error) = client.shutdown(Shutdown::Write) {
        panic!("client write shutdown should succeed: {error}");
    }
    let mut response = String::new();
    if let Err(error) = client.read_to_string(&mut response) {
        panic!("client response read should succeed: {error}");
    }

    response
}

pub(super) fn read_http_request_with_body(stream: &mut TcpStream) -> String {
    let mut request = Vec::new();
    loop {
        let mut buffer = [0_u8; 512];
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(bytes_read) => {
                request.extend_from_slice(&buffer[..bytes_read]);
                if http_message_has_complete_body(&request) {
                    break;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                panic!("mock HTTP upstream timed out before full request: {error}");
            }
            Err(error) => panic!("mock HTTP upstream should read request: {error}"),
        }
    }

    String::from_utf8_lossy(&request).into_owned()
}

pub(super) fn http_message_has_complete_body(message: &[u8]) -> bool {
    let text = String::from_utf8_lossy(message);
    let Some(header_end) = text.find("\r\n\r\n") else {
        return false;
    };
    let Some(headers) = text.get(..header_end) else {
        return false;
    };
    let Some(content_length) = headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("content-length") {
            value.trim().parse::<usize>().ok()
        } else {
            None
        }
    }) else {
        return false;
    };
    message.len() >= header_end + "\r\n\r\n".len() + content_length
}

pub(super) fn connect_with_retry(port: u16) -> TcpStream {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut last_error = None;
    while std::time::Instant::now() < deadline {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => return stream,
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(5));
            }
        }
    }

    panic!("client should connect to CLI serve listener: {last_error:?}");
}

pub(super) fn connect_websocket_with_retry(
    port: u16,
    local_token: &str,
) -> tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut last_error = None;
    while std::time::Instant::now() < deadline {
        let mut request = match format!("ws://127.0.0.1:{port}/v1/responses").into_client_request()
        {
            Ok(request) => request,
            Err(error) => panic!("local websocket request should build: {error}"),
        };
        let header_value = match HeaderValue::from_str(local_token) {
            Ok(value) => value,
            Err(error) => panic!("local websocket token header should build: {error}"),
        };
        request
            .headers_mut()
            .insert("X-Codex-Router-Token", header_value);
        match connect(request) {
            Ok((client, _response)) => return client,
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(5));
            }
        }
    }

    panic!("local websocket client should connect to CLI serve listener: {last_error:?}");
}

pub(super) fn connect_tokenless_websocket_with_retry(
    port: u16,
) -> tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut last_error = None;
    while std::time::Instant::now() < deadline {
        let request = match format!("ws://127.0.0.1:{port}/v1/responses").into_client_request() {
            Ok(request) => request,
            Err(error) => panic!("local websocket request should build: {error}"),
        };
        match connect(request) {
            Ok((client, _response)) => return client,
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(5));
            }
        }
    }

    panic!("local websocket client should connect to CLI serve listener: {last_error:?}");
}
