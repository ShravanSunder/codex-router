use std::io::Write;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let arguments: Vec<String> = std::env::args().collect();
    let database = turso::sync::Builder::new_remote(&arguments[1])
        .with_remote_url(&arguments[2])
        .with_auth_token("a-epoch-1")
        .build()
        .await?;
    let connection = database.connect().await?;
    for sequence in 6..=8 {
        connection
            .execute(
                "INSERT INTO records VALUES(?,?)",
                turso::params![sequence, format!("A-{sequence}")],
            )
            .await?;
    }
    println!(
        "TAIL_DURABLE pending={} local_sequences=1..8",
        database.stats().await?.cdc_operations
    );
    std::io::stdout().flush()?;
    // Parent kills this process after the durable-commit receipt, with no SDK close/checkpoint.
    std::future::pending::<()>().await;
    Ok(())
}
