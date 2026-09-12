//! Persistent project communication; schema setup completes before serving requests.
mod board_connection;
mod board_schema_migrations;
pub use board_connection::{BoardStorageError, BoardStore};

mod inbox_records;
mod message_records;
mod project_records;
mod storage_support;
mod thread_records;

mod board_topic_records;
mod message_row_decoding;
mod message_write_operations;
mod repository_records;

mod message_history_reads;
