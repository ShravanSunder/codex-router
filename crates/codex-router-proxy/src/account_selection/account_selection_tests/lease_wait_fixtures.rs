use super::*;

pub(super) fn proxy_test_database_path(name: &str) -> PathBuf {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!(
        "codex-router-proxy-{name}-{}-{counter}.sqlite",
        std::process::id()
    ))
}

pub(super) async fn wait_for_active_client_count(
    store: &AsyncSqliteStateStore,
    route_band: &str,
    expected_count: u32,
    now_unix_seconds: u64,
) {
    for _attempt in 0..50 {
        let counts = store
            .active_client_counts_for_route_band(route_band, now_unix_seconds, 7_200)
            .await
            .unwrap_or_else(|error| panic!("active client count should load: {error}"));
        let actual_count = counts
            .iter()
            .map(|count| count.active_clients())
            .sum::<u32>();
        if actual_count == expected_count {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("active client count did not reach {expected_count}");
}
