//! Non-spawning inspection, revalidation, and single-use commit service.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use codex_router_core::ids::AccountId;

use super::QuotaResetError;
use super::credentials::CredentialFingerprint;
use super::credentials::PinnedResetAuthority;
use super::credentials::load_reset_credential_authority;
use super::domain::ActiveCredentialGeneration;
use super::domain::AttemptGeneration;
use super::domain::ConsumePortResult;
use super::domain::CreditInventoryPortResult;
use super::domain::LiveUsagePortResult;
use super::domain::LiveWeeklyUsage;
use super::domain::RedeemRequestId;
use super::domain::RenderSafeFailure;
use super::domain::SelectedResetCreditSnapshot;
use super::domain::ValidatedCreditInventory;
use super::domain::validate_credit_inventory;
use super::provider::HttpLiveQuotaResetProvider;
use super::provider::LiveQuotaResetProvider;
use super::provider::LiveResetAccountAuth;
use super::provider::PreparedConsumeRequest;

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

    async fn read_authority(
        &self,
        account_id: &AccountId,
        expected_generation: ActiveCredentialGeneration,
        now_unix_seconds: u64,
    ) -> Result<Self::Authority, RenderSafeFailure> {
        load_reset_credential_authority(
            &self.state_database_path,
            &self.secret_root,
            account_id,
            expected_generation,
            now_unix_seconds,
        )
        .await
        .map_err(|error| match error {
            super::credentials::CredentialAuthorityError::AccountUnavailable => {
                RenderSafeFailure::AccountUnavailable
            }
            super::credentials::CredentialAuthorityError::GenerationChanged => {
                RenderSafeFailure::CredentialGenerationChanged
            }
            super::credentials::CredentialAuthorityError::Expired => {
                RenderSafeFailure::CredentialExpired
            }
            _ => RenderSafeFailure::CredentialUnavailable,
        })
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
        QuotaResetError::ProviderRequest { .. } => RenderSafeFailure::Transport,
        QuotaResetError::ProviderStatus { .. } => RenderSafeFailure::ProviderStatus,
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

pub(in crate::quota_reset) trait ResetAuthorityReader: Send + Sync {
    type Authority: ResetAuthority;

    fn read_authority(
        &self,
        account_id: &AccountId,
        expected_generation: ActiveCredentialGeneration,
        now_unix_seconds: u64,
    ) -> impl Future<Output = Result<Self::Authority, RenderSafeFailure>> + Send;
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

pub(in crate::quota_reset) struct CommitCapability<TAuthority, TPreparedConsume>
where
    TAuthority: ResetAuthority,
{
    _authority: Arc<TAuthority>,
    _attempt_generation: AttemptGeneration,
    _redeem_request_id: RedeemRequestId,
    prepared_consume: TPreparedConsume,
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

    pub(in crate::quota_reset) async fn resolve_inspection_authority(
        &self,
        account_id: &AccountId,
        expected_generation: ActiveCredentialGeneration,
        now_unix_seconds: u64,
    ) -> Result<InspectionAuthority<TAuthorityReader::Authority>, RenderSafeFailure> {
        self.authority_reader
            .read_authority(account_id, expected_generation, now_unix_seconds)
            .await
            .map(|authority| InspectionAuthority {
                authority: Arc::new(authority),
            })
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

    pub(in crate::quota_reset) async fn revalidate(
        &self,
        confirmation: ConfirmationAuthority<TAuthorityReader::Authority>,
        now_unix_seconds: u64,
        redeem_request_id: RedeemRequestId,
    ) -> Result<
        CommitCapability<TAuthorityReader::Authority, TProvider::PreparedConsume>,
        RenderSafeFailure,
    > {
        let fresh_authority = Arc::new(
            self.authority_reader
                .read_authority(
                    confirmation.authority.account_id(),
                    confirmation.authority.active_credential_generation(),
                    now_unix_seconds,
                )
                .await?,
        );
        if fresh_authority.fingerprint() != confirmation.authority.fingerprint() {
            return Err(RenderSafeFailure::CredentialUnavailable);
        }
        if fresh_authority
            .expires_unix_seconds()
            .is_some_and(|expires_at| expires_at <= now_unix_seconds)
        {
            return Err(RenderSafeFailure::CredentialExpired);
        }

        let auth = fresh_authority.auth();
        let (usage, inventory) = tokio::join!(
            self.provider.fetch_usage(auth.clone()),
            self.provider
                .fetch_inventory(auth.clone(), now_unix_seconds as i64),
        );
        let LiveUsagePortResult::Known(usage) = usage else {
            return Err(RenderSafeFailure::EligibilityRefused);
        };
        let CreditInventoryPortResult::Validated(inventory) = inventory else {
            return Err(RenderSafeFailure::SelectedCreditChanged);
        };
        if usage != confirmation.weekly_usage || usage.remaining_percent() >= 1 {
            return Err(RenderSafeFailure::EligibilityRefused);
        }
        let selected_credit = inventory
            .earliest_usable_snapshot()
            .ok_or(RenderSafeFailure::SelectedCreditChanged)?;
        if selected_credit != confirmation.selected_credit
            || selected_credit
                .expires_unix_seconds()
                .is_some_and(|expires_at| expires_at <= now_unix_seconds as i64)
        {
            return Err(RenderSafeFailure::SelectedCreditChanged);
        }
        let prepared_consume =
            self.provider
                .prepare_consume(&auth, &selected_credit, &redeem_request_id)?;
        Ok(CommitCapability {
            _authority: fresh_authority,
            _attempt_generation: confirmation.attempt_generation,
            _redeem_request_id: redeem_request_id,
            prepared_consume,
        })
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
mod tests;
