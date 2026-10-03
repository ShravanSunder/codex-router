//! Serialized graceful floor-switch decision at established WebSocket turn boundaries.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use codex_router_core::ids::AccountId;
use codex_router_core::routes::RouteBand;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::account_selection::AccountSourceAdmission;
use crate::account_selection::FloorSwitchPeerAssessment;
use crate::account_selection::LiveAccountAdmissionAssessor;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct FloorSwitchIntent {
    pub(super) epoch: u64,
    pub(super) pending: bool,
}

#[derive(Clone)]
pub(super) struct AccountTurnAdmission {
    active_turn: Arc<AsyncMutex<bool>>,
    turn_activity: watch::Sender<bool>,
    intent: watch::Receiver<FloorSwitchIntent>,
    early_reconnect: CancellationToken,
    hard_reconnect: CancellationToken,
    source_account_id: Option<AccountId>,
    pinned_credential_generation: Option<u64>,
    peer_assessor: Option<Arc<dyn LiveAccountAdmissionAssessor>>,
    credit_backed_admission_seen: Arc<AtomicBool>,
}

impl AccountTurnAdmission {
    pub(super) fn new(
        intent: watch::Receiver<FloorSwitchIntent>,
        early_reconnect: CancellationToken,
        hard_reconnect: CancellationToken,
        source_account_id: Option<AccountId>,
        pinned_credential_generation: Option<u64>,
        peer_assessor: Option<Arc<dyn LiveAccountAdmissionAssessor>>,
        initial_turn_active: bool,
    ) -> Self {
        let (turn_activity, _) = watch::channel(initial_turn_active);
        Self {
            active_turn: Arc::new(AsyncMutex::new(initial_turn_active)),
            turn_activity,
            intent,
            early_reconnect,
            hard_reconnect,
            source_account_id,
            pinned_credential_generation,
            peer_assessor,
            credit_backed_admission_seen: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(super) fn with_credit_backed_admission_seen(
        self,
        credit_backed_at_selection: bool,
    ) -> Self {
        self.credit_backed_admission_seen
            .store(credit_backed_at_selection, Ordering::Release);
        self
    }

    pub(super) async fn before_next_create(&self) -> bool {
        let mut turn_activity = self.turn_activity.subscribe();
        loop {
            if self.hard_reconnect.is_cancelled() || self.early_reconnect.is_cancelled() {
                return true;
            }
            let active_turn = tokio::select! {
                biased;
                () = self.hard_reconnect.cancelled() => return true,
                () = self.early_reconnect.cancelled() => return true,
                active_turn = self.active_turn.lock() => active_turn,
            };
            if self.hard_reconnect.is_cancelled() || self.early_reconnect.is_cancelled() {
                return true;
            }
            let active_turn_when_source_assessment_started = *active_turn;
            drop(active_turn);
            if self.source_account_requires_reconnect().await {
                let active_turn = tokio::select! {
                    biased;
                    () = self.hard_reconnect.cancelled() => return true,
                    () = self.early_reconnect.cancelled() => return true,
                    active_turn = self.active_turn.lock() => active_turn,
                };
                if *active_turn {
                    drop(active_turn);
                    tokio::select! {
                        biased;
                        () = self.hard_reconnect.cancelled() => return true,
                        () = self.early_reconnect.cancelled() => return true,
                        changed = turn_activity.changed() => {
                            if changed.is_err() { return true; }
                        }
                    }
                    continue;
                }
                if active_turn_when_source_assessment_started {
                    drop(active_turn);
                    continue;
                }
                self.early_reconnect.cancel();
                return true;
            }
            let mut active_turn = tokio::select! {
                biased;
                () = self.hard_reconnect.cancelled() => return true,
                () = self.early_reconnect.cancelled() => return true,
                active_turn = self.active_turn.lock() => active_turn,
            };
            if self.hard_reconnect.is_cancelled() || self.early_reconnect.is_cancelled() {
                return true;
            }
            let observed_intent = *self.intent.borrow();
            if observed_intent.pending && *active_turn {
                drop(active_turn);
                tokio::select! {
                    biased;
                    () = self.hard_reconnect.cancelled() => return true,
                    () = self.early_reconnect.cancelled() => return true,
                    changed = turn_activity.changed() => {
                        if changed.is_err() { return true; }
                    }
                }
                continue;
            }
            if observed_intent.pending
                && self.switch_if_safe(&mut active_turn, observed_intent).await
            {
                return true;
            }
            if *self.intent.borrow() != observed_intent {
                continue;
            }
            if self.hard_reconnect.is_cancelled() || self.early_reconnect.is_cancelled() {
                return true;
            }
            *active_turn = true;
            self.turn_activity.send_replace(true);
            return false;
        }
    }

    pub(super) async fn deliver_terminal_and_release_turn<E>(
        &self,
        delivery: impl Future<Output = Result<(), E>>,
    ) -> Result<(), E> {
        let mut active_turn = self.active_turn.lock().await;
        delivery.await?;
        *active_turn = false;
        self.turn_activity.send_replace(false);
        let observed_intent = *self.intent.borrow();
        if observed_intent.pending {
            let _switched = self.switch_if_safe(&mut active_turn, observed_intent).await;
        }
        Ok(())
    }

    pub(super) async fn on_idle_intent(&self) {
        let mut active_turn = self.active_turn.lock().await;
        if *active_turn {
            return;
        }
        let observed_intent = *self.intent.borrow();
        if observed_intent.pending {
            let _switched = self.switch_if_safe(&mut active_turn, observed_intent).await;
        }
    }

    async fn switch_if_safe(
        &self,
        active_turn: &mut bool,
        observed_intent: FloorSwitchIntent,
    ) -> bool {
        if *active_turn || self.hard_reconnect.is_cancelled() {
            return self.hard_reconnect.is_cancelled();
        }
        let Some(peer_assessor) = &self.peer_assessor else {
            return false;
        };
        let Some(source_account_id) = self.source_account_id.as_ref() else {
            return false;
        };
        let outcome = tokio::select! {
            biased;
            () = self.hard_reconnect.cancelled() => return true,
            () = self.early_reconnect.cancelled() => return true,
            outcome = peer_assessor.assess_peer(source_account_id, RouteBand::Responses) => outcome,
        };
        if outcome == FloorSwitchPeerAssessment::SelectablePeer
            && !self.hard_reconnect.is_cancelled()
            && *self.intent.borrow() == observed_intent
            && observed_intent.pending
            && !*active_turn
        {
            self.early_reconnect.cancel();
            return true;
        }
        self.hard_reconnect.is_cancelled() || self.early_reconnect.is_cancelled()
    }

    async fn source_account_requires_reconnect(&self) -> bool {
        let (Some(assessor), Some(source_account_id), Some(pinned_generation)) = (
            self.peer_assessor.as_ref(),
            self.source_account_id.as_ref(),
            self.pinned_credential_generation,
        ) else {
            return false;
        };
        let credit_backed_admission_seen =
            self.credit_backed_admission_seen.load(Ordering::Acquire);
        let assessment = tokio::select! {
            biased;
            () = self.hard_reconnect.cancelled() => return true,
            assessment = tokio::time::timeout(
                super::POST_EXHAUSTION_ALTERNATIVE_SELECTION_TIMEOUT,
                assessor.assess_source_account(
                    source_account_id,
                    pinned_generation,
                    RouteBand::Responses,
                    credit_backed_admission_seen,
                ),
            ) => match assessment {
                Ok(assessment) => assessment,
                Err(_) => AccountSourceAdmission::ReconnectRequired,
            },
        };
        match assessment {
            AccountSourceAdmission::Permitted => false,
            AccountSourceAdmission::PermittedByCreditBackedQuota => {
                self.credit_backed_admission_seen
                    .store(true, Ordering::Release);
                false
            }
            AccountSourceAdmission::ReconnectRequired => true,
        }
    }
}
