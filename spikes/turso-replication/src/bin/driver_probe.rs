#[path = "../server_process.rs"]
mod server_process;
use anyhow::{Result, ensure};
use sqlx_turso::{
    TursoConnectOptions, TursoConnection, TursoSyncOptions,
    sqlx::{ConnectOptions, Connection, Executor},
};
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
#[derive(Debug, PartialEq)]
struct RecordRow {
    sequence: i64,
    body: String,
}
async fn checked_rows(connection: &mut TursoConnection) -> Result<Vec<RecordRow>> {
    Ok(sqlx_turso::query_as!(RecordRow,"SELECT sequence AS \"sequence!: i64\", body AS \"body!: String\" FROM records ORDER BY sequence").fetch_all(connection).await?)
}
#[tokio::main]
async fn main() -> Result<()> {
    let root = PathBuf::from(format!(
        "tmp/turso-driver/{}-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
        std::process::id()
    ));
    std::fs::create_dir_all(&root)?;
    println!(
        "DRIVER pin=b7e5fab909f5403f67f69d69d621f8f72e472a1f turso=0.7.0-pre.3 root={}",
        root.display()
    );
    let mut local = TursoConnectOptions::new()
        .filename(root.join("local.db"))
        .create_if_missing(true)
        .connect()
        .await?;
    local
        .execute("CREATE TABLE records(sequence INTEGER PRIMARY KEY,body TEXT NOT NULL)")
        .await?;
    sqlx_turso::query!(
        "INSERT INTO records(sequence,body) VALUES(?,?)",
        1_i64,
        "local"
    )
    .execute(&mut local)
    .await?;
    let row=sqlx_turso::query!("SELECT sequence AS \"sequence!: i64\", body AS \"body!: String\" FROM records WHERE sequence=?",1_i64).fetch_one(&mut local).await?;
    println!(
        "DRIVER query! typed sequence={} body={}",
        row.sequence, row.body
    );
    ensure!(
        row.sequence == 1 && row.body == "local",
        "checked macro result mismatch"
    );
    println!("DRIVER query_as! {:?}", checked_rows(&mut local).await?);
    let excess_binds = sqlx_turso::query!("SELECT ? AS \"value!: i64\"", 1_i64, 2_i64)
        .fetch_one(&mut local)
        .await;
    println!(
        "DRIVER extra bind compiled runtime_result={:?}",
        excess_binds.map(|row| row.value)
    );
    let readonly = TursoConnectOptions::new()
        .filename(root.join("local.db"))
        .read_only(true)
        .connect()
        .await;
    println!("DRIVER read_only open={readonly:?}");
    ensure!(readonly.is_err(), "read_only capability changed");
    let server = server_process::SpikeServer::start(&root, "sync", true).await?;
    let mut writer = TursoConnectOptions::new()
        .filename(root.join("writer.db"))
        .create_if_missing(true)
        .with_sync_options(TursoSyncOptions::new(server.url()))
        .connect()
        .await?;
    writer
        .execute("CREATE TABLE records(sequence INTEGER PRIMARY KEY,body TEXT NOT NULL)")
        .await?;
    sqlx_turso::query!(
        "INSERT INTO records(sequence,body) VALUES(?,?)",
        1_i64,
        "first"
    )
    .execute(&mut writer)
    .await?;
    writer.sync_push().await?;
    let mut reader = TursoConnectOptions::new()
        .filename(root.join("reader.db"))
        .create_if_missing(true)
        .with_sync_options(TursoSyncOptions::new(server.url()))
        .connect()
        .await?;
    println!(
        "DRIVER native_sync first={:?}",
        checked_rows(&mut reader).await?
    );
    for sequence in 2..=4 {
        sqlx_turso::query!(
            "INSERT INTO records(sequence,body) VALUES(?,?)",
            sequence,
            format!("item-{sequence}")
        )
        .execute(&mut writer)
        .await?;
        writer.sync_push().await?;
        println!(
            "DRIVER native_sync before pull sequence={sequence} rows={:?}",
            checked_rows(&mut reader).await?
        );
        let changed = reader.sync_pull().await?;
        let rows = checked_rows(&mut reader).await?;
        println!("DRIVER native_sync same_connection pull={changed} rows={rows:?}");
        ensure!(
            rows.len() == sequence as usize,
            "persistent checked SQLx driver missed pull"
        );
    }
    sqlx_turso::query!(
        "UPDATE records SET body=? WHERE sequence=?",
        "updated",
        1_i64
    )
    .execute(&mut writer)
    .await?;
    writer.sync_push().await?;
    reader.sync_pull().await?;
    let row=sqlx_turso::query!("SELECT sequence AS \"sequence!: i64\", body AS \"body!: String\" FROM records WHERE sequence=?",1_i64).fetch_one(&mut reader).await?;
    println!("DRIVER same_connection update query! body={}", row.body);
    ensure!(row.body == "updated", "cached query missed update");
    println!("DRIVER stats={:?}", reader.sync_stats().await?);
    local.close().await?;
    writer.close().await?;
    reader.close().await?;
    println!("DRIVER PASS checked macros, native Sync persistent read connection");
    Ok(())
}
