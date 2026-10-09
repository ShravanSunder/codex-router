//! A `tursodb` 0.8.1 sync server owned by one test, and synced stores that talk to it
//!
//! The binary comes from `SQLX_TURSO_SYNC_SERVER` or `<workspace>/tmp/rust-tools/bin/tursodb`
//! (installed by `scripts/tooling/install-turso-sync-server.py`). A missing or wrong binary
//! fails the test; it never skips.

use std::{
    future::Future,
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use sqlx_turso::{TursoConnectOptions, TursoConnection, TursoSyncOptions, sqlx::ConnectOptions};
use tokio::{
    net::TcpStream,
    process::{Child, Command},
    time::timeout,
};

use super::TestResult;

const EXPECTED_VERSION: &str = "Turso 0.8.1";
const READY_TIMEOUT: Duration = Duration::from_secs(15);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
/// Upper bound for one open, push or pull against a local server
pub const REMOTE_TIMEOUT: Duration = Duration::from_secs(30);
const INSTALL_HINT: &str = "install it with `python3 scripts/tooling/install-turso-sync-server.py` \
    or point SQLX_TURSO_SYNC_SERVER at a tursodb 0.8.1 binary";

/// One running `tursodb --sync-server` process on an ephemeral loopback port
pub struct SyncServer {
    binary: PathBuf,
    arguments: Vec<String>,
    address: SocketAddr,
    log_path: PathBuf,
    child: Option<Child>,
}

impl SyncServer {
    /// Serves one database file, `hub.db`, inside `directory`
    pub async fn start_single_file(directory: &Path) -> TestResult<Self> {
        let address = free_loopback_address()?;
        let database = directory.join("hub.db");
        let arguments = vec![
            database.display().to_string(),
            "--sync-server".to_owned(),
            address.to_string(),
        ];
        Self::start(directory, address, arguments).await
    }

    /// Serves any number of databases under `directory/hubs`, addressed as `/db/<name>`
    pub async fn start_directory(directory: &Path) -> TestResult<Self> {
        let address = free_loopback_address()?;
        let hubs = directory.join("hubs");
        tokio::fs::create_dir_all(&hubs).await?;
        let arguments = vec![
            "--sync-server".to_owned(),
            address.to_string(),
            "--sync-dir".to_owned(),
            hubs.display().to_string(),
        ];
        Self::start(directory, address, arguments).await
    }

    async fn start(
        directory: &Path,
        address: SocketAddr,
        arguments: Vec<String>,
    ) -> TestResult<Self> {
        let binary = sync_server_binary()?;
        verify_server_version(&binary).await?;
        let mut server = Self {
            binary,
            arguments,
            address,
            log_path: directory.join("sync-server.log"),
            child: None,
        };
        server.spawn_and_wait_ready().await?;
        Ok(server)
    }

    /// The remote URL of a single-file server
    pub fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// The remote URL of database `name` on a directory server
    pub fn database_url(&self, name: &str) -> String {
        format!("http://{}/db/{name}", self.address)
    }

    /// Kills the server and waits for it to exit, under a bound
    pub async fn shutdown(&mut self) -> TestResult {
        if let Some(mut child) = self.child.take() {
            if child.try_wait()?.is_none() {
                child.start_kill()?;
            }
            timeout(SHUTDOWN_TIMEOUT, child.wait())
                .await
                .map_err(|_elapsed| "tursodb did not exit after kill")??;
        }
        Ok(())
    }

    /// Starts the server again on the same address and data
    pub async fn restart(&mut self) -> TestResult {
        self.shutdown().await?;
        self.spawn_and_wait_ready().await
    }

    async fn spawn_and_wait_ready(&mut self) -> TestResult {
        let log = std::fs::File::options()
            .create(true)
            .append(true)
            .open(&self.log_path)?;
        let child = Command::new(&self.binary)
            .args(&self.arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            // Fallback only: tests stop the server through `shutdown`.
            .kill_on_drop(true)
            .spawn()?;
        self.child = Some(child);
        timeout(READY_TIMEOUT, self.wait_until_accepting())
            .await
            .map_err(|_elapsed| {
                format!(
                    "tursodb did not accept connections within {READY_TIMEOUT:?}; log: {}",
                    self.log_path.display()
                )
            })?
    }

    async fn wait_until_accepting(&mut self) -> TestResult {
        loop {
            let child = self.child.as_mut().ok_or("tursodb was not started")?;
            if let Some(status) = child.try_wait()? {
                return Err(format!(
                    "tursodb exited early with {status}; log: {}",
                    self.log_path.display()
                )
                .into());
            }
            if TcpStream::connect(self.address).await.is_ok() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

fn free_loopback_address() -> TestResult<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?)
}

fn sync_server_binary() -> TestResult<PathBuf> {
    let binary = match std::env::var_os("SQLX_TURSO_SYNC_SERVER") {
        Some(path) => PathBuf::from(path),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/rust-tools/bin/tursodb"),
    };
    if !binary.is_file() {
        return Err(format!("tursodb not found at {}; {INSTALL_HINT}", binary.display()).into());
    }
    Ok(binary)
}

async fn verify_server_version(binary: &Path) -> TestResult {
    let output = timeout(
        READY_TIMEOUT,
        Command::new(binary).arg("--version").output(),
    )
    .await
    .map_err(|_elapsed| "tursodb --version did not finish")??;
    let reported = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() || !reports_pinned_version(&reported) {
        return Err(format!(
            "{} reports {:?}, expected {EXPECTED_VERSION}; {INSTALL_HINT}",
            binary.display(),
            reported.trim()
        )
        .into());
    }
    Ok(())
}

/// Whether `tursodb --version` output names exactly the pinned release
///
/// An exact match: `Turso 0.8.10` and `Turso 0.8.1-dev` are different engines.
fn reports_pinned_version(version_output: &str) -> bool {
    version_output.trim() == EXPECTED_VERSION
}

#[test]
fn only_the_exact_pinned_version_is_accepted() {
    assert!(reports_pinned_version("Turso 0.8.1\n"));
    for other in [
        "Turso 0.8.10",
        "Turso 0.8.1-dev",
        "Turso 0.8.12\n",
        "Turso 0.8.1 extra",
        "",
    ] {
        assert!(!reports_pinned_version(other), "{other:?}");
    }
}

/// Installs rustls's aws-lc-rs provider as the process default
///
/// Turso's Sync worker builds its HTTPS connector from rustls's process default. A build that
/// also enables rustls's `ring` provider (any build unified with Router's TLS clients) cannot
/// choose one by itself and the worker panics, so the tests choose explicitly. A later call
/// finds the default already installed, which is fine.
pub fn install_sync_tls_provider() {
    let _already_installed = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

/// Waits for `operation` against the sync server, failing instead of hanging
pub async fn bounded<T, E>(
    label: &str,
    operation: impl Future<Output = Result<T, E>>,
) -> TestResult<T>
where
    E: std::error::Error + Send + Sync + 'static,
{
    match timeout(REMOTE_TIMEOUT, operation).await {
        Ok(result) => Ok(result?),
        Err(_elapsed) => Err(format!("{label} did not finish within {REMOTE_TIMEOUT:?}").into()),
    }
}

/// Opens a synced store at `path` against `remote_url`, bootstrapping it if empty
pub async fn open_synced_store(path: &Path, remote_url: &str) -> TestResult<TursoConnection> {
    install_sync_tls_provider();
    let options = TursoConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .with_sync_options(TursoSyncOptions::new(remote_url).with_client_name("sqlx-turso-tests"));
    bounded("synced open", options.connect()).await
}

/// Pushes local changes, bounded
pub async fn push(connection: &TursoConnection) -> TestResult {
    bounded("sync push", connection.sync_push()).await
}

/// Pulls remote changes into the same connection, bounded
pub async fn pull(connection: &TursoConnection) -> TestResult<bool> {
    bounded("sync pull", connection.sync_pull()).await
}
