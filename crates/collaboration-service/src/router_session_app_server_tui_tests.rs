use super::test_support::ScriptedSessionBackend;
use super::*;
use std::{os::unix::fs::PermissionsExt, sync::Arc};
use tokio_util::sync::CancellationToken;

/// A completed approval in the attached snapshot is historical context, not
/// a new form. Oracle: pinned Codex 0.157.1 TUI server request display.
#[tokio::test]
#[ignore = "requires installed codex-cli 0.157.1 and a pseudo-terminal"]
async fn pinned_codex_tui_reattach_has_no_resolved_approval_form()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::{NativeControlBackend, NativeGenerationGate, ServiceInteractionBroker};
    use session_event_model::{
        ApprovalChoice, ApprovalEffect, ApprovalRequest, ApprovalScope, OfferedOption,
        OfferedOptionId, OfferedOptions, OptionsOrigin, PendingInteraction, SessionEvent,
        SessionItem, SessionItemKind, StopReason, TurnOutcome,
    };
    use std::process::Stdio;
    use tokio::{
        io::AsyncWriteExt,
        net::UnixStream,
        process::Command,
        time::{Duration, Instant, sleep, timeout},
    };
    use tokio_tungstenite::client_async;

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
    let terminal_log = directory.path().join("reattach-terminal.log");
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
        directory.path().join("approvals.json"),
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
        .with_interaction_broker(broker)
        .with_project_trust(Arc::new(
            codex_native_integration::CodexHomeProjectTrust::new(codex_home.clone()),
        )),
    );
    let listener = RouterSessionAppServerListener::bind(&socket_path, context)?;
    let shutdown = CancellationToken::new();
    let running = tokio::spawn(listener.run(shutdown.clone()));

    let stream = UnixStream::connect(&socket_path).await?;
    let (mut client, _) = client_async("ws://localhost/rpc", stream).await?;
    client
        .send(Message::Text(
            json!({"id":1,"method":"thread/start","params":{"cwd":project}})
                .to_string()
                .into(),
        ))
        .await?;
    let started: Value =
        serde_json::from_str(client.next().await.ok_or("thread/start")??.to_text()?)?;
    let thread_id = started["result"]["thread"]["id"]
        .as_str()
        .ok_or("thread id")?
        .to_owned();
    client.close(None).await?;

    let approval = ApprovalRequest {
        request_id: "resolved-before-reattach".into(),
        title: "Historic approval".into(),
        description: None,
        subject: Some(session_event_model::ApprovalSubject::Command {
            command: "pwd".into(),
            cwd: project.display().to_string(),
        }),
        options_origin: OptionsOrigin::AgentOffered,
        options: OfferedOptions::new(vec![OfferedOption {
            option_id: OfferedOptionId::new("allow-once")?,
            label: "Allow once".into(),
            choice: ApprovalChoice::new(ApprovalEffect::Allow, ApprovalScope::Once),
        }])?,
    };
    *backend.history.lock().expect("test lock") = vec![
        crate::HubEvent {
            sequence: 1,
            event: SessionEvent::TurnStarted {
                turn_id: "historical-turn".into(),
                input_id: session_event_model::InputId::generate(),
            },
        },
        crate::HubEvent {
            sequence: 2,
            event: SessionEvent::ItemStarted {
                item: SessionItem {
                    item_id: "history-prompt".into(),
                    kind: SessionItemKind::UserMessage,
                    text: Some("Recall the completed answer".into()),
                },
            },
        },
        crate::HubEvent {
            sequence: 3,
            event: SessionEvent::ItemCompleted {
                item_id: "history-prompt".into(),
            },
        },
        crate::HubEvent {
            sequence: 4,
            event: SessionEvent::ItemStarted {
                item: SessionItem {
                    item_id: "history-marker".into(),
                    kind: SessionItemKind::AgentMessage,
                    text: Some("PR4_RESOLVED_HISTORY_MARKER".into()),
                },
            },
        },
        crate::HubEvent {
            sequence: 5,
            event: SessionEvent::ItemCompleted {
                item_id: "history-marker".into(),
            },
        },
        crate::HubEvent {
            sequence: 6,
            event: SessionEvent::InteractionRequested {
                interaction: PendingInteraction::Approval {
                    approver: actor,
                    request: Box::new(approval),
                },
            },
        },
        crate::HubEvent {
            sequence: 7,
            event: SessionEvent::InteractionResolved {
                request_id: "resolved-before-reattach".into(),
            },
        },
        crate::HubEvent {
            sequence: 8,
            event: SessionEvent::TurnEnded {
                turn_id: "historical-turn".into(),
                outcome: TurnOutcome::Ended {
                    stop_reason: StopReason::EndTurn,
                    local_cause: None,
                },
            },
        },
    ];
    let mut tui = Command::new("script")
        .arg("-q")
        .arg(&terminal_log)
        .arg("/bin/sh")
        .arg("-c")
        .arg("stty cols 100 rows 30; exec \"$@\"")
        .arg("sh")
        .arg("codex")
        .arg("resume")
        .arg("--remote")
        .arg(format!("unix://{}", socket_path.display()))
        .arg("--no-alt-screen")
        .arg("--cd")
        .arg(&project)
        .arg(&thread_id)
        .current_dir(&project)
        .env("CODEX_HOME", &codex_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut history_visible = false;
    loop {
        if std::fs::read_to_string(&terminal_log)
            .is_ok_and(|terminal| terminal.contains("PR4_RESOLVED_HISTORY_MARKER"))
        {
            history_visible = true;
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    sleep(Duration::from_millis(1_500)).await;
    if let Some(mut stdin) = tui.stdin.take() {
        let _ = stdin.write_all(b"\x03\x03").await;
    }
    if timeout(Duration::from_secs(3), tui.wait()).await.is_err() {
        let _ = tui.kill().await;
    }
    shutdown.cancel();
    running.await??;
    let terminal = std::fs::read_to_string(&terminal_log)?;
    let tail = terminal
        .chars()
        .rev()
        .take(500)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    assert!(
        history_visible,
        "pinned TUI did not display completed history: {tail:?}"
    );
    assert!(
        !terminal.contains("required unanswered") && !terminal.contains("Historic approval"),
        "resolved approval resurfaced as an unanswered TUI form"
    );
    Ok(())
}

/// Oracle: pinned Codex app-server-protocol v2/turn.rs:32-39,506-528 and
/// v2/item.rs:236-429. A real TUI must render the projected live Item.
#[tokio::test]
#[ignore = "requires installed codex-cli 0.157.1 and a pseudo-terminal"]
async fn pinned_codex_tui_renders_a_provider_reply_and_completed_turn()
-> Result<(), Box<dyn std::error::Error>> {
    use session_event_model::{
        SessionEvent, SessionItem, SessionItemKind, StopReason, TurnOutcome,
    };
    use std::process::Stdio;
    use tokio::{
        io::AsyncWriteExt,
        process::Command,
        time::{Duration, Instant, sleep, timeout},
    };

    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket_path = directory.path().join("claude.sock");
    let project = directory.path().join("project");
    let codex_home = directory.path().join("codex-home");
    let terminal_log = directory.path().join("terminal.log");
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
    let context = Arc::new(
        RouterSessionAppServerContext::new(
            backend.endpoint.clone(),
            ScriptedSessionBackend::actor()?,
            Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
            Arc::clone(&backend) as Arc<dyn SessionEventHub>,
            tokio::sync::watch::channel(Vec::new()).1,
        )
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
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    timeout(Duration::from_secs(20), backend.created.notified()).await?;
    sleep(Duration::from_secs(2)).await;
    if let Some(stdin) = tui.stdin.as_mut() {
        stdin.write_all(b"hello").await?;
    }
    sleep(Duration::from_millis(500)).await;
    if let Some(stdin) = tui.stdin.as_mut() {
        stdin.write_all(b"\r").await?;
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while backend
        .prompt_commands
        .lock()
        .expect("test lock")
        .is_empty()
        && Instant::now() < deadline
    {
        sleep(Duration::from_millis(50)).await;
    }
    let submitted = !backend
        .prompt_commands
        .lock()
        .expect("test lock")
        .is_empty();
    if submitted {
        sleep(Duration::from_millis(200)).await;
        let _sent = backend.events.send(HubEvent {
            sequence: 2,
            event: SessionEvent::ItemStarted {
                item: SessionItem {
                    item_id: "reply-1".into(),
                    kind: SessionItemKind::AgentMessage,
                    text: Some("PR4_TUI_REPLY_MARKER".into()),
                },
            },
        });
        let _sent = backend.events.send(HubEvent {
            sequence: 3,
            event: SessionEvent::ItemCompleted {
                item_id: "reply-1".into(),
            },
        });
        let _sent = backend.events.send(HubEvent {
            sequence: 4,
            event: SessionEvent::TurnEnded {
                turn_id: "turn-1".into(),
                outcome: TurnOutcome::Ended {
                    stop_reason: StopReason::EndTurn,
                    local_cause: None,
                },
            },
        });
    }
    sleep(Duration::from_secs(1)).await;
    if let Some(mut stdin) = tui.stdin.take() {
        let _ = stdin.write_all(b"\x03\x03").await;
    }
    if timeout(Duration::from_secs(3), tui.wait()).await.is_err() {
        let _ = tui.kill().await;
    }
    shutdown.cancel();
    running.await??;
    let terminal = std::fs::read_to_string(&terminal_log)?;
    assert!(
        submitted,
        "pinned TUI did not call turn/start; terminal markers: welcome={}, working={}, error={}, inputEcho={}",
        terminal.contains("OpenAI Codex"),
        terminal.contains("Working"),
        terminal.contains("error"),
        terminal.contains("hello")
    );
    assert!(
        terminal.contains("PR4_TUI_REPLY_MARKER"),
        "pinned TUI did not render the provider Item"
    );
    Ok(())
}

/// The real Codex TUI version is intentionally pinned because its app-server
/// v2 response parser has no protocol version negotiation.
#[tokio::test]
#[ignore = "requires installed codex-cli 0.157.1 and a pseudo-terminal"]
async fn pinned_codex_tui_boots_and_starts_a_provider_thread()
-> Result<(), Box<dyn std::error::Error>> {
    use std::process::Stdio;
    use tokio::{
        io::AsyncWriteExt,
        process::Command,
        time::{Duration, timeout},
    };

    let version = Command::new("codex").arg("--version").output().await?;
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout)?.trim(),
        "codex-cli 0.157.1"
    );

    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket_path = directory.path().join("claude.sock");
    let project_directory = directory.path().join("project");
    let codex_home = directory.path().join("codex-home");
    std::fs::create_dir(&project_directory)?;
    std::fs::create_dir(&codex_home)?;
    std::fs::write(
        codex_home.join("config.toml"),
        format!(
            "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            project_directory.display()
        ),
    )?;
    let backend = ScriptedSessionBackend::new()?;
    let context = Arc::new(
        RouterSessionAppServerContext::new(
            backend.endpoint.clone(),
            ScriptedSessionBackend::actor()?,
            Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
            Arc::clone(&backend) as Arc<dyn SessionEventHub>,
            tokio::sync::watch::channel(Vec::new()).1,
        )
        .with_project_trust(Arc::new(
            codex_native_integration::CodexHomeProjectTrust::new(codex_home.clone()),
        )),
    );
    let listener = RouterSessionAppServerListener::bind(&socket_path, context)?;
    let shutdown = CancellationToken::new();
    let running = tokio::spawn(listener.run(shutdown.clone()));
    let mut tui = Command::new("script")
        .arg("-q")
        .arg("/dev/null")
        .arg("codex")
        .arg("--remote")
        .arg(format!("unix://{}", socket_path.display()))
        .arg("--no-alt-screen")
        .arg("--cd")
        .arg(&project_directory)
        .current_dir(&project_directory)
        .env("CODEX_HOME", &codex_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let boot_result = timeout(Duration::from_secs(20), backend.created.notified()).await;
    if let Some(mut stdin) = tui.stdin.take() {
        let _ = stdin.write_all(b"\x03\x03").await;
    }
    if timeout(Duration::from_secs(3), tui.wait()).await.is_err() {
        let _ = tui.kill().await;
    }
    shutdown.cancel();
    running.await??;
    boot_result.map_err(|_| "Codex TUI did not call thread/start")?;
    assert_eq!(backend.create_commands.lock().expect("test lock").len(), 1);
    assert_eq!(
        backend.create_commands.lock().expect("test lock")[0].working_directory,
        project_directory
    );
    Ok(())
}

/// Oracle: pinned Codex tui/src/config_update.rs:225-375 and
/// tui/src/onboarding/onboarding_screen.rs:687-725. A failed trust write keeps
/// the TUI prompt open and displays its error.
#[tokio::test]
#[ignore = "requires installed codex-cli 0.157.1 and a pseudo-terminal"]
async fn pinned_codex_tui_shows_untrusted_project_and_refuses_persistence()
-> Result<(), Box<dyn std::error::Error>> {
    use std::process::Stdio;
    use tokio::{
        io::AsyncWriteExt,
        process::Command,
        time::{Duration, sleep, timeout},
    };

    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket_path = directory.path().join("claude.sock");
    let project = directory.path().join("project");
    let codex_home = directory.path().join("codex-home");
    let terminal_log = directory.path().join("terminal.log");
    std::fs::create_dir(&project)?;
    std::fs::create_dir(&codex_home)?;
    let backend = ScriptedSessionBackend::new()?;
    let context = Arc::new(
        RouterSessionAppServerContext::new(
            backend.endpoint.clone(),
            ScriptedSessionBackend::actor()?,
            Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
            Arc::clone(&backend) as Arc<dyn SessionEventHub>,
            tokio::sync::watch::channel(Vec::new()).1,
        )
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
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    sleep(Duration::from_secs(2)).await;
    if let Some(stdin) = tui.stdin.as_mut() {
        stdin.write_all(b"\r").await?;
    }
    sleep(Duration::from_secs(2)).await;
    if let Some(mut stdin) = tui.stdin.take() {
        let _ = stdin.write_all(b"\x03\x03").await;
    }
    if timeout(Duration::from_secs(3), tui.wait()).await.is_err() {
        let _ = tui.kill().await;
    }
    shutdown.cancel();
    running.await??;
    let after = std::fs::read_to_string(&terminal_log).unwrap_or_default();
    // The TUI interleaves terminal controls through the disclosure sentence.
    assert!(after.contains("Trust"), "TUI did not show trust prompt");
    assert!(
        after.contains("trust this project in Codex itself"),
        "TUI did not display refusal: {}",
        after.chars().take(1000).collect::<String>()
    );
    assert!(
        backend
            .create_commands
            .lock()
            .expect("test lock")
            .is_empty()
    );
    assert!(
        !std::fs::read_to_string(codex_home.join("config.toml"))
            .unwrap_or_default()
            .contains("trust_level")
    );
    Ok(())
}
