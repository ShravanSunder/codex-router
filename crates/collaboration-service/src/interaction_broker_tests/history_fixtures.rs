//! Independent durable typed-history observations for broker integration tests.
pub(super) async fn stored_interaction_records(
    directory: &std::path::Path,
) -> std::collections::BTreeMap<String, crate::interaction_broker::InteractionHistoryRecord> {
    let mut observer = <sqlx::SqliteConnection as sqlx::Connection>::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(directory.join("interaction.sqlite"))
            .read_only(true),
    )
    .await
    .expect("independent history observer");
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT request_id,record_json FROM interaction_history_records ORDER BY request_id",
    )
    .fetch_all(&mut observer)
    .await
    .expect("durable history rows");
    rows.into_iter()
        .map(|(request_id, record_json)| {
            (
                request_id,
                serde_json::from_str(&record_json).expect("typed stored record"),
            )
        })
        .collect()
}
