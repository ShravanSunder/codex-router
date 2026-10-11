//! Real child-startup failures must remain diagnosable and joined on cleanup.
use super::*;
use std::{io::Write, sync::Mutex, time::Duration};
use tracing::instrument::WithSubscriber;

#[derive(Clone, Default)]
struct StartupTrace(Arc<Mutex<Vec<u8>>>);

impl Write for StartupTrace {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("trace lock").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for StartupTrace {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

async fn failed_startup_trace(script: &str) -> String {
    let root = tempfile::tempdir().expect("owned fixture root");
    let endpoint = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("unused endpoint")
        .local_addr()
        .expect("fixture address");
    let pid_path = root.path().join("child-pid");
    let config = HostConfig::new(crate::HostConfigInputs {
        coordination_paths: crate::HostCoordinationPaths::new(
            root.path().join("operator.sock"),
            root.path().join("host.lock"),
        ),
        router_endpoint: endpoint,
        mcp_bind: "127.0.0.1:0".parse().expect("MCP endpoint"),
        app_server_socket: root.path().join("native.sock"),
        managed_executable: "/unused/codex".into(),
        deadlines: crate::HostDeadlines::new(crate::HostDeadlineInputs {
            router_start: Duration::from_millis(250),
            app_server_start: Duration::from_secs(1),
            remote_control: Duration::from_secs(1),
            endpoint_inspection: Duration::from_secs(1),
            operator_request: Duration::from_secs(1),
        })
        .expect("bounded fixture deadlines"),
    });
    let command = ChildCommandSpec::new("/bin/sh".into())
        .with_arguments([
            "-c".into(),
            script.into(),
            "fixture".into(),
            pid_path.into_os_string(),
        ])
        .with_output(crate::ChildOutput::Null);
    let trace = StartupTrace::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(trace.clone())
        .finish();
    let outcome = super::start_router(&config, Some(&command))
        .with_subscriber(subscriber)
        .await;
    assert!(matches!(outcome, Err(HostError::RouterUnavailable)));
    let child_pid = std::fs::read_to_string(root.path().join("child-pid"))
        .expect("fixture child ran")
        .trim()
        .to_owned();
    let remaining = std::process::Command::new("/bin/kill")
        .args(["-0", &child_pid])
        .output()
        .expect("read-only child liveness check");
    assert!(
        !remaining.status.success(),
        "startup failure must reap its child"
    );
    let bytes = trace.0.lock().expect("trace lock").clone();
    String::from_utf8(bytes).expect("UTF-8 diagnostic")
}

#[tokio::test]
async fn router_startup_reports_early_exit_and_reaps_owned_child() {
    let trace = failed_startup_trace("echo $$ > \"$1\"; exit 23").await;
    assert!(trace.contains("child_state=\"exited\""), "{trace}");
    assert!(trace.contains("exit_code=Some(23)"), "{trace}");
    assert!(trace.contains("elapsed_ms="), "{trace}");
    assert!(trace.contains("connect_error=ConnectionRefused"), "{trace}");
}

#[tokio::test]
async fn router_startup_reports_exit_signal_and_reaps_owned_child() {
    let trace = failed_startup_trace("echo $$ > \"$1\"; kill -TERM $$").await;
    assert!(trace.contains("child_state=\"exited\""), "{trace}");
    assert!(trace.contains("exit_code=None"), "{trace}");
    assert!(trace.contains("exit_signal=Some(15)"), "{trace}");
}

#[tokio::test]
async fn router_startup_reports_live_nonlistener_and_reaps_owned_child() {
    let trace = failed_startup_trace("echo $$ > \"$1\"; exec /bin/sleep 30").await;
    assert!(trace.contains("child_state=\"running\""), "{trace}");
    assert!(trace.contains("elapsed_ms="), "{trace}");
    assert!(trace.contains("connect_error=ConnectionRefused"), "{trace}");
}
