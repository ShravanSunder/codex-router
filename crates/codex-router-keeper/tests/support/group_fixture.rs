use codex_router_descriptor_boundary::DescriptorGate;
use serde::{Deserialize, Serialize};
use std::{io::Write, process::Stdio};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{ChildStdout, Command},
    signal::unix::{SignalKind, signal},
    time::{Duration, timeout},
};
pub type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
#[derive(Serialize, Deserialize)]
pub struct FixtureReady {
    pub pid: i32,
    pub group: i32,
    pub descendant: Option<i32>,
}
pub fn ready_command(mode: &str) -> Result<Command, Box<dyn std::error::Error + Send + Sync>> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args([
            "--ignored",
            "--exact",
            "group_fixture_entrypoint",
            "--nocapture",
        ])
        .env("GROUP_FIXTURE_MODE", mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    Ok(command)
}
pub async fn read_ready(
    output: &mut ChildStdout,
) -> Result<FixtureReady, Box<dyn std::error::Error + Send + Sync>> {
    timeout(Duration::from_secs(3), async {
        let mut lines = BufReader::new(output).lines();
        while let Some(line) = lines.next_line().await? {
            if let Some(body) = line.strip_prefix("READY ") {
                return Ok(serde_json::from_str(body)?);
            }
        }
        Err("fixture ended without READY".into())
    })
    .await?
}
pub async fn run_fixture() -> TestResult {
    // Signal handler installed before descendant launch and before READY.
    let mut term = signal(SignalKind::terminate())?;
    let mode = std::env::var("GROUP_FIXTURE_MODE")?;
    let descendant = if mode == "parent-exits" || mode == "family" {
        let mut command = ready_command(if mode == "parent-exits" {
            "ignore"
        } else {
            "cooperative"
        })?;
        // Descendants inherit this fixture's existing group; they do not become new leaders.
        let mut child = DescriptorGate::global().spawn_child(&mut command).await?;
        let mut out = child.stdout.take().ok_or("descendant output absent")?;
        let ready = read_ready(&mut out).await?;
        Some((child, ready, out))
    } else {
        None
    };
    let ready = FixtureReady {
        pid: rustix::process::getpid().as_raw_pid(),
        group: rustix::process::getpgrp().as_raw_pid(),
        descendant: descendant.as_ref().map(|(_, ready, _)| ready.pid),
    };
    println!("READY {}", serde_json::to_string(&ready)?);
    std::io::stdout().flush()?;
    match mode.as_str() {
        "cooperative" | "family" => {
            term.recv().await.ok_or("TERM stream ended")?;
            if let Some((mut child, _, _out)) = descendant {
                child.wait().await?;
            }
        }
        "parent-exits" => {
            let mut input = [0; 1];
            stdin_reader().await?.read_exact(&mut input).await?;
            if &input != b"E" {
                return Err("exit marker changed".into());
            }
            drop(descendant);
        }
        "eof" => {
            let mut input = [0; 1];
            if stdin_reader().await?.read(&mut input).await? != 0 {
                return Err("expected EOF".into());
            }
        }
        "ignore" => std::future::pending::<()>().await,
        _ => return Err("unknown fixture mode".into()),
    }
    Ok(())
}

async fn stdin_reader()
-> Result<codex_router_descriptor_boundary::PipeReader, Box<dyn std::error::Error + Send + Sync>> {
    let gate = DescriptorGate::global();
    Ok(codex_router_descriptor_boundary::PipeReader::from_owned(
        gate.duplicate(rustix::stdio::stdin()).await?,
        gate,
    )
    .await?)
}
