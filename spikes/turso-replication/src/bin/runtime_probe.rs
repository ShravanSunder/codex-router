//! Narrow public lower-SDK proof: IO requests run on the application's Tokio runtime.
#[path = "../server_process.rs"]
mod server_process;
use anyhow::{Result, ensure};
use local_sdk::rsapi::{TursoConnection, TursoDatabaseConfig, TursoStatusCode};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use sync_sdk::{
    rsapi::{TursoDatabaseSync, TursoDatabaseSyncConfig},
    sync_engine_io::SyncEngineIoRequest,
    turso_async_operation::{TursoAsyncOperationResult, TursoDatabaseAsyncOperation},
};
struct ApplicationSyncIo {
    client: reqwest::Client,
    base_url: String,
}
impl ApplicationSyncIo {
    async fn drive(
        &self,
        database: &Arc<TursoDatabaseSync<Vec<u8>>>,
        operation: Box<TursoDatabaseAsyncOperation>,
    ) -> Result<Option<TursoAsyncOperationResult>> {
        loop {
            match operation
                .resume()
                .map_err(|error| anyhow::anyhow!("{error:?}"))?
            {
                TursoStatusCode::Done => return Ok(operation.take_result().ok()),
                TursoStatusCode::Io => {
                    while let Some(item) = database.take_io_item() {
                        let (request, completion) = item.into_parts();
                        match request {
                            SyncEngineIoRequest::Http {
                                url,
                                method,
                                path,
                                body,
                                headers,
                            } => {
                                let url = format!(
                                    "{}{path}",
                                    url.as_deref()
                                        .unwrap_or(&self.base_url)
                                        .trim_end_matches('/')
                                );
                                let mut request = self.client.request(method.parse()?, url);
                                for (name, value) in headers {
                                    request = request.header(name, value);
                                }
                                if let Some(body) = body {
                                    request = request.body(body);
                                }
                                let response = request.send().await?;
                                completion.status(response.status().as_u16().into());
                                completion.push_buffer(response.bytes().await?.to_vec());
                                completion.done();
                            }
                            SyncEngineIoRequest::FullRead { path } => {
                                match tokio::fs::read(path).await {
                                    Ok(bytes) => {
                                        completion.status(200);
                                        completion.push_buffer(bytes);
                                    }
                                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                                        completion.status(404)
                                    }
                                    Err(error) => {
                                        completion.poison(error.to_string());
                                    }
                                }
                                completion.done();
                            }
                            SyncEngineIoRequest::FullWrite { path, content } => {
                                let temporary = format!("{path}.application-io-temp");
                                let mut file = tokio::fs::File::create(&temporary).await?;
                                tokio::io::AsyncWriteExt::write_all(&mut file, &content).await?;
                                file.sync_all().await?;
                                drop(file);
                                tokio::fs::rename(&temporary, &path).await?;
                                if let Some(parent) = Path::new(&path).parent() {
                                    let directory = std::fs::File::open(parent)?;
                                    directory.sync_all()?;
                                }
                                completion.status(200);
                                completion.done();
                            }
                        }
                    }
                    database.step_io_callbacks();
                }
                status => anyhow::bail!("unexpected sync operation status {status:?}"),
            }
        }
    }
    async fn open(&self, path: &Path) -> Result<Arc<TursoDatabaseSync<Vec<u8>>>> {
        let path = path.to_str().unwrap().to_string();
        let config = TursoDatabaseConfig {
            path: path.clone(),
            experimental_features: None,
            async_io: false,
            encryption: None,
            vfs: Default::default(),
            io: None,
            db_file: None,
            page_codec: None,
            open_flags: Default::default(),
        };
        let sync = TursoDatabaseSyncConfig {
            remote_url: Some(self.base_url.clone()),
            path,
            client_name: "application-runtime-probe".into(),
            long_poll_timeout_ms: None,
            bootstrap_if_empty: true,
            reserved_bytes: None,
            partial_sync_opts: None,
            remote_encryption_key: None,
            push_operations_threshold: None,
            pull_bytes_threshold: None,
            logical_mvcc_pull: None,
        };
        let database =
            TursoDatabaseSync::new(config, sync).map_err(|error| anyhow::anyhow!("{error:?}"))?;
        self.drive(&database, database.create()).await?;
        Ok(database)
    }
    async fn connection(
        &self,
        database: &Arc<TursoDatabaseSync<Vec<u8>>>,
    ) -> Result<Arc<TursoConnection>> {
        match self.drive(database, database.connect()).await? {
            Some(TursoAsyncOperationResult::Connection { connection }) => Ok(connection),
            _ => anyhow::bail!("missing connection"),
        }
    }
    async fn pull(&self, database: &Arc<TursoDatabaseSync<Vec<u8>>>) -> Result<bool> {
        match self.drive(database, database.wait_changes()).await? {
            Some(TursoAsyncOperationResult::Changes { changes }) if !changes.empty() => {
                self.drive(database, database.apply_changes(changes))
                    .await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
fn sql(connection: &Arc<TursoConnection>, query: &str) -> Result<Vec<Vec<String>>> {
    let mut statement = connection
        .prepare_single(query)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let mut rows = Vec::new();
    loop {
        match statement
            .step(None)
            .map_err(|error| anyhow::anyhow!("{error:?}"))?
        {
            TursoStatusCode::Done => break,
            TursoStatusCode::Row => {
                let mut values = Vec::new();
                for column in 0..statement.column_count() {
                    values.push(format!(
                        "{:?}",
                        statement
                            .row_value(column)
                            .map_err(|error| anyhow::anyhow!("{error:?}"))?
                    ));
                }
                rows.push(values);
            }
            status => anyhow::bail!("unexpected SQL status {status:?}"),
        }
    }
    statement
        .finalize(None)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    Ok(rows)
}
#[tokio::main]
async fn main() -> Result<()> {
    let root = PathBuf::from(format!(
        "tmp/turso-runtime/{}-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
        std::process::id()
    ));
    std::fs::create_dir_all(&root)?;
    println!(
        "RUNTIME lower SDK 0.8.1 application runtime only; current Tokio available={}",
        tokio::runtime::Handle::try_current().is_ok()
    );
    let server = server_process::SpikeServer::start(&root, "hub", true).await?;
    let io = ApplicationSyncIo {
        client: reqwest::Client::new(),
        base_url: server.url(),
    };
    let writer = io.open(&root.join("writer.db")).await?;
    let connection = io.connection(&writer).await?;
    sql(
        &connection,
        "CREATE TABLE records(id INTEGER PRIMARY KEY,body TEXT)",
    )?;
    sql(&connection, "INSERT INTO records VALUES(1,'first')")?;
    io.drive(&writer, writer.push_changes()).await?;
    let reader = io.open(&root.join("reader.db")).await?;
    let reader_connection = io.connection(&reader).await?;
    println!(
        "RUNTIME bootstrapped reader={:?}",
        sql(&reader_connection, "SELECT * FROM records")?
    );
    sql(&connection, "UPDATE records SET body='second' WHERE id=1")?;
    io.drive(&writer, writer.push_changes()).await?;
    println!(
        "RUNTIME before pull={:?}",
        sql(&reader_connection, "SELECT * FROM records")?
    );
    let changed = io.pull(&reader).await?;
    let rows = sql(&reader_connection, "SELECT * FROM records")?;
    println!("RUNTIME pull={changed} reader={rows:?}");
    ensure!(
        format!("{rows:?}").contains("second"),
        "lower SDK pull missed update"
    );
    println!(
        "RUNTIME PASS no turso::sync Builder / IoWorker / runtime constructor invoked; HTTP await and file IO owned by application"
    );
    Ok(())
}
