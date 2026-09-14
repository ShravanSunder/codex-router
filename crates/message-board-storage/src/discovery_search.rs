//! Name/description search with strict ancestry and live metadata pagination.
use crate::BoardStore;
use crate::board_topic_records::{require_board, require_topic};
use crate::project_records::require_project;
use crate::storage_support::{
    BoardTransaction, decode_cursor, encode_cursor, invalid_cursor, invalid_record, storage_error,
};
use message_board::*;
use serde::{Deserialize, Serialize};
use sqlx::Connection;

const PAGE_BYTE_BUDGET: usize = 900 * 1024;

#[derive(Serialize, Deserialize)]
struct DiscoveryCursor {
    operation: String,
    fingerprint: String,
    last_kind: i64,
    last_id: String,
}

struct DiscoveryRow {
    resource_kind: i64,
    resource_id: String,
}

impl BoardStore {
    pub async fn search_discovery(
        &mut self,
        request: DiscoverySearchRequest,
    ) -> Result<DiscoverySearchResult, BoardError> {
        if matches!(
            (&request.scope, request.kind),
            (DiscoveryScope::Board { .. }, DiscoveryKind::Project)
        ) {
            return Err(BoardError::invalid_field(
                "kind",
                "project results cannot be selected within a board",
            ));
        }
        let fingerprint = serde_json::to_string(&(
            &request.query,
            &request.scope,
            request.kind,
            request.include_archived,
        ))
        .map_err(|_| invalid_cursor())?;
        let (last_kind, last_id) = match request.page.cursor.as_deref() {
            None => (-1, String::new()),
            Some(cursor) => {
                let cursor: DiscoveryCursor = decode_cursor(&self.cursor_key, cursor)?;
                if cursor.operation != "discoverySearch" || cursor.fingerprint != fingerprint {
                    return Err(invalid_cursor());
                }
                (cursor.last_kind, cursor.last_id)
            }
        };
        let mut transaction = self.connection.begin().await.map_err(storage_error)?;
        let (scope_kind, scope_id) = match &request.scope {
            DiscoveryScope::AllProjects => ("all", ""),
            DiscoveryScope::Project { project_id } => {
                require_project(&mut transaction, project_id).await?;
                ("project", project_id.as_str())
            }
            DiscoveryScope::Board { board_id } => {
                require_board(&mut transaction, board_id).await?;
                ("board", board_id.as_str())
            }
        };
        let query = request.query.as_str().to_ascii_lowercase();
        let selected_kind = match request.kind {
            DiscoveryKind::All => -1,
            DiscoveryKind::Project => 0,
            DiscoveryKind::Board => 1,
            DiscoveryKind::Topic => 2,
        };
        let limit = i64::from(request.page.limit.get()) + 1;
        let rows = sqlx::query_as!(DiscoveryRow,
            r#"SELECT resource_kind AS "resource_kind!: i64",resource_id AS "resource_id!: String" FROM (
                 SELECT 0 AS resource_kind,p.project_id AS resource_id FROM board_projects p
                 WHERE (?='all' OR (?='project' AND p.project_id=?)) AND (instr(lower(p.name),?)>0 OR instr(lower(p.description),?)>0)
                 UNION ALL
                 SELECT 1,b.board_id FROM project_boards b
                 WHERE (?='all' OR (?='project' AND b.project_id=?) OR (?='board' AND b.board_id=?))
                   AND (? OR b.state='active') AND (instr(lower(b.name),?)>0 OR instr(lower(b.description),?)>0)
                 UNION ALL
                 SELECT 2,t.topic_id FROM board_topics t JOIN project_boards b ON b.board_id=t.board_id
                 WHERE (?='all' OR (?='project' AND b.project_id=?) OR (?='board' AND b.board_id=?))
                   AND (? OR b.state='active') AND (instr(lower(t.name),?)>0 OR instr(lower(t.description),?)>0)
               ) WHERE (?=-1 OR resource_kind=?) AND (resource_kind>? OR (resource_kind=? AND resource_id>?))
               ORDER BY resource_kind,resource_id LIMIT ?"#,
            scope_kind,scope_kind,scope_id,query,query,
            scope_kind,scope_kind,scope_id,scope_kind,scope_id,request.include_archived,query,query,
            scope_kind,scope_kind,scope_id,scope_kind,scope_id,request.include_archived,query,query,
            selected_kind,selected_kind,last_kind,last_kind,last_id,limit)
            .fetch_all(&mut *transaction).await.map_err(storage_error)?;
        let mut has_more = rows.len() > request.page.limit.get() as usize;
        let mut records = Vec::new();
        let mut bytes = 2;
        let mut next_position = (last_kind, last_id);
        for row in rows.iter().take(request.page.limit.get() as usize) {
            let hit = load_discovery_hit(&mut transaction, row).await?;
            let size = serde_json::to_vec(&hit)
                .map_err(|_| invalid_record())?
                .len()
                + usize::from(!records.is_empty());
            if bytes + size > PAGE_BYTE_BUDGET {
                has_more = true;
                break;
            }
            bytes += size;
            records.push(hit);
            next_position = (row.resource_kind, row.resource_id.clone());
        }
        let next_cursor = has_more
            .then(|| {
                encode_cursor(
                    &self.cursor_key,
                    &DiscoveryCursor {
                        operation: "discoverySearch".to_owned(),
                        fingerprint,
                        last_kind: next_position.0,
                        last_id: next_position.1,
                    },
                )
            })
            .transpose()?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(DiscoverySearchResult {
            page: Page {
                records,
                next_cursor,
            },
        })
    }
}

async fn load_discovery_hit(
    transaction: &mut BoardTransaction<'_>,
    row: &DiscoveryRow,
) -> Result<DiscoveryHit, BoardError> {
    match row.resource_kind {
        0 => Ok(DiscoveryHit::Project {
            project: require_project(
                transaction,
                &ProjectId::try_from(row.resource_id.clone()).map_err(|_| invalid_record())?,
            )
            .await?,
        }),
        1 => {
            let board = require_board(
                transaction,
                &BoardId::try_from(row.resource_id.clone()).map_err(|_| invalid_record())?,
            )
            .await?;
            let project = require_project(transaction, &board.project_id).await?;
            Ok(DiscoveryHit::Board { project, board })
        }
        2 => {
            let topic = require_topic(
                transaction,
                &TopicId::try_from(row.resource_id.clone()).map_err(|_| invalid_record())?,
            )
            .await?;
            let board = require_board(transaction, &topic.board_id).await?;
            let project = require_project(transaction, &board.project_id).await?;
            Ok(DiscoveryHit::Topic {
                project,
                board,
                topic,
            })
        }
        _ => Err(invalid_record()),
    }
}
