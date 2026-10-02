//! A Human Approver remains reachable through the CLI after provider creation.
#![allow(clippy::indexing_slicing, clippy::panic_in_result_fn)]

use codex_router_host::{
    CollaborationRuntime, CollaborationRuntimeInputs, ExternalProviderLaunchBinding,
    ExternalProviderStartup,
};
use collaboration_client::ControlClient;
use collaboration_protocol::{
    NativeSessionScope, NativeSessionSource, NativeSessionView, ProviderIdentity,
    ProviderSessionListParams, ProviderSessionListenRequest, ProviderSessionSummary, SessionRef,
};
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt as _, path::Path, time::Duration};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const PROVIDER: &str = r#"#!/usr/bin/python3
import json,sys
def send(value): print(json.dumps(value),flush=True)
request=json.loads(sys.stdin.readline())
assert request['method']=='initialize',request
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'human-approver-fixture','version':'1'}}})
pending_prompt=None
next_session=0
for line in sys.stdin:
 request=json.loads(line)
 method=request.get('method')
 if method=='session/new':
  next_session+=1
  send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'human-approval-session-'+str(next_session)}})
 elif method=='session/prompt':
  pending_prompt=request['id']
  send({'jsonrpc':'2.0','id':91,'method':'session/request_permission','params':{'sessionId':request['params']['sessionId'],'toolCall':{'toolCallId':'permission-tool'},'options':[{'optionId':'allow-once','name':'Allow once','kind':'allow_once'}]}})
 elif request.get('id')==91:
  assert request['result']['outcome']=={'outcome':'selected','optionId':'allow-once'},request
  assert pending_prompt is not None
  send({'jsonrpc':'2.0','id':pending_prompt,'result':{'stopReason':'end_turn'}})
  pending_prompt=None
 else:
  raise AssertionError(request)
"#;

#[tokio::test]
async fn cli_human_approver_from_create_can_decide_provider_permission() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let provider_path = root.path().join("human-approver-provider.py");
    std::fs::write(&provider_path, PROVIDER)?;
    std::fs::set_permissions(&provider_path, std::fs::Permissions::from_mode(0o700))?;
    let runtime = CollaborationRuntime::start_with_external_providers(
        CollaborationRuntimeInputs {
            directory: root.path().to_owned(),
            codex_home: root.path().to_owned(),
            backend_socket: root.path().join("unused-backend.sock"),
            mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            native_schema: None,
            peer_registry_directory: None,
            remote_control_server_name: None,
            owner_human_id: None,
        },
        vec![ExternalProviderStartup::Launch(
            ExternalProviderLaunchBinding::claude(provider_path, vec![])?,
        )],
    )
    .await?;
    let mut observer = ControlClient::connect(root.path(), "human-approver-observer", "1").await?;
    let provider = observer
        .list_endpoints()
        .await?
        .endpoints
        .into_iter()
        .find(|entry| String::from(entry.endpoint.endpoint_id.clone()) == "claude-local")
        .ok_or("Claude fixture endpoint missing")?;
    let creator = json!({
        "endpoint":{"serviceId":provider.endpoint.service_id,"endpointId":"codex-local"},
        "sessionId":"human-approver-cli-caller"
    });
    let human = json!({"kind":"human","humanId":"fixture-owner"});
    let invalid_from = run_cli(
        root.path(),
        &[
            "conversation",
            "create",
            "--endpoint",
            "claude-local",
            "--cwd",
        ],
        &[
            root.path().display().to_string(),
            "--access".into(),
            "workspace-write".into(),
            "--from".into(),
            "null".into(),
            "--json".into(),
        ],
    )
    .await?;
    assert_eq!(invalid_from.status.code(), Some(2));
    let invalid_from: Value = serde_json::from_slice(&invalid_from.stdout)?;
    assert_eq!(invalid_from["error"]["kind"], "invalidField");
    let create = run_cli(
        root.path(),
        &[
            "conversation",
            "create",
            "--endpoint",
            "claude-local",
            "--cwd",
        ],
        &[
            root.path().display().to_string(),
            "--access".into(),
            "workspace-write".into(),
            "--from".into(),
            creator.to_string(),
            "--approver".into(),
            human.to_string(),
            "--json".into(),
        ],
    )
    .await?;
    assert_eq!(
        create.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&create.stdout),
        String::from_utf8_lossy(&create.stderr)
    );
    let created = output_line(&create.stdout, "created")?;
    let target: SessionRef = serde_json::from_value(created["target"].clone())?;
    observer
        .listen_provider_session(ProviderSessionListenRequest {
            target: target.clone(),
        })
        .await?;

    let sent = run_cli(
        root.path(),
        &["message", "send", "--to"],
        &[
            serde_json::to_string(&target)?,
            "--from".into(),
            creator.to_string(),
            "--text".into(),
            "request permission".into(),
            "--json".into(),
        ],
    )
    .await?;
    assert_eq!(
        sent.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&sent.stdout),
        String::from_utf8_lossy(&sent.stderr)
    );
    wait_for_interaction(&mut observer).await?;

    let detailed = run_cli(
        root.path(),
        &[
            "approval",
            "list",
            "--pending",
            "--include-options",
            "--json",
        ],
        &[],
    )
    .await?;
    assert_eq!(detailed.status.code(), Some(0));
    let detailed: Value = serde_json::from_slice(&detailed.stdout)?;
    let row = &detailed["result"]["record"]["approvals"][0];
    assert_eq!(row["approver"], human);
    let request_id = row["requestId"].as_str().ok_or("request ID missing")?;
    let legacy = run_cli(
        root.path(),
        &["approval", "list", "--pending", "--json"],
        &[],
    )
    .await?;
    let legacy: Value = serde_json::from_slice(&legacy.stdout)?;
    assert_eq!(legacy["result"]["record"]["approvals"], json!([]));

    let wrong_actor = json!({"kind":"human","humanId":"someone-else"});
    let wrong = decide(root.path(), request_id, &wrong_actor).await?;
    assert_eq!(wrong.status.code(), Some(4));
    let wrong: Value = serde_json::from_slice(&wrong.stdout)?;
    assert_eq!(wrong["error"]["serviceKind"], "wrongActor", "{wrong}");
    assert_eq!(wrong["error"]["data"]["kind"], "wrongActor");
    let decided = decide(root.path(), request_id, &human).await?;
    assert_eq!(
        decided.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&decided.stdout),
        String::from_utf8_lossy(&decided.stderr)
    );
    wait_for_turn_end(&mut observer).await?;

    let human_created = run_cli(
        root.path(),
        &[
            "conversation",
            "create",
            "--endpoint",
            "claude-local",
            "--cwd",
        ],
        &[
            root.path().display().to_string(),
            "--access".into(),
            "workspace-write".into(),
            "--from".into(),
            human.to_string(),
            "--approver".into(),
            human.to_string(),
            "--json".into(),
        ],
    )
    .await?;
    assert_eq!(
        human_created.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&human_created.stdout),
        String::from_utf8_lossy(&human_created.stderr)
    );
    let human_target: SessionRef =
        serde_json::from_value(output_line(&human_created.stdout, "created")?["target"].clone())?;
    let listed = observer
        .list_provider_sessions(ProviderSessionListParams {
            endpoint: human_target.endpoint.clone(),
            view: NativeSessionView::Stored,
            scope: NativeSessionScope::Any,
            source: NativeSessionSource::All,
            query: None,
            page_size: 10,
            cursor: None,
        })
        .await?;
    let human_record = listed
        .sessions
        .iter()
        .find(|record| record.target() == &human_target)
        .ok_or("Human-created Session not listed")?;
    let ProviderSessionSummary::HostedProvider {
        created_by,
        approver,
        ..
    } = human_record
    else {
        return Err("Human-created Session had an interactive registry shape".into());
    };
    assert!(matches!(created_by, ProviderIdentity::Human { .. }));
    assert!(matches!(approver, ProviderIdentity::Human { .. }));

    let owner_lookup = std::process::Command::new("/usr/bin/id")
        .arg("-un")
        .output()?;
    assert!(owner_lookup.status.success());
    let expected_owner = String::from_utf8(owner_lookup.stdout)?;
    let owner_created = run_cli(
        root.path(),
        &[
            "conversation",
            "create",
            "--endpoint",
            "claude-local",
            "--cwd",
        ],
        &[
            root.path().display().to_string(),
            "--access".into(),
            "workspace-write".into(),
            "--from".into(),
            creator.to_string(),
            "--approver-owner".into(),
            "--json".into(),
        ],
    )
    .await?;
    assert_eq!(
        owner_created.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&owner_created.stdout),
        String::from_utf8_lossy(&owner_created.stderr)
    );
    let owner_target: SessionRef =
        serde_json::from_value(output_line(&owner_created.stdout, "created")?["target"].clone())?;
    let listed = observer
        .list_provider_sessions(ProviderSessionListParams {
            endpoint: owner_target.endpoint.clone(),
            view: NativeSessionView::Stored,
            scope: NativeSessionScope::Any,
            source: NativeSessionSource::All,
            query: None,
            page_size: 10,
            cursor: None,
        })
        .await?;
    let owner_record = listed
        .sessions
        .iter()
        .find(|record| record.target() == &owner_target)
        .ok_or("owner-Approver Session not listed")?;
    let ProviderSessionSummary::HostedProvider { approver, .. } = owner_record else {
        return Err("owner-Approver Session had an interactive registry shape".into());
    };
    assert!(matches!(approver,
        ProviderIdentity::Human { human_id } if human_id.as_str() == expected_owner.trim()
    ));
    runtime.shutdown().await?;
    Ok(())
}

async fn run_cli(
    root: &Path,
    prefix: &[&str],
    suffix: &[String],
) -> TestResult<std::process::Output> {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"));
    command
        .args(prefix)
        .args(suffix)
        .args(["--service-directory"])
        .arg(root);
    tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .map_err(|_| format!("CLI {prefix:?} timed out"))?
        .map_err(Into::into)
}

async fn decide(root: &Path, request_id: &str, actor: &Value) -> TestResult<std::process::Output> {
    run_cli(
        root,
        &[
            "approval",
            "decide",
            "--request-id",
            request_id,
            "--option-id",
            "allow-once",
            "--actor",
        ],
        &[actor.to_string(), "--json".into()],
    )
    .await
}

fn output_line(stdout: &[u8], kind: &str) -> TestResult<Value> {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .find(|row| row["kind"] == kind)
        .ok_or_else(|| format!("CLI omitted {kind}: {}", String::from_utf8_lossy(stdout)).into())
}

async fn wait_for_interaction(observer: &mut ControlClient) -> TestResult {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let notification =
            tokio::time::timeout_at(deadline, observer.next_provider_session_notification())
                .await??;
        if notification["event"]["kind"] == "interactionRequested" {
            return Ok(());
        }
    }
}

async fn wait_for_turn_end(observer: &mut ControlClient) -> TestResult {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let notification =
            tokio::time::timeout_at(deadline, observer.next_provider_session_notification())
                .await??;
        if notification["event"]["kind"] == "turnEnded" {
            assert_eq!(notification["event"]["outcome"]["stop_reason"], "endTurn");
            return Ok(());
        }
    }
}
