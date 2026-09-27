use super::test_support::ScriptedSessionBackend;
use super::*;
use crate::{NativeControlBackend, NativeGenerationGate, ServiceInteractionBroker};
use session_event_model::{PendingInteraction, QuestionRequest, SessionEvent};
use std::{
    os::unix::fs::PermissionsExt,
    process::Stdio,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::{Duration, Instant, sleep, timeout},
};
use tokio_util::sync::CancellationToken;

/// Oracle: pinned Codex 0.157.1 tui/bottom_pane/mcp_server_elicitation.rs:617-619.
/// MultiSelect cannot be answered in this TUI, so it remains pending elsewhere
/// and appears as a completed read-only notice here.
#[tokio::test]
#[ignore = "requires installed codex-cli 0.157.1 and a pseudo-terminal"]
async fn pinned_codex_tui_shows_unsupported_multichoice_question()
-> Result<(), Box<dyn std::error::Error>> {
    let version = Command::new("codex").arg("--version").output().await?;
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout)?.trim(),
        "codex-cli 0.157.1"
    );
    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket_path = directory.path().join("claude.sock");
    let project = directory.path().join("project");
    let codex_home = directory.path().join("codex-home");
    let terminal_log = directory.path().join("multichoice-terminal.log");
    std::fs::create_dir(&project)?;
    std::fs::create_dir(&codex_home)?;
    std::fs::write(
        codex_home.join("config.toml"),
        format!(
            "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            project.display()
        ),
    )?;
    let backend = ScriptedSessionBackend::new()?;
    let actor = ScriptedSessionBackend::actor()?;
    let native_endpoint = serde_json::from_value(json!({
        "serviceId":backend.endpoint.service_id,
        "endpointId":"claude-local"
    }))?;
    let broker = ServiceInteractionBroker::load(
        backend.endpoint.service_id.as_str().to_owned().try_into()?,
        NativeControlBackend {
            endpoint: native_endpoint,
            gate: NativeGenerationGate::default(),
            codex_home: codex_home.clone(),
        },
        directory.path().join("questions.json"),
    )
    .await?;
    let context = Arc::new(
        RouterSessionAppServerContext::new(
            backend.endpoint.clone(),
            actor.clone(),
            Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
            Arc::clone(&backend) as Arc<dyn SessionEventHub>,
            tokio::sync::watch::channel(Vec::new()).1,
        )
        .with_interaction_broker(Arc::clone(&broker))
        .with_project_trust(Arc::new(
            codex_native_integration::CodexHomeProjectTrust::new(codex_home.clone()),
        )),
    );
    let listener = RouterSessionAppServerListener::bind(&socket_path, context)?;
    let shutdown = CancellationToken::new();
    let running = tokio::spawn(listener.run(shutdown.clone()));
    let mut tui = Command::new("script")
        .arg("-q")
        .arg(&terminal_log)
        .arg("/bin/sh")
        .arg("-c")
        .arg("stty cols 100 rows 30; exec \"$@\"")
        .arg("sh")
        .arg("codex")
        .arg("--remote")
        .arg(format!("unix://{}", socket_path.display()))
        .arg("--no-alt-screen")
        .arg("--cd")
        .arg(&project)
        .current_dir(&project)
        .env("CODEX_HOME", &codex_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let output = Arc::new(Mutex::new(Vec::<u8>::new()));
    let mut tui_stdout = tui.stdout.take().ok_or("TUI stdout")?;
    let captured_output = Arc::clone(&output);
    let output_reader = tokio::spawn(async move {
        let mut chunk = [0_u8; 4096];
        loop {
            let read = tui_stdout.read(&mut chunk).await?;
            if read == 0 {
                break;
            }
            captured_output
                .lock()
                .expect("output lock")
                .extend_from_slice(&chunk[..read]);
        }
        Ok::<(), std::io::Error>(())
    });
    timeout(Duration::from_secs(20), backend.created.notified()).await?;
    sleep(Duration::from_secs(1)).await;
    if let Some(stdin) = tui.stdin.as_mut() {
        stdin.write_all(b"hello").await?;
    }
    sleep(Duration::from_millis(300)).await;
    if let Some(stdin) = tui.stdin.as_mut() {
        stdin.write_all(b"\r").await?;
    }
    let prompt_deadline = Instant::now() + Duration::from_secs(10);
    while backend
        .prompt_commands
        .lock()
        .expect("test lock")
        .is_empty()
        && Instant::now() < prompt_deadline
    {
        sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !backend
            .prompt_commands
            .lock()
            .expect("test lock")
            .is_empty(),
        "pinned TUI did not start a provider Turn"
    );
    sleep(Duration::from_millis(300)).await;
    let question: QuestionRequest = serde_json::from_value(json!({
        "requestId":"multichoice-tui","prompt":"Pick project checks","fields":[
            {"kind":"multiChoice","fieldId":"checks","label":"Checks","description":null,
             "required":true,"min":1,"max":2,"options":[
                {"optionId":"lint","label":"Run lint"},
                {"optionId":"tests","label":"Run tests"}
             ]}
        ]
    }))?;
    let _agent_reply = broker
        .request_question(
            backend.session.clone(),
            actor.clone(),
            question.clone(),
            None,
        )
        .await?;
    assert!(
        backend.events.receiver_count() > 0,
        "TUI event stream detached"
    );
    let sent = backend.events.send(crate::HubEvent {
        sequence: 2,
        event: SessionEvent::InteractionRequested {
            interaction: PendingInteraction::Question {
                approver: actor,
                request: Box::new(question),
            },
        },
    });
    assert!(sent.is_ok(), "multi-choice event had no TUI receiver");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut rendered = false;
    while Instant::now() < deadline {
        if String::from_utf8_lossy(&output.lock().expect("output lock")).contains("unsupportedHere")
        {
            rendered = true;
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    if let Some(mut stdin) = tui.stdin.take() {
        let _ = stdin.write_all(b"\x03\x03").await;
    }
    if timeout(Duration::from_secs(3), tui.wait()).await.is_err() {
        let _ = tui.kill().await;
    }
    shutdown.cancel();
    running.await??;
    output_reader.await??;
    let terminal = String::from_utf8(output.lock().expect("output lock").clone())?;
    let tail = terminal
        .chars()
        .rev()
        .take(1_500)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    assert!(
        rendered,
        "pinned TUI did not show unsupportedHere notice while the Turn was active: pending={}, unsupported={}, answer={}, tail={tail:?}",
        terminal.contains("Question pending"),
        terminal.contains("unsupported"),
        terminal.contains("Answer with")
    );
    assert!(
        broker
            .list_questions(true)
            .await
            .iter()
            .any(|record| record.request_id() == "multichoice-tui")
    );
    Ok(())
}
