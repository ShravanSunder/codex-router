use anyhow::{Context, Result, bail};
use std::{
    fs::File,
    net::TcpListener,
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
};

pub struct SpikeServer {
    child: Option<Child>,
    binary_path: String,
    arguments: Vec<String>,
    log_path: String,
    pub address: String,
}

impl SpikeServer {
    pub async fn start(root: &Path, case: &str, sync_server: bool) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?.to_string();
        drop(listener);
        let directory = root.join(case);
        std::fs::create_dir_all(&directory)?;
        let (binary_path, arguments) = if sync_server {
            (
                "tmp/tools/turso_cli-aarch64-apple-darwin/tursodb",
                vec![
                    directory.join("primary.db").display().to_string(),
                    "--sync-server".into(),
                    address.clone(),
                ],
            )
        } else {
            (
                "tmp/tools/libsql-server-aarch64-apple-darwin/sqld",
                vec![
                    "--db-path".into(),
                    directory.join("primary.sqld").display().to_string(),
                    "--http-listen-addr".into(),
                    address.clone(),
                    "--no-welcome".into(),
                    "--disable-metrics".into(),
                ],
            )
        };
        let mut server = Self {
            child: None,
            binary_path: binary_path.into(),
            arguments,
            log_path: directory.join("server.log").display().to_string(),
            address,
        };
        server.restart().await?;
        Ok(server)
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub async fn restart(&mut self) -> Result<()> {
        println!("SERVER {} {}", self.binary_path, self.arguments.join(" "));
        let output = File::options()
            .append(true)
            .create(true)
            .open(&self.log_path)?;
        self.child = Some(
            Command::new(&self.binary_path)
                .args(&self.arguments)
                .stdin(Stdio::null())
                .stdout(output.try_clone()?)
                .stderr(output)
                .spawn()?,
        );
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if let Some(status) = self.child.as_mut().context("missing server")?.try_wait()? {
                    bail!(
                        "server exited {status}: {}",
                        std::fs::read_to_string(&self.log_path)?
                    );
                }
                if tokio::net::TcpStream::connect(&self.address).await.is_ok() {
                    return Ok(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .context("server readiness timeout")?
    }

    pub fn stop(&mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            child.kill()?;
            let status = child.wait()?;
            println!("SERVER stopped owned pid={} status={status}", child.id());
        }
        Ok(())
    }
}

impl Drop for SpikeServer {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
