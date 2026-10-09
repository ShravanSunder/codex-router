//! Opening the Turso engine connection that a [`TursoConnection`](crate::TursoConnection) wraps
//!
//! A local store opens `turso::Database` directly. A synced store opens through Turso's
//! high-level Sync builder, which starts one `turso-sync-io` thread with its own small runtime
//! per handle; dropping the handle ends it. Both paths apply `busy_timeout` and
//! `foreign_keys` before the connection is returned.

use std::{io, path::Path};

use sqlx_core::error::Error;

use crate::{
    TursoConnectOptions, TursoDatabaseTarget, connect_options::OpenMode, error::map_turso_error,
};

/// The raw engine connection and, for synced stores, the Sync database handle
pub(crate) struct EngineConnection {
    pub(crate) raw: turso::Connection,
    #[cfg(feature = "sync")]
    pub(crate) sync: Option<turso::sync::Database>,
}

impl EngineConnection {
    pub(crate) async fn open(options: &TursoConnectOptions) -> Result<Self, Error> {
        let path = engine_path(options).await?;

        let engine = match options.sync_options() {
            Some(sync) => open_synced(&path, sync).await?,
            None => open_local(&path).await?,
        };
        apply_connection_settings(options, &engine.raw).await?;
        Ok(engine)
    }
}

/// The path string the engine opens: `:memory:` or a file that exists or may be created
async fn engine_path(options: &TursoConnectOptions) -> Result<String, Error> {
    match options.target() {
        TursoDatabaseTarget::Memory => Ok(":memory:".to_owned()),
        TursoDatabaseTarget::File(path) => {
            ensure_file_may_open(options.open_mode(), path).await?;
            Ok(path.to_string_lossy().into_owned())
        }
    }
}

async fn ensure_file_may_open(open_mode: OpenMode, path: &Path) -> Result<(), Error> {
    match open_mode {
        OpenMode::CreateIfMissing => Ok(()),
        OpenMode::ReadWrite if tokio::fs::try_exists(path).await? => Ok(()),
        OpenMode::ReadWrite => Err(Error::Io(io::Error::new(
            io::ErrorKind::NotFound,
            format!("database file {} does not exist", path.display()),
        ))),
    }
}

async fn open_local(path: &str) -> Result<EngineConnection, Error> {
    let database = turso::Builder::new_local(path)
        .build()
        .await
        .map_err(map_turso_error)?;
    let raw = database.connect().map_err(map_turso_error)?;
    Ok(EngineConnection {
        raw,
        #[cfg(feature = "sync")]
        sync: None,
    })
}

#[cfg(feature = "sync")]
async fn open_synced(
    path: &str,
    sync: &crate::TursoSyncOptions,
) -> Result<EngineConnection, Error> {
    let mut builder = turso::sync::Builder::new_remote(path)
        .with_remote_url(sync.remote_url())
        .bootstrap_if_empty(sync.bootstrap_if_empty());
    if let Some(auth_token) = sync.auth_token() {
        builder = builder.with_auth_token(auth_token);
    }
    if let Some(client_name) = sync.client_name() {
        builder = builder.with_client_name(client_name);
    }
    if let Some(timeout) = sync.long_poll_timeout() {
        builder = builder.with_long_poll_timeout(timeout);
    }

    let database = builder.build().await.map_err(map_turso_error)?;
    let raw = database.connect().await.map_err(map_turso_error)?;
    Ok(EngineConnection {
        raw,
        sync: Some(database),
    })
}

#[cfg(not(feature = "sync"))]
async fn open_synced(
    _path: &str,
    _sync: &crate::TursoSyncOptions,
) -> Result<EngineConnection, Error> {
    Err(crate::TursoAdapterError::SyncFeatureDisabled.into())
}

async fn apply_connection_settings(
    options: &TursoConnectOptions,
    connection: &turso::Connection,
) -> Result<(), Error> {
    connection
        .pragma_update("busy_timeout", options.get_busy_timeout().as_millis())
        .await
        .map_err(map_turso_error)?;
    let foreign_keys = if options.get_foreign_keys() {
        "ON"
    } else {
        "OFF"
    };
    connection
        .pragma_update("foreign_keys", foreign_keys)
        .await
        .map_err(map_turso_error)?;
    Ok(())
}
