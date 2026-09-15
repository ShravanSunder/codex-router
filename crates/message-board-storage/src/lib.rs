//! Persistent project communication; schema setup completes before serving requests.
mod board_connection;
mod board_schema_migrations;
pub use board_connection::{BoardStorageError, BoardStore};

mod inbox_records;
mod message_records;
mod participant_records;
mod participant_row_decoding;
mod project_records;
mod storage_support;
mod thread_listen_records;
mod thread_records;

mod board_topic_records;
mod message_row_decoding;
mod message_write_operations;
mod repository_records;

mod discovery_search;
mod message_history_reads;
mod message_search;
