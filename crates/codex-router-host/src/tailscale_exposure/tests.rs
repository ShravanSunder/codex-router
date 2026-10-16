use super::*;
use crate::ChildCommandSpec;
use std::{num::NonZeroU16, path::PathBuf};

const PREFLIGHT_FIXTURE: &str = r#"
import json, os, sys
mode = os.environ['TAILNET_TEST_MODE']
if sys.argv[1:] == ['status', '--json']:
    if mode == 'malformed':
        print('not-json')
    else:
        print(json.dumps({'BackendState':'Stopped' if mode == 'offline' else 'Running','Self':{'DNSName':'node.example.ts.net.'},'CertDomains':[] if mode == 'no_https' else ['node.example.ts.net']}))
elif sys.argv[1:] == ['serve', 'status', '--json']:
    print(json.dumps({'Foreground':{'foreign':{'TCP':{'8443':{'HTTPS':True}}}}} if mode == 'collision' else {}))
else:
    sys.exit(73)
"#;

fn fixture(mode: &str) -> TailscaleCommand {
    TailscaleCommand::fixture(
        ChildCommandSpec::new(PathBuf::from("/usr/bin/python3"))
            .with_arguments(["-u", "-c", PREFLIGHT_FIXTURE])
            .with_environment("TAILNET_TEST_MODE", mode),
    )
}

fn access() -> RemoteAccess {
    RemoteAccess::tailscale(NonZeroU16::new(8443).expect("port"))
}

#[tokio::test]
async fn disabled_mode_performs_no_tailscale_io_even_with_an_unusable_executable() {
    let command = TailscaleCommand::fixture(ChildCommandSpec::new(PathBuf::from(
        "/missing/tailscale-must-not-run",
    )));
    assert!(
        PreparedTailscaleExposure::prepare_with_command(None, command)
            .await
            .expect("disabled mode")
            .is_none()
    );
}

#[tokio::test]
async fn preflight_rejects_foreign_cli_failures_before_granting_proxy_authority() {
    for mode in ["malformed", "offline", "no_https", "collision"] {
        let result =
            PreparedTailscaleExposure::prepare_with_command(Some(access()), fixture(mode)).await;
        assert!(
            result.is_err(),
            "{mode} unexpectedly granted a proxy authority"
        );
    }
    let prepared =
        PreparedTailscaleExposure::prepare_with_command(Some(access()), fixture("normal"))
            .await
            .expect("preflight")
            .expect("opted in");
    assert_eq!(prepared.authority().authority(), "node.example.ts.net:8443");
}

const LEASE_FIXTURE: &str = r#"
import json, os, signal, sys
state_path = os.environ['TAILNET_TEST_STATE']
if sys.argv[1:] == ['serve', 'status', '--json']:
    with open(state_path) as state:
        print(state.read())
    sys.exit(0)
def write_state(state):
    with open(state_path, 'w') as output:
        json.dump(state, output)
state = {'Foreground':{'foreign':{'TCP':{'443':{'HTTPS':True}}},'fixture-owned':{'TCP':{'8443':{'HTTPS':True}}}}}
write_state(state)
def stop(*_):
    if os.environ['TAILNET_TEST_KEEP_LEASE'] != 'yes':
        state['Foreground'].pop('fixture-owned')
        write_state(state)
    sys.exit(0)
signal.signal(signal.SIGINT, stop)
print('https://node.example.ts.net:8443/', flush=True)
while True:
    signal.pause()
"#;

#[tokio::test]
async fn shutdown_reaps_the_owned_child_and_verifies_only_its_foreground_lease_retired() {
    for keep_lease in [false, true] {
        let directory = tempfile::tempdir().expect("private state directory");
        let state_path = directory.path().join("foreign-serve-state.json");
        let command = TailscaleCommand::fixture(
            ChildCommandSpec::new(PathBuf::from("/usr/bin/python3"))
                .with_arguments(["-u", "-c", LEASE_FIXTURE])
                .with_environment("TAILNET_TEST_STATE", &state_path)
                .with_environment(
                    "TAILNET_TEST_KEEP_LEASE",
                    if keep_lease { "yes" } else { "no" },
                ),
        );
        let mut process = command
            .spawn_serve(
                NonZeroU16::new(8443).expect("port"),
                "127.0.0.1:19088".parse().expect("backend"),
                "node.example.ts.net",
            )
            .expect("real child");
        let pid = process.process_id().expect("pid");
        process
            .advertised()
            .await
            .expect("foreign child advertises");
        // This test begins after advertisement; it makes no TLS/readiness claim.
        let mut exposure = TailscaleExposure {
            command,
            process,
            session_id: ServeSessionId::try_from("fixture-owned".to_owned()).expect("session ID"),
            url: "https://node.example.ts.net:8443/mcp".to_owned(),
        };
        let result = exposure.shutdown().await;
        assert_eq!(
            result.is_err(),
            keep_lease,
            "cleanup must refuse a retained own lease"
        );
        let remaining: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&state_path).expect("state readback"))
                .expect("state");
        assert_eq!(
            remaining["Foreground"]["foreign"],
            serde_json::json!({"TCP":{"443":{"HTTPS":true}}}),
            "unrelated foreground lease changed"
        );
        let pid = rustix::process::Pid::from_raw(i32::try_from(pid).expect("pid fits"))
            .expect("positive pid");
        assert!(
            rustix::process::test_kill_process(pid).is_err(),
            "owned Serve child was not reaped"
        );
    }
}
