#[tokio::main]
async fn main() {
    std::process::exit(codex_router_cli::run_async().await);
}
