//! Cross-process authority for one account in one canonical Router state database.

use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::path::Path;
use std::path::PathBuf;

use codex_router_core::ids::AccountId;
use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;

/// A redacted failure to establish cross-process credential authority.
#[derive(Debug, Error)]
#[error("account credential lock unavailable")]
pub struct AccountCredentialLockError;

/// Held until provider use and any blocking secret write have actually ended.
pub struct AccountCredentialLock {
    _file: File,
}

impl AccountCredentialLock {
    pub fn acquire(
        database_path: &Path,
        account_id: &AccountId,
    ) -> Result<Self, AccountCredentialLockError> {
        let canonical_database =
            fs::canonicalize(database_path).map_err(|_| AccountCredentialLockError)?;
        let database_name = canonical_database
            .file_name()
            .ok_or(AccountCredentialLockError)?;
        let mut lock_dir_name = database_name.to_os_string();
        lock_dir_name.push(".credential-locks");
        let lock_dir = canonical_database
            .parent()
            .ok_or(AccountCredentialLockError)?
            .join(lock_dir_name);
        ensure_private_lock_directory(&lock_dir)?;

        let digest = Sha256::digest(account_id.as_str().as_bytes());
        let lock_path = lock_dir.join(format!("{digest:x}.lock"));
        reject_symlink(&lock_path)?;
        let file = open_private_lock_file(&lock_path)?;
        file.lock().map_err(|_| AccountCredentialLockError)?;
        Ok(Self { _file: file })
    }
}

fn ensure_private_lock_directory(path: &Path) -> Result<(), AccountCredentialLockError> {
    reject_symlink(path)?;
    fs::create_dir_all(path).map_err(|_| AccountCredentialLockError)?;
    reject_symlink(path)?;
    if !path.is_dir() {
        return Err(AccountCredentialLockError);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| AccountCredentialLockError)?;
    }
    Ok(())
}

fn reject_symlink(path: &Path) -> Result<(), AccountCredentialLockError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(AccountCredentialLockError),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(AccountCredentialLockError),
    }
}

#[cfg(unix)]
fn open_private_lock_file(path: &PathBuf) -> Result<File, AccountCredentialLockError> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(|_| AccountCredentialLockError)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::process::Command;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;
    use std::time::Instant;

    static TEST_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn symlinked_state_path_waits_on_the_same_cross_process_account_lock() {
        let account_id = AccountId::new("shared-lock-account").expect("account id");
        if let Some(database_path) = std::env::var_os("CODEX_ROUTER_LOCK_TEST_CHILD_DB") {
            let ready_path = PathBuf::from(
                std::env::var_os("CODEX_ROUTER_LOCK_TEST_CHILD_READY").expect("ready path"),
            );
            let release_path = PathBuf::from(
                std::env::var_os("CODEX_ROUTER_LOCK_TEST_CHILD_RELEASE").expect("release path"),
            );
            let _lock = AccountCredentialLock::acquire(Path::new(&database_path), &account_id)
                .expect("child should lock canonical database");
            fs::write(ready_path, b"held").expect("child should report lock held");
            let deadline = Instant::now() + Duration::from_secs(5);
            while !release_path.exists() {
                assert!(Instant::now() < deadline, "child lock release timed out");
                thread::yield_now();
            }
            return;
        }

        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "router-account-lock-test-{}-{sequence}",
            std::process::id(),
        ));
        fs::create_dir(&root).expect("test root should create");
        let database_path = root.join("state.sqlite");
        let alias_path = root.join("alias.sqlite");
        let ready_path = root.join("child-ready");
        let release_path = root.join("child-release");
        fs::write(&database_path, b"fixture database identity")
            .expect("database file should exist");
        symlink(&database_path, &alias_path).expect("alias should point to state database");
        let mut child = Command::new(std::env::current_exe().expect("test executable"))
            .arg("--exact")
            .arg("account_credential_lock::tests::symlinked_state_path_waits_on_the_same_cross_process_account_lock")
            .env("CODEX_ROUTER_LOCK_TEST_CHILD_DB", &database_path)
            .env("CODEX_ROUTER_LOCK_TEST_CHILD_READY", &ready_path)
            .env("CODEX_ROUTER_LOCK_TEST_CHILD_RELEASE", &release_path)
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("child should start");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready_path.exists() {
            assert!(Instant::now() < deadline, "child did not establish lock");
            thread::yield_now();
        }
        let (attempt_sender, attempt_receiver) = mpsc::channel();
        let (acquired_sender, acquired_receiver) = mpsc::channel();
        let lock_thread = thread::spawn(move || {
            attempt_sender.send(()).expect("attempt should report");
            let _lock = AccountCredentialLock::acquire(&alias_path, &account_id)
                .expect("alias should acquire after child exits");
            acquired_sender.send(()).expect("acquisition should report");
        });
        attempt_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("alias attempt should begin");
        assert!(matches!(
            acquired_receiver.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        fs::write(&release_path, b"release").expect("child should release");
        assert!(child.wait().expect("child should exit").success());
        acquired_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("alias should acquire after child release");
        lock_thread.join().expect("alias lock thread should finish");
        fs::remove_dir_all(root).expect("test root should remove");
    }
}

#[cfg(not(unix))]
fn open_private_lock_file(path: &PathBuf) -> Result<File, AccountCredentialLockError> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)
        .map_err(|_| AccountCredentialLockError)
}
