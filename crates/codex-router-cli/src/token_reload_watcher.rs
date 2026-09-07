//! Reloads local authentication when the stored token generation changes.
use crate::token::LocalRouterTokenService;
use codex_router_proxy::server::LocalAuthReloader;
use codex_router_secret_store::file_backend::FileSecretStore;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub(super) struct LocalTokenReloadWatcher {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl LocalTokenReloadWatcher {
    pub(super) fn start(
        secret_store: FileSecretStore,
        reloader: LocalAuthReloader,
        initial_generation: codex_router_core::ids::TokenGeneration,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let token_service = LocalRouterTokenService::new(secret_store);
            let mut last_generation = initial_generation;
            while !stop_for_thread.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(50));
                let auth = match token_service.load_auth() {
                    Ok(auth) => auth,
                    Err(_error) => continue,
                };
                let current_generation = auth.current_generation();
                if current_generation != last_generation {
                    reloader.reload_auth(auth);
                    last_generation = current_generation;
                }
            }
        });

        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for LocalTokenReloadWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _result = thread.join();
        }
    }
}
