use sqlx::{Connection, sqlite::SqliteConnectOptions};
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: sqlx-probe FILE"))?;
    let mut connection = sqlx::SqliteConnection::connect_with(
        &SqliteConnectOptions::new().filename(&path).read_only(true),
    )
    .await?;
    let version: String = sqlx::query_scalar("SELECT sqlite_version()")
        .fetch_one(&mut connection)
        .await?;
    let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&mut connection)
        .await?;
    let values: Vec<(i64, String)> = sqlx::query_as("SELECT id,body FROM board")
        .fetch_all(&mut connection)
        .await?;
    println!("SQLX file={path} sqlite={version} mode={mode} rows={values:?}");
    connection.close().await?;
    Ok(())
}
