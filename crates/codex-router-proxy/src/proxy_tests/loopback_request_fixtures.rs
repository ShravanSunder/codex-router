use super::*;

pub(super) fn send_loopback_request(
    server_address: std::net::SocketAddr,
    request_line: &str,
    body: &[u8],
) -> String {
    let mut client = match TcpStream::connect(server_address) {
        Ok(client) => client,
        Err(error) => panic!("client should connect to loopback listener: {error}"),
    };
    let request = format!(
        "{request_line}Host: 127.0.0.1\r\nConnection: close\r\nX-Codex-Router-Token: current-token\r\nContent-Length: {}\r\n\r\n{}",
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

pub(super) fn send_loopback_request_with_session(
    server_address: std::net::SocketAddr,
    session_id: &str,
    body: &[u8],
) -> String {
    let mut client =
        TcpStream::connect(server_address).expect("client should connect to loopback listener");
    let request = format!(
        "POST /v1/responses HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nX-Codex-Router-Token: current-token\r\nsession-id: {session_id}\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        String::from_utf8_lossy(body)
    );
    client
        .write_all(request.as_bytes())
        .expect("client request write should succeed");
    client
        .shutdown(Shutdown::Write)
        .expect("client write shutdown should succeed");
    let mut response = String::new();
    client
        .read_to_string(&mut response)
        .expect("client response read should succeed");
    response
}

pub(super) fn send_loopback_request_with_read_timeout(
    server_address: std::net::SocketAddr,
    request_line: &str,
    body: &[u8],
    read_timeout: Duration,
) -> String {
    let mut client = match TcpStream::connect(server_address) {
        Ok(client) => client,
        Err(error) => panic!("client should connect to loopback listener: {error}"),
    };
    if let Err(error) = client.set_read_timeout(Some(read_timeout)) {
        panic!("client read timeout should be set: {error}");
    }
    let request = format!(
        "{request_line}Host: 127.0.0.1\r\nContent-Length: {}\r\n\r\n{}",
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
        panic!("client response read should succeed before timeout: {error}");
    }

    response
}

pub(super) fn connect_local_websocket_with_timeout(
    server_address: std::net::SocketAddr,
    read_timeout: Duration,
) -> WebSocket<TcpStream> {
    let mut client = match TcpStream::connect(server_address) {
        Ok(client) => client,
        Err(error) => panic!("websocket client should connect to loopback listener: {error}"),
    };
    if let Err(error) = client.set_read_timeout(Some(read_timeout)) {
        panic!("websocket client read timeout should be set: {error}");
    }
    if let Err(error) = client.set_write_timeout(Some(read_timeout)) {
        panic!("websocket client write timeout should be set: {error}");
    }
    let request = format!(
        "GET /v1/responses HTTP/1.1\r\nHost: {server_address}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    if let Err(error) = client.write_all(request.as_bytes()) {
        panic!("websocket client should write handshake: {error}");
    }
    let handshake_response = read_http_response_headers(&mut client);
    assert!(
        handshake_response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"),
        "websocket handshake should complete, got:\n{handshake_response}"
    );

    WebSocket::from_raw_socket(client, Role::Client, None)
}

pub(super) fn send_loopback_request_with_token(
    server_address: std::net::SocketAddr,
    token: Option<&str>,
    body: &[u8],
) -> String {
    let mut client = match TcpStream::connect(server_address) {
        Ok(client) => client,
        Err(error) => panic!("client should connect to loopback listener: {error}"),
    };
    let token_header = token
        .map(|token| format!("X-Codex-Router-Token: {token}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "POST /v1/responses HTTP/1.1\r\nHost: 127.0.0.1\r\n{token_header}Content-Length: {}\r\n\r\n{}",
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

pub(super) fn read_test_http_request(stream: &mut TcpStream) -> String {
    let mut request_bytes = Vec::new();
    let header_length = loop {
        if let Some(header_end) = request_bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|position| position + 4)
        {
            break header_end;
        }
        let mut buffer = [0_u8; 1024];
        let read = match stream.read(&mut buffer) {
            Ok(read) => read,
            Err(error) => panic!("mock upstream should read request bytes: {error}"),
        };
        if read == 0 {
            panic!("mock upstream request ended before headers completed");
        }
        request_bytes.extend_from_slice(&buffer[..read]);
    };
    let headers = String::from_utf8_lossy(&request_bytes[..header_length]);
    let transfer_encoding_is_chunked = headers.lines().any(|line| {
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        name.eq_ignore_ascii_case("transfer-encoding")
            && value
                .split(',')
                .any(|encoding| encoding.trim().eq_ignore_ascii_case("chunked"))
    });
    if transfer_encoding_is_chunked {
        while !request_bytes[header_length..]
            .windows(5)
            .any(|window| window == b"0\r\n\r\n")
        {
            let mut buffer = [0_u8; 1024];
            let read = match stream.read(&mut buffer) {
                Ok(read) => read,
                Err(error) => panic!("mock upstream should read chunked request body: {error}"),
            };
            if read == 0 {
                panic!("mock upstream request ended before chunked body completed");
            }
            request_bytes.extend_from_slice(&buffer[..read]);
        }

        return String::from_utf8_lossy(&request_bytes).into_owned();
    }
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or_default();
    let body_end = header_length + content_length;
    while request_bytes.len() < body_end {
        let mut buffer = [0_u8; 1024];
        let read = match stream.read(&mut buffer) {
            Ok(read) => read,
            Err(error) => panic!("mock upstream should read request body: {error}"),
        };
        if read == 0 {
            panic!("mock upstream request ended before body completed");
        }
        request_bytes.extend_from_slice(&buffer[..read]);
    }

    String::from_utf8_lossy(&request_bytes[..body_end]).into_owned()
}

pub(super) fn read_http_response_headers(stream: &mut TcpStream) -> String {
    let mut response = Vec::new();
    loop {
        let mut buffer = [0_u8; 512];
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(bytes_read) => {
                response.extend_from_slice(&buffer[..bytes_read]);
                if response.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            Err(error) => panic!("client should read response headers: {error}"),
        }
    }

    String::from_utf8_lossy(&response).into_owned()
}

pub(super) fn read_until_contains(
    stream: &mut TcpStream,
    needle: &str,
    deadline_after: Duration,
) -> std::io::Result<String> {
    let deadline = Instant::now() + deadline_after;
    let mut bytes = Vec::new();
    loop {
        if String::from_utf8_lossy(&bytes).contains(needle) {
            return Ok(String::from_utf8_lossy(&bytes).into_owned());
        }
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("timed out waiting for `{needle}`"),
            ));
        }

        let mut buffer = [0_u8; 128];
        match stream.read(&mut buffer) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!("EOF before `{needle}`"),
                ));
            }
            Ok(read) => bytes.extend_from_slice(&buffer[..read]),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(error) => return Err(error),
        }
    }
}

pub(super) fn http_response_from_one_connection(
    handle_stream: impl FnOnce(TcpStream) + Send + 'static,
) -> String {
    let address = match LoopbackBindAddress::new("127.0.0.1", 0) {
        Ok(address) => address,
        Err(error) => panic!("loopback address should validate: {error}"),
    };
    let runtime = match LoopbackServerRuntime::bind(address) {
        Ok(runtime) => runtime,
        Err(error) => panic!("loopback bind should succeed: {error}"),
    };
    let listener = match runtime.listener().try_clone() {
        Ok(listener) => listener,
        Err(error) => panic!("listener clone should succeed: {error}"),
    };
    let server_thread = thread::spawn(move || {
        let (stream, _peer_address) = match listener.accept() {
            Ok(accepted) => accepted,
            Err(error) => panic!("server should accept one client: {error}"),
        };
        handle_stream(stream);
    });
    let mut client = match TcpStream::connect(runtime.local_addr()) {
        Ok(client) => client,
        Err(error) => panic!("client should connect to loopback listener: {error}"),
    };
    let request = concat!(
        "POST /v1/responses HTTP/1.1\r\n",
        "Host: 127.0.0.1\r\n",
        "X-Codex-Router-Token: current-token\r\n",
        "Content-Length: 17\r\n",
        "\r\n",
        "{\"model\":\"gpt-5\"}"
    );
    must_ok(client.write_all(request.as_bytes()));
    must_ok(client.shutdown(Shutdown::Write));
    let mut response = String::new();
    must_ok(client.read_to_string(&mut response));
    match server_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("server thread panicked: {error:?}"),
    }
    response
}
