use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

const SERVICE: &str = "00000000-0000-4000-8000-000000000001";
const EPOCH: &str = "00000000-0000-4000-8000-000000000002";
type HeldRequest = (serde_json::Value, oneshot::Sender<serde_json::Value>);

struct ReadinessHttpFixture {
    url: String,
    incoming: mpsc::UnboundedReceiver<HeldRequest>,
    request_count: Arc<AtomicUsize>,
    shutdown: CancellationToken,
    server: JoinHandle<io::Result<()>>,
}

impl ReadinessHttpFixture {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let url = format!("http://{}/mcp", listener.local_addr().expect("address"));
        let (sender, incoming) = mpsc::unbounded_channel::<HeldRequest>();
        let request_count = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&request_count);
        let shutdown = CancellationToken::new();
        let stopped = shutdown.clone();
        let server = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    () = stopped.cancelled() => break,
                    connection = listener.accept() => {
                        let (stream, _) = connection?;
                        let sender = sender.clone();
                        let count = Arc::clone(&count);
                        connections.spawn(serve_request(stream, sender, count));
                    }
                }
            }
            while let Some(connection) = connections.join_next().await {
                connection.map_err(io::Error::other)??;
            }
            Ok(())
        });
        Self {
            url,
            incoming,
            request_count,
            shutdown,
            server,
        }
    }

    fn verification(&self) -> JoinHandle<io::Result<()>> {
        let url = self.url.clone();
        tokio::spawn(async move {
            verify(
                &url,
                &UuidIdentity::try_from(SERVICE.to_owned()).expect("service"),
                &UuidIdentity::try_from(EPOCH.to_owned()).expect("epoch"),
            )
            .await
        })
    }

    async fn request(
        &mut self,
        verification: &mut JoinHandle<io::Result<()>>,
        id: u64,
        method: &str,
    ) -> oneshot::Sender<serde_json::Value> {
        let (request, reply) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::select! {
                request = self.incoming.recv() => request.expect("HTTP request"),
                result = verification => panic!("readiness ended before expected request {id}: {result:?}"),
            }
        }).await.expect("request event bound");
        assert_eq!(request["id"], id);
        assert_eq!(request["method"], method);
        assert_eq!(request["jsonrpc"], "2.0");
        if id == 2 {
            assert_eq!(
                request["params"],
                json!({"name":"endpoints_list","arguments":{}})
            );
        }
        reply
    }

    async fn stop(mut self, expected_requests: usize) {
        self.shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(2), &mut self.server)
            .await
            .expect("server shutdown bound")
            .expect("server task")
            .expect("server stopped");
        assert_eq!(
            self.request_count.load(Ordering::SeqCst),
            expected_requests,
            "no request replay"
        );
    }
}

impl Drop for ReadinessHttpFixture {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.server.abort();
    }
}

// Literal HTTP peer fixture only: production server composition stays unchanged.
async fn serve_request(
    stream: tokio::net::TcpStream,
    sender: mpsc::UnboundedSender<HeldRequest>,
    count: Arc<AtomicUsize>,
) -> io::Result<()> {
    use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};
    let mut stream = tokio::io::BufReader::new(stream);
    let mut content_length = None;
    loop {
        let mut line = String::new();
        if stream.read_line(&mut line).await? == 0 {
            return Err(io::Error::other("incomplete fixture HTTP headers"));
        }
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = Some(value.trim().parse::<usize>().map_err(io::Error::other)?);
        }
    }
    let mut body = vec![0; content_length.ok_or_else(|| io::Error::other("fixture body length"))?];
    stream.read_exact(&mut body).await?;
    let request = serde_json::from_slice(&body).map_err(io::Error::other)?;
    count.fetch_add(1, Ordering::SeqCst);
    let (reply, response) = oneshot::channel();
    sender
        .send((request, reply))
        .map_err(|_| io::Error::other("fixture receiver closed"))?;
    let response = response
        .await
        .unwrap_or_else(|_| json!({"fixture":"released"}));
    let body = serde_json::to_vec(&response).map_err(io::Error::other)?;
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    // A timed-out requester may already have closed its socket. This is fixture
    // teardown, not a substituted readiness result or a retried request.
    let _sent = async {
        stream.get_mut().write_all(headers.as_bytes()).await?;
        stream.get_mut().write_all(&body).await?;
        stream.get_mut().shutdown().await
    }
    .await;
    Ok(())
}

async fn advance_held_reply(seconds: u64) {
    // Request arrival is observed before pausing. Resume before any further real
    // socket wait so Tokio cannot auto-advance while waiting on operating-system IO.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(seconds)).await;
    tokio::time::resume();
}

fn tools_response() -> serde_json::Value {
    json!({"jsonrpc":"2.0","id":1,"result":{"tools":[
        {"name":"endpoints_list"},{"name":"board_project_list"}
    ]}})
}

fn endpoint_response() -> serde_json::Value {
    json!({"jsonrpc":"2.0","id":2,"result":{"isError":false,"structuredContent":{
        "serviceEpoch":EPOCH,"sequence":0,"endpoints":[{
            "endpoint":{"serviceId":SERVICE,"endpointId":"codex-local"},
            "label":"budget fixture","availability":{"state":"unavailable",
                "observedAt":"2026-10-16T00:00:00Z","reason":"identity fixture","fix":null},
            "channels":[]
        }]
    }}})
}

async fn completion(verification: JoinHandle<io::Result<()>>) -> io::Result<()> {
    tokio::time::timeout(Duration::from_secs(2), verification)
        .await
        .expect("readiness completion event bound")
        .expect("verification task")
}

#[tokio::test]
async fn cold_certificate_window_accepts_a_35_second_first_reply_without_replay() {
    let mut fixture = ReadinessHttpFixture::start().await;
    let mut verification = fixture.verification();
    let first = fixture.request(&mut verification, 1, "tools/list").await;
    advance_held_reply(35).await;
    let _released = first.send(tools_response());
    let second = fixture.request(&mut verification, 2, "tools/call").await;
    second
        .send(endpoint_response())
        .expect("second request waiting");
    completion(verification)
        .await
        .expect("35-second cold certificate window must fit readiness");
    fixture.stop(2).await;
}

#[tokio::test]
async fn request_budget_times_out_a_held_first_reply_without_replay() {
    let mut fixture = ReadinessHttpFixture::start().await;
    let mut verification = fixture.verification();
    let first = fixture.request(&mut verification, 1, "tools/list").await;
    advance_held_reply(66).await;
    let failure = completion(verification)
        .await
        .expect_err("65-second request budget");
    assert_eq!(failure.kind(), io::ErrorKind::TimedOut);
    assert!(
        failure.to_string().contains("transport failed"),
        "{failure}"
    );
    drop(first);
    fixture.stop(1).await;
}

#[tokio::test]
async fn total_budget_times_out_across_two_individually_bounded_requests_without_replay() {
    let mut fixture = ReadinessHttpFixture::start().await;
    let mut verification = fixture.verification();
    let first = fixture.request(&mut verification, 1, "tools/list").await;
    advance_held_reply(40).await;
    let _released = first.send(tools_response());
    let second = fixture.request(&mut verification, 2, "tools/call").await;
    advance_held_reply(36).await;
    let failure = completion(verification)
        .await
        .expect_err("75-second total budget");
    assert_eq!(failure.kind(), io::ErrorKind::TimedOut);
    assert!(
        failure
            .to_string()
            .contains("readiness timed out; no request was retried"),
        "{failure}"
    );
    drop(second);
    fixture.stop(2).await;
}
