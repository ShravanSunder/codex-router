mod compatibility;
mod server_process;
mod sync_cases;
use anyhow::Result;
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[tokio::main]
async fn main() -> Result<()> {
    let case = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    let root = PathBuf::from(format!(
        "tmp/turso-runs/{}-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
        std::process::id()
    ));
    std::fs::create_dir_all(&root)?;
    println!(
        "RUN root={} case={case}; application Tokio runtime",
        root.display()
    );
    let mut failed = 0;
    if case != "compatibility" {
        match sync_cases::run(&root, &case).await {
            Ok(()) => println!("CASE sync COMPLETE"),
            Err(error) => {
                failed += 1;
                println!("CASE sync ERROR {error:#}");
            }
        }
    }
    if case == "all" || case == "compatibility" {
        match compatibility::run(&root).await {
            Ok(()) => println!("CASE compatibility COMPLETE"),
            Err(error) => {
                failed += 1;
                println!("CASE compatibility ERROR {error:#}");
            }
        }
    }
    println!("HARNESS failures={failed}");
    anyhow::ensure!(
        failed == 0,
        "{failed} harness cases failed (see individual observed errors)"
    );
    Ok(())
}
