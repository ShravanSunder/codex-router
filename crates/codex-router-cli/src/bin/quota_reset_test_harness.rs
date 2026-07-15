#[tokio::main]
async fn main() {
    std::process::exit(codex_router_cli::run_quota_reset_test_harness().await);
}
