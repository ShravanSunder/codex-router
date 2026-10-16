//! Lifecycle of one owned capture, including detached headless-browser resources.
use super::{TestResult, ensure};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static CAPTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) fn has_owned_browser(output: &Path) -> TestResult<bool> {
    let directory = fs::read_to_string(output.join("browser-temp.directory"))?;
    Ok(!browser_processes(Path::new(directory.trim()))?.is_empty())
}

pub(super) fn create_browser_temporary_directory(output: &Path) -> TestResult<PathBuf> {
    let sequence = CAPTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!("vhs-run-{}-{sequence}", std::process::id()));
    fs::create_dir(&directory)?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    fs::write(
        output.join("browser-temp.directory"),
        directory.to_string_lossy().as_bytes(),
    )?;
    Ok(directory)
}

struct OwnedBrowserProcess {
    process_id: u32,
}

fn browser_processes(directory: &Path) -> TestResult<Vec<OwnedBrowserProcess>> {
    let owner_uid = fs::metadata(directory)?.uid();
    let marker = format!("--user-data-dir={}/", directory.display());
    let observation = Command::new("/bin/ps")
        .args(["-axo", "pid=,uid=,command="])
        .output()?;
    ensure(
        observation.status.success(),
        "browser process inventory failed",
    )?;
    let mut processes = Vec::new();
    for line in String::from_utf8_lossy(&observation.stdout).lines() {
        let mut fields = line.split_whitespace();
        let Some(process_id) = fields.next() else {
            continue;
        };
        let Some(process_uid) = fields.next() else {
            continue;
        };
        let command = fields.collect::<Vec<_>>().join(" ");
        if !command.contains(&marker) {
            continue;
        }
        ensure(
            process_uid.parse::<u32>()? == owner_uid,
            "foreign UID uses this private browser profile",
        )?;
        processes.push(OwnedBrowserProcess {
            process_id: process_id.parse()?,
        });
    }
    Ok(processes)
}

fn stop_owned_browsers(directory: &Path) -> TestResult<()> {
    let marker = format!("--user-data-dir={}/", directory.display());
    let owner_uid = fs::metadata(directory)?.uid();
    for process in browser_processes(directory)? {
        let pid = process.process_id.to_string();
        let snapshot = Command::new("/bin/ps")
            .args(["-p", &pid, "-o", "uid=,command="])
            .output()?;
        if !snapshot.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&snapshot.stdout);
        let mut fields = text.split_whitespace();
        let uid: u32 = fields.next().ok_or("browser UID missing")?.parse()?;
        let command = fields.collect::<Vec<_>>().join(" ");
        ensure(
            uid == owner_uid && command.contains(&marker),
            "refusing to stop a foreign browser PID",
        )?;
        let _ = Command::new("/bin/kill").args(["-TERM", &pid]).output()?;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while !browser_processes(directory)?.is_empty() {
        ensure(Instant::now() < deadline, "owned browser survived TERM")?;
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}

pub(super) struct OwnedCapture {
    pub(super) child: Child,
    pub(super) group_released: bool,
    pub(super) output: PathBuf,
    pub(super) capability: String,
    pub(super) browser_temporary_directory: PathBuf,
}

impl OwnedCapture {
    pub(super) fn wait(&mut self, timeout: Duration) -> TestResult<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                self.release_process_group()?;
                return Err(
                    std::io::Error::other("owned VHS run exceeded its lifecycle deadline").into(),
                );
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    pub(super) fn release_process_group(&mut self) -> TestResult<()> {
        if !self.group_released {
            // ttyd's PTY child may have a separate session. A signal to VHS's
            // group alone cannot establish its cleanup on an early error.
            // A failed identity check must never signal a foreign PID, but it
            // also must not skip cleanup of later independently owned resources.
            let outcomes = [
                terminate_owned_pid(&self.output.join("quota-child.pid"), &self.capability),
                terminate_owned_pid(
                    &self.output.join("quota-wrapper.pid"),
                    &self.output.join("fixed_quota_child.py").to_string_lossy(),
                ),
                terminate_owned_pid(
                    &self.output.join("ttyd.pid"),
                    &self.output.join("fixed_quota_child.py").to_string_lossy(),
                ),
                Command::new("/bin/kill")
                    .args(["-TERM", &format!("-{}", self.child.id())])
                    .output()
                    .map(|_| ())
                    .map_err(Into::into),
                stop_owned_browsers(&self.browser_temporary_directory),
            ];
            self.group_released = true;
            let failures = outcomes
                .into_iter()
                .filter_map(Result::err)
                .map(|error| error.to_string())
                .collect::<Vec<_>>();
            if !failures.is_empty() {
                return Err(std::io::Error::other(failures.join("; ")).into());
            }
            fs::remove_dir_all(&self.browser_temporary_directory)?;
        }
        Ok(())
    }
}

impl Drop for OwnedCapture {
    fn drop(&mut self) {
        if let Err(error) = self.release_process_group() {
            let _ = fs::write(self.output.join("cleanup-error.txt"), error.to_string());
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(super) fn ensure_owned_child_stopped(output: &Path) -> TestResult<()> {
    for record_name in ["quota-child.pid", "quota-wrapper.pid", "ttyd.pid"] {
        let pid: u32 = fs::read_to_string(output.join(record_name))?
            .trim()
            .parse()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .output()?
            .status
            .success()
        {
            ensure(
                Instant::now() < deadline,
                "owned storyboard process survived cleanup",
            )?;
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    let browser_directory = fs::read_to_string(output.join("browser-temp.directory"))?;
    ensure(
        !Path::new(browser_directory.trim()).exists(),
        "owned browser temporary profile survived cleanup",
    )?;
    ensure(
        !output.join("cleanup-error.txt").exists(),
        "owned cleanup reported a resource error",
    )?;
    Ok(())
}

fn terminate_owned_pid(record: &Path, expected_command_marker: &str) -> TestResult<()> {
    if !record.exists() {
        return Ok(());
    }
    let pid: u32 = fs::read_to_string(record)?.trim().parse()?;
    let snapshot = Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "stat=,command="])
        .output()?;
    if !snapshot.status.success() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&snapshot.stdout);
    if text
        .split_whitespace()
        .next()
        .is_some_and(|state| state.starts_with('Z'))
    {
        return Ok(()); // Already exited; its parent must reap it. Never signal it.
    }
    ensure(
        text.contains(expected_command_marker),
        "refusing to signal a PID whose command does not match this owned capture",
    )?;
    let _ = Command::new("/bin/kill")
        .args(["-TERM", &pid.to_string()])
        .output()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .output()?
        .status
        .success()
    {
        ensure(
            Instant::now() < deadline,
            "owned process did not stop after TERM",
        )?;
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}
