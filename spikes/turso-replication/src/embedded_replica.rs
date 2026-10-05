use crate::server_process::SpikeServer;
use anyhow::{Result, ensure};
use std::{path::Path, time::Instant};

async fn board_value(connection: &libsql::Connection) -> Result<String> {
    let mut rows = connection
        .query("SELECT body FROM board WHERE id=1", ())
        .await?;
    Ok(rows
        .next()
        .await?
        .ok_or_else(|| anyhow::anyhow!("missing row"))?
        .get(0)?)
}

struct PostRequest {
    expected_body: String,
    new_body: String,
}

async fn admit_post(connection: &libsql::Connection, request: PostRequest) -> Result<()> {
    ensure!(!request.new_body.trim().is_empty(), "body must be nonempty");
    let transaction = connection
        .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
        .await?;
    let mut rows = transaction
        .query("SELECT body FROM board WHERE id=1", ())
        .await?;
    let stored_body: String = rows
        .next()
        .await?
        .ok_or_else(|| anyhow::anyhow!("missing board row"))?
        .get(0)?;
    if stored_body != request.expected_body {
        transaction.rollback().await?;
        anyhow::bail!("stale expected body");
    }
    transaction
        .execute("UPDATE board SET body=? WHERE id=1", [request.new_body])
        .await?;
    transaction.commit().await?;
    Ok(())
}

pub async fn run(root: &Path) -> Result<()> {
    let mut server = SpikeServer::start(root, "embedded", false).await?;
    let primary = libsql::Builder::new_remote(server.url(), String::new())
        .build()
        .await?;
    let primary_connection = primary.connect()?;
    primary_connection.execute_batch("CREATE TABLE board(id INTEGER PRIMARY KEY, body TEXT NOT NULL); INSERT INTO board VALUES(1,'initial');").await?;
    let first_path = root.join("embedded/first.db");
    let first = libsql::Builder::new_remote_replica(
        first_path.to_str().unwrap(),
        server.url(),
        String::new(),
    )
    .build()
    .await?;
    let second = libsql::Builder::new_remote_replica(
        root.join("embedded/second.db").to_str().unwrap(),
        server.url(),
        String::new(),
    )
    .build()
    .await?;
    first.sync().await?;
    second.sync().await?;
    let first_connection = first.connect()?;
    let second_connection = second.connect()?;
    let start = Instant::now();
    first_connection
        .execute("UPDATE board SET body='from-first' WHERE id=1", ())
        .await?;
    println!(
        "EMBEDDED first write elapsed_us={} first={} second={} primary={}",
        start.elapsed().as_micros(),
        board_value(&first_connection).await?,
        board_value(&second_connection).await?,
        board_value(&primary_connection).await?
    );
    ensure!(
        board_value(&first_connection).await? == "from-first",
        "read-your-writes"
    );
    ensure!(
        board_value(&second_connection).await? == "initial",
        "explicit sync lag"
    );
    let start = Instant::now();
    println!("EMBEDDED second.sync={:?}", second.sync().await?);
    println!(
        "EMBEDDED pull elapsed_us={} second={}",
        start.elapsed().as_micros(),
        board_value(&second_connection).await?
    );
    server.stop()?;
    println!(
        "EMBEDDED down reads first={} second={}",
        board_value(&first_connection).await?,
        board_value(&second_connection).await?
    );
    let write = first_connection
        .execute("UPDATE board SET body='outage-write' WHERE id=1", ())
        .await;
    println!("EMBEDDED down write={write:?}");
    ensure!(write.is_err(), "outage write unexpectedly accepted");
    println!("EMBEDDED down sync={:?}", second.sync().await);
    server.restart().await?;
    first.sync().await?;
    second.sync().await?;
    println!(
        "EMBEDDED reconnect first={} second={}",
        board_value(&first_connection).await?,
        board_value(&second_connection).await?
    );
    println!(
        "EMBEDDED old remote connection after restart={:?}",
        board_value(&primary_connection).await
    );
    let primary = libsql::Builder::new_remote(server.url(), String::new())
        .build()
        .await?;
    let primary_connection = primary.connect()?;
    let request = PostRequest {
        expected_body: "from-first".into(),
        new_body: "admitted".into(),
    };
    admit_post(&primary_connection, request).await?;
    let rejected = admit_post(
        &primary_connection,
        PostRequest {
            expected_body: "from-first".into(),
            new_body: "rejected".into(),
        },
    )
    .await;
    println!(
        "ADMISSION accepted={} stale={rejected:?}",
        board_value(&primary_connection).await?
    );
    ensure!(
        rejected.is_err() && board_value(&primary_connection).await? == "admitted",
        "stale admission must rollback"
    );
    let invalid = admit_post(
        &primary_connection,
        PostRequest {
            expected_body: "admitted".into(),
            new_body: " ".into(),
        },
    )
    .await;
    println!("ADMISSION invalid={invalid:?}");
    ensure!(invalid.is_err(), "invalid request accepted");
    second.sync().await?;
    ensure!(
        board_value(&second_connection).await? == "admitted",
        "admitted write replicated"
    );
    let mode: String = first_connection
        .query("PRAGMA journal_mode", ())
        .await?
        .next()
        .await?
        .unwrap()
        .get(0)?;
    println!("EMBEDDED journal_mode={mode}");
    println!("SQLX_PROBE embedded file={}", first_path.display());
    println!(
        "EMBEDDED set DELETE={:?}",
        first_connection
            .execute("PRAGMA journal_mode=DELETE", ())
            .await
    );
    Ok(())
}
