//! Non-spawning inspection, revalidation, and single-use commit service.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use codex_router_core::ids::AccountId;

use super::QuotaResetError;
use super::credential_authority::CredentialFingerprint;
use super::credential_authority::PinnedResetAuthority;
use super::credential_authority::PreparedCredentialAuthorityRead;
use super::credential_authority::prepare_reset_credential_authority_read;
use super::provider_protocol::HttpLiveQuotaResetProvider;
use super::provider_protocol::LiveResetAccountAuth;
use super::provider_protocol::PreparedConsumeRequest;
use super::reset_credit_policy::ActiveCredentialGeneration;
use super::reset_credit_policy::AttemptGeneration;
use super::reset_credit_policy::ConsumePortResult;
use super::reset_credit_policy::CreditInventoryPortResult;
use super::reset_credit_policy::LiveUsagePortResult;
use super::reset_credit_policy::LiveWeeklyUsage;
use super::reset_credit_policy::RedeemRequestId;
use super::reset_credit_policy::RenderSafeFailure;
use super::reset_credit_policy::SelectedResetCreditSnapshot;
use super::reset_credit_policy::ValidatedCreditInventory;
use super::reset_credit_policy::validate_credit_inventory;

pub(in crate::quota_reset) struct LiveResetAuthorityReader {
    state_database_path: PathBuf,
    secret_root: PathBuf,
}

impl LiveResetAuthorityReader {
    pub(in crate::quota_reset) const fn new(
        state_database_path: PathBuf,
        secret_root: PathBuf,
    ) -> Self {
        Self {
            state_database_path,
            secret_root,
        }
    }
}

impl ResetAuthority for PinnedResetAuthority {
    type Fingerprint = CredentialFingerprint;

    fn account_id(&self) -> &AccountId {
        self.account_id()
    }
    fn active_credential_generation(&self) -> ActiveCredentialGeneration {
        self.active_credential_generation()
    }
    fn auth(&self) -> LiveResetAccountAuth {
        LiveResetAccountAuth {
            access_token: self.access_token().clone(),
            chatgpt_account_id: self.chatgpt_account_id().to_owned(),
        }
    }
    fn expires_unix_seconds(&self) -> Option<u64> {
        self.expires_unix_seconds()
    }
    fn fingerprint(&self) -> &Self::Fingerprint {
        self.fingerprint()
    }
}

impl ResetAuthorityReader for LiveResetAuthorityReader {
    type Authority = PinnedResetAuthority;
    type PreparedRead = PreparedCredentialAuthorityRead;

    async fn prepare_authority_read(
        &self,
        account_id: &AccountId,
        expected_generation: ActiveCredentialGeneration,
        now_unix_seconds: u64,
    ) -> Result<Self::PreparedRead, RenderSafeFailure> {
        prepare_reset_credential_authority_read(
            &self.state_database_path,
            &self.secret_root,
            account_id,
            expected_generation,
            now_unix_seconds,
        )
        .await
        .map_err(render_safe_credential_failure)
    }

    fn start_authority_read(prepared: Self::PreparedRead) -> StartedAuthorityRead<Self::Authority> {
        let mut started = prepared.start();
        Box::pin(async move {
            started
                .drain()
                .await
                .map_err(render_safe_credential_failure)
        })
    }
}

fn render_safe_credential_failure(
    error: super::credential_authority::CredentialAuthorityError,
) -> RenderSafeFailure {
    match error {
        super::credential_authority::CredentialAuthorityError::AccountUnavailable => {
            RenderSafeFailure::AccountUnavailable
        }
        super::credential_authority::CredentialAuthorityError::GenerationChanged => {
            RenderSafeFailure::CredentialGenerationChanged
        }
        super::credential_authority::CredentialAuthorityError::Expired => {
            RenderSafeFailure::CredentialExpired
        }
        _ => RenderSafeFailure::CredentialUnavailable,
    }
}

impl ResetServiceProvider for HttpLiveQuotaResetProvider {
    type PreparedConsume = PreparedConsumeRequest;

    async fn fetch_usage(&self, auth: LiveResetAccountAuth) -> LiveUsagePortResult {
        match self.fetch_weekly_remaining_percent(&auth).await {
            Ok(Some(percent)) => LiveUsagePortResult::Known(LiveWeeklyUsage::new(percent)),
            Ok(None) => LiveUsagePortResult::Failed(RenderSafeFailure::EligibilityRefused),
            Err(error) => LiveUsagePortResult::Failed(render_safe_provider_failure(&error)),
        }
    }

    async fn fetch_inventory(
        &self,
        auth: LiveResetAccountAuth,
        now_unix_seconds: i64,
    ) -> CreditInventoryPortResult {
        match self.fetch_reset_credits(&auth).await {
            Ok(credits) => match validate_credit_inventory(credits, now_unix_seconds) {
                Ok(inventory) => CreditInventoryPortResult::Validated(inventory),
                Err(_) => CreditInventoryPortResult::Failed(RenderSafeFailure::InvalidResponse),
            },
            Err(error) => CreditInventoryPortResult::Failed(render_safe_provider_failure(&error)),
        }
    }

    fn prepare_consume(
        &self,
        auth: &LiveResetAccountAuth,
        selected_credit: &SelectedResetCreditSnapshot,
        redeem_request_id: &RedeemRequestId,
    ) -> Result<Self::PreparedConsume, RenderSafeFailure> {
        self.prepare_consume_reset_credit(auth, selected_credit.id(), redeem_request_id.as_str())
            .map_err(|error| render_safe_provider_failure(&error))
    }

    async fn invoke_prepared(&self, prepared: Self::PreparedConsume) -> ConsumePortResult {
        self.invoke_prepared_consume(prepared).await
    }
}

fn render_safe_provider_failure(error: &QuotaResetError) -> RenderSafeFailure {
    match error {
        QuotaResetError::Request { .. } => RenderSafeFailure::Transport,
        QuotaResetError::Status { .. } => RenderSafeFailure::ProviderStatus,
        _ => RenderSafeFailure::InvalidResponse,
    }
}

pub(in crate::quota_reset) trait ResetAuthority:
    Send + Sync + 'static
{
    type Fingerprint: Eq + Send + Sync;

    fn account_id(&self) -> &AccountId;
    fn active_credential_generation(&self) -> ActiveCredentialGeneration;
    fn auth(&self) -> LiveResetAccountAuth;
    fn expires_unix_seconds(&self) -> Option<u64>;
    fn fingerprint(&self) -> &Self::Fingerprint;
}

pub(in crate::quota_reset) type StartedAuthorityRead<TAuthority> =
    Pin<Box<dyn Future<Output = Result<TAuthority, RenderSafeFailure>> + Send + 'static>>;

pub(in crate::quota_reset) trait ResetAuthorityReader: Send + Sync {
    type Authority: ResetAuthority;
    type PreparedRead: Send + 'static;

    fn prepare_authority_read(
        &self,
        account_id: &AccountId,
        expected_generation: ActiveCredentialGeneration,
        now_unix_seconds: u64,
    ) -> impl Future<Output = Result<Self::PreparedRead, RenderSafeFailure>> + Send;

    fn start_authority_read(prepared: Self::PreparedRead) -> StartedAuthorityRead<Self::Authority>;
}

pub(in crate::quota_reset) trait ResetServiceProvider: Send + Sync {
    type PreparedConsume: Send;

    fn fetch_usage(
        &self,
        auth: LiveResetAccountAuth,
    ) -> impl Future<Output = LiveUsagePortResult> + Send;

    fn fetch_inventory(
        &self,
        auth: LiveResetAccountAuth,
        now_unix_seconds: i64,
    ) -> impl Future<Output = CreditInventoryPortResult> + Send;

    fn prepare_consume(
        &self,
        auth: &LiveResetAccountAuth,
        selected_credit: &SelectedResetCreditSnapshot,
        redeem_request_id: &RedeemRequestId,
    ) -> Result<Self::PreparedConsume, RenderSafeFailure>;

    fn invoke_prepared(
        &self,
        prepared: Self::PreparedConsume,
    ) -> impl Future<Output = ConsumePortResult> + Send;
}

pub(in crate::quota_reset) struct InspectionAuthority<TAuthority: ResetAuthority> {
    authority: Arc<TAuthority>,
}

impl<TAuthority: ResetAuthority> Clone for InspectionAuthority<TAuthority> {
    fn clone(&self) -> Self {
        Self {
            authority: Arc::clone(&self.authority),
        }
    }
}

pub(in crate::quota_reset) struct ConfirmationAuthority<TAuthority: ResetAuthority> {
    authority: Arc<TAuthority>,
    attempt_generation: AttemptGeneration,
    weekly_usage: LiveWeeklyUsage,
    selected_credit: SelectedResetCreditSnapshot,
}

impl<TAuthority: ResetAuthority> ConfirmationAuthority<TAuthority> {
    pub(in crate::quota_reset) fn account_id(&self) -> &AccountId {
        self.authority.account_id()
    }

    pub(in crate::quota_reset) fn active_credential_generation(
        &self,
    ) -> ActiveCredentialGeneration {
        self.authority.active_credential_generation()
    }
}

pub(in crate::quota_reset) struct RevalidationContext<TAuthority: ResetAuthority> {
    fresh_authority: Arc<TAuthority>,
    attempt_generation: AttemptGeneration,
    expected_weekly_usage: LiveWeeklyUsage,
    expected_selected_credit: SelectedResetCreditSnapshot,
}

impl<TAuthority: ResetAuthority> Clone for RevalidationContext<TAuthority> {
    fn clone(&self) -> Self {
        Self {
            fresh_authority: Arc::clone(&self.fresh_authority),
            attempt_generation: self.attempt_generation,
            expected_weekly_usage: self.expected_weekly_usage,
            expected_selected_credit: self.expected_selected_credit.clone(),
        }
    }
}

pub(in crate::quota_reset) struct ValidatedRevalidation<TAuthority: ResetAuthority> {
    fresh_authority: Arc<TAuthority>,
    attempt_generation: AttemptGeneration,
    selected_credit: SelectedResetCreditSnapshot,
}

pub(in crate::quota_reset) struct CommitCapability<TAuthority, TPreparedConsume>
where
    TAuthority: ResetAuthority,
{
    _authority: Arc<TAuthority>,
    _attempt_generation: AttemptGeneration,
    _redeem_request_id: RedeemRequestId,
    prepared_consume: TPreparedConsume,
}

#[cfg(test)]
pub(in crate::quota_reset) struct RevalidationReceipt<TAuthority, TPreparedConsume>
where
    TAuthority: ResetAuthority,
{
    pub(in crate::quota_reset) authorization:
        Result<CommitCapability<TAuthority, TPreparedConsume>, RenderSafeFailure>,
}

impl<TAuthority, TPreparedConsume> std::fmt::Debug
    for CommitCapability<TAuthority, TPreparedConsume>
where
    TAuthority: ResetAuthority,
{
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CommitCapability(<opaque>)")
    }
}

pub(in crate::quota_reset) struct ResetWorkflowService<TAuthorityReader, TProvider> {
    authority_reader: TAuthorityReader,
    provider: TProvider,
}

impl<TAuthorityReader, TProvider> ResetWorkflowService<TAuthorityReader, TProvider>
where
    TAuthorityReader: ResetAuthorityReader,
    TProvider: ResetServiceProvider,
{
    pub(in crate::quota_reset) const fn new(
        authority_reader: TAuthorityReader,
        provider: TProvider,
    ) -> Self {
        Self {
            authority_reader,
            provider,
        }
    }

    pub(in crate::quota_reset) async fn prepare_authority_read(
        &self,
        account_id: &AccountId,
        expected_generation: ActiveCredentialGeneration,
        now_unix_seconds: u64,
    ) -> Result<TAuthorityReader::PreparedRead, RenderSafeFailure> {
        self.authority_reader
            .prepare_authority_read(account_id, expected_generation, now_unix_seconds)
            .await
    }

    pub(in crate::quota_reset) fn start_authority_read(
        prepared: TAuthorityReader::PreparedRead,
    ) -> StartedAuthorityRead<TAuthorityReader::Authority> {
        TAuthorityReader::start_authority_read(prepared)
    }

    pub(in crate::quota_reset) fn bind_inspection_authority(
        authority: TAuthorityReader::Authority,
    ) -> InspectionAuthority<TAuthorityReader::Authority> {
        InspectionAuthority {
            authority: Arc::new(authority),
        }
    }

    #[cfg(test)]
    pub(in crate::quota_reset) async fn resolve_inspection_authority(
        &self,
        account_id: &AccountId,
        expected_generation: ActiveCredentialGeneration,
        now_unix_seconds: u64,
    ) -> Result<InspectionAuthority<TAuthorityReader::Authority>, RenderSafeFailure> {
        let prepared = self
            .prepare_authority_read(account_id, expected_generation, now_unix_seconds)
            .await?;
        let authority = Self::start_authority_read(prepared).await?;
        Ok(Self::bind_inspection_authority(authority))
    }

    pub(in crate::quota_reset) async fn inspect_usage(
        &self,
        inspection: InspectionAuthority<TAuthorityReader::Authority>,
    ) -> LiveUsagePortResult {
        self.provider.fetch_usage(inspection.authority.auth()).await
    }

    pub(in crate::quota_reset) async fn inspect_inventory(
        &self,
        inspection: InspectionAuthority<TAuthorityReader::Authority>,
        now_unix_seconds: i64,
    ) -> CreditInventoryPortResult {
        self.provider
            .fetch_inventory(inspection.authority.auth(), now_unix_seconds)
            .await
    }

    pub(in crate::quota_reset) fn bind_confirmation(
        inspection: InspectionAuthority<TAuthorityReader::Authority>,
        attempt_generation: AttemptGeneration,
        weekly_usage: LiveWeeklyUsage,
        inventory: &ValidatedCreditInventory,
    ) -> Result<ConfirmationAuthority<TAuthorityReader::Authority>, RenderSafeFailure> {
        let selected_credit = inventory
            .earliest_usable_snapshot()
            .ok_or(RenderSafeFailure::EligibilityRefused)?;
        Ok(ConfirmationAuthority {
            authority: inspection.authority,
            attempt_generation,
            weekly_usage,
            selected_credit,
        })
    }

    pub(in crate::quota_reset) fn bind_revalidation_context(
        confirmation: ConfirmationAuthority<TAuthorityReader::Authority>,
        fresh_authority: TAuthorityReader::Authority,
        now_unix_seconds: u64,
    ) -> Result<RevalidationContext<TAuthorityReader::Authority>, RenderSafeFailure> {
        let fresh_authority = Arc::new(fresh_authority);
        if fresh_authority.fingerprint() != confirmation.authority.fingerprint() {
            return Err(RenderSafeFailure::CredentialUnavailable);
        }
        if fresh_authority
            .expires_unix_seconds()
            .is_some_and(|expires_at| expires_at <= now_unix_seconds)
        {
            return Err(RenderSafeFailure::CredentialExpired);
        }

        Ok(RevalidationContext {
            fresh_authority,
            attempt_generation: confirmation.attempt_generation,
            expected_weekly_usage: confirmation.weekly_usage,
            expected_selected_credit: confirmation.selected_credit,
        })
    }

    pub(in crate::quota_reset) async fn revalidate_usage(
        &self,
        context: RevalidationContext<TAuthorityReader::Authority>,
    ) -> LiveUsagePortResult {
        self.provider
            .fetch_usage(context.fresh_authority.auth())
            .await
    }

    pub(in crate::quota_reset) async fn revalidate_inventory(
        &self,
        context: RevalidationContext<TAuthorityReader::Authority>,
        now_unix_seconds: i64,
    ) -> CreditInventoryPortResult {
        self.provider
            .fetch_inventory(context.fresh_authority.auth(), now_unix_seconds)
            .await
    }

    pub(in crate::quota_reset) fn validate_revalidation(
        context: RevalidationContext<TAuthorityReader::Authority>,
        usage: &LiveUsagePortResult,
        inventory: &CreditInventoryPortResult,
    ) -> Result<ValidatedRevalidation<TAuthorityReader::Authority>, RenderSafeFailure> {
        let LiveUsagePortResult::Known(live_usage) = usage else {
            return Err(RenderSafeFailure::EligibilityRefused);
        };
        let CreditInventoryPortResult::Validated(validated_inventory) = inventory else {
            return Err(RenderSafeFailure::SelectedCreditChanged);
        };
        if *live_usage != context.expected_weekly_usage || live_usage.remaining_percent() >= 1 {
            return Err(RenderSafeFailure::EligibilityRefused);
        }
        let selected_credit = match validated_inventory.earliest_usable_snapshot() {
            Some(selected_credit) => selected_credit,
            None => return Err(RenderSafeFailure::SelectedCreditChanged),
        };
        if selected_credit != context.expected_selected_credit {
            return Err(RenderSafeFailure::SelectedCreditChanged);
        }

        Ok(ValidatedRevalidation {
            fresh_authority: context.fresh_authority,
            attempt_generation: context.attempt_generation,
            selected_credit,
        })
    }

    pub(in crate::quota_reset) fn authorize_commit(
        &self,
        validated: ValidatedRevalidation<TAuthorityReader::Authority>,
        commit_unix_seconds: u64,
        redeem_request_id: RedeemRequestId,
    ) -> Result<
        CommitCapability<TAuthorityReader::Authority, TProvider::PreparedConsume>,
        RenderSafeFailure,
    > {
        Self::validate_commit_time(&validated, commit_unix_seconds)?;
        let auth = validated.fresh_authority.auth();
        let prepared_consume =
            self.provider
                .prepare_consume(&auth, &validated.selected_credit, &redeem_request_id)?;
        Ok(CommitCapability {
            _authority: validated.fresh_authority,
            _attempt_generation: validated.attempt_generation,
            _redeem_request_id: redeem_request_id,
            prepared_consume,
        })
    }

    pub(in crate::quota_reset) fn validate_commit_time(
        validated: &ValidatedRevalidation<TAuthorityReader::Authority>,
        commit_unix_seconds: u64,
    ) -> Result<(), RenderSafeFailure> {
        if validated
            .fresh_authority
            .expires_unix_seconds()
            .is_some_and(|expires_at| expires_at <= commit_unix_seconds)
        {
            return Err(RenderSafeFailure::CredentialExpired);
        }
        let commit_unix_seconds =
            i64::try_from(commit_unix_seconds).map_err(|_| RenderSafeFailure::InvalidResponse)?;
        if validated
            .selected_credit
            .expires_unix_seconds()
            .is_some_and(|expires_at| expires_at <= commit_unix_seconds)
        {
            return Err(RenderSafeFailure::SelectedCreditChanged);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(in crate::quota_reset) async fn revalidate(
        &self,
        confirmation: ConfirmationAuthority<TAuthorityReader::Authority>,
        now_unix_seconds: u64,
        redeem_request_id: RedeemRequestId,
    ) -> RevalidationReceipt<TAuthorityReader::Authority, TProvider::PreparedConsume> {
        let account_id = confirmation.authority.account_id().clone();
        let generation = confirmation.authority.active_credential_generation();
        let prepared = match self
            .prepare_authority_read(&account_id, generation, now_unix_seconds)
            .await
        {
            Ok(prepared) => prepared,
            Err(failure) => return refused_revalidation(failure),
        };
        let fresh_authority = match Self::start_authority_read(prepared).await {
            Ok(authority) => authority,
            Err(failure) => return refused_revalidation(failure),
        };
        let context = match Self::bind_revalidation_context(
            confirmation,
            fresh_authority,
            now_unix_seconds,
        ) {
            Ok(context) => context,
            Err(failure) => return refused_revalidation(failure),
        };
        let usage = self.revalidate_usage(context.clone()).await;
        let inventory = self
            .revalidate_inventory(context.clone(), now_unix_seconds as i64)
            .await;
        let authorization =
            Self::validate_revalidation(context, &usage, &inventory).and_then(|validated| {
                self.authorize_commit(validated, now_unix_seconds, redeem_request_id)
            });
        RevalidationReceipt { authorization }
    }

    pub(in crate::quota_reset) async fn consume(
        &self,
        capability: CommitCapability<TAuthorityReader::Authority, TProvider::PreparedConsume>,
    ) -> ConsumePortResult {
        self.provider
            .invoke_prepared(capability.prepared_consume)
            .await
    }
}

#[cfg(test)]
fn refused_revalidation<TAuthority, TPreparedConsume>(
    failure: RenderSafeFailure,
) -> RevalidationReceipt<TAuthority, TPreparedConsume>
where
    TAuthority: ResetAuthority,
{
    RevalidationReceipt {
        authorization: Err(failure),
    }
}

#[cfg(test)]
#[path = "reset_commit_service_test.rs"]
mod reset_commit_service_test;
