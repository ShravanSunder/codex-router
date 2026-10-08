use std::sync::Arc;
use tokio::{
    sync::Mutex,
    task::{JoinError, JoinHandle},
};

/// A cancelled observer leaves the same handle stored; no task is aborted by this wait.
pub(crate) async fn await_actor_task_completion(
    task: &Arc<Mutex<Option<JoinHandle<()>>>>,
) -> Result<(), JoinError> {
    let mut stored = task.lock().await;
    let result = match stored.as_mut() {
        Some(task) => task.await,
        None => Ok(()),
    };
    *stored = None;
    result
}
