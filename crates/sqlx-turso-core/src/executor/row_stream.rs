//! The row stream over a running Turso statement
//!
//! Reads one row ahead so the final row is followed by a query result carrying the statement's
//! change count, as SQLx's `fetch_many` contract expects. The query logger travels with the
//! stream and logs once, when the stream ends or is dropped.

use std::sync::Arc;

use either::Either;
use sqlx_core::{HashMap, column::Column, error::Error, ext::ustr::UStr, logger::QueryLogger};

use crate::{TursoColumn, TursoQueryResult, TursoRow, TursoValue, error::map_turso_error};

pub(super) enum RowStreamState {
    Rows {
        rows: turso::Rows,
        statement: turso::Statement,
        columns: Arc<[TursoColumn]>,
        column_names: Arc<HashMap<UStr, usize>>,
        pending_row: Option<turso::Row>,
        logger: QueryLogger,
    },
    QueryResult(TursoQueryResult, QueryLogger),
    Done,
}

impl RowStreamState {
    pub(super) async fn next(
        self,
    ) -> Result<Option<(Either<TursoQueryResult, TursoRow>, Self)>, Error> {
        let (mut rows, statement, columns, column_names, pending_row, mut logger) = match self {
            Self::Rows {
                rows,
                statement,
                columns,
                column_names,
                pending_row,
                logger,
            } => (rows, statement, columns, column_names, pending_row, logger),
            // Dropping the logger here logs the finished statement.
            Self::QueryResult(result, _logger) => {
                return Ok(Some((Either::Left(result), Self::Done)));
            }
            Self::Done => return Ok(None),
        };

        let row = match pending_row {
            Some(row) => Some(row),
            None => rows.next().await.map_err(map_turso_error)?,
        };

        let Some(row) = row else {
            let rows_affected = statement.n_change();
            logger.increase_rows_affected(rows_affected);
            return Ok(Some((
                Either::Left(TursoQueryResult::new(rows_affected)),
                Self::Done,
            )));
        };

        let mut values = Vec::with_capacity(columns.len());
        for (index, column) in columns.iter().enumerate() {
            values.push(
                TursoValue::from_turso(row.get_value(index).map_err(map_turso_error)?)
                    .with_type_info(column.type_info().clone()),
            );
        }

        logger.increment_rows_returned();
        let pending_row = rows.next().await.map_err(map_turso_error)?;
        let next_state = if pending_row.is_some() {
            Self::Rows {
                rows,
                statement,
                columns: Arc::clone(&columns),
                column_names: Arc::clone(&column_names),
                pending_row,
                logger,
            }
        } else {
            let rows_affected = statement.n_change();
            logger.increase_rows_affected(rows_affected);
            Self::QueryResult(TursoQueryResult::new(rows_affected), logger)
        };

        Ok(Some((
            Either::Right(TursoRow::with_shared_columns(columns, column_names, values)),
            next_state,
        )))
    }
}
