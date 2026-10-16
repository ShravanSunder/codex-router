use super::*;
use std::{num::NonZeroU16, path::Path};

// The test double is the foreign CLI only. Spawn, pipes, waits, signals and reaping stay real.
const CHILD_FIXTURE: &str = r#"
import json, os, signal, socket, sys
mode = os.environ['TAILNET_TEST_MODE']
if sys.argv[1:] == ['status', '--json']:
    if mode == 'overflow':
        print('x' * 65537)
    elif mode == 'bad_status':
        print('not-json')
    else:
        print(json.dumps({'BackendState':'Running','Self':{'DNSName':'node.example.ts.net.'},'CertDomains':['node.example.ts.net']}))
    sys.exit(0)
if mode == 'early_exit':
    sys.exit(23)
signal.signal(signal.SIGINT, lambda *_: sys.exit(0))
print('https://node.example.ts.net:8443/', flush=True)
if mode == 'controlled_exit':
    connection = socket.socket(socket.AF_UNIX)
    connection.connect(os.environ['TAILNET_TEST_CONTROL'])
    connection.sendall(b'ready')
    connection.recv(1)
    sys.exit(23)
while True:
    signal.pause()
"#;

fn command_fixture(mode: &str, control: Option<&Path>) -> TailscaleCommand {
    let mut specification = ChildCommandSpec::new(PathBuf::from("/usr/bin/python3"))
        .with_arguments(["-u", "-c", CHILD_FIXTURE])
        .with_environment("TAILNET_TEST_MODE", mode);
    if let Some(control) = control {
        specification = specification.with_environment("TAILNET_TEST_CONTROL", control);
    }
    TailscaleCommand::fixture(specification)
}

fn port() -> NonZeroU16 {
    NonZeroU16::new(8443).expect("port")
}

fn assert_reaped(process_id: u32) {
    let process_id = i32::try_from(process_id).expect("pid fits");
    let pid = rustix::process::Pid::from_raw(process_id).expect("positive pid");
    assert!(
        rustix::process::test_kill_process(pid).is_err(),
        "owned child survived its explicit shutdown"
    );
}

#[test]
fn serve_argv_is_literal_foreground_https_root_and_actual_loopback_backend() {
    assert_eq!(
        serve_arguments(port(), "127.0.0.1:19088".parse().expect("backend")),
        [
            "serve",
            "--bg=false",
            "--https=8443",
            "--set-path=/",
            "http://127.0.0.1:19088",
        ]
        .map(OsString::from)
    );
}

#[tokio::test]
async fn finite_status_calls_bound_output_and_reject_malformed_status_at_the_parser() {
    let status = command_fixture("normal", None)
        .capture(&["status", "--json"])
        .await
        .expect("real status subprocess");
    assert_eq!(
        crate::tailscale_status::https_node_name(&status).expect("status"),
        "node.example.ts.net"
    );
    let bad = command_fixture("bad_status", None)
        .capture(&["status", "--json"])
        .await
        .expect("subprocess exits successfully");
    assert!(crate::tailscale_status::https_node_name(&bad).is_err());
    assert!(
        command_fixture("overflow", None)
            .capture(&["status", "--json"])
            .await
            .is_err()
    );
}

#[tokio::test]
async fn startup_failure_is_reaped_without_a_ready_receipt() {
    let mut process = command_fixture("early_exit", None)
        .spawn_serve(
            port(),
            "127.0.0.1:19088".parse().expect("backend"),
            "node.example.ts.net",
        )
        .expect("spawn");
    let process_id = process.process_id().expect("pid");
    assert!(process.advertised().await.is_err());
    process.stop().await.expect("settle failed startup");
    assert_reaped(process_id);
}

#[tokio::test]
async fn normal_shutdown_reaps_the_advertised_foreground_child() {
    let mut process = command_fixture("normal", None)
        .spawn_serve(
            port(),
            "127.0.0.1:19088".parse().expect("backend"),
            "node.example.ts.net",
        )
        .expect("spawn");
    let process_id = process.process_id().expect("pid");
    process.advertised().await.expect("advertised");
    process.stop().await.expect("graceful child shutdown");
    assert_reaped(process_id);
}

#[tokio::test]
async fn exit_after_advertisement_is_a_failure_and_reaps_the_same_child() {
    let directory = tempfile::tempdir_in("/tmp").expect("short socket directory");
    let control = directory.path().join("exit.sock");
    let listener = tokio::net::UnixListener::bind(&control).expect("control listener");
    let mut process = command_fixture("controlled_exit", Some(&control))
        .spawn_serve(
            port(),
            "127.0.0.1:19088".parse().expect("backend"),
            "node.example.ts.net",
        )
        .expect("spawn");
    let process_id = process.process_id().expect("pid");
    process
        .advertised()
        .await
        .expect("advertised before exit event");
    let (mut connection, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .expect("control bound")
        .expect("control");
    tokio::io::AsyncWriteExt::write_all(&mut connection, b"x")
        .await
        .expect("exit event");
    let failure = tokio::time::timeout(Duration::from_secs(5), process.failure())
        .await
        .expect("exit observed");
    assert!(
        failure.to_string().contains("exited unexpectedly"),
        "{failure}"
    );
    process.stop().await.expect("settle observed exit");
    assert_reaped(process_id);
}

#[tokio::test]
async fn cancelling_an_observation_keeps_the_child_owned_until_explicit_shutdown() {
    let mut process = command_fixture("normal", None)
        .spawn_serve(
            port(),
            "127.0.0.1:19088".parse().expect("backend"),
            "node.example.ts.net",
        )
        .expect("spawn");
    let process_id = process.process_id().expect("pid");
    process.advertised().await.expect("advertised");
    // Cancel a genuinely polled observation without sleeping or releasing ownership.
    {
        let observation = process.failure();
        tokio::pin!(observation);
        assert!(futures_util::poll!(&mut observation).is_pending());
    }
    process
        .stop()
        .await
        .expect("explicit owner shutdown after cancellation");
    assert_reaped(process_id);
}
