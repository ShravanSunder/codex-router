use std::io::Read;
use std::io::Write;
use std::net::SocketAddr;
use std::net::TcpListener;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use super::fixture::TestResult;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SafeRequestRecord {
    pub(super) method: String,
    pub(super) path: String,
    pub(super) routing_account: Option<String>,
}

pub(super) struct HeldLoopbackProvider {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    get_response_gate: Arc<(Mutex<bool>, Condvar)>,
    post_response_gate: Arc<(Mutex<bool>, Condvar)>,
    request_receiver: mpsc::Receiver<SafeRequestRecord>,
    records: Vec<SafeRequestRecord>,
    accept_thread: Option<thread::JoinHandle<TestResult<()>>>,
}

impl HeldLoopbackProvider {
    pub(super) fn bind() -> TestResult<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let get_response_gate = Arc::new((Mutex::new(false), Condvar::new()));
        let post_response_gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (request_sender, request_receiver) = mpsc::channel();
        let thread_stop = Arc::clone(&stop);
        let thread_get_gate = Arc::clone(&get_response_gate);
        let thread_post_gate = Arc::clone(&post_response_gate);
        let accept_thread = thread::spawn(move || {
            let mut handlers = Vec::new();
            while let Ok((stream, _peer)) = listener.accept() {
                if thread_stop.load(Ordering::Acquire) {
                    break;
                }
                let handler_sender = request_sender.clone();
                let handler_get_gate = Arc::clone(&thread_get_gate);
                let handler_post_gate = Arc::clone(&thread_post_gate);
                handlers.push(thread::spawn(move || {
                    handle_provider_connection(
                        stream,
                        handler_sender,
                        handler_get_gate,
                        handler_post_gate,
                    )
                }));
            }
            release_response_gate(&thread_get_gate)?;
            release_response_gate(&thread_post_gate)?;
            for handler in handlers {
                handler.join().map_err(|_panic| {
                    std::io::Error::other("loopback connection handler panicked")
                })??;
            }
            Ok(())
        });
        Ok(Self {
            address,
            stop,
            get_response_gate,
            post_response_gate,
            request_receiver,
            records: Vec::new(),
            accept_thread: Some(accept_thread),
        })
    }

    pub(super) const fn address(&self) -> SocketAddr {
        self.address
    }

    pub(super) fn wait_for_request_count(
        &mut self,
        expected_count: usize,
        timeout: Duration,
    ) -> TestResult<&[SafeRequestRecord]> {
        let deadline = Instant::now() + timeout;
        while self.records.len() < expected_count {
            let remaining = deadline.saturating_duration_since(Instant::now());
            self.records
                .push(self.request_receiver.recv_timeout(remaining)?);
        }
        Ok(&self.records)
    }

    pub(super) fn release_get_responses(&self) -> TestResult<()> {
        release_response_gate(&self.get_response_gate)
    }

    pub(super) fn release_post_response(&self) -> TestResult<()> {
        release_response_gate(&self.post_response_gate)
    }

    pub(super) fn finish(mut self) -> TestResult<Vec<SafeRequestRecord>> {
        self.shutdown()?;
        self.records.extend(self.request_receiver.try_iter());
        Ok(std::mem::take(&mut self.records))
    }

    fn shutdown(&mut self) -> TestResult<()> {
        if self.accept_thread.is_none() {
            return Ok(());
        }
        self.stop.store(true, Ordering::Release);
        release_response_gate(&self.get_response_gate)?;
        release_response_gate(&self.post_response_gate)?;
        let _ = TcpStream::connect(self.address);
        if let Some(accept_thread) = self.accept_thread.take() {
            accept_thread
                .join()
                .map_err(|_panic| std::io::Error::other("loopback accept thread panicked"))??;
        }
        Ok(())
    }
}

impl Drop for HeldLoopbackProvider {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn release_response_gate(response_gate: &(Mutex<bool>, Condvar)) -> TestResult<()> {
    let (gate, wake) = response_gate;
    *gate
        .lock()
        .map_err(|_poisoned| std::io::Error::other("response gate lock poisoned"))? = true;
    wake.notify_all();
    Ok(())
}

fn handle_provider_connection(
    mut stream: TcpStream,
    request_sender: mpsc::Sender<SafeRequestRecord>,
    get_response_gate: Arc<(Mutex<bool>, Condvar)>,
    post_response_gate: Arc<(Mutex<bool>, Condvar)>,
) -> TestResult<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let request = read_bounded_request_head(&mut stream)?;
    let record = safe_request_record(&request);
    let method = record.method.clone();
    let path = record.path.clone();
    request_sender.send(record)?;
    let response_gate = if method == "POST" {
        post_response_gate
    } else {
        get_response_gate
    };
    let (gate, wake) = &*response_gate;
    let mut released = gate
        .lock()
        .map_err(|_poisoned| std::io::Error::other("response gate lock poisoned"))?;
    while !*released {
        released = wake
            .wait(released)
            .map_err(|_poisoned| std::io::Error::other("response gate wait poisoned"))?;
    }
    drop(released);
    let body = if method == "POST" {
        r#"{"code":"reset","windows_reset":2}"#
    } else if path.ends_with("/usage") {
        r#"{"rate_limit":{"primary_window":{"used_percent":10,"reset_at":2000000000,"limit_window_seconds":18000},"secondary_window":{"used_percent":100,"reset_at":2000000000,"limit_window_seconds":604800}},"additional_rate_limits":[]}"#
    } else {
        r#"{"credits":[{"id":"pty-credit-earliest","status":"available","expires_at":"2030-01-01T00:00:00Z","title":"PTY weekly reset"}],"available_count":1}"#
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    Ok(())
}

fn read_bounded_request_head(stream: &mut TcpStream) -> TestResult<Vec<u8>> {
    const MAXIMUM_REQUEST_HEAD: usize = 16 * 1024;
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    while request.len() < MAXIMUM_REQUEST_HEAD {
        let bytes = stream.read(&mut chunk)?;
        if bytes == 0 {
            break;
        }
        let bytes = chunk
            .get(..bytes)
            .ok_or_else(|| std::io::Error::other("request read exceeded chunk"))?;
        request.extend_from_slice(bytes);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    if request.len() >= MAXIMUM_REQUEST_HEAD {
        return Err(std::io::Error::other("loopback request head exceeded limit").into());
    }
    Ok(request)
}

fn safe_request_record(request: &[u8]) -> SafeRequestRecord {
    let request = String::from_utf8_lossy(request);
    let mut lines = request.lines();
    let mut request_line = lines.next().unwrap_or_default().split_whitespace();
    let method = request_line.next().unwrap_or_default().to_owned();
    let path = request_line.next().unwrap_or_default().to_owned();
    let routing_account = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("chatgpt-account-id")
            .then(|| value.trim().to_owned())
    });
    SafeRequestRecord {
        method,
        path,
        routing_account,
    }
}
