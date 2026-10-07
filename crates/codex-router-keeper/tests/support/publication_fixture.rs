//! External app-server stand-in only: real listened physical sockets and advertised aliases.
use codex_router_descriptor_boundary::{DescriptorGate, OwnedListener, OwnedSocket, UnixReceipt};
use codex_router_keeper_protocol::{GenerationAliasPath, GenerationId};
use std::{os::unix::fs::PermissionsExt, path::Path};
use tokio::{
    sync::watch,
    task::JoinSet,
    time::{Duration, timeout},
};
pub type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
pub fn private_directory() -> Result<tempfile::TempDir, std::io::Error> {
    let directory = tempfile::Builder::new().prefix("pub-").tempdir_in("/tmp")?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    Ok(directory)
}
pub struct NativeSocketFixture {
    pub generation: GenerationId,
    pub alias: GenerationAliasPath,
    shutdown: watch::Sender<bool>,
    server: JoinSet<TestResult>,
}
async fn serve_connection(connection: OwnedSocket, reply: &'static [u8; 2]) -> TestResult {
    let gate = DescriptorGate::global();
    let writer = connection.duplicate(gate).await?.into_writer();
    let receipt = UnixReceipt::new(connection);
    loop {
        let mut request = [0; 4];
        let mut offset = 0;
        while let Some(remaining) = request.get_mut(offset..).filter(|part| !part.is_empty()) {
            let count = receipt.read(remaining, false, gate).await?.bytes;
            if count == 0 {
                return if offset == 0 {
                    Ok(())
                } else {
                    Err("fixture partial request EOF".into())
                };
            }
            offset += count;
        }
        if &request != b"PING" {
            return Err("fixture received different literal request".into());
        }
        writer.write_all(reply).await?;
    }
}
impl NativeSocketFixture {
    pub async fn start(
        root: &Path,
        identity: &str,
        alias_name: &str,
        reply: &'static [u8; 2],
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let generation: GenerationId = serde_json::from_str(identity)?;
        let physical = root.join(format!("physical-{}.sock", generation.number.get()));
        let listener = OwnedListener::bind_unix(&physical, DescriptorGate::global()).await?;
        let alias = GenerationAliasPath::try_from(root.join(alias_name))?;
        std::os::unix::fs::symlink(&physical, alias.as_path())?;
        let (shutdown, mut stopping) = watch::channel(false);
        let mut server = JoinSet::new();
        server.spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    _changed=stopping.changed()=>break,
                    result=connections.join_next(), if !connections.is_empty()=> {
                        result.ok_or("fixture connection result absent")???;
                    }
                    result=listener.accept(DescriptorGate::global())=> {
                        let connection=result?;
                        let mut stop_connection=stopping.clone();
                        connections.spawn(async move {
                            tokio::select! {
                                _changed=stop_connection.changed()=>Ok(()),
                                result=serve_connection(connection,reply)=>result,
                            }
                        });
                    }
                }
            }
            while let Some(result) = connections.join_next().await {
                result??;
            }
            Ok(())
        });
        Ok(Self {
            generation,
            alias,
            shutdown,
            server,
        })
    }
    pub async fn finish(mut self) -> TestResult {
        self.shutdown.send_replace(true);
        timeout(Duration::from_secs(2), async {
            while let Some(result) = self.server.join_next().await {
                result??;
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        })
        .await??;
        Ok(())
    }
}
impl Drop for NativeSocketFixture {
    fn drop(&mut self) {
        self.shutdown.send_replace(true);
    }
}
