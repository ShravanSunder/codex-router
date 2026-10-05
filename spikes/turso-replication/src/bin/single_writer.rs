#[path = "../lease_gateway.rs"]
mod lease_gateway;
#[path = "../server_process.rs"]
mod server_process;
#[path = "../single_writer_cases.rs"]
mod single_writer_cases;
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let scenario = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    let root = PathBuf::from(format!(
        "tmp/turso-single-writer/{}-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
        std::process::id()
    ));
    std::fs::create_dir_all(&root)?;
    println!("SINGLE_WRITER root={} scenario={scenario}", root.display());
    single_writer_cases::run(&root, &scenario).await
}
