//! Reloads local authentication when the stored token generation changes.
use codex_router_core::local_auth::LocalRouterAuth;
use codex_router_secret_store::file_backend::FileSecretStore;
use codex_router_secret_store::local_router_token::LocalRouterTokenService;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub struct LocalTokenReloadWatcher {
    stop_requested: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl LocalTokenReloadWatcher {
    pub fn start(
        secret_store: FileSecretStore,
        initial_generation: codex_router_core::ids::TokenGeneration,
        reload_auth: impl Fn(LocalRouterAuth) + Send + 'static,
    ) -> Self {
        let stop_requested = CancellationToken::new();
        let worker_stop = stop_requested.clone();
        let task = tokio::spawn(async move {
            let mut last_generation = initial_generation;
            loop {
                tokio::select! {
                    biased;
                    _ = worker_stop.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(50)) => {}
                }
                let read_store = secret_store.clone();
                let loaded_auth = tokio::task::spawn_blocking(move || {
                    LocalRouterTokenService::new(read_store).load_auth()
                })
                .await;
                if worker_stop.is_cancelled() {
                    break;
                }
                let Ok(Ok(auth)) = loaded_auth else {
                    continue;
                };
                let current_generation = auth.current_generation();
                if current_generation != last_generation {
                    reload_auth(auth);
                    last_generation = current_generation;
                }
            }
        });

        Self {
            stop_requested,
            task: Some(task),
        }
    }

    pub(crate) fn request_stop(&self) {
        self.stop_requested.cancel();
    }

    pub async fn shutdown(&mut self) {
        self.request_stop();
        let _result = self.join_stopped().await;
    }

    pub(crate) async fn join_stopped(&mut self) -> Result<(), tokio::task::JoinError> {
        let result = match self.task.as_mut() {
            Some(task) => task.await,
            None => Ok(()),
        };
        self.task = None;
        result
    }
}

impl Drop for LocalTokenReloadWatcher {
    fn drop(&mut self) {
        self.request_stop();
    }
}
