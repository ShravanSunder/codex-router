#[path = "../embedded_replica.rs"]
mod embedded_replica;
#[path = "../server_process.rs"]
mod server_process;
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let root = PathBuf::from(format!(
        "tmp/turso-runs/{}-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
        std::process::id()
    ));
    std::fs::create_dir_all(&root)?;
    println!(
        "RUN root={} case=embedded; one Tokio runtime",
        root.display()
    );
    embedded_replica::run(&root).await
}
