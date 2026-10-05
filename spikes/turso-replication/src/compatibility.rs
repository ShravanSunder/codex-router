use crate::{server_process::SpikeServer, sync_cases::snapshot};
use anyhow::{Result, ensure};
use sqlx::{Connection, sqlite::SqliteConnectOptions};
use std::path::Path;

pub async fn run(root: &Path) -> Result<()> {
    let server = SpikeServer::start(root, "compatibility", true).await?;
    let path = root.join("compatibility/replica.db");
    let database = turso::sync::Builder::new_remote(path.to_str().unwrap())
        .with_remote_url(server.url())
        .build()
        .await?;
    let mut connection = database.connect().await?;
    connection.execute_batch("CREATE TABLE board(id INTEGER PRIMARY KEY, body TEXT NOT NULL); INSERT INTO board VALUES(1,'initial');").await?;
    println!(
        "TURSO journal_mode={:?}",
        snapshot(&connection, "PRAGMA journal_mode").await?
    );
    println!(
        "TURSO set DELETE={:?}",
        snapshot(&connection, "PRAGMA journal_mode=DELETE").await
    );
    println!(
        "TURSO mode after DELETE={:?}",
        snapshot(&connection, "PRAGMA journal_mode").await?
    );
    // Typed domain request validation, state precondition and write share one transaction.
    let request = BoardPost::try_new("admitted", "initial")?;
    let transaction = connection
        .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
        .await?;
    let stored: String = transaction
        .query("SELECT body FROM board WHERE id=1", ())
        .await?
        .next()
        .await?
        .unwrap()
        .get(0)?;
    ensure!(stored == request.expected_body, "revision mismatch");
    transaction
        .execute("UPDATE board SET body=? WHERE id=1", [request.body])
        .await?;
    transaction.commit().await?;
    println!(
        "TURSO admission local={:?}",
        snapshot(&connection, "SELECT * FROM board").await?
    );
    println!(
        "TURSO admission invalid={:?}",
        BoardPost::try_new(" ", "admitted")
    );
    database.push().await?;
    println!("TURSO admission push=Ok");
    let peer =
        turso::sync::Builder::new_remote(root.join("compatibility/peer.db").to_str().unwrap())
            .with_remote_url(server.url())
            .build()
            .await?;
    let peer_connection = peer.connect().await?;
    let peer_body: String = peer_connection
        .query("SELECT body FROM board WHERE id=1", ())
        .await?
        .next()
        .await?
        .unwrap()
        .get(0)?;
    ensure!(peer_body == "admitted", "typed write did not replicate");
    println!("TURSO admission peer read={peer_body}");
    let open = sqlx::SqliteConnection::connect_with(
        &SqliteConnectOptions::new().filename(&path).read_only(true),
    )
    .await;
    match open {
        Ok(mut sqlite) => {
            let rows: std::result::Result<Vec<(i64, String)>, _> =
                sqlx::query_as("SELECT id,body FROM board")
                    .fetch_all(&mut sqlite)
                    .await;
            println!("SQLX turso live read_only={rows:?}");
            ensure!(
                rows? == vec![(1, "admitted".to_string())],
                "SQLx live read mismatch"
            );
            peer_connection
                .execute("UPDATE board SET body='remote-update' WHERE id=1", ())
                .await?;
            peer.push().await?;
            println!(
                "SQLX remote primary after update={}",
                crate::sync_cases::primary_query(
                    &server.url(),
                    "SELECT body FROM board WHERE id=1"
                )
                .await?
            );
            println!("SQLX local pull={:?}", database.pull().await?);
            println!(
                "SQLX Turso connection after pull={:?}",
                snapshot(&connection, "SELECT * FROM board").await?
            );
            let after: Vec<(i64, String)> = sqlx::query_as("SELECT id,body FROM board")
                .fetch_all(&mut sqlite)
                .await?;
            println!("SQLX turso same live connection after pull={after:?}");

            sqlite.close().await?;
            let mut reopened = sqlx::SqliteConnection::connect_with(
                &SqliteConnectOptions::new().filename(&path).read_only(true),
            )
            .await?;
            let values: Vec<(i64, String)> = sqlx::query_as("SELECT id,body FROM board")
                .fetch_all(&mut reopened)
                .await?;
            println!("SQLX reopened live after pull={values:?}");
            println!("SQLX explicit checkpoint={:?}", database.checkpoint().await);
            let values: Vec<(i64, String)> = sqlx::query_as("SELECT id,body FROM board")
                .fetch_all(&mut reopened)
                .await?;
            println!("SQLX live after explicit checkpoint={values:?}");
            reopened.close().await?;
        }
        Err(error) => println!("SQLX turso live open read_only=Err({error})"),
    }
    println!("TURSO checkpoint={:?}", database.checkpoint().await);
    drop(connection);
    drop(database);
    let mut sqlite = sqlx::SqliteConnection::connect_with(
        &SqliteConnectOptions::new().filename(&path).read_only(true),
    )
    .await?;
    let rows: std::result::Result<Vec<(i64, String)>, _> =
        sqlx::query_as("SELECT id,body FROM board")
            .fetch_all(&mut sqlite)
            .await;
    println!("SQLX turso closed/checkpointed read_only={rows:?}");
    ensure!(
        rows? == vec![(1, "remote-update".to_string())],
        "SQLx closed file mismatch"
    );
    sqlite.close().await?;
    println!(
        "TURSO Tokio runtime={} crate spawns second Tokio runtime on dedicated IO thread (source verified)",
        tokio::runtime::Handle::try_current().is_ok()
    );
    Ok(())
}

#[derive(Debug)]
struct BoardPost {
    body: String,
    expected_body: String,
}
impl BoardPost {
    fn try_new(body: &str, expected_body: &str) -> Result<Self> {
        ensure!(!body.trim().is_empty(), "body must be nonempty");
        Ok(Self {
            body: body.into(),
            expected_body: expected_body.into(),
        })
    }
}
