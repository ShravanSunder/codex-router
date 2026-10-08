//! Admission and lifetime supervision for proxy credential renewal tasks.

use std::future::Future;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

#[cfg(test)]
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::CredentialResolverError;

/// Tracks admitted credential rotations across caller cancellation and runtime drain.
#[derive(Clone, Debug)]
pub struct CredentialRefreshTaskSupervisor {
    state: Arc<Mutex<CredentialRefreshTaskState>>,
}

#[derive(Debug)]
struct CredentialRefreshTaskState {
    admission: RenewalTaskAdmission,
    waiting_lock_cancellation: CancellationToken,
    tasks: TaskTracker,
    #[cfg(test)]
    task_pending_observer: Option<UnboundedSender<()>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RenewalTaskAdmission {
    Open,
    Closed,
}

impl CredentialRefreshTaskSupervisor {
    /// Creates one supervisor shared by all resolvers in a runtime.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(CredentialRefreshTaskState {
                admission: RenewalTaskAdmission::Open,
                waiting_lock_cancellation: CancellationToken::new(),
                tasks: TaskTracker::new(),
                #[cfg(test)]
                task_pending_observer: None,
            })),
        }
    }

    /// Closes renewal admission and cancels tasks that are still waiting for account locks.
    pub fn close_admission(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.admission == RenewalTaskAdmission::Open {
            state.admission = RenewalTaskAdmission::Closed;
            state.waiting_lock_cancellation.cancel();
            state.tasks.close();
        }
    }

    /// Waits for every tracked task to finish, up to `limit`.
    ///
    /// Call [`Self::close_admission`] first when the caller needs a finite set of tasks.
    /// A timeout stops only this wait and never aborts the admitted tasks.
    pub async fn wait_for_completion(&self, limit: Duration) -> bool {
        let tasks = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .tasks
            .clone();
        tokio::time::timeout(limit, tasks.wait()).await.is_ok()
    }

    /// Waits for terminal completion on the same tracker without imposing a deadline.
    /// Call `close_admission` first; observer cancellation never aborts admitted work.
    pub async fn wait_until_completed(&self) {
        let tasks = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .tasks
            .clone();
        tasks.wait().await;
    }

    /// Closes admission, cancels lock waiters, and waits for admitted work.
    pub async fn drain(&self, limit: Duration) -> bool {
        self.close_admission();
        self.wait_for_completion(limit).await
    }

    #[cfg(test)]
    pub(crate) fn set_test_task_pending_observer(&self, observer: UnboundedSender<()>) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .task_pending_observer = Some(observer);
    }

    pub(super) fn spawn<F, T>(
        &self,
        create_task: impl FnOnce(CancellationToken) -> F,
    ) -> Result<JoinHandle<T>, CredentialResolverError>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.admission == RenewalTaskAdmission::Closed {
            return Err(CredentialResolverError::RenewalAdmissionClosed);
        }
        let cancellation = state.waiting_lock_cancellation.clone();
        #[cfg(test)]
        let task = observe_first_pending(
            create_task(cancellation),
            state.task_pending_observer.clone(),
        );
        #[cfg(not(test))]
        let task = create_task(cancellation);
        Ok(state.tasks.spawn(task))
    }
}

#[cfg(test)]
fn observe_first_pending<F>(
    task: F,
    observer: Option<UnboundedSender<()>>,
) -> impl Future<Output = F::Output> + Send
where
    F: Future + Send,
{
    let mut task = Box::pin(task);
    let mut pending_reported = false;
    std::future::poll_fn(move |context| {
        let result = task.as_mut().poll(context);
        if matches!(result, std::task::Poll::Pending) && !pending_reported {
            if let Some(observer) = &observer {
                let _ = observer.send(());
            }
            pending_reported = true;
        }
        result
    })
}

impl Default for CredentialRefreshTaskSupervisor {
    fn default() -> Self {
        Self::new()
    }
}
