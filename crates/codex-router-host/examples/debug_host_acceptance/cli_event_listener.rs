//! Owned executable observation, with readiness before any proof submission.
use serde_json::Value;
use std::{error::Error, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader, Lines},
    process::{Child, ChildStdout, Command},
};

pub struct CliEventListener {
    child: Child,
    lines: Lines<BufReader<ChildStdout>>,
    thread: String,
    generation: Value,
}
impl CliEventListener {
    pub async fn attach(
        executable: &Path,
        directory: &Path,
        thread: &str,
    ) -> Result<Self, Box<dyn Error>> {
        let mut child = Command::new(executable)
            .args([
                "events",
                "listen",
                "--endpoint",
                "codex-local",
                "--session",
                thread,
                "--attach",
                "--timeout-seconds",
                "180",
                "--service-directory",
            ])
            .arg(directory)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let stdout = child.stdout.take().ok_or("listener stdout unavailable")?;
        let mut lines = BufReader::new(stdout).lines();
        let first = tokio::time::timeout(Duration::from_secs(35), lines.next_line())
            .await??
            .ok_or("listener exited before readiness")?;
        let ready: Value = serde_json::from_str(&first)?;
        if ready.get("kind").and_then(Value::as_str) != Some("listenerReady")
            || ready.pointer("/target/sessionId").and_then(Value::as_str) != Some(thread)
        {
            return Err("CLI listener did not attach the exact owned thread".into());
        }
        let generation = ready
            .get("generation")
            .ok_or("listener generation missing")?
            .clone();
        println!("{ready}");
        Ok(Self {
            child,
            lines,
            thread: thread.to_owned(),
            generation,
        })
    }
    pub fn generation(&self) -> &Value {
        &self.generation
    }
    pub async fn wait_for(
        &mut self,
        matches: impl Fn(&Value) -> bool,
    ) -> Result<Value, Box<dyn Error>> {
        tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                let line = self
                    .lines
                    .next_line()
                    .await?
                    .ok_or("owned listener closed before expected event")?;
                if line.len() > 1024 * 1024 {
                    return Err("proof event exceeded bound".into());
                }
                let record: Value = serde_json::from_str(&line)?;
                if record.get("kind").and_then(Value::as_str) != Some("nativeMessage")
                    || record.get("generation") != Some(&self.generation)
                    || record.pointer("/target/sessionId").and_then(Value::as_str)
                        != Some(self.thread.as_str())
                {
                    return Err("listener lost its scoped observation".into());
                }
                let message = record.get("message").ok_or("missing native event")?;
                if message.get("method").is_some() && message.get("id").is_some() {
                    return Err("delivery proof encountered a callback; no approval granted".into());
                }
                if matches(message) {
                    return Ok(message.clone());
                }
            }
        })
        .await?
    }
    pub async fn close(mut self) -> Result<(), Box<dyn Error>> {
        if self.child.try_wait()?.is_none() {
            self.child.kill().await?;
        }
        self.child.wait().await?;
        Ok(())
    }
}

pub async fn run_json_command(
    executable: &Path,
    arguments: &[String],
) -> Result<Value, Box<dyn Error>> {
    let output = tokio::time::timeout(
        Duration::from_secs(40),
        Command::new(executable)
            .args(arguments)
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    if output.stdout.len() > 1024 * 1024 || output.stderr.len() > 65536 {
        return Err("proof CLI output exceeded bound".into());
    }
    let value: Value = serde_json::from_slice(&output.stdout)?;
    if !output.status.success() {
        println!(
            "{}",
            serde_json::json!({"kind":"deliveryCliRejected","exit":output.status.code(),"record":value})
        );
        return Err("delivery CLI command failed; no retry".into());
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| "CLI result absent".into())
}
