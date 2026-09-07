//! Model-driven CLI collaboration; the harness observes but never forwards the answer.
use codex_native_integration::NativeProtocolConnection;
use communication_protocol::SessionRef;
use serde_json::Value;
use std::{error::Error, path::Path, time::Duration};

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
pub fn initial_task(
    executable: &Path,
    directory: &Path,
    first: &SessionRef,
    second: &SessionRef,
    denied: &mut DeniedTargets,
) -> Result<String, Box<dyn Error>> {
    let executable = quote(executable.to_str().ok_or("CLI path not UTF-8")?);
    let directory = quote(directory.to_str().ok_or("service directory not UTF-8")?);
    let first = quote(&serde_json::to_string(first)?);
    let second = quote(&serde_json::to_string(second)?);
    let reply_prefix = format!(
        "{executable} message send --service-directory {directory} --from {second} --to {first} --json --text"
    );
    let task_b = format!(
        "You are agent B in an authorized debug-only communication test. Compute 6 multiplied by 7. Use your shell tool to send the result back to agent A using this exact CLI prefix: {reply_prefix} followed by a shell-quoted message containing AGENT_RESULT_42 if that is the correct result. The CLI automatically sends during active work or starts idle work. Execute the reply command exactly once. Do not edit files, inspect accounts, use other models or operate on any other threads. If the command fails report its actual error; do not retry or change permissions. After successful sending, finish with B_REPLY_SENT."
    );
    let task_file = denied.write_task(&task_b)?;
    let send = format!(
        "{executable} message send --service-directory {directory} --from {first} --to {second} --json --text-file {}",
        quote(task_file.to_str().ok_or("task path is not UTF-8")?)
    );
    let denied_command = denied.command()?;
    Ok(format!(
        "First run this exact access check with your shell tool: {denied_command}. It must print NARROW_ACCESS_DENIED. If it fails, stop without delegating. You are agent A in an authorized debug-only communication test. Use your shell tool to run this exact command once to delegate a calculation to B:\n{send}\nDo not edit files, inspect accounts, use other models or operate on other threads. If the command fails report its actual error; do not retry or change permissions. After successful submission you may finish with A_DISPATCHED. When B's reply arrives, check its calculation and finish with exactly AGENT_RESULT_42 if correct. Do not invent B's response or send another message."
    ))
}

pub async fn observe(
    first: &mut NativeProtocolConnection,
    second: &mut NativeProtocolConnection,
    first_id: &str,
    second_id: &str,
) -> Result<(), Box<dyn Error>> {
    let mut first_tool = false;
    let mut second_tool = false;
    let mut answer = false;
    let mut denied_checked = false;
    tokio::time::timeout(Duration::from_secs(300), async {
        loop {
            let (event, is_first) = tokio::select! {
                event = first.next_message() => (event?, true),
                event = second.next_message() => (event?, false),
            };
            let Some(params) = event.get("params") else { continue; };
            let expected = if is_first { first_id } else { second_id };
            if params.get("threadId").and_then(Value::as_str) != Some(expected) { continue; }
            if event.get("id").is_some() { return Err::<(),Box<dyn Error>>("agent proof requested approval; none granted".into()); }
            if event.get("method").and_then(Value::as_str) == Some("item/completed") {
                let item = params.get("item").ok_or("missing item")?;
                if item.get("type").and_then(Value::as_str) == Some("commandExecution") {
                    let command = item.get("command").and_then(Value::as_str).unwrap_or_default();
                    let successful = item.get("exitCode").and_then(Value::as_i64) == Some(0);
                    if is_first && command.contains("NARROW_ACCESS_DENIED") {
                        let output = item.get("aggregatedOutput").and_then(Value::as_str).unwrap_or_default();
                        if !successful || !output.contains("NARROW_ACCESS_DENIED") { return Err("narrow access denial check failed".into()); }
                        denied_checked = true;
                        println!("{}",serde_json::json!({"kind":"unrelatedConnectionsDenied"}));
                    }
                    if command.contains("message send") && successful {
                        if is_first { first_tool = true; } else { second_tool = true; }
                        println!("{}",serde_json::json!({"kind":"agentCliToolCompleted","agent":if is_first {"A"}else{"B"},"exitCode":0}));
                    } else if command.contains("message send") {
                        println!("{}",serde_json::json!({"kind":"agentCliToolFailed","agent":if is_first {"A"}else{"B"},"exitCode":item.get("exitCode")}));
                        return Err("agent CLI execution failed; no retry".into());
                    }
                }
                if is_first && item.get("type").and_then(Value::as_str) == Some("agentMessage")
                    && item.get("text").and_then(Value::as_str).is_some_and(|text| text.trim() == "AGENT_RESULT_42") {
                    answer = true;
                }
            }
            if denied_checked && first_tool && second_tool && answer {
                println!("{}",serde_json::json!({"kind":"agentCliCollaborationPassed","proof":"two Luna agents executed public CLI sends and A checked returned calculation"}));
                return Ok(());
            }
        }
    }).await??;
    Ok(())
}

/// Both targets are live, owned fixtures; denial cannot pass merely because no server exists.
pub struct DeniedTargets {
    socket: std::path::PathBuf,
    task_file: std::path::PathBuf,
    task_created: bool,
    _unix: std::os::unix::net::UnixListener,
    tcp: std::net::TcpListener,
}
impl DeniedTargets {
    pub fn bind() -> Result<Self, Box<dyn Error>> {
        static FIXTURE_SEQUENCE: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(0);
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let socket = std::path::PathBuf::from(format!(
            "/tmp/denied-agent-socket-{}-{sequence}",
            std::process::id()
        ));
        let unix = std::os::unix::net::UnixListener::bind(&socket)?;
        let tcp = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        Ok(Self {
            task_file: socket.with_extension("task.txt"),
            task_created: false,
            socket,
            _unix: unix,
            tcp,
        })
    }
    fn write_task(&mut self, task: &str) -> Result<&Path, Box<dyn Error>> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        // The model reads a fixture through the documented --text-file surface;
        // it never has to reproduce shell escaping inside another agent's task.
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&self.task_file)?;
        self.task_created = true;
        file.write_all(task.as_bytes())?;
        Ok(&self.task_file)
    }
    fn command(&self) -> Result<String, Box<dyn Error>> {
        let script = format!(
            "import socket,errno\nfor family,address in [(socket.AF_UNIX,{}),(socket.AF_INET,(\"127.0.0.1\",{}))]:\n s=socket.socket(family,socket.SOCK_STREAM);s.settimeout(2)\n try:\n  s.connect(address)\n except OSError as e:\n  assert e.errno in (errno.EPERM,errno.EACCES), \"connection did not fail by permission\"\n else:\n  raise RuntimeError(\"unrelated connection allowed\")\n finally:\n  s.close()\nprint(\"NARROW_ACCESS_DENIED\")",
            serde_json::to_string(self.socket.to_str().ok_or("socket path")?)?,
            self.tcp.local_addr()?.port()
        );
        Ok(format!("python3 -c {}", quote(&script)))
    }
}
impl Drop for DeniedTargets {
    fn drop(&mut self) {
        let _cleanup = std::fs::remove_file(&self.socket);
        if self.task_created {
            let _task_cleanup = std::fs::remove_file(&self.task_file);
        }
    }
}

#[cfg(test)]
mod task_fixture_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn task_fixture_preserves_nested_content_and_removes_only_its_own_file() {
        let mut targets = DeniedTargets::bind().unwrap();
        let task = "Send 'quoted' content with $variables, `commands`, and\nnewlines.";
        let path = targets.write_task(task).unwrap().to_owned();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), task);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(targets);
        assert!(!path.exists());
    }

    #[test]
    fn existing_task_file_is_never_overwritten_or_removed() {
        let mut targets = DeniedTargets::bind().unwrap();
        let path = targets.task_file.clone();
        std::fs::write(&path, "preexisting fixture").unwrap();
        assert!(targets.write_task("replacement").is_err());
        drop(targets);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "preexisting fixture"
        );
        std::fs::remove_file(path).unwrap();
    }
}
