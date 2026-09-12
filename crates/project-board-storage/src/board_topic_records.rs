//! Board and topic metadata operations.
use crate::BoardStore;
use crate::project_records::{decode_metadata_cursor, require_project, trim_metadata_page};
use crate::storage_support::{
    BoardTransaction, archived_board, attribute_invalid_record, invalid_record,
    invalid_stored_identifier, name_conflict, resource_already_exists, resource_not_found,
    storage_error,
};
use project_board::*;
use serde::Serialize;
use sqlx::Connection;

#[derive(Serialize)]
struct RawBoard {
    board_id: String,
    project_id: String,
    name: String,
    description: String,
    state: String,
}

#[derive(Serialize)]
struct RawTopic {
    topic_id: String,
    board_id: String,
    name: String,
    description: String,
}

impl BoardStore {
    pub async fn create_board(
        &mut self,
        request: BoardCreateRequest,
    ) -> Result<BoardCreateResult, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        require_project(&mut transaction, &request.project_id).await?;
        if board_exists(&mut transaction, &request.board_id).await? {
            return Err(resource_already_exists(ResourceIdentity::Board {
                board_id: request.board_id,
            }));
        }
        reject_board_name(
            &mut transaction,
            request.project_id.as_str(),
            request.name.as_str(),
            None,
        )
        .await?;
        sqlx::query!(
            "INSERT INTO project_boards(board_id,project_id,name,description,state) \
             VALUES(?,?,?,?,'active')",
            request.board_id.as_str(),
            request.project_id.as_str(),
            request.name.as_str(),
            request.description.as_str(),
        )
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let board = Board {
            board_id: request.board_id,
            project_id: request.project_id,
            name: request.name,
            description: request.description,
            state: BoardState::Active,
        };
        transaction.commit().await.map_err(storage_error)?;
        Ok(BoardCreateResult {
            board,
            outcome: "Board created.".to_owned(),
        })
    }

    pub async fn update_board(
        &mut self,
        request: BoardUpdateRequest,
    ) -> Result<BoardUpdateResult, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let current = require_board(&mut transaction, &request.board_id).await?;
        if current.state == BoardState::Archived {
            return Err(archived_board());
        }
        reject_board_name(
            &mut transaction,
            current.project_id.as_str(),
            request.name.as_str(),
            Some(request.board_id.as_str()),
        )
        .await?;
        sqlx::query!(
            "UPDATE project_boards SET name=?,description=? WHERE board_id=?",
            request.name.as_str(),
            request.description.as_str(),
            request.board_id.as_str(),
        )
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let board = Board {
            name: request.name,
            description: request.description,
            ..current
        };
        transaction.commit().await.map_err(storage_error)?;
        Ok(BoardUpdateResult {
            board,
            outcome: "Board updated.".to_owned(),
        })
    }

    pub async fn show_board(
        &mut self,
        request: BoardShowRequest,
    ) -> Result<BoardShowResult, BoardError> {
        let mut transaction = self.connection.begin().await.map_err(storage_error)?;
        let board = require_board(&mut transaction, &request.board_id).await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(BoardShowResult { board })
    }

    pub async fn list_boards(
        &mut self,
        request: BoardListRequest,
    ) -> Result<BoardListResult, BoardError> {
        let filter = format!(
            "{}:{}",
            request
                .project_id
                .as_ref()
                .map(ProjectId::as_str)
                .unwrap_or(""),
            request.include_archived
        );
        let after = decode_metadata_cursor(
            &self.cursor_key,
            request.page.cursor.as_deref(),
            "boards",
            &filter,
        )?;
        let project_filter = request.project_id.as_ref().map(ProjectId::as_str);
        let page_size = i64::from(request.page.limit.get()) + 1;
        let mut transaction = self.connection.begin().await.map_err(storage_error)?;
        let rows = sqlx::query_as!(
            RawBoard,
            "SELECT board_id,project_id,name,description,state FROM project_boards \
             WHERE board_id>? AND (? IS NULL OR project_id=?) AND (? OR state='active') \
             ORDER BY board_id LIMIT ?",
            after,
            project_filter,
            project_filter,
            request.include_archived,
            page_size,
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let (rows, next_cursor) = trim_metadata_page(
            &self.cursor_key,
            rows,
            request.page.limit,
            "boards",
            &filter,
            |row| row.board_id.as_str(),
        )?;
        let records = rows
            .into_iter()
            .map(decode_board)
            .collect::<Result<Vec<_>, _>>()?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(BoardListResult {
            page: Page {
                records,
                next_cursor,
            },
        })
    }

    pub async fn archive_board(
        &mut self,
        request: BoardArchiveRequest,
    ) -> Result<BoardArchiveResult, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let mut board = require_board(&mut transaction, &request.board_id).await?;
        let result = sqlx::query!(
            "UPDATE project_boards SET state='archived' WHERE board_id=? AND state='active'",
            request.board_id.as_str(),
        )
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let changed = result.rows_affected() == 1;
        board.state = BoardState::Archived;
        transaction.commit().await.map_err(storage_error)?;
        Ok(BoardArchiveResult {
            board,
            outcome: if changed {
                "Board archived."
            } else {
                "Board was already archived."
            }
            .to_owned(),
        })
    }

    pub async fn create_topic(
        &mut self,
        request: TopicCreateRequest,
    ) -> Result<TopicCreateResult, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let board = require_board(&mut transaction, &request.board_id).await?;
        if board.state == BoardState::Archived {
            return Err(archived_board());
        }
        if topic_exists(&mut transaction, &request.topic_id).await? {
            return Err(resource_already_exists(ResourceIdentity::Topic {
                topic_id: request.topic_id,
            }));
        }
        reject_topic_name(
            &mut transaction,
            request.board_id.as_str(),
            request.name.as_str(),
            None,
        )
        .await?;
        sqlx::query!(
            "INSERT INTO board_topics(topic_id,board_id,name,description) VALUES(?,?,?,?)",
            request.topic_id.as_str(),
            request.board_id.as_str(),
            request.name.as_str(),
            request.description.as_str(),
        )
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let topic = Topic {
            topic_id: request.topic_id,
            board_id: request.board_id,
            name: request.name,
            description: request.description,
        };
        transaction.commit().await.map_err(storage_error)?;
        Ok(TopicCreateResult {
            topic,
            outcome: "Topic created.".to_owned(),
        })
    }

    pub async fn update_topic(
        &mut self,
        request: TopicUpdateRequest,
    ) -> Result<TopicUpdateResult, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let current = require_topic(&mut transaction, &request.topic_id).await?;
        if require_board(&mut transaction, &current.board_id)
            .await?
            .state
            == BoardState::Archived
        {
            return Err(archived_board());
        }
        reject_topic_name(
            &mut transaction,
            current.board_id.as_str(),
            request.name.as_str(),
            Some(request.topic_id.as_str()),
        )
        .await?;
        sqlx::query!(
            "UPDATE board_topics SET name=?,description=? WHERE topic_id=?",
            request.name.as_str(),
            request.description.as_str(),
            request.topic_id.as_str(),
        )
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let topic = Topic {
            name: request.name,
            description: request.description,
            ..current
        };
        transaction.commit().await.map_err(storage_error)?;
        Ok(TopicUpdateResult {
            topic,
            outcome: "Topic updated.".to_owned(),
        })
    }

    pub async fn list_topics(
        &mut self,
        request: TopicListRequest,
    ) -> Result<TopicListResult, BoardError> {
        let filter = request.board_id.as_str();
        let after = decode_metadata_cursor(
            &self.cursor_key,
            request.page.cursor.as_deref(),
            "topics",
            filter,
        )?;
        let page_size = i64::from(request.page.limit.get()) + 1;
        let mut transaction = self.connection.begin().await.map_err(storage_error)?;
        require_board(&mut transaction, &request.board_id).await?;
        let rows = sqlx::query_as!(
            RawTopic,
            "SELECT topic_id,board_id,name,description FROM board_topics \
             WHERE board_id=? AND topic_id>? ORDER BY topic_id LIMIT ?",
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
            "topics",
            filter,
            |row| row.topic_id.as_str(),
        )?;
        let records = rows
            .into_iter()
            .map(decode_topic)
            .collect::<Result<Vec<_>, _>>()?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(TopicListResult {
            page: Page {
                records,
                next_cursor,
            },
        })
    }
}

pub(crate) async fn require_board(
    transaction: &mut BoardTransaction<'_>,
    id: &BoardId,
) -> Result<Board, BoardError> {
    let row = sqlx::query_as!(
        RawBoard,
        "SELECT board_id,project_id,name,description,state FROM project_boards WHERE board_id=?",
        id.as_str(),
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)?
    .ok_or_else(|| {
        resource_not_found(ResourceIdentity::Board {
            board_id: id.clone(),
        })
    })?;
    decode_board(row).map_err(|error| {
        attribute_invalid_record(
            error,
            ResourceIdentity::Board {
                board_id: id.clone(),
            },
        )
    })
}

pub(crate) async fn require_topic(
    transaction: &mut BoardTransaction<'_>,
    id: &TopicId,
) -> Result<Topic, BoardError> {
    let row = sqlx::query_as!(
        RawTopic,
        "SELECT topic_id,board_id,name,description FROM board_topics WHERE topic_id=?",
        id.as_str(),
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)?
    .ok_or_else(|| {
        resource_not_found(ResourceIdentity::Topic {
            topic_id: id.clone(),
        })
    })?;
    decode_topic(row).map_err(|error| {
        attribute_invalid_record(
            error,
            ResourceIdentity::Topic {
                topic_id: id.clone(),
            },
        )
    })
}

async fn board_exists(
    transaction: &mut BoardTransaction<'_>,
    id: &BoardId,
) -> Result<bool, BoardError> {
    let exists = sqlx::query_scalar!(
        "SELECT EXISTS(SELECT 1 FROM project_boards WHERE board_id=?)",
        id.as_str(),
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(storage_error)?;
    Ok(exists != 0)
}

async fn topic_exists(
    transaction: &mut BoardTransaction<'_>,
    id: &TopicId,
) -> Result<bool, BoardError> {
    let exists = sqlx::query_scalar!(
        "SELECT EXISTS(SELECT 1 FROM board_topics WHERE topic_id=?)",
        id.as_str(),
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(storage_error)?;
    Ok(exists != 0)
}

async fn reject_board_name(
    transaction: &mut BoardTransaction<'_>,
    project_id: &str,
    name: &str,
    except: Option<&str>,
) -> Result<(), BoardError> {
    let used = sqlx::query_scalar!(
        "SELECT EXISTS(SELECT 1 FROM project_boards WHERE project_id=? AND name=? AND (? IS NULL OR board_id<>?))",
        project_id, name, except, except,
    )
    .fetch_one(&mut **transaction).await.map_err(storage_error)?;
    if used != 0 {
        return Err(name_conflict(
            "That name is already used within its parent.",
        ));
    }
    Ok(())
}

async fn reject_topic_name(
    transaction: &mut BoardTransaction<'_>,
    board_id: &str,
    name: &str,
    except: Option<&str>,
) -> Result<(), BoardError> {
    let used = sqlx::query_scalar!(
        "SELECT EXISTS(SELECT 1 FROM board_topics WHERE board_id=? AND name=? AND (? IS NULL OR topic_id<>?))",
        board_id, name, except, except,
    )
    .fetch_one(&mut **transaction).await.map_err(storage_error)?;
    if used != 0 {
        return Err(name_conflict(
            "That name is already used within its parent.",
        ));
    }
    Ok(())
}

fn decode_board(row: RawBoard) -> Result<Board, BoardError> {
    let board_id = BoardId::try_from(row.board_id)
        .map_err(|_| invalid_stored_identifier("boardId", "board-list"))?;
    let decoded = (|| {
        let canonical_name =
            ResourceName::try_from(row.name.clone()).map_err(|_| invalid_record())?;
        if canonical_name.as_str() != row.name {
            return Err(invalid_record());
        }
        Ok(Board {
            board_id: board_id.clone(),
            project_id: ProjectId::try_from(row.project_id).map_err(|_| invalid_record())?,
            name: canonical_name,
            description: Description::try_from(row.description).map_err(|_| invalid_record())?,
            state: match row.state.as_str() {
                "active" => BoardState::Active,
                "archived" => BoardState::Archived,
                _ => return Err(invalid_record()),
            },
        })
    })();
    decoded.map_err(|error| {
        attribute_invalid_record(
            error,
            ResourceIdentity::Board {
                board_id: board_id.clone(),
            },
        )
    })
}

fn decode_topic(row: RawTopic) -> Result<Topic, BoardError> {
    let topic_id = TopicId::try_from(row.topic_id)
        .map_err(|_| invalid_stored_identifier("topicId", "topic-list"))?;
    let decoded = (|| {
        let canonical_name =
            ResourceName::try_from(row.name.clone()).map_err(|_| invalid_record())?;
        if canonical_name.as_str() != row.name {
            return Err(invalid_record());
        }
        Ok(Topic {
            topic_id: topic_id.clone(),
            board_id: BoardId::try_from(row.board_id).map_err(|_| invalid_record())?,
            name: canonical_name,
            description: Description::try_from(row.description).map_err(|_| invalid_record())?,
        })
    })();
    decoded.map_err(|error| {
        attribute_invalid_record(
            error,
            ResourceIdentity::Topic {
                topic_id: topic_id.clone(),
            },
        )
    })
}
