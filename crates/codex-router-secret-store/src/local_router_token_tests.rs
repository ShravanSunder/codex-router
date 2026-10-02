use std::fmt::Display;
use std::sync::Arc;
use std::sync::Barrier;
use std::thread;

use crate::file_backend::FileSecretStore;
use crate::local_router_token::LocalRouterTokenService;

#[test]
fn token_service_rotates_through_real_secret_store() {
    let test_root = must_ok(tempfile::tempdir());
    let store = must_ok(FileSecretStore::open(test_root.path()));
    let service = LocalRouterTokenService::new(store);

    let first = must_ok(service.rotate_with_token("first-token"));
    let second = must_ok(service.rotate_with_token("second-token"));

    assert_eq!(first.generation().as_u64(), 1);
    assert_eq!(second.generation().as_u64(), 2);

    let loaded = must_ok(service.load_current());
    assert_eq!(loaded.token().expose_secret(), "second-token");
    assert_eq!(loaded.generation().as_u64(), 2);
}

#[test]
fn ensure_local_token_preserves_existing_token() {
    let test_root = must_ok(tempfile::tempdir());
    let store = must_ok(FileSecretStore::open(test_root.path()));
    let service = LocalRouterTokenService::new(store);
    let existing_token = must_ok(service.rotate_with_token("existing-token"));

    let ensured_token = must_ok(service.ensure_local_token(test_root.path()));

    assert_eq!(ensured_token, existing_token);
}

#[test]
fn concurrent_local_token_creation_reuses_one_generation() {
    let test_root = must_ok(tempfile::tempdir());
    let secret_root = test_root.path().join("secrets");
    let _initial_store = must_ok(FileSecretStore::open(&secret_root));
    let start_barrier = Arc::new(Barrier::new(3));
    let mut initializer_threads = Vec::new();

    for _worker_index in 0..2 {
        let secret_root = secret_root.clone();
        let start_barrier = Arc::clone(&start_barrier);
        initializer_threads.push(thread::spawn(move || {
            let store = match FileSecretStore::open(&secret_root) {
                Ok(store) => store,
                Err(error) => panic!("token store should open: {error}"),
            };
            let token_service = LocalRouterTokenService::new(store);
            start_barrier.wait();
            token_service.ensure_local_token(&secret_root)
        }));
    }

    start_barrier.wait();
    let initialized_tokens = initializer_threads
        .into_iter()
        .map(|initializer_thread| {
            let result = match initializer_thread.join() {
                Ok(result) => result,
                Err(error) => panic!("token initializer should not panic: {error:?}"),
            };
            must_ok(result)
        })
        .collect::<Vec<_>>();
    let first_token = initialized_tokens[0].token().expose_secret();

    assert_eq!(initialized_tokens.len(), 2);
    for initialized_token in &initialized_tokens {
        assert_eq!(initialized_token.generation().as_u64(), 1);
        assert_eq!(initialized_token.token().expose_secret(), first_token);
    }
    assert!(secret_root.join(".token.lock").is_file());
}

fn must_ok<T, E: Display>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|error| panic!("operation should succeed: {error}"))
}
