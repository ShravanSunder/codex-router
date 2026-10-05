use crate::server_process::SpikeServer;
use anyhow::{Result, ensure};
use std::path::{Path, PathBuf};
use turso::{
    Connection,
    sync::{Builder, Database},
};

pub async fn snapshot(connection: &Connection, sql: &str) -> Result<Vec<Vec<String>>> {
    let mut rows = connection.query(sql, ()).await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        let mut values = Vec::new();
        for column in 0..row.column_count() {
            values.push(format!("{:?}", row.get_value(column)?));
        }
        result.push(values);
    }
    Ok(result)
}

struct SyncPair {
    server: SpikeServer,
    first: Database,
    second: Database,
    first_connection: Connection,
    second_connection: Connection,
    directory: PathBuf,
}

impl SyncPair {
    async fn new(root: &Path, case: &str, seed: &str) -> Result<Self> {
        let server = SpikeServer::start(root, case, true).await?;
        let directory = root.join(case);
        let first = Builder::new_remote(directory.join("first.db").to_str().unwrap())
            .with_remote_url(server.url())
            .build()
            .await?;
        let first_connection = first.connect().await?;
        first_connection.execute_batch(seed).await?;
        first.push().await?;
        let second = Builder::new_remote(directory.join("second.db").to_str().unwrap())
            .with_remote_url(server.url())
            .build()
            .await?;
        let second_connection = second.connect().await?;
        Ok(Self {
            server,
            first,
            second,
            first_connection,
            second_connection,
            directory,
        })
    }

    async fn show(&self, label: &str, sql: &str) -> Result<()> {
        let response = primary_query(&self.server.url(), sql).await?;
        let remote_rows = &response["results"][0]["response"]["result"]["rows"];
        println!(
            "{label} A={:?} B={:?} primary={remote_rows}",
            snapshot(&self.first_connection, sql).await?,
            snapshot(&self.second_connection, sql).await?
        );
        Ok(())
    }
}

async fn last_push(root: &Path) -> Result<()> {
    let mut pair = SyncPair::new(root, "last-push", "CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT); INSERT INTO notes VALUES(1,'seed');").await?;
    pair.server.stop()?;
    pair.first_connection
        .execute("UPDATE notes SET body='offline-A' WHERE id=1", ())
        .await?;
    pair.second_connection
        .execute("UPDATE notes SET body='offline-B' WHERE id=1", ())
        .await?;
    println!(
        "LAST_PUSH offline A={:?} B={:?}",
        snapshot(&pair.first_connection, "SELECT * FROM notes").await?,
        snapshot(&pair.second_connection, "SELECT * FROM notes").await?
    );
    println!("LAST_PUSH down push={:?}", pair.first.push().await);
    pair.server.restart().await?;
    pair.first.push().await?;
    pair.show("LAST_PUSH after A push", "SELECT * FROM notes")
        .await?;
    // Pull with pending B edits must replay them, rather than erase them.
    println!("LAST_PUSH B pull pending={:?}", pair.second.pull().await);
    pair.show("LAST_PUSH after B pull pending", "SELECT * FROM notes")
        .await?;
    pair.second.push().await?;
    pair.first.pull().await?;
    pair.second.pull().await?;
    pair.show("LAST_PUSH final", "SELECT * FROM notes").await?;
    let values = snapshot(&pair.first_connection, "SELECT body FROM notes").await?;
    ensure!(
        values == vec![vec!["Text(\"offline-B\")".to_string()]],
        "last-push-wins observation changed"
    );
    Ok(())
}

async fn uniqueness(root: &Path) -> Result<()> {
    let mut pair = SyncPair::new(
        root,
        "unique",
        "CREATE TABLE accounts(id INTEGER PRIMARY KEY, handle TEXT UNIQUE, body TEXT);",
    )
    .await?;
    pair.server.stop()?;
    pair.first_connection
        .execute("INSERT INTO accounts VALUES(10,'same','A')", ())
        .await?;
    pair.second_connection
        .execute("INSERT INTO accounts VALUES(20,'same','B')", ())
        .await?;
    pair.server.restart().await?;
    println!("UNIQUE A push={:?}", pair.first.push().await);
    println!(
        "UNIQUE B pending before push={:?}",
        pair.second.stats().await?
    );
    println!("UNIQUE B push={:?}", pair.second.push().await);
    println!(
        "UNIQUE B pending after failed push={:?}",
        pair.second.stats().await?
    );
    pair.show("UNIQUE before pull", "SELECT * FROM accounts ORDER BY id")
        .await?;
    println!("UNIQUE A pull={:?}", pair.first.pull().await);
    println!("UNIQUE B pull={:?}", pair.second.pull().await);
    pair.show("UNIQUE final", "SELECT * FROM accounts ORDER BY id")
        .await?;
    println!("UNIQUE B stats={:?}", pair.second.stats().await?);
    ensure!(
        snapshot(&pair.first_connection, "SELECT * FROM accounts").await?
            == snapshot(&pair.second_connection, "SELECT * FROM accounts").await?,
        "unique case did not converge"
    );
    Ok(())
}

async fn delete_edit(root: &Path, delete_last: bool) -> Result<()> {
    let case = if delete_last {
        "delete-last"
    } else {
        "edit-last"
    };
    let mut pair = SyncPair::new(root, case, "CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT); INSERT INTO notes VALUES(1,'seed');").await?;
    pair.server.stop()?;
    pair.first_connection
        .execute("DELETE FROM notes WHERE id=1", ())
        .await?;
    pair.second_connection
        .execute("UPDATE notes SET body='edited-offline' WHERE id=1", ())
        .await?;
    pair.server.restart().await?;
    if delete_last {
        pair.second.push().await?;
        pair.first.push().await?;
    } else {
        pair.first.push().await?;
        pair.second.push().await?;
    }
    println!(
        "DELETE_EDIT {case} A pull={:?} B pull={:?}",
        pair.first.pull().await,
        pair.second.pull().await
    );
    pair.show(&format!("DELETE_EDIT {case} final"), "SELECT * FROM notes")
        .await?;
    ensure!(
        snapshot(&pair.first_connection, "SELECT * FROM notes")
            .await?
            .is_empty()
            && snapshot(&pair.second_connection, "SELECT * FROM notes")
                .await?
                .is_empty(),
        "delete/edit behavior changed"
    );
    Ok(())
}

async fn fencing(root: &Path) -> Result<()> {
    let pair = SyncPair::new(root, "fencing", "CREATE TABLE lease(id INTEGER PRIMARY KEY, epoch INTEGER); INSERT INTO lease VALUES(1,1); CREATE TABLE project(id INTEGER PRIMARY KEY, epoch INTEGER, body TEXT); INSERT INTO project VALUES(1,1,'seed');").await?;
    let create_trigger = pair.first_connection.execute_batch("CREATE TRIGGER fence_insert BEFORE INSERT ON project WHEN NEW.epoch != (SELECT epoch FROM lease WHERE id=1) BEGIN SELECT RAISE(ABORT,'stale epoch'); END; CREATE TRIGGER fence_update BEFORE UPDATE ON project WHEN NEW.epoch != (SELECT epoch FROM lease WHERE id=1) BEGIN SELECT RAISE(ABORT,'stale epoch'); END;").await;
    println!("FENCE create triggers={create_trigger:?}");
    if create_trigger.is_ok() {
        println!("FENCE push trigger={:?}", pair.first.push().await);
        println!("FENCE B pull trigger={:?}", pair.second.pull().await);
    }
    pair.second_connection.execute_batch("UPDATE lease SET epoch=2 WHERE id=1; UPDATE project SET epoch=2,body='successor' WHERE id=1;").await?;
    pair.second.push().await?;
    pair.first_connection
        .execute("UPDATE project SET body='stale-holder' WHERE id=1", ())
        .await?;
    let push = pair.first.push().await;
    println!("FENCE stale push={push:?}");
    pair.show(
        "FENCE trigger definitions",
        "SELECT name,sql FROM sqlite_schema WHERE type='trigger'",
    )
    .await?;
    pair.show("FENCE after stale push", "SELECT * FROM project")
        .await?;
    println!("FENCE stale retry={:?}", pair.first.push().await);
    println!("FENCE stale pull={:?}", pair.first.pull().await);
    pair.show("FENCE after retry/pull", "SELECT * FROM project")
        .await?;
    println!("FENCE stats={:?}", pair.first.stats().await?);
    // Test whether an old holder can overwrite the lease table itself.
    println!(
        "FENCE lease rollback local={:?}",
        pair.first_connection
            .execute("UPDATE lease SET epoch=1 WHERE id=1", ())
            .await
    );
    println!("FENCE lease rollback push={:?}", pair.first.push().await);
    pair.show("FENCE lease after rollback push", "SELECT * FROM lease")
        .await?;
    println!("FENCE files={}", pair.directory.display());
    Ok(())
}

async fn schema(root: &Path) -> Result<()> {
    let pair = SyncPair::new(root, "schema", "CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT); INSERT INTO notes VALUES(1,'seed'); CREATE TABLE migrations(version INTEGER PRIMARY KEY); INSERT INTO migrations VALUES(1);").await?;
    pair.first_connection.execute_batch("BEGIN IMMEDIATE; ALTER TABLE notes ADD COLUMN label TEXT; UPDATE notes SET label='added'; INSERT INTO migrations VALUES(2); COMMIT;").await?;
    println!("SCHEMA add push={:?}", pair.first.push().await);
    println!("SCHEMA add pull={:?}", pair.second.pull().await);
    pair.show("SCHEMA add", "SELECT * FROM notes").await?;
    pair.first_connection.execute_batch("BEGIN IMMEDIATE; ALTER TABLE notes RENAME COLUMN body TO content; UPDATE notes SET content='renamed'; INSERT INTO migrations VALUES(3); COMMIT;").await?;
    println!("SCHEMA rename push={:?}", pair.first.push().await);
    println!("SCHEMA rename pull={:?}", pair.second.pull().await);
    pair.show("SCHEMA rename", "SELECT * FROM notes").await?;
    pair.show(
        "SCHEMA migrations",
        "SELECT * FROM migrations ORDER BY version",
    )
    .await?;
    pair.show(
        "SCHEMA definitions",
        "SELECT name,sql FROM sqlite_schema WHERE name='notes'",
    )
    .await?;
    println!("SCHEMA A stats={:?}", pair.first.stats().await?);
    Ok(())
}

pub async fn run(root: &Path, selected: &str) -> Result<()> {
    // Scenarios keep independent databases: a broken push must not contaminate DDL proof.
    let mut failed = 0;
    for name in [
        "last-push",
        "uniqueness",
        "edit-last",
        "delete-last",
        "fencing",
        "fence-update",
        "fence-update-body",
        "fence-delete",
        "fence-abort",
        "fence-rollback",
        "schema",
        "schema-swap",
    ] {
        if selected != "all" && selected != "sync" && selected != name {
            continue;
        }
        let result = match name {
            "last-push" => last_push(root).await,
            "uniqueness" => uniqueness(root).await,
            "edit-last" => delete_edit(root, false).await,
            "delete-last" => delete_edit(root, true).await,
            "fencing" => fencing(root).await,
            "schema" => schema(root).await,
            "schema-swap" => schema_swap(root).await,
            "fence-update" => fence_update(root, true).await,
            "fence-update-body" => fence_update(root, false).await,
            "fence-delete" => fence_delete(root).await,
            "fence-abort" => fence_failure(root, false).await,
            "fence-rollback" => fence_failure(root, true).await,
            _ => unreachable!(),
        };
        println!("SYNC_SCENARIO {name}: {result:?}");
        if result.is_err() {
            failed += 1;
        }
    }
    ensure!(failed == 0, "{failed} sync scenarios failed");
    Ok(())
}

pub async fn primary_query(url: &str, sql: &str) -> Result<serde_json::Value> {
    let body = serde_json::json!({"requests": [{"type": "execute", "stmt": {"sql": sql, "args": [], "want_rows": true}}]}).to_string();
    let output = tokio::process::Command::new("curl")
        .args([
            "--fail-with-body",
            "--silent",
            "--show-error",
            "--max-time",
            "10",
            "-H",
            "Content-Type: application/json",
            "--data-binary",
            &body,
            &format!("{url}/v2/pipeline"),
        ])
        .output()
        .await?;
    ensure!(
        output.status.success(),
        "primary HTTP {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout)
    );
    let response: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    ensure!(
        response["results"][0]["type"] == "ok",
        "primary SQL error: {response}"
    );
    Ok(response)
}

async fn schema_swap(root: &Path) -> Result<()> {
    let pair = SyncPair::new(root, "schema-swap", "CREATE TABLE notes(id INTEGER PRIMARY KEY, a TEXT, b TEXT); INSERT INTO notes VALUES(1,'A1','B1');").await?;
    pair.first_connection.execute_batch("BEGIN IMMEDIATE; ALTER TABLE notes RENAME COLUMN a TO temporary; ALTER TABLE notes RENAME COLUMN b TO a; ALTER TABLE notes RENAME COLUMN temporary TO b; COMMIT;").await?;
    println!("SCHEMA_SWAP push={:?}", pair.first.push().await);
    println!("SCHEMA_SWAP pull={:?}", pair.second.pull().await);
    pair.show("SCHEMA_SWAP final", "SELECT id,a,b FROM notes")
        .await?;
    pair.show(
        "SCHEMA_SWAP definitions",
        "SELECT name,sql FROM sqlite_schema WHERE name='notes'",
    )
    .await?;
    Ok(())
}

async fn fence_failure(root: &Path, rollback: bool) -> Result<()> {
    let case = if rollback {
        "fence-rollback"
    } else {
        "fence-abort"
    };
    let pair = SyncPair::new(root, case, "CREATE TABLE lease(id INTEGER PRIMARY KEY, epoch INTEGER); INSERT INTO lease VALUES(1,1); CREATE TABLE events(id INTEGER PRIMARY KEY, epoch INTEGER, body TEXT);").await?;
    let action = if rollback { "ROLLBACK" } else { "ABORT" };
    let sql = format!(
        "CREATE TRIGGER fence_event BEFORE INSERT ON events WHEN NEW.epoch != (SELECT epoch FROM lease WHERE id=1) BEGIN SELECT RAISE({action},'stale epoch'); END"
    );
    primary_query(&pair.server.url(), &sql).await?;
    pair.first.pull().await?;
    pair.second.pull().await?;
    pair.show(
        &format!("{case} installed server trigger"),
        "SELECT name FROM sqlite_schema WHERE type='trigger'",
    )
    .await?;
    pair.second_connection
        .execute_batch(
            "UPDATE lease SET epoch=2 WHERE id=1; INSERT INTO events VALUES(2,2,'successor');",
        )
        .await?;
    pair.second.push().await?;
    // A has old epoch=1, and admits a local append; INSERT carries epoch on wire.
    pair.first_connection
        .execute("INSERT INTO events VALUES(1,1,'stale-append')", ())
        .await?;
    println!("{case} pending before push={:?}", pair.first.stats().await?);
    println!("{case} stale push={:?}", pair.first.push().await);
    println!("{case} pending after push={:?}", pair.first.stats().await?);
    pair.show(
        &format!("{case} after push"),
        "SELECT * FROM events ORDER BY id",
    )
    .await?;
    println!("{case} stale retry={:?}", pair.first.push().await);
    println!("{case} stale pull={:?}", pair.first.pull().await);
    println!("{case} pending after pull={:?}", pair.first.stats().await?);
    pair.show(
        &format!("{case} after pull"),
        "SELECT * FROM events ORDER BY id",
    )
    .await?;
    // A new authorized append still works after rejection.
    pair.second_connection
        .execute("INSERT INTO events VALUES(3,2,'fresh-after-reject')", ())
        .await?;
    println!("{case} current holder push={:?}", pair.second.push().await);
    pair.show(
        &format!("{case} current holder final"),
        "SELECT * FROM events ORDER BY id",
    )
    .await?;
    Ok(())
}

async fn fence_update(root: &Path, include_epoch: bool) -> Result<()> {
    let case = if include_epoch {
        "fence-update"
    } else {
        "fence-update-body"
    };
    let pair = SyncPair::new(root, case, "CREATE TABLE lease(id INTEGER PRIMARY KEY, epoch INTEGER); INSERT INTO lease VALUES(1,1); CREATE TABLE project(id INTEGER PRIMARY KEY, epoch INTEGER, body TEXT); INSERT INTO project VALUES(1,1,'seed');").await?;
    primary_query(&pair.server.url(), "CREATE TRIGGER fence_project BEFORE UPDATE ON project WHEN NEW.epoch != (SELECT epoch FROM lease WHERE id=1) BEGIN SELECT RAISE(ROLLBACK,'stale epoch'); END").await?;
    pair.first.pull().await?;
    pair.second.pull().await?;
    pair.show(
        "FENCE_UPDATE installed trigger",
        "SELECT name FROM sqlite_schema WHERE type='trigger'",
    )
    .await?;
    pair.second_connection.execute_batch("UPDATE lease SET epoch=2 WHERE id=1; UPDATE project SET epoch=2,body='successor' WHERE id=1;").await?;
    pair.second.push().await?;
    let stale_sql = if include_epoch {
        "UPDATE project SET epoch=1,body='stale-holder' WHERE id=1"
    } else {
        "UPDATE project SET body='stale-holder' WHERE id=1"
    };
    pair.first_connection.execute(stale_sql, ()).await?;
    println!(
        "FENCE_UPDATE {case} stale push={:?}",
        pair.first.push().await
    );
    pair.show("FENCE_UPDATE final", "SELECT * FROM project")
        .await?;
    Ok(())
}

async fn fence_delete(root: &Path) -> Result<()> {
    let pair = SyncPair::new(root, "fence-delete", "CREATE TABLE lease(id INTEGER PRIMARY KEY, epoch INTEGER); INSERT INTO lease VALUES(1,1); CREATE TABLE project(id INTEGER PRIMARY KEY, epoch INTEGER, body TEXT); INSERT INTO project VALUES(1,1,'seed');").await?;
    primary_query(&pair.server.url(), "CREATE TRIGGER fence_project_delete BEFORE DELETE ON project WHEN OLD.epoch != (SELECT epoch FROM lease WHERE id=1) BEGIN SELECT RAISE(ROLLBACK,'stale epoch'); END").await?;
    pair.first.pull().await?;
    pair.second.pull().await?;
    pair.show(
        "FENCE_DELETE installed trigger",
        "SELECT name FROM sqlite_schema WHERE type='trigger'",
    )
    .await?;
    pair.second_connection.execute_batch("UPDATE lease SET epoch=2 WHERE id=1; UPDATE project SET epoch=2,body='successor' WHERE id=1;").await?;
    pair.second.push().await?;
    pair.first_connection
        .execute("DELETE FROM project WHERE id=1", ())
        .await?;
    println!("FENCE_DELETE stale push={:?}", pair.first.push().await);
    pair.show("FENCE_DELETE final", "SELECT * FROM project")
        .await?;
    Ok(())
}
