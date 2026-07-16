use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use codex_router_core::ids::AccountId;
use codex_router_core::redaction::SecretString;
use tokio::sync::Notify;

use super::protocol::PinnedTargetInvalidationReason;
use super::protocol::ResetEligibilityDisabledReason;
use super::protocol::ResetValueProvenance;
use super::*;
use crate::quota_reset::domain::ActiveCredentialGeneration;
use crate::quota_reset::domain::KnownConsumeOutcome;
use crate::quota_reset::domain::LiveResetCredit;
use crate::quota_reset::domain::RedeemRequestId;
use crate::quota_reset::domain::SelectedResetCreditSnapshot;
use crate::quota_reset::domain::validate_credit_inventory;
use crate::quota_reset::provider::LiveResetAccountAuth;
use crate::quota_reset::service::ResetAuthority;

mod lifecycle;

#[derive(Clone)]
struct FakeAuthority {
    account_id: AccountId,
    generation: ActiveCredentialGeneration,
}

impl ResetAuthority for FakeAuthority {
    type Fingerprint = u64;

    fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    fn active_credential_generation(&self) -> ActiveCredentialGeneration {
        self.generation
    }

    fn auth(&self) -> LiveResetAccountAuth {
        LiveResetAccountAuth {
            access_token: SecretString::new("fake-supervisor-token"),
            chatgpt_account_id: "fake-supervisor-routing".to_owned(),
        }
    }

    fn expires_unix_seconds(&self) -> Option<u64> {
        Some(500)
    }

    fn fingerprint(&self) -> &Self::Fingerprint {
        static FINGERPRINT: u64 = 7;
        &FINGERPRINT
    }
}

struct FakeAuthorityReader {
    reads: Arc<AtomicUsize>,
}

impl ResetAuthorityReader for FakeAuthorityReader {
    type Authority = FakeAuthority;

    async fn read_authority(
        &self,
        account_id: &AccountId,
        expected_generation: ActiveCredentialGeneration,
        _now_unix_seconds: u64,
    ) -> Result<Self::Authority, RenderSafeFailure> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(FakeAuthority {
            account_id: account_id.clone(),
            generation: expected_generation,
        })
    }
}

#[derive(Default)]
struct FakeProviderControl {
    usage_calls: AtomicUsize,
    inventory_calls: AtomicUsize,
    post_calls: AtomicUsize,
    inspection_usage_started: Notify,
    inspection_usage_returned: Notify,
    inspection_inventory_started: Notify,
    inspection_inventory_returned: Notify,
    inspection_usage_release: Notify,
    inspection_inventory_release: Notify,
    revalidation_usage_started: Notify,
    revalidation_inventory_started: Notify,
    revalidation_release: Notify,
    consume_started: Notify,
    consume_release: Notify,
    inspection_inventory_dropped: AtomicBool,
    revalidation_usage_percent: AtomicUsize,
}

struct FakeProvider {
    control: Arc<FakeProviderControl>,
}

struct FakePreparedConsume;

impl ResetServiceProvider for FakeProvider {
    type PreparedConsume = FakePreparedConsume;

    async fn fetch_usage(&self, _auth: LiveResetAccountAuth) -> LiveUsagePortResult {
        let call = self.control.usage_calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            self.control.inspection_usage_started.notify_one();
            self.control.inspection_usage_release.notified().await;
            self.control.inspection_usage_returned.notify_one();
        } else {
            self.control.revalidation_usage_started.notify_one();
            self.control.revalidation_release.notified().await;
        }
        let remaining_percent = if call == 0 {
            0
        } else {
            self.control
                .revalidation_usage_percent
                .load(Ordering::SeqCst) as u32
        };
        LiveUsagePortResult::Known(LiveWeeklyUsage::new(remaining_percent))
    }

    async fn fetch_inventory(
        &self,
        _auth: LiveResetAccountAuth,
        _now_unix_seconds: i64,
    ) -> CreditInventoryPortResult {
        let call = self.control.inventory_calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            self.control.inspection_inventory_started.notify_one();
            let drop_signal = DropSignal(&self.control.inspection_inventory_dropped);
            self.control.inspection_inventory_release.notified().await;
            std::mem::forget(drop_signal);
            self.control.inspection_inventory_returned.notify_one();
        } else {
            self.control.revalidation_inventory_started.notify_one();
            self.control.revalidation_release.notified().await;
        }
        validated_inventory()
    }

    fn prepare_consume(
        &self,
        _auth: &LiveResetAccountAuth,
        _selected_credit: &SelectedResetCreditSnapshot,
        _redeem_request_id: &RedeemRequestId,
    ) -> Result<Self::PreparedConsume, RenderSafeFailure> {
        Ok(FakePreparedConsume)
    }

    async fn invoke_prepared(&self, _prepared: Self::PreparedConsume) -> ConsumePortResult {
        self.control.post_calls.fetch_add(1, Ordering::SeqCst);
        self.control.consume_started.notify_one();
        self.control.consume_release.notified().await;
        ConsumePortResult::Known(KnownConsumeOutcome::Reset { windows_reset: 2 })
    }
}

struct DropSignal<'a>(&'a AtomicBool);

impl Drop for DropSignal<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct FakeRedeemRequestIdFactory {
    mints: Arc<AtomicUsize>,
}

impl RedeemRequestIdFactory for FakeRedeemRequestIdFactory {
    fn mint(&self) -> Result<RedeemRequestId, RenderSafeFailure> {
        let mint = self.mints.fetch_add(1, Ordering::SeqCst);
        RedeemRequestId::new(format!("supervisor-redeem-{mint}"))
            .map_err(|_| RenderSafeFailure::InvalidResponse)
    }
}

#[test]
fn non_spawning_service_cannot_own_effect_handles() {
    // Arrange
    let service_source = include_str!("../service.rs");

    // Act
    let owns_spawn_or_join_handle = service_source.contains("tokio::spawn")
        || service_source.contains("JoinHandle")
        || service_source.contains("JoinSet");

    // Assert
    assert!(!owns_spawn_or_join_handle);
    let supervisor_source = include_str!("../supervisor.rs");
    assert!(!supervisor_source.contains("sqlx::"));
    assert!(!supervisor_source.contains("StateStore"));
    assert!(!supervisor_source.contains("persist_"));
    assert!(!supervisor_source.contains("refresh_quota"));
}

#[tokio::test]
async fn cancel_reaps_partial_get_and_allows_second_inspection() {
    // Arrange
    let fixture = session_fixture();
    let mut snapshot_receiver = fixture.ports.snapshot_receiver;
    let intent_sender = fixture.ports.intent_sender;
    let session_task = tokio::spawn(fixture.session.run());

    // Act
    begin_inspection(&intent_sender).await;
    fixture.control.inspection_usage_started.notified().await;
    fixture
        .control
        .inspection_inventory_started
        .notified()
        .await;
    fixture.control.inspection_usage_release.notify_one();
    fixture.control.inspection_usage_returned.notified().await;
    snapshot_receiver.changed().await.expect("partial snapshot");
    assert_eq!(
        snapshot_receiver.borrow().phase(),
        WorkflowPhase::Inspecting
    );
    intent_sender
        .send(ResetSessionIntent::Cancel)
        .await
        .expect("cancel intent");
    wait_for_phase(&mut snapshot_receiver, WorkflowPhase::Browse).await;
    begin_inspection(&intent_sender).await;
    fixture.control.revalidation_usage_started.notified().await;
    fixture
        .control
        .revalidation_inventory_started
        .notified()
        .await;
    fixture.control.revalidation_release.notify_waiters();
    wait_for_phase(&mut snapshot_receiver, WorkflowPhase::Inspected).await;
    intent_sender
        .send(ResetSessionIntent::Shutdown)
        .await
        .expect("shutdown intent");
    let outcome = session_task.await.expect("session task");

    // Assert
    assert_eq!(outcome, ResetSessionOutcome::Cancelled);
    assert!(
        fixture
            .control
            .inspection_inventory_dropped
            .load(Ordering::SeqCst)
    );
    assert_eq!(fixture.control.post_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn inventory_first_completion_remains_partial_until_usage_finishes() {
    // Arrange
    let fixture = session_fixture();
    let mut snapshot_receiver = fixture.ports.snapshot_receiver;
    let intent_sender = fixture.ports.intent_sender;
    let session_task = tokio::spawn(fixture.session.run());
    begin_inspection(&intent_sender).await;
    fixture.control.inspection_usage_started.notified().await;
    fixture
        .control
        .inspection_inventory_started
        .notified()
        .await;

    // Act
    fixture.control.inspection_inventory_release.notify_one();
    fixture
        .control
        .inspection_inventory_returned
        .notified()
        .await;
    snapshot_receiver.changed().await.expect("partial snapshot");

    // Assert
    assert_eq!(
        snapshot_receiver.borrow().phase(),
        WorkflowPhase::Inspecting
    );
    fixture.control.inspection_usage_release.notify_one();
    wait_for_phase(&mut snapshot_receiver, WorkflowPhase::Inspected).await;
    let snapshot = snapshot_receiver.borrow().clone();
    let rendered_snapshot = format!("{snapshot:?}");
    assert_eq!(
        snapshot
            .target()
            .expect("pinned target")
            .active_credential_generation,
        2
    );
    assert_eq!(
        snapshot.live_weekly().expect("live weekly").provenance,
        ResetValueProvenance::CurrentLive
    );
    assert_eq!(snapshot.credit_inventory().len(), 1);
    assert_eq!(
        snapshot.credit_inventory_provenance(),
        Some(ResetValueProvenance::CurrentLive)
    );
    let displayed_credit = snapshot
        .credit_inventory()
        .first()
        .expect("displayed credit");
    assert!(displayed_credit.earliest_usable);
    assert_eq!(displayed_credit.id_hint, "…8f42");
    assert!(snapshot.selected_credit().is_some());
    assert!(!rendered_snapshot.contains("credit-full-secret-canary-8f42"));
    assert!(!rendered_snapshot.contains("fake-supervisor-token"));
    drop(intent_sender);
    assert_eq!(
        session_task.await.expect("session task"),
        ResetSessionOutcome::Cancelled
    );
    assert_eq!(fixture.control.post_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn repeated_confirm_mints_once_and_committed_post_survives_presentation_teardown() {
    // Arrange
    let fixture = session_fixture();
    let mut snapshot_receiver = fixture.ports.snapshot_receiver;
    let intent_sender = fixture.ports.intent_sender;
    let session_task = tokio::spawn(fixture.session.run());
    begin_inspection(&intent_sender).await;
    fixture.control.inspection_usage_started.notified().await;
    fixture
        .control
        .inspection_inventory_started
        .notified()
        .await;
    fixture.control.inspection_usage_release.notify_one();
    fixture.control.inspection_inventory_release.notify_one();
    wait_for_phase(&mut snapshot_receiver, WorkflowPhase::Inspected).await;
    intent_sender
        .send(ResetSessionIntent::OpenConfirmation)
        .await
        .expect("open confirmation");
    intent_sender
        .send(ResetSessionIntent::SelectYes)
        .await
        .expect("select yes");

    // Act
    intent_sender
        .send(ResetSessionIntent::Confirm {
            now_unix_seconds: 100,
        })
        .await
        .expect("first confirm");
    fixture.control.revalidation_usage_started.notified().await;
    fixture
        .control
        .revalidation_inventory_started
        .notified()
        .await;
    intent_sender
        .send(ResetSessionIntent::Confirm {
            now_unix_seconds: 100,
        })
        .await
        .expect("duplicate confirm");
    fixture.control.revalidation_release.notify_waiters();
    fixture.control.consume_started.notified().await;
    wait_for_phase(&mut snapshot_receiver, WorkflowPhase::Committing).await;
    drop(intent_sender);

    // Assert
    assert!(!session_task.is_finished());
    assert_eq!(fixture.mints.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.control.usage_calls.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.control.inventory_calls.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.control.post_calls.load(Ordering::SeqCst), 1);
    fixture.control.consume_release.notify_one();
    assert_eq!(
        session_task.await.expect("session task"),
        ResetSessionOutcome::Finished(WorkflowResult::Known(KnownConsumeOutcome::Reset {
            windows_reset: 2
        }))
    );
}

struct SessionFixture {
    session: QuotaInteractiveSession<FakeAuthorityReader, FakeProvider, FakeRedeemRequestIdFactory>,
    ports: ResetSessionPorts,
    control: Arc<FakeProviderControl>,
    mints: Arc<AtomicUsize>,
}

fn session_fixture() -> SessionFixture {
    let control = Arc::new(FakeProviderControl::default());
    let mints = Arc::new(AtomicUsize::new(0));
    let service = ResetWorkflowService::new(
        FakeAuthorityReader {
            reads: Arc::new(AtomicUsize::new(0)),
        },
        FakeProvider {
            control: Arc::clone(&control),
        },
    );
    let (session, ports) = QuotaInteractiveSession::new(
        service,
        FakeRedeemRequestIdFactory {
            mints: Arc::clone(&mints),
        },
        8,
    );
    SessionFixture {
        session,
        ports,
        control,
        mints,
    }
}

async fn begin_inspection(intent_sender: &mpsc::Sender<ResetSessionIntent>) {
    intent_sender
        .send(ResetSessionIntent::BeginInspection {
            account_id: AccountId::new("acct_supervisor").expect("account id"),
            active_credential_generation: 2,
            now_unix_seconds: 100,
        })
        .await
        .expect("begin inspection");
}

async fn drive_to_committing(
    control: &FakeProviderControl,
    intent_sender: &mpsc::Sender<ResetSessionIntent>,
    snapshot_receiver: &mut watch::Receiver<ResetWorkflowSnapshot>,
) {
    begin_inspection(intent_sender).await;
    control.inspection_usage_started.notified().await;
    control.inspection_inventory_started.notified().await;
    control.inspection_usage_release.notify_one();
    control.inspection_inventory_release.notify_one();
    wait_for_phase(snapshot_receiver, WorkflowPhase::Inspected).await;
    intent_sender
        .send(ResetSessionIntent::OpenConfirmation)
        .await
        .expect("open confirmation");
    intent_sender
        .send(ResetSessionIntent::SelectYes)
        .await
        .expect("select yes");
    intent_sender
        .send(ResetSessionIntent::Confirm {
            now_unix_seconds: 100,
        })
        .await
        .expect("confirm");
    control.revalidation_usage_started.notified().await;
    control.revalidation_inventory_started.notified().await;
    control.revalidation_release.notify_waiters();
    control.consume_started.notified().await;
    wait_for_phase(snapshot_receiver, WorkflowPhase::Committing).await;
}

async fn wait_for_phase(
    snapshot_receiver: &mut watch::Receiver<ResetWorkflowSnapshot>,
    expected_phase: WorkflowPhase,
) {
    loop {
        if snapshot_receiver.borrow().phase() == expected_phase {
            return;
        }
        snapshot_receiver.changed().await.expect("snapshot update");
    }
}

async fn wait_for_weekly_remaining(
    snapshot_receiver: &mut watch::Receiver<ResetWorkflowSnapshot>,
    expected_remaining: u32,
) {
    loop {
        if snapshot_receiver
            .borrow()
            .live_weekly()
            .is_some_and(|weekly| weekly.remaining_percent == expected_remaining)
        {
            return;
        }
        snapshot_receiver.changed().await.expect("snapshot update");
    }
}

fn validated_inventory() -> CreditInventoryPortResult {
    CreditInventoryPortResult::Validated(
        validate_credit_inventory(
            vec![LiveResetCredit {
                id: "credit-full-secret-canary-8f42".to_owned(),
                status: "available".to_owned(),
                expires_unix_seconds: Some(200),
                expires_at: Some("unix-200".to_owned()),
                title: Some("Weekly reset".to_owned()),
            }],
            100,
        )
        .expect("valid inventory"),
    )
}
