use crate::{
    lease_gateway::{LeaseClaim, LeaseGateway},
    server_process::SpikeServer,
};
use anyhow::{Result, ensure};
use std::{
    path::{Path, PathBuf},
    time::Instant,
};
use turso::{Connection, sync::Database};

pub async fn read_records(connection: &Connection) -> Result<Vec<(i64, String)>> {
    let mut rows = connection
        .query("SELECT sequence,body FROM records ORDER BY sequence", ())
        .await?;
    let mut records = Vec::new();
    while let Some(row) = rows.next().await? {
        records.push((row.get(0)?, row.get(1)?));
    }
    Ok(records)
}
async fn build(path: &Path, url: &str, token: &str) -> Result<Database> {
    Ok(turso::sync::Builder::new_remote(path.to_str().unwrap())
        .with_remote_url(url)
        .with_auth_token(token)
        .build()
        .await?)
}
struct ProjectCopies {
    _server: SpikeServer,
    gateway: LeaseGateway,
    directory: PathBuf,
    writer: Database,
    writer_connection: Connection,
    reader: Database,
    reader_connection: Connection,
}
impl ProjectCopies {
    async fn new(root: &Path, scenario: &str) -> Result<Self> {
        let server = SpikeServer::start(root, scenario, true).await?;
        let directory = root.join(scenario);
        let gateway =
            LeaseGateway::start(&directory.join("fleet-control.db"), server.url()).await?;
        let writer = build(&directory.join("A.db"), &gateway.url, "a-epoch-1").await?;
        let writer_connection = writer.connect().await?;
        writer_connection.execute_batch("CREATE TABLE records(sequence INTEGER PRIMARY KEY, body TEXT NOT NULL); CREATE TABLE schema_version(singleton INTEGER PRIMARY KEY, version INTEGER NOT NULL); INSERT INTO schema_version VALUES(1,1);").await?;
        writer.push().await?;
        let reader = build(&directory.join("B.db"), &gateway.url, "reader-b").await?;
        let reader_connection = reader.connect().await?;
        Ok(Self {
            _server: server,
            gateway,
            directory,
            writer,
            writer_connection,
            reader,
            reader_connection,
        })
    }
    async fn append(&self, sequence: i64) -> Result<()> {
        self.gateway
            .admit_local(&LeaseClaim {
                holder: "A".into(),
                epoch: 1,
                writable: true,
            })
            .await?;
        self.writer_connection
            .execute(
                "INSERT INTO records VALUES(?,?)",
                turso::params![sequence, format!("A-{sequence}")],
            )
            .await?;
        Ok(())
    }
}
async fn steady(root: &Path) -> Result<()> {
    let copies = ProjectCopies::new(root, "steady").await?;
    for batch in 0..3 {
        for sequence in batch * 5 + 1..=batch * 5 + 5 {
            copies.append(sequence).await?;
        }
        println!(
            "STEADY batch={batch} committed_local={} reader_before_push={:?}",
            read_records(&copies.writer_connection).await?.len(),
            read_records(&copies.reader_connection).await?
        );
        let start = Instant::now();
        copies.writer.push().await?;
        println!(
            "STEADY push_us={} reader_before_pull={:?}",
            start.elapsed().as_micros(),
            read_records(&copies.reader_connection).await?
        );
        let start = Instant::now();
        let changed = copies.reader.pull().await?;
        let writer = read_records(&copies.writer_connection).await?;
        let reader = read_records(&copies.reader_connection).await?;
        println!(
            "STEADY pull={changed} pull_us={} rows={reader:?}",
            start.elapsed().as_micros()
        );
        ensure!(writer == reader, "read copies did not catch up");
    }
    Ok(())
}
async fn clean(root: &Path) -> Result<()> {
    let copies = ProjectCopies::new(root, "clean").await?;
    for sequence in 1..=10 {
        copies.append(sequence).await?;
    }
    copies.writer.push().await?;
    let pending = copies.writer.stats().await?.cdc_operations;
    println!("CLEAN drained pending={pending}");
    ensure!(pending == 0, "dirty release");
    copies.gateway.transfer(true).await?;
    let refused = copies
        .gateway
        .admit_local(&LeaseClaim {
            holder: "A".into(),
            epoch: 1,
            writable: true,
        })
        .await;
    println!("CLEAN old holder local admission={refused:?}");
    ensure!(refused.is_err(), "old holder admitted");
    drop(copies.reader_connection);
    drop(copies.reader);
    let next = build(
        &copies.directory.join("B.db"),
        &copies.gateway.url,
        "b-epoch-2",
    )
    .await?;
    println!("CLEAN B catch_up={:?}", next.pull().await?);
    let connection = next.connect().await?;
    ensure!(
        read_records(&connection).await?.len() == 10,
        "B missing drain"
    );
    for sequence in 11..=15 {
        connection
            .execute(
                "INSERT INTO records VALUES(?,?)",
                turso::params![sequence, format!("B-{sequence}")],
            )
            .await?;
    }
    next.push().await?;
    // Drop writer capability, reopen old machine through a reader capability.
    drop(copies.writer_connection);
    drop(copies.writer);
    let old = build(
        &copies.directory.join("A.db"),
        &copies.gateway.url,
        "reader-a",
    )
    .await?;
    old.pull().await?;
    let old_connection = old.connect().await?;
    let rows = read_records(&old_connection).await?;
    let next_rows = read_records(&connection).await?;
    ensure!(
        rows == next_rows && rows.iter().map(|row| row.0).eq(1..=15),
        "loss/duplicate across clean switch"
    );
    println!(
        "CLEAN final old_reader={rows:?} new_writer={next_rows:?}; exact sequence 1..15, no loss/duplicates"
    );
    Ok(())
}
async fn unclean(root: &Path) -> Result<()> {
    let copies = ProjectCopies::new(root, "unclean").await?;
    for sequence in 1..=5 {
        copies.append(sequence).await?;
    }
    copies.writer.push().await?;
    drop(copies.writer_connection);
    drop(copies.writer);
    let mut child =
        tokio::process::Command::new("spikes/turso-replication/target/debug/offline-writer")
            .arg(copies.directory.join("A.db"))
            .arg(&copies.gateway.url)
            .stdout(std::process::Stdio::piped())
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
    let output = child.stdout.take().unwrap();
    let mut lines = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(output));
    let receipt = tokio::time::timeout(std::time::Duration::from_secs(20), lines.next_line())
        .await??
        .ok_or_else(|| anyhow::anyhow!("offline writer exited without durable receipt"))?;
    ensure!(
        receipt.starts_with("TAIL_DURABLE pending=3"),
        "unexpected child receipt: {receipt}"
    );
    println!("UNCLEAN child before kill {receipt}");
    let child_pid = child.id();
    child.kill().await?;
    println!(
        "UNCLEAN holder killed pid={:?} status={}",
        child_pid,
        child.wait().await?
    );
    let tail_before: Vec<_> = (1..=8)
        .map(|sequence| (sequence, format!("A-{sequence}")))
        .collect();
    copies.gateway.expire_clock();
    copies.gateway.transfer(false).await?;
    drop(copies.reader_connection);
    drop(copies.reader);
    let next = build(
        &copies.directory.join("B.db"),
        &copies.gateway.url,
        "b-epoch-2",
    )
    .await?;
    next.pull().await?;
    let connection = next.connect().await?;
    println!(
        "UNCLEAN new holder hydrated={:?}",
        read_records(&connection).await?
    );
    ensure!(
        read_records(&connection).await?.len() == 5,
        "B inherited unpushed tail"
    );
    for sequence in 6..=7 {
        connection
            .execute(
                "INSERT INTO records VALUES(?,?)",
                turso::params![sequence, format!("B-{sequence}")],
            )
            .await?;
    }
    next.push().await?;
    let current = read_records(&connection).await?;
    let old = build(
        &copies.directory.join("A.db"),
        &copies.gateway.url,
        "a-epoch-1",
    )
    .await?;
    let old_connection = old.connect().await?;
    println!(
        "UNCLEAN old return before push rows={:?} pending={}",
        read_records(&old_connection).await?,
        old.stats().await?.cdc_operations
    );
    let counters_before = copies.gateway.counters();
    let refused = old.push().await;
    println!(
        "UNCLEAN stale push={refused:?} gateway_before={counters_before:?} after={:?}",
        copies.gateway.counters()
    );
    ensure!(
        refused.is_err() && copies.gateway.counters().0 == counters_before.0,
        "stale request forwarded"
    );
    let after = read_records(&old_connection).await?;
    ensure!(after == tail_before, "refused tail changed");
    println!(
        "UNCLEAN after refusal old rows={after:?} pending={}",
        old.stats().await?.cdc_operations
    );
    next.pull().await?;
    ensure!(
        read_records(&connection).await? == current,
        "stale tail changed primary/new holder"
    );
    // Preserve entire original sync DB, CDC and sidecars. Never pull the rejected dirty file.
    let rejected: Vec<_> = after
        .iter()
        .filter(|row| row.0 > 5)
        .map(|row| serde_json::json!({"sequence":row.0,"body":row.1}))
        .collect();
    let receipt = serde_json::json!({"status":"rejected_stale_epoch","holder":"A","epoch":1,"current_epoch":2,"database":copies.directory.join("A.db"),"last_replicated_sequence":5,"tail":rejected});
    std::fs::write(
        copies.directory.join("rejected-tail.json"),
        serde_json::to_string_pretty(&receipt)?,
    )?;
    old.checkpoint().await?;
    drop(old_connection);
    drop(old);
    let archived = turso::Builder::new_local(copies.directory.join("A.db").to_str().unwrap())
        .build()
        .await?;
    let archive_connection = archived.connect()?;
    ensure!(
        read_records(&archive_connection).await? == tail_before,
        "reopened rejected database differs"
    );
    println!(
        "UNCLEAN retained rejected DB readable={:?} rejected_receipt={receipt}",
        read_records(&archive_connection).await?
    );
    let reset = build(
        &copies.directory.join("A-current.db"),
        &copies.gateway.url,
        "reader-a",
    )
    .await?;
    let reset_connection = reset.connect().await?;
    ensure!(
        read_records(&reset_connection).await? == current,
        "reset did not hydrate authoritative state"
    );
    let active_path = copies.directory.join("A-active.json");
    std::fs::write(
        &active_path,
        serde_json::to_string_pretty(
            &serde_json::json!({"role":"reader","active_database":"A-current.db","rejected_database":"A.db","rejected_receipt":"rejected-tail.json"}),
        )?,
    )?;
    println!(
        "UNCLEAN machine active read-copy pointer={}",
        active_path.display()
    );
    println!(
        "UNCLEAN reset new read copy={:?}; retained rejected A.db still={:?}",
        read_records(&reset_connection).await?,
        read_records(&archive_connection).await?
    );
    Ok(())
}
async fn schema(root: &Path) -> Result<()> {
    let copies = ProjectCopies::new(root, "schema-switch").await?;
    copies.append(1).await?;
    copies.writer.push().await?;
    copies.writer_connection.execute_batch("BEGIN IMMEDIATE; ALTER TABLE records ADD COLUMN label TEXT; UPDATE records SET label='v2'; UPDATE schema_version SET version=2 WHERE singleton=1; COMMIT;").await?;
    copies.writer.push().await?;
    copies.reader.pull().await?;
    let version: i64 = copies
        .reader_connection
        .query("SELECT version FROM schema_version WHERE singleton=1", ())
        .await?
        .next()
        .await?
        .unwrap()
        .get(0)?;
    println!("SCHEMA add replicated version={version}");
    ensure!(version == 2, "version not replicated");
    ensure!(
        copies.writer.stats().await?.cdc_operations == 0,
        "schema upgrade not drained"
    );
    copies.gateway.transfer(true).await?;
    let old_router = writer_schema_gate(&copies.reader_connection, 1).await;
    println!("SCHEMA switched old Router supported=1 admission={old_router:?}");
    ensure!(
        old_router.is_err() && copies.reader.stats().await?.cdc_operations == 0,
        "old Router admitted write"
    );
    // Rename on a fresh project; never release a holder with a failed/undrained migration.
    let rename = ProjectCopies::new(root, "schema-rename").await?;
    rename.append(1).await?;
    rename.writer.push().await?;
    rename.writer_connection.execute_batch("BEGIN IMMEDIATE; ALTER TABLE records RENAME COLUMN body TO content; UPDATE records SET content='renamed'; UPDATE schema_version SET version=3 WHERE singleton=1; COMMIT;").await?;
    let renamed = rename.writer.push().await;
    println!("SCHEMA rename push={renamed:?}");
    rename.reader.pull().await?;
    println!(
        "SCHEMA rename writer_schema={:?} reader_schema={:?}",
        table_definition(&rename.writer_connection, "records").await?,
        table_definition(&rename.reader_connection, "records").await?
    );
    let remote_version: i64 = rename
        .reader_connection
        .query("SELECT version FROM schema_version WHERE singleton=1", ())
        .await?
        .next()
        .await?
        .unwrap()
        .get(0)?;
    println!(
        "SCHEMA rename peer version={remote_version}; no lease release after failed migration"
    );
    if renamed.is_err() {
        schema_rebuild(root).await?;
    }

    Ok(())
}
async fn writer_schema_gate(connection: &Connection, supported: i64) -> Result<()> {
    let version: i64 = connection
        .query("SELECT version FROM schema_version WHERE singleton=1", ())
        .await?
        .next()
        .await?
        .unwrap()
        .get(0)?;
    ensure!(
        version == supported,
        "schema version {version} incompatible with Router version {supported}"
    );
    Ok(())
}
pub async fn run(root: &Path, selected: &str) -> Result<()> {
    for scenario in ["steady", "clean", "unclean", "schema"] {
        if selected != "all" && selected != scenario {
            continue;
        }
        match scenario {
            "steady" => steady(root).await?,
            "clean" => clean(root).await?,
            "unclean" => unclean(root).await?,
            "schema" => schema(root).await?,
            _ => unreachable!(),
        }
        println!("SINGLE_WRITER {scenario} PASS");
    }
    Ok(())
}

async fn table_definition(connection: &Connection, name: &str) -> Result<String> {
    Ok(connection
        .query(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?",
            [name],
        )
        .await?
        .next()
        .await?
        .ok_or_else(|| anyhow::anyhow!("missing table {name}"))?
        .get(0)?)
}
async fn schema_rebuild(root: &Path) -> Result<()> {
    let copies = ProjectCopies::new(root, "schema-rebuild").await?;
    for sequence in 1..=3 {
        copies.append(sequence).await?;
    }
    copies.writer.push().await?;
    let rows = read_records(&copies.writer_connection).await?;
    copies.writer_connection.execute_batch("BEGIN IMMEDIATE; DROP TABLE records; CREATE TABLE records(sequence INTEGER PRIMARY KEY, content TEXT NOT NULL); ").await?;
    for (sequence, body) in &rows {
        copies
            .writer_connection
            .execute(
                "INSERT INTO records VALUES(?,?)",
                turso::params![*sequence, body.clone()],
            )
            .await?;
    }
    copies
        .writer_connection
        .execute_batch("UPDATE schema_version SET version=3 WHERE singleton=1; COMMIT;")
        .await?;
    let pushed = copies.writer.push().await;
    println!("SCHEMA_REBUILD DROP/CREATE same-name push={pushed:?}");
    let pulled = copies.reader.pull().await;
    println!("SCHEMA_REBUILD reader pull={pulled:?}");
    println!(
        "SCHEMA_REBUILD writer_schema={:?} reader_schema={:?}",
        table_definition(&copies.writer_connection, "records").await?,
        table_definition(&copies.reader_connection, "records").await?
    );
    let query = copies
        .reader_connection
        .query("SELECT sequence,content FROM records ORDER BY sequence", ())
        .await;
    match query {
        Ok(mut values) => {
            let mut result: Vec<(i64, String)> = Vec::new();
            while let Some(row) = values.next().await? {
                result.push((row.get(0)?, row.get(1)?));
            }
            println!("SCHEMA_REBUILD peer content={result:?}");
            ensure!(result == rows, "rebuild lost records");
        }
        Err(error) => println!("SCHEMA_REBUILD peer content ERROR={error}"),
    }
    copies.gateway.transfer(true).await?;
    let blocked = writer_schema_gate(&copies.reader_connection, 1).await;
    println!("SCHEMA_REBUILD old Router admission={blocked:?}");
    ensure!(blocked.is_err(), "version skew admitted");
    Ok(())
}
