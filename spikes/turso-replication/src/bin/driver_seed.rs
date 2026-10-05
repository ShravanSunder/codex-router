use sqlx_turso::{
    TursoConnectOptions,
    sqlx::{ConnectOptions, Executor},
};
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "tmp/turso-driver/metadata.db".into());
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut connection = TursoConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .connect()
        .await?;
    connection
        .execute(
            "CREATE TABLE IF NOT EXISTS records(sequence INTEGER PRIMARY KEY, body TEXT NOT NULL)",
        )
        .await?;
    println!("DRIVER_SEED path={path}");
    Ok(())
}
