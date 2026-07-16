//! Correlated effect completions and monotonic session generations.

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use crate::quota_reset::domain::AttemptGeneration;
use crate::quota_reset::domain::ConsumePortResult;
use crate::quota_reset::domain::CreditInventoryPortResult;
use crate::quota_reset::domain::LiveUsagePortResult;
use crate::quota_reset::domain::OperationGeneration;
use crate::quota_reset::domain::RedeemRequestId;
use crate::quota_reset::domain::RenderSafeFailure;
use crate::quota_reset::service::InspectionAuthority;
use crate::quota_reset::service::ResetAuthorityReader;
use crate::quota_reset::service::ResetServiceProvider;
use crate::quota_reset::service::RevalidationReceipt;
use crate::quota_reset::workflow::InspectionStart;
use crate::quota_reset::workflow::OperationCorrelation;

pub(in crate::quota_reset) trait RedeemRequestIdFactory:
    Send + Sync + 'static
{
    fn mint(&self) -> Result<RedeemRequestId, RenderSafeFailure>;
}

static REDEEM_REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(in crate::quota_reset) struct ProductionRedeemRequestIdFactory;

impl RedeemRequestIdFactory for ProductionRedeemRequestIdFactory {
    fn mint(&self) -> Result<RedeemRequestId, RenderSafeFailure> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| RenderSafeFailure::CredentialUnavailable)?
            .as_nanos();
        let counter = REDEEM_REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        RedeemRequestId::new(format!(
            "codex-router-{}-{nanos}-{counter}",
            std::process::id()
        ))
        .map_err(|_| RenderSafeFailure::InvalidResponse)
    }
}

#[derive(Default)]
pub(super) struct GenerationAllocator {
    next_attempt: u64,
    next_operation: u64,
}

impl GenerationAllocator {
    pub(super) fn allocate_attempt(&mut self) -> AttemptGeneration {
        let generation = AttemptGeneration::new(self.next_attempt);
        self.next_attempt = self.next_attempt.wrapping_add(1);
        generation
    }

    pub(super) fn allocate_operation(&mut self) -> OperationGeneration {
        let generation = OperationGeneration::new(self.next_operation);
        self.next_operation = self.next_operation.wrapping_add(1);
        generation
    }

    pub(super) fn current_attempt(&self) -> Option<AttemptGeneration> {
        self.next_attempt.checked_sub(1).map(AttemptGeneration::new)
    }
}

pub(super) enum SessionTaskOutput<TAuthorityReader, TProvider>
where
    TAuthorityReader: ResetAuthorityReader,
    TProvider: ResetServiceProvider,
{
    InspectionAuthorityResolved {
        start: InspectionStart,
        now_unix_seconds: u64,
        authority: Result<InspectionAuthority<TAuthorityReader::Authority>, RenderSafeFailure>,
    },
    InspectionUsageCompleted {
        correlation: OperationCorrelation,
        terminal: LiveUsagePortResult,
    },
    InspectionInventoryCompleted {
        correlation: OperationCorrelation,
        terminal: CreditInventoryPortResult,
    },
    RevalidationCompleted {
        usage_correlation: OperationCorrelation,
        inventory_correlation: OperationCorrelation,
        receipt: RevalidationReceipt<TAuthorityReader::Authority, TProvider::PreparedConsume>,
    },
    ConsumeCompleted {
        correlation: OperationCorrelation,
        terminal: ConsumePortResult,
    },
}
