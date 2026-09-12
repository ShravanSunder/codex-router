//! Project metadata operations and shared metadata pagination.
use crate::BoardStore;
use crate::repository_records::repository_key;
use crate::storage_support::{
    BoardTransaction, attribute_invalid_record, decode_cursor as decode_signed_cursor,
    encode_cursor, invalid_cursor, invalid_record, invalid_stored_identifier, name_conflict,
    resource_already_exists, resource_not_found, storage_error,
};
use project_board::*;
use serde::{Deserialize, Serialize};
use sqlx::Connection;

const METADATA_PAGE_RECORDS_BYTE_BUDGET: usize = 900 * 1024;

#[derive(Serialize, Deserialize)]
struct MetadataCursor {
    operation: String,
    last_key: String,
    filter: String,
}

#[derive(Serialize)]
struct RawProject {
    project_id: String,
    name: String,
    description: String,
}

impl BoardStore {
    pub async fn create_project(
        &mut self,
        request: ProjectCreateRequest,
    ) -> Result<ProjectCreateResult, BoardError> {
        let project = Project {
            project_id: request.project_id,
            name: request.name,
            description: request.description,
        };
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        if project_exists(&mut transaction, &project.project_id).await? {
            return Err(resource_already_exists(ResourceIdentity::Project {
                project_id: project.project_id,
            }));
        }
        reject_project_name(&mut transaction, project.name.as_str(), None).await?;
        sqlx::query!(
            "INSERT INTO board_projects(project_id,name,description) VALUES(?,?,?)",
            project.project_id.as_str(),
            project.name.as_str(),
            project.description.as_str(),
        )
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(ProjectCreateResult {
            project,
            outcome: "Project created.".to_owned(),
        })
    }

    pub async fn update_project(
        &mut self,
        request: ProjectUpdateRequest,
    ) -> Result<ProjectUpdateResult, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        require_project(&mut transaction, &request.project_id).await?;
        reject_project_name(
            &mut transaction,
            request.name.as_str(),
            Some(request.project_id.as_str()),
        )
        .await?;
        sqlx::query!(
            "UPDATE board_projects SET name=?,description=? WHERE project_id=?",
            request.name.as_str(),
            request.description.as_str(),
            request.project_id.as_str(),
        )
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let project = Project {
            project_id: request.project_id,
            name: request.name,
            description: request.description,
        };
        transaction.commit().await.map_err(storage_error)?;
        Ok(ProjectUpdateResult {
            project,
            outcome: "Project updated.".to_owned(),
        })
    }

    pub async fn show_project(
        &mut self,
        request: ProjectShowRequest,
    ) -> Result<ProjectShowResult, BoardError> {
        let mut transaction = self.connection.begin().await.map_err(storage_error)?;
        let project = require_project(&mut transaction, &request.project_id).await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(ProjectShowResult { project })
    }

    pub async fn list_projects(
        &mut self,
        request: ProjectListRequest,
    ) -> Result<ProjectListResult, BoardError> {
        let filter = request
            .repository
            .as_ref()
            .map(repository_key)
            .unwrap_or_default();
        let after = decode_metadata_cursor(
            &self.cursor_key,
            request.page.cursor.as_deref(),
            "projects",
            &filter,
        )?;
        let page_size = i64::from(request.page.limit.get()) + 1;
        let mut transaction = self.connection.begin().await.map_err(storage_error)?;
        let rows = sqlx::query_as!(
            RawProject,
            r#"SELECT DISTINCT p.project_id AS "project_id!", p.name AS "name!", p.description AS "description!"
               FROM board_projects p LEFT JOIN project_repositories r ON r.project_id=p.project_id
               WHERE p.project_id>? AND (?='' OR r.repository_key=?) ORDER BY p.project_id LIMIT ?"#,
            after, filter, filter, page_size,
        )
        .fetch_all(&mut *transaction).await.map_err(storage_error)?;
        let (rows, next_cursor) = trim_metadata_page(
            &self.cursor_key,
            rows,
            request.page.limit,
            "projects",
            &filter,
            |row| row.project_id.as_str(),
        )?;
        let records = rows
            .into_iter()
            .map(decode_project)
            .collect::<Result<Vec<_>, _>>()?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(ProjectListResult {
            page: Page {
                records,
                next_cursor,
            },
        })
    }
}

pub(crate) async fn require_project(
    transaction: &mut BoardTransaction<'_>,
    id: &ProjectId,
) -> Result<Project, BoardError> {
    let row = sqlx::query_as!(
        RawProject,
        "SELECT project_id,name,description FROM board_projects WHERE project_id=?",
        id.as_str(),
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)?
    .ok_or_else(|| {
        resource_not_found(ResourceIdentity::Project {
            project_id: id.clone(),
        })
    })?;
    decode_project(row).map_err(|error| {
        attribute_invalid_record(
            error,
            ResourceIdentity::Project {
                project_id: id.clone(),
            },
        )
    })
}

async fn project_exists(
    transaction: &mut BoardTransaction<'_>,
    id: &ProjectId,
) -> Result<bool, BoardError> {
    let exists = sqlx::query_scalar!(
        "SELECT EXISTS(SELECT 1 FROM board_projects WHERE project_id=?)",
        id.as_str(),
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(storage_error)?;
    Ok(exists != 0)
}

async fn reject_project_name(
    transaction: &mut BoardTransaction<'_>,
    name: &str,
    except: Option<&str>,
) -> Result<(), BoardError> {
    let used = sqlx::query_scalar!(
        "SELECT EXISTS(SELECT 1 FROM board_projects WHERE name=? AND (? IS NULL OR project_id<>?))",
        name,
        except,
        except,
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(storage_error)?;
    if used != 0 {
        return Err(name_conflict("A project with that name already exists."));
    }
    Ok(())
}

fn decode_project(row: RawProject) -> Result<Project, BoardError> {
    let project_id = ProjectId::try_from(row.project_id)
        .map_err(|_| invalid_stored_identifier("projectId", "project-list"))?;
    let decoded = (|| {
        let canonical_name =
            ResourceName::try_from(row.name.clone()).map_err(|_| invalid_record())?;
        if canonical_name.as_str() != row.name {
            return Err(invalid_record());
        }
        Ok(Project {
            project_id: project_id.clone(),
            name: canonical_name,
            description: Description::try_from(row.description).map_err(|_| invalid_record())?,
        })
    })();
    decoded.map_err(|error| {
        attribute_invalid_record(
            error,
            ResourceIdentity::Project {
                project_id: project_id.clone(),
            },
        )
    })
}

pub(crate) fn decode_metadata_cursor(
    cursor_key: &[u8; 32],
    cursor: Option<&str>,
    operation: &str,
    filter: &str,
) -> Result<String, BoardError> {
    let Some(cursor) = cursor else {
        return Ok(String::new());
    };
    let decoded: MetadataCursor = decode_signed_cursor(cursor_key, cursor)?;
    if decoded.operation != operation || decoded.filter != filter {
        return Err(invalid_cursor());
    }
    Ok(decoded.last_key)
}

pub(crate) fn trim_metadata_page<TRecord: Serialize>(
    cursor_key: &[u8; 32],
    rows: Vec<TRecord>,
    limit: PageLimit,
    operation: &str,
    filter: &str,
    record_key: impl Fn(&TRecord) -> &str,
) -> Result<(Vec<TRecord>, Option<String>), BoardError> {
    let mut emitted = Vec::with_capacity(rows.len().min(limit.get() as usize));
    let mut encoded_bytes = 2_usize;
    let mut has_more = false;
    for row in rows {
        if emitted.len() == limit.get() as usize {
            has_more = true;
            break;
        }
        let row_bytes = serde_json::to_vec(&row)
            .map_err(|_| invalid_record())?
            .len();
        let separator_bytes = usize::from(!emitted.is_empty());
        if encoded_bytes + separator_bytes + row_bytes > METADATA_PAGE_RECORDS_BYTE_BUDGET {
            if emitted.is_empty() {
                return Err(invalid_record());
            }
            has_more = true;
            break;
        }
        encoded_bytes += separator_bytes + row_bytes;
        emitted.push(row);
    }
    if !has_more {
        return Ok((emitted, None));
    }
    let last_key = emitted
        .last()
        .map(record_key)
        .ok_or_else(invalid_record)?
        .to_owned();
    let cursor = encode_cursor(
        cursor_key,
        &MetadataCursor {
            operation: operation.to_owned(),
            last_key,
            filter: filter.to_owned(),
        },
    )?;
    Ok((emitted, Some(cursor)))
}
