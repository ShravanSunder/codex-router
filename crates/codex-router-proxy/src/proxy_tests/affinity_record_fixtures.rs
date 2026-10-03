use super::*;

pub(super) static TEST_AFFINITY_SECRET_PROVIDER: TestAffinitySecretProvider =
    TestAffinitySecretProvider;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct TestAffinitySecretProvider;

impl HttpAffinitySecretProvider for TestAffinitySecretProvider {
    fn load_or_create_affinity_secret(&self) -> Result<RouterAffinityHashSecret, HttpProxyError> {
        Ok(test_affinity_secret())
    }
}

pub(super) fn unbounded_test_websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(None)
        .max_frame_size(None)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FixedAffinitySecretProvider {
    secret: RouterAffinityHashSecret,
}

impl FixedAffinitySecretProvider {
    pub(super) fn new(secret: RouterAffinityHashSecret) -> Self {
        Self { secret }
    }
}

impl HttpAffinitySecretProvider for FixedAffinitySecretProvider {
    fn load_or_create_affinity_secret(&self) -> Result<RouterAffinityHashSecret, HttpProxyError> {
        Ok(self.secret.clone())
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct RecordingAffinityOwnerRecorder {
    records: Arc<Mutex<Vec<PreviousResponseAffinityOwnerRecord>>>,
}

pub(super) fn lock_test_mutex<'a, T>(mutex: &'a Mutex<T>, label: &str) -> MutexGuard<'a, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(error) => panic!("{label} lock should be available: {error}"),
    }
}

impl RecordingAffinityOwnerRecorder {
    pub(super) fn take_records(&self) -> Vec<PreviousResponseAffinityOwnerRecord> {
        lock_test_mutex(&self.records, "test recorder")
            .drain(..)
            .collect()
    }

    pub(super) fn records_snapshot(&self) -> Vec<PreviousResponseAffinityOwnerRecord> {
        lock_test_mutex(&self.records, "test recorder").clone()
    }
}

impl HttpAffinityOwnerRecorder for RecordingAffinityOwnerRecorder {
    fn record_affinity_owner(
        &self,
        owner: &PreviousResponseAffinityOwnerRecord,
    ) -> Result<(), HttpProxyError> {
        lock_test_mutex(&self.records, "test recorder").push(owner.clone());
        Ok(())
    }
}

pub(super) struct BlockingAffinityOwnerRecorder {
    records: Arc<Mutex<Vec<PreviousResponseAffinityOwnerRecord>>>,
    entered: Mutex<Option<mpsc::Sender<()>>>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl BlockingAffinityOwnerRecorder {
    pub(super) fn new(entered: mpsc::Sender<()>, release: mpsc::Receiver<()>) -> Self {
        Self {
            records: Arc::new(Mutex::new(Vec::new())),
            entered: Mutex::new(Some(entered)),
            release: Mutex::new(release),
        }
    }

    pub(super) fn records_snapshot(&self) -> Vec<PreviousResponseAffinityOwnerRecord> {
        lock_test_mutex(&self.records, "blocking recorder records").clone()
    }
}

impl HttpAffinityOwnerRecorder for BlockingAffinityOwnerRecorder {
    fn record_affinity_owner(
        &self,
        owner: &PreviousResponseAffinityOwnerRecord,
    ) -> Result<(), HttpProxyError> {
        if let Some(entered) = lock_test_mutex(&self.entered, "blocking recorder entered").take() {
            let _result = entered.send(());
        }
        lock_test_mutex(&self.release, "blocking recorder release")
            .recv_timeout(Duration::from_secs(2))
            .map_err(|error| HttpProxyError::Upstream {
                message: error.to_string(),
            })?;
        lock_test_mutex(&self.records, "blocking recorder records").push(owner.clone());
        Ok(())
    }
}

impl crate::http_sse::AsyncHttpAffinityOwnerRecorder for BlockingAffinityOwnerRecorder {
    fn record_affinity_owner<'a>(
        &'a self,
        owner: PreviousResponseAffinityOwnerRecord,
    ) -> BoxFuture<'a, Result<(), HttpProxyError>> {
        Box::pin(async move { HttpAffinityOwnerRecorder::record_affinity_owner(self, &owner) })
    }
}

pub(super) async fn wait_for_affinity_records(
    recorder: &RecordingAffinityOwnerRecorder,
    expected_count: usize,
) -> Vec<PreviousResponseAffinityOwnerRecord> {
    for _attempt in 0..50 {
        let records = recorder.records_snapshot();
        if records.len() >= expected_count {
            return records;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("expected {expected_count} affinity records before timeout");
}
