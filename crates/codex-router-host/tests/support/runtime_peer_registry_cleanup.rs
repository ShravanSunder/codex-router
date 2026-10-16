//! Await every owned fixture lifetime and retain cleanup failures independently of scenario errors.
use super::*;

impl RegistryCase {
    pub(super) async fn finish(&mut self) -> TestResult {
        let mut failures = Vec::new();
        let host_was_spawned = self.host.is_some();
        if let Some(mut child) = self.host.take() {
            let host_pid = child.id();
            match child.try_wait() {
                Ok(Some(_)) => failures.push("Host exited before owned signal shutdown".into()),
                Ok(None) => {
                    if let Some(pid) = child.id()
                        && let Err(error) = signal_owned(pid, rustix::process::Signal::TERM)
                    {
                        failures.push(format!("Host signal: {error}"));
                    }
                }
                Err(error) => failures.push(format!("Host status: {error}")),
            }
            match tokio::time::timeout(WAIT, child.wait()).await {
                Ok(Ok(status)) if status.success() => {}
                Ok(Ok(_)) => failures.push("Host signal shutdown was not successful".into()),
                Ok(Err(error)) => failures.push(format!("Host reap: {error}")),
                Err(_) => {
                    failures.push("Host graceful shutdown timed out".into());
                    if let Err(error) = child.kill().await {
                        failures.push(format!("Host forced reap: {error}"));
                    }
                }
            }
            if let Some(pid) = host_pid
                && let Err(error) = require_process_absent(pid)
            {
                failures.push(format!("Host process absence: {error}"));
            }
        }
        if let Some(peer) = self.peer.take() {
            peer.stop.cancel();
            match peer.task.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failures.push(format!("peer observer: {error}")),
                Err(error) => failures.push(format!("peer observer join: {error}")),
            }
            check_cleanup(
                peer.received.is_empty(),
                "unexpected extra peer frame",
                &mut failures,
            );
        }
        if let Some(router) = self.router.take()
            && let Err(error) = router.finish().await
        {
            failures.push(format!("health fixture join: {error}"));
        }
        let root = self.root.path();
        match recorded_pids(&root.join("native-pids.log")) {
            Ok(pids) => {
                for pid in pids {
                    if let Err(error) = require_process_absent(pid) {
                        failures.push(format!("native child still present: {error}"));
                        if let Err(error) = signal_owned(pid, rustix::process::Signal::TERM) {
                            failures.push(format!("native fallback signal: {error}"));
                        }
                        if let Err(error) = await_process_absent(pid).await {
                            failures.push(format!("native fallback settlement: {error}"));
                            if let Err(error) = signal_owned(pid, rustix::process::Signal::KILL) {
                                failures.push(format!("native forced signal: {error}"));
                            }
                            if let Err(error) = await_process_absent(pid).await {
                                failures.push(format!("native forced settlement: {error}"));
                            }
                        }
                    }
                }
            }
            Err(error) => failures.push(format!("native PID accounting: {error}")),
        }
        let registry = root.join("peer-registry");
        match std::fs::read_dir(&registry) {
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Ok(entry) => {
                            if let Err(error) = std::fs::remove_file(entry.path()) {
                                failures.push(format!("peer artifact cleanup: {error}"));
                            }
                        }
                        Err(error) => failures.push(format!("peer artifact inventory: {error}")),
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => failures.push(format!("peer artifact inventory: {error}")),
        }
        for socket in [
            root.join("operator.sock"),
            root.join("native.sock"),
            root.join("api/control.sock"),
            root.join("api/codex-native.sock"),
            root.join("api/codex-acp.sock"),
            registry.join("peer.sock"),
            root.join("api/service.json"),
        ] {
            if let Err(error) = require_path_absent(&socket) {
                failures.push(format!("owned publication: {error}"));
            }
        }
        let lock = root.join("host.lock");
        if host_was_spawned {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(lock)
                .and_then(|file| file.try_lock().map_err(std::io::Error::other))
            {
                Ok(()) => {}
                Err(error) => failures.push(format!("Host lock not released: {error}")),
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(std::io::Error::other(failures.join("; ")).into())
        }
    }
}

fn signal_owned(pid: u32, signal: rustix::process::Signal) -> TestResult {
    let pid = rustix::process::Pid::from_raw(i32::try_from(pid)?).ok_or("invalid owned PID")?;
    match rustix::process::kill_process(pid, signal) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn require_process_absent(pid: u32) -> TestResult {
    let pid = rustix::process::Pid::from_raw(i32::try_from(pid)?).ok_or("invalid owned PID")?;
    match rustix::process::test_kill_process(pid) {
        Err(rustix::io::Errno::SRCH) => Ok(()),
        _ => Err("captured owned process is not proven absent".into()),
    }
}

async fn await_process_absent(pid: u32) -> TestResult {
    tokio::time::timeout(WAIT, async {
        while require_process_absent(pid).is_err() {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(())
}

fn recorded_pids(path: &Path) -> TestResult<Vec<u32>> {
    match std::fs::read_to_string(path) {
        Ok(text) => text
            .lines()
            .map(|line| line.parse::<u32>().map_err(Into::into))
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

fn require_path_absent(path: &Path) -> TestResult {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err("owned socket/publication still exists or is unreadable".into()),
    }
}

fn check_cleanup(condition: bool, message: &str, failures: &mut Vec<String>) {
    if !condition {
        failures.push(message.into());
    }
}
