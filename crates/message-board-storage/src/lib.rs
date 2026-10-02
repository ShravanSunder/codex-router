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
mod subscription_batch_settlement_records;
mod subscription_window_records;
mod thread_delivery_position_writer;
mod thread_records;
mod thread_subscription_backfill;
mod thread_subscription_lifecycle_records;
mod thread_subscription_records;
mod thread_subscription_row_decoding;
mod thread_subscription_row_reads;

mod board_topic_records;
mod message_row_decoding;
mod message_write_operations;
mod repository_records;

mod discovery_search;
mod message_history_reads;
mod message_search;

#[cfg(test)]
mod participant_history_test_support;
#[cfg(test)]
mod participant_history_write_tests;
