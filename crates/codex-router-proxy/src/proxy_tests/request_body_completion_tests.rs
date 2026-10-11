use super::auth_rejection_fixtures::AuthRejectionFixture;
use super::*;
use http_body_util::BodyExt;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;

const FIRST_JSON_FRAME: &[u8] = br#"{"model":"gpt-5"}"#;
const TRAILING_WHITESPACE: &[u8] = b" \t\r\n ";
const COMPLETE_BODY: &[u8] = b"{\"model\":\"gpt-5\"} \t\r\n ";
const AUTH_REJECTION: &str = r#"{"error":{"code":"token_revoked","message":"Encountered invalidated oauth token for user, failing request"}}"#;

#[derive(Clone, Copy, Eq, PartialEq)]
enum RequestFraming {
    ContentLength,
    Chunked,
    ChunkedWithTrailers,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ProviderOutcome {
    AuthRejectThenSuccess,
    DisconnectAfterBody,
}

struct ObservedProviderRequest {
    authorization: String,
    account_header: Option<String>,
    content_length: Option<usize>,
    transfer_encoding: Option<String>,
    body: Result<Vec<u8>, String>,
    trailer: Option<String>,
}

fn prove_request_body_completion(framing: RequestFraming, outcome: ProviderOutcome) {
    let fixture = AuthRejectionFixture::new("request_body_completion");
    let metadata_before = fixture.credential_metadata();
    let listener = TcpListener::bind("127.0.0.1:0").expect("provider should bind");
    listener
        .set_nonblocking(true)
        .expect("provider should be async");
    let runtime = fixture.start(listener.local_addr().expect("provider address should read"));
    let router_address = runtime.local_addr();
    let records = Arc::new(Mutex::new(Vec::new()));
    let provider_records = Arc::clone(&records);
    let provider_shutdown = tokio_util::sync::CancellationToken::new();
    let provider_stop = provider_shutdown.clone();
    let (accepted_sender, accepted_receiver) = mpsc::channel();
    let provider_thread = thread::spawn(move || {
        let provider_runtime = must_ok(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build(),
        );
        provider_runtime.block_on(async {
            let listener =
                tokio::net::TcpListener::from_std(listener).expect("provider should register");
            let connections = tokio_util::task::TaskTracker::new();
            loop {
                let stream = tokio::select! {
                    biased;
                    () = provider_stop.cancelled() => break,
                    accepted = listener.accept() => accepted.expect("provider should accept").0,
                };
                accepted_sender
                    .send(())
                    .expect("accept observation should send");
                let records = Arc::clone(&provider_records);
                connections.spawn(async move {
                    let service = service_fn(move |request: http::Request<Incoming>| {
                        let records = Arc::clone(&records);
                        async move {
                            let (parts, body) = request.into_parts();
                            let header = |name: &str| {
                                parts.headers.get(name).map(|value| {
                                    value
                                        .to_str()
                                        .expect("fixture header should decode")
                                        .to_owned()
                                })
                            };
                            let authorization =
                                header("authorization").expect("selected token should exist");
                            let content_length = header("content-length")
                                .map(|value| value.parse::<usize>().expect("length should parse"));
                            let (body, trailer) = match body.collect().await {
                                Ok(collected) => {
                                    let trailer = collected
                                        .trailers()
                                        .and_then(|headers| headers.get("x-completion-trailer"))
                                        .map(|value| {
                                            value
                                                .to_str()
                                                .expect("trailer should decode")
                                                .to_owned()
                                        });
                                    (Ok(collected.to_bytes().to_vec()), trailer)
                                }
                                Err(error) => (Err(error.to_string()), None),
                            };
                            let status = if authorization == "Bearer primary-token" {
                                401
                            } else {
                                200
                            };
                            records.lock().expect("provider records should lock").push(
                                ObservedProviderRequest {
                                    authorization,
                                    account_header: header("chatgpt-account-id"),
                                    content_length,
                                    transfer_encoding: header("transfer-encoding"),
                                    body,
                                    trailer,
                                },
                            );
                            if outcome == ProviderOutcome::DisconnectAfterBody {
                                return Err(std::io::Error::other(
                                    "fixture disconnect after full body",
                                ));
                            }
                            Ok::<_, std::io::Error>(
                                http::Response::builder()
                                    .status(status)
                                    .header("connection", "close")
                                    .body(Full::new(bytes::Bytes::from_static(if status == 401 {
                                        AUTH_REJECTION.as_bytes()
                                    } else {
                                        b"complete-body-success"
                                    })))
                                    .expect("provider response should build"),
                            )
                        }
                    });
                    // The old partial replay can abort its mismatched Content-Length;
                    // retain that real transport outcome for the independent body oracle.
                    let _connection_result = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
            connections.close();
            connections.wait().await;
        });
    });
    let runtime_thread = thread::spawn(move || runtime.serve_http_connections(1));
    let (first_frame_sender, first_frame_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let client_thread = thread::spawn(move || {
        let mut client = TcpStream::connect(router_address).expect("client should connect");
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("client read should be bounded");
        client
            .set_write_timeout(Some(Duration::from_secs(5)))
            .expect("client write should be bounded");
        write!(client, "POST /v1/responses HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nX-Codex-Router-Token: current-token\r\n").expect("headers should write");
        match framing {
            RequestFraming::ContentLength => {
                write!(client, "Content-Length: {}\r\n\r\n", COMPLETE_BODY.len())
                    .expect("length should write");
                client
                    .write_all(FIRST_JSON_FRAME)
                    .expect("first frame should write");
            }
            RequestFraming::Chunked | RequestFraming::ChunkedWithTrailers => {
                client
                    .write_all(
                        b"Transfer-Encoding: chunked\r\nTrailer: x-completion-trailer\r\n\r\n",
                    )
                    .expect("chunked headers should write");
                write!(client, "{:x}\r\n", FIRST_JSON_FRAME.len())
                    .expect("first chunk size should write");
                client
                    .write_all(FIRST_JSON_FRAME)
                    .expect("first frame should write");
                client
                    .write_all(b"\r\n")
                    .expect("first chunk should finish");
            }
        }
        first_frame_sender
            .send(())
            .expect("first-frame event should send");
        release_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("trailing bytes must stay gated until the observation completes");
        match framing {
            RequestFraming::ContentLength => client
                .write_all(TRAILING_WHITESPACE)
                .expect("tail should write"),
            RequestFraming::Chunked | RequestFraming::ChunkedWithTrailers => {
                write!(client, "{:x}\r\n", TRAILING_WHITESPACE.len())
                    .expect("tail size should write");
                client
                    .write_all(TRAILING_WHITESPACE)
                    .expect("tail should write");
                client
                    .write_all(b"\r\n0\r\n")
                    .expect("data chunks should finish");
                if framing == RequestFraming::ChunkedWithTrailers {
                    client
                        .write_all(b"X-Completion-Trailer: complete\r\n")
                        .expect("trailer should write");
                }
                client.write_all(b"\r\n").expect("body EOF should write");
            }
        }
        client
            .shutdown(Shutdown::Write)
            .expect("request should finish");
        let mut response = String::new();
        client
            .read_to_string(&mut response)
            .expect("response should finish");
        response
    });
    first_frame_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("first valid JSON frame should arrive while EOF remains gated");
    // This is a bounded wait on an actual dispatch event, not a scheduling sleep.
    // The exact body/header oracles below also fail independently of this window.
    let dispatched_before_eof = match accepted_receiver.recv_timeout(Duration::from_millis(250)) {
        Ok(()) => true,
        Err(mpsc::RecvTimeoutError::Timeout) => false,
        Err(error) => panic!("dispatch observer must remain alive: {error}"),
    };
    release_sender
        .send(())
        .expect("tail and EOF should release");
    let response = client_thread.join().expect("client should join");
    assert_eq!(
        must_ok(runtime_thread.join().expect("runtime should join")),
        1
    );
    provider_shutdown.cancel();
    provider_thread.join().expect("provider should join");
    assert_eq!(fixture.credential_metadata(), metadata_before);
    let records = records.lock().expect("provider records should lock");
    assert!(
        !dispatched_before_eof,
        "valid JSON is not a complete Incoming body: no upstream may dispatch while the trailing frame/EOF is held"
    );
    let trailers_present = framing == RequestFraming::ChunkedWithTrailers;
    let single_attempt = trailers_present || outcome == ProviderOutcome::DisconnectAfterBody;
    assert!(
        response.starts_with(if single_attempt {
            "HTTP/1.1 502"
        } else {
            "HTTP/1.1 200"
        }),
        "complete-body outcome: {response}"
    );
    assert_eq!(records.len(), if single_attempt { 1 } else { 2 });
    for (index, record) in records.iter().enumerate() {
        assert_eq!(
            record
                .body
                .as_ref()
                .expect("each selected provider must receive a complete body"),
            COMPLETE_BODY
        );
        assert_eq!(
            record.authorization,
            if index == 0 {
                "Bearer primary-token"
            } else {
                "Bearer fallback-token"
            }
        );
        assert_eq!(
            record.account_header.as_deref(),
            Some(if index == 0 {
                "synthetic-openai-primary"
            } else {
                "synthetic-openai-fallback"
            })
        );
        if let Some(length) = record.content_length {
            assert_eq!(
                length,
                COMPLETE_BODY.len(),
                "wire Content-Length must cover every replayed byte"
            );
        } else {
            assert_eq!(record.transfer_encoding.as_deref(), Some("chunked"));
        }
        // Existing header sanitization strips Trailer; provider trailer
        // transmission is outside this exact-data/no-replay correction.
        assert_eq!(record.trailer.as_deref(), None);
    }
}

#[test]
fn request_body_completion_content_length_auth_retry_preserves_exact_bytes() {
    prove_request_body_completion(
        RequestFraming::ContentLength,
        ProviderOutcome::AuthRejectThenSuccess,
    );
}

#[test]
fn request_body_completion_chunked_auth_retry_preserves_exact_bytes() {
    prove_request_body_completion(
        RequestFraming::Chunked,
        ProviderOutcome::AuthRejectThenSuccess,
    );
}

#[test]
fn request_body_completion_complete_data_disables_auth_replay_when_trailers_are_present() {
    prove_request_body_completion(
        RequestFraming::ChunkedWithTrailers,
        ProviderOutcome::AuthRejectThenSuccess,
    );
}

#[test]
fn request_local_http_unknown_effect_disconnect_does_not_replay_complete_body() {
    prove_request_body_completion(
        RequestFraming::Chunked,
        ProviderOutcome::DisconnectAfterBody,
    );
}
