//! Process-image and singleton observations for owned Host replacement fixtures.

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

// A cold launch of the copied ad-hoc-signed debug CLI can spend over 10 seconds in macOS dyld.
const COPIED_CLI_VERSION_TIMEOUT: Duration = Duration::from_secs(30);

pub async fn binary_version(binary: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let result = tokio::time::timeout(
        COPIED_CLI_VERSION_TIMEOUT,
        tokio::process::Command::new(binary)
            .arg("--version")
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    if !result.status.success() {
        return Err(format!(
            "binary {} did not report its version: status={}, stdout={:?}, stderr={:?}",
            binary.display(),
            result.status,
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr),
        )
        .into());
    }
    Ok(String::from_utf8(result.stdout)?.trim().to_owned())
}

pub fn verify_host_image(process_id: u32, expected: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::MetadataExt;
        let output = std::process::Command::new("/usr/sbin/lsof")
            .args(["-a", "-p", &process_id.to_string(), "-d", "txt", "-Fin"])
            .output()?;
        let expected_image = format!(
            "i{}\nn{}",
            std::fs::metadata(expected)?.ino(),
            std::fs::canonicalize(expected)?.display(),
        );
        if output.status.success()
            && String::from_utf8_lossy(&output.stdout).contains(&expected_image)
        {
            return Ok(());
        }
        Err(std::io::Error::other(format!(
            "Host PID {process_id} does not map the expected installed image: {}",
            String::from_utf8_lossy(&output.stdout),
        )))
    }
    #[cfg(not(target_os = "macos"))]
    {
        if std::fs::read_link(format!("/proc/{process_id}/exe"))?
            == std::fs::canonicalize(expected)?
        {
            Ok(())
        } else {
            Err(std::io::Error::other("Host maps another executable"))
        }
    }
}

pub async fn observe_continuous_lock(
    lock_path: PathBuf,
    mut finish: tokio::sync::oneshot::Receiver<()>,
) -> std::io::Result<()> {
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock_path)?;
    let mut tick = tokio::time::interval(Duration::from_millis(5));
    loop {
        match lock.try_lock() {
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
            Ok(()) => {
                lock.unlock()?;
                return Err(std::io::Error::other(
                    "singleton lock became available during restart",
                ));
            }
        }
        tokio::select! {
            _ = &mut finish => return Ok(()),
            _ = tick.tick() => {}
        }
    }
}
