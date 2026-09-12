//! Project and repository association operations.
use crate::BoardStore;
use crate::project_records::{decode_metadata_cursor, require_project, trim_metadata_page};
use crate::storage_support::{invalid_record, storage_error};
use project_board::*;
use serde::Serialize;
use sqlx::Connection;

#[derive(Serialize)]
struct RawRepository {
    repository_key: String,
    kind: String,
    origin: Option<String>,
    service_id: Option<String>,
    common_directory: Option<String>,
}

struct RepositoryParts<'a> {
    key: String,
    kind: &'static str,
    origin: Option<&'a str>,
    service_id: Option<&'a str>,
    common_directory: Option<&'a str>,
}

impl BoardStore {
    pub async fn attach_repository(
        &mut self,
        request: RepositoryAttachRequest,
    ) -> Result<RepositoryAttachResult, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        require_project(&mut transaction, &request.project_id).await?;
        let parts = repository_parts(&request.repository);
        let result = sqlx::query!(
            "INSERT INTO project_repositories(project_id,repository_key,kind,origin,service_id,common_directory) \
             VALUES(?,?,?,?,?,?) ON CONFLICT(project_id,repository_key) DO NOTHING",
            request.project_id.as_str(),
            parts.key,
            parts.kind,
            parts.origin,
            parts.service_id,
            parts.common_directory,
        )
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let inserted = result.rows_affected() == 1;
        transaction.commit().await.map_err(storage_error)?;
        Ok(RepositoryAttachResult {
            project_id: request.project_id,
            repository: request.repository,
            attached: true,
            outcome: if inserted {
                "Repository attached."
            } else {
                "Repository was already attached."
            }
            .to_owned(),
        })
    }

    pub async fn detach_repository(
        &mut self,
        request: RepositoryDetachRequest,
    ) -> Result<RepositoryDetachResult, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        require_project(&mut transaction, &request.project_id).await?;
        let key = repository_key(&request.repository);
        let result = sqlx::query!(
            "DELETE FROM project_repositories WHERE project_id=? AND repository_key=?",
            request.project_id.as_str(),
            key,
        )
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let removed = result.rows_affected() == 1;
        transaction.commit().await.map_err(storage_error)?;
        Ok(RepositoryDetachResult {
            project_id: request.project_id,
            repository: request.repository,
            attached: false,
            outcome: if removed {
                "Repository detached."
            } else {
                "Repository was already detached."
            }
            .to_owned(),
        })
    }

    pub async fn list_repositories(
        &mut self,
        request: RepositoryListRequest,
    ) -> Result<RepositoryListResult, BoardError> {
        let filter = request.project_id.as_str();
        let after = decode_metadata_cursor(
            &self.cursor_key,
            request.page.cursor.as_deref(),
            "repositories",
            filter,
        )?;
        let page_size = i64::from(request.page.limit.get()) + 1;
        let mut transaction = self.connection.begin().await.map_err(storage_error)?;
        require_project(&mut transaction, &request.project_id).await?;
        let rows = sqlx::query_as!(
            RawRepository,
            "SELECT repository_key,kind,origin,service_id,common_directory \
             FROM project_repositories WHERE project_id=? AND repository_key>? \
             ORDER BY repository_key LIMIT ?",
            filter,
            after,
            page_size,
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let (rows, next_cursor) = trim_metadata_page(
            &self.cursor_key,
            rows,
            request.page.limit,
            "repositories",
            filter,
            |row| row.repository_key.as_str(),
        )?;
        let records = rows
            .into_iter()
            .map(decode_repository)
            .collect::<Result<Vec<_>, _>>()?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(RepositoryListResult {
            page: Page {
                records,
                next_cursor,
            },
        })
    }
}

pub(crate) fn repository_key(repository: &RepositoryRef) -> String {
    match repository {
        RepositoryRef::Origin { normalized_origin } => {
            format!("origin:{}", normalized_origin.as_str())
        }
        RepositoryRef::Local {
            service_id,
            common_directory,
        } => format!(
            "local:{}:{}",
            service_id.as_str(),
            common_directory.as_str()
        ),
    }
}

fn repository_parts(repository: &RepositoryRef) -> RepositoryParts<'_> {
    match repository {
        RepositoryRef::Origin { normalized_origin } => RepositoryParts {
            key: repository_key(repository),
            kind: "origin",
            origin: Some(normalized_origin.as_str()),
            service_id: None,
            common_directory: None,
        },
        RepositoryRef::Local {
            service_id,
            common_directory,
        } => RepositoryParts {
            key: repository_key(repository),
            kind: "local",
            origin: None,
            service_id: Some(service_id.as_str()),
            common_directory: Some(common_directory.as_str()),
        },
    }
}

fn decode_repository(row: RawRepository) -> Result<RepositoryRef, BoardError> {
    let repository = match row.kind.as_str() {
        "origin" if row.service_id.is_none() && row.common_directory.is_none() => {
            RepositoryRef::Origin {
                normalized_origin: NormalizedOrigin::try_from(
                    row.origin.ok_or_else(invalid_record)?,
                )
                .map_err(|_| invalid_record())?,
            }
        }
        "local" if row.origin.is_none() => RepositoryRef::Local {
            service_id: ServiceId::try_from(row.service_id.ok_or_else(invalid_record)?)
                .map_err(|_| invalid_record())?,
            common_directory: CommonDirectory::try_from(
                row.common_directory.ok_or_else(invalid_record)?,
            )
            .map_err(|_| invalid_record())?,
        },
        _ => return Err(invalid_record()),
    };
    if repository_key(&repository) != row.repository_key {
        return Err(invalid_record());
    }
    Ok(repository)
}
