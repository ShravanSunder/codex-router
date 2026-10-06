use super::*;

impl CollaborationRuntime {
    /// The lifecycle event loop must monitor this future; listener death is not healthy readiness.
    pub async fn listener_failure(&mut self) -> io::Error {
        loop {
            let failure = tokio::select! {
                task = self.tasks.join_next() => match task {
                    Some(Ok(Err(error))) => error,
                    Some(Err(_)) => io::Error::other("collaboration listener task failed"),
                    _ => io::Error::other("collaboration listener stopped unexpectedly"),
                },
                task = self.provider_retirement_tasks.join_next(), if !self.provider_retirement_tasks.is_empty() => match task {
                    Some(Ok(Ok(()))) => continue,
                    Some(Ok(Err(error))) => error,
                    Some(Err(_)) => io::Error::other("provider retirement publication task failed"),
                    None => continue,
                },
                failure = async {
                    match &mut self.mcp {
                        Some(mcp) => mcp.listener_failure().await,
                        None => std::future::pending().await,
                    }
                } => failure,
            };
            self.manifest.take();
            self.shutdown.cancel();
            if let Some(service) = &self.subscription_delivery {
                service.cancel();
            }
            let _retire = self.publication.admission_gate().retire();
            return failure;
        }
    }
    pub async fn shutdown(mut self) -> io::Result<()> {
        self.manifest.take();
        self.shutdown.cancel();
        if let Some(service) = self.subscription_delivery.take() {
            service.shutdown().await;
        }
        let mut failure = None;
        if let Some(mcp) = self.mcp.take()
            && let Err(error) = mcp.shutdown().await
        {
            failure.get_or_insert(error);
        }
        if let Err(error) = self.publication.admission_gate().retire() {
            failure.get_or_insert(error);
        }
        if let Some(route) = self.provider_delivery_route.take() {
            route.shutdown_queue().await;
        }
        if let Some(supervisor) = self.external_provider_supervisor.take()
            && let Err(message) = supervisor.shutdown().await
        {
            failure.get_or_insert(io::Error::other(message));
        }
        while let Some(result) = self.provider_retirement_tasks.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    failure.get_or_insert(error);
                }
                Err(_) => {
                    failure.get_or_insert(io::Error::other(
                        "provider retirement publication task failed",
                    ));
                }
            }
        }
        self.drain_native_observer().await;
        while let Some(result) = self.tasks.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    failure.get_or_insert(error);
                }
                Err(_) => {
                    failure.get_or_insert(io::Error::other("collaboration task shutdown failed"));
                }
            }
        }
        if let Some(maintenance) = self.maintenance.take() {
            match maintenance.await {
                Ok(Ok(())) => {}
                _ => {
                    failure.get_or_insert(io::Error::other("lifecycle maintenance failed"));
                }
            }
        }
        if let Some(task) = self.automation_task.take()
            && task.await.is_err()
        {
            failure.get_or_insert(io::Error::other("automation worker shutdown failed"));
        }
        if let Some(task) = self.schedule_task.take()
            && task.await.is_err()
        {
            failure.get_or_insert(io::Error::other("schedule worker shutdown failed"));
        }
        if let Some(task) = self.automation_maintenance.take()
            && task.await.is_err()
        {
            failure.get_or_insert(io::Error::other("automation maintenance shutdown failed"));
        }
        if let Some(task) = self.provider_retention.take()
            && task.await.is_err()
        {
            failure.get_or_insert(io::Error::other("provider retention maintenance failed"));
        }
        if let Some(store) = self.provider_store.take() {
            match std::sync::Arc::try_unwrap(store) {
                Ok(store) => {
                    if store.into_inner().close().await.is_err() {
                        failure.get_or_insert(io::Error::other(
                            "provider operation storage close failed",
                        ));
                    }
                }
                Err(_) => {
                    failure.get_or_insert(io::Error::other(
                        "provider operation storage still owned after listener shutdown",
                    ));
                }
            }
        }
        if let Some(store) = self.board_store.take() {
            match std::sync::Arc::try_unwrap(store) {
                Ok(store) => {
                    if store.into_inner().close().await.is_err() {
                        failure.get_or_insert(io::Error::other("board storage close failed"));
                    }
                }
                Err(_) => {
                    failure.get_or_insert(io::Error::other(
                        "board storage still owned after listener shutdown",
                    ));
                }
            }
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
