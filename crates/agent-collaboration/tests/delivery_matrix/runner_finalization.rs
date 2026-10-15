//! Async finalization boundary for the original three matrix runners only.
use super::proof_context::ProofResult;
use futures_util::FutureExt as _;
use std::{
    fmt,
    future::Future,
    io::Write as _,
    ops::AsyncFnOnce,
    panic::{AssertUnwindSafe, resume_unwind},
};

type MatrixError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug)]
struct CleanupFailures {
    resource_cleanup: MatrixError,
    config_verification: MatrixError,
}
impl fmt::Display for CleanupFailures {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "resource cleanup failed: {}; config verification failed: {}",
            self.resource_cleanup, self.config_verification
        )
    }
}
impl std::error::Error for CleanupFailures {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.resource_cleanup.as_ref())
    }
}

#[derive(Debug)]
struct ScenarioCleanupFailure {
    scenario_error: MatrixError,
    cleanup_error: MatrixError,
}
impl fmt::Display for ScenarioCleanupFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "matrix scenario failed: {}; cleanup failed: {}",
            self.scenario_error, self.cleanup_error
        )
    }
}
impl std::error::Error for ScenarioCleanupFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.scenario_error.as_ref())
    }
}

pub(super) async fn run_matrix_scenario<TResource, TScenario, TCleanup, TCleanupFuture, TVerify>(
    mut owned_resource: TResource,
    scenario: TScenario,
    cleanup: TCleanup,
    verify: TVerify,
) -> ProofResult<()>
where
    TScenario: for<'resource> AsyncFnOnce(&'resource mut TResource) -> ProofResult<()>,
    TCleanup: FnOnce(TResource) -> TCleanupFuture,
    TCleanupFuture: Future<Output = ProofResult<()>>,
    TVerify: FnOnce() -> ProofResult<()>,
{
    // Invocation belongs inside the catch, as well as polling the returned future.
    let scenario_outcome = AssertUnwindSafe(async { scenario(&mut owned_resource).await })
        .catch_unwind()
        .await;
    let resource_cleanup = cleanup(owned_resource).await;
    let config_verification = verify();
    let cleanup_outcome: ProofResult<()> = match (resource_cleanup, config_verification) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(resource_cleanup), Err(config_verification)) => Err(Box::new(CleanupFailures {
            resource_cleanup,
            config_verification,
        })),
    };
    match scenario_outcome {
        Ok(Ok(())) => cleanup_outcome,
        Ok(Err(scenario_error)) => match cleanup_outcome {
            Ok(()) => Err(scenario_error),
            Err(cleanup_error) => Err(Box::new(ScenarioCleanupFailure {
                scenario_error,
                cleanup_error,
            })),
        },
        Err(payload) => {
            if let Err(error) = cleanup_outcome {
                let _reported = writeln!(
                    std::io::stderr(),
                    "matrix cleanup failed before resuming scenario panic: {error}"
                );
            }
            resume_unwind(payload)
        }
    }
}

#[cfg(test)]
#[path = "runner_finalization_tests.rs"]
mod tests;
