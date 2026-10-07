//! Async generation-scoped credential renewal and shutdown supervision.

use codex_router_secret_store::account_tokens::first_unused_account_credential_generation;

use super::*;

/// Async resolver for provider credentials through router-owned state and secret stores.
#[derive(Clone, Debug)]
pub struct AsyncRouterCredentialResolver<S, C>
where
    S: SecretStore + Clone,
    C: CredentialRefreshClient + Clone,
{
    state_store: AsyncSqliteStateStore,
    secret_store: S,
    refresh_client: C,
    fixed_now_unix_seconds: Option<u64>,
    refresh_leases: AsyncRefreshLeaseRegistry,
    refresh_tasks: CredentialRefreshTaskSupervisor,
    #[cfg(test)]
    file_lock_acquired_observer: Option<std::sync::mpsc::Sender<()>>,
}

/// Default async router credential resolver for OpenAI OAuth account tokens.
pub type DefaultAsyncRouterCredentialResolver<S> =
    AsyncRouterCredentialResolver<S, ProviderCredentialRefreshClients>;

impl<S> AsyncRouterCredentialResolver<S, ProviderCredentialRefreshClients>
where
    S: SecretStore + Clone + Send + Sync + 'static,
{
    /// Creates a default OpenAI OAuth async resolver with shared refresh leases.
    #[must_use]
    pub fn new_default_oauth_with_refresh_leases(
        state_store: AsyncSqliteStateStore,
        secret_store: S,
        fixed_now_unix_seconds: Option<u64>,
        refresh_leases: AsyncRefreshLeaseRegistry,
    ) -> Self {
        Self::new_with_refresh_leases(
            state_store,
            secret_store,
            ProviderCredentialRefreshClients::new(),
            fixed_now_unix_seconds,
            refresh_leases,
        )
    }
}

impl<S, C> AsyncRouterCredentialResolver<S, C>
where
    S: SecretStore + Clone + Send + Sync + 'static,
    C: CredentialRefreshClient + Clone + Send + Sync + 'static,
{
    /// Creates an async credential resolver.
    #[must_use]
    pub fn new(
        state_store: AsyncSqliteStateStore,
        secret_store: S,
        refresh_client: C,
        fixed_now_unix_seconds: Option<u64>,
    ) -> Self {
        Self {
            state_store,
            secret_store,
            refresh_client,
            fixed_now_unix_seconds,
            refresh_leases: AsyncRefreshLeaseRegistry::new(),
            refresh_tasks: CredentialRefreshTaskSupervisor::new(),
            #[cfg(test)]
            file_lock_acquired_observer: None,
        }
    }

    /// Creates an async credential resolver with shared refresh leases.
    #[must_use]
    pub fn new_with_refresh_leases(
        state_store: AsyncSqliteStateStore,
        secret_store: S,
        refresh_client: C,
        fixed_now_unix_seconds: Option<u64>,
        refresh_leases: AsyncRefreshLeaseRegistry,
    ) -> Self {
        Self {
            state_store,
            secret_store,
            refresh_client,
            fixed_now_unix_seconds,
            refresh_leases,
            refresh_tasks: CredentialRefreshTaskSupervisor::new(),
            #[cfg(test)]
            file_lock_acquired_observer: None,
        }
    }

    /// Shares normal-shutdown supervision across request-scoped resolvers.
    #[must_use]
    pub fn with_refresh_task_supervisor(
        mut self,
        refresh_tasks: CredentialRefreshTaskSupervisor,
    ) -> Self {
        self.refresh_tasks = refresh_tasks;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_test_file_lock_acquired_observer(
        mut self,
        observer: std::sync::mpsc::Sender<()>,
    ) -> Self {
        self.file_lock_acquired_observer = Some(observer);
        self
    }

    /// Resolves credentials immediately before provider egress.
    pub async fn resolve_provider_credentials(
        &self,
        account_id: &AccountId,
        expected_provider: Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        let (active_generation, bundle) = self
            .read_active_bundle(account_id, expected_provider)
            .await?;
        let now_unix_seconds = self.observed_now_unix_seconds()?;
        if self.bundle_is_expired(&bundle, now_unix_seconds) {
            let (resolved_generation, refreshed, _provider_used) = self
                .renew_credentials(
                    account_id,
                    expected_provider,
                    now_unix_seconds,
                    RenewalTrigger::ExpiredAccess,
                )
                .await?;
            return Ok(ResolvedProviderCredential::new(
                account_id.clone(),
                refreshed.access_token().clone(),
                resolved_generation,
            )
            .with_chatgpt_account_id(refreshed.chatgpt_account_id()));
        }

        Ok(ResolvedProviderCredential::new(
            account_id.clone(),
            bundle.access_token().clone(),
            active_generation,
        )
        .with_chatgpt_account_id(bundle.chatgpt_account_id()))
    }

    /// Proactively maintains one enabled account even without quota or client traffic.
    pub async fn maintain_account_credentials(
        &self,
        account_id: &AccountId,
    ) -> Result<(), CredentialResolverError> {
        let account = self
            .state_store
            .load_account(account_id)
            .await
            .map_err(map_state_error)?
            .ok_or(CredentialResolverError::AccountUnavailable)?;
        let now_unix_seconds = self.observed_now_unix_seconds()?;
        let (_generation, _bundle, _) = self
            .renew_credentials(
                account_id,
                account.provider(),
                now_unix_seconds,
                RenewalTrigger::Proactive,
            )
            .await?;
        Ok(())
    }

    /// Returns the next useful upkeep wake for this enabled account.
    pub async fn next_maintenance_due_unix_seconds(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<u64>, CredentialResolverError> {
        let now_unix_seconds = self.observed_now_unix_seconds()?;
        let account = self
            .state_store
            .load_account(account_id)
            .await
            .map_err(map_state_error)?
            .ok_or(CredentialResolverError::AccountUnavailable)?;
        if account.status() != AccountStatus::Enabled {
            return Err(CredentialResolverError::AccountIneligible);
        }
        let generation = account
            .active_credential_generation()
            .ok_or(CredentialResolverError::AccountIneligible)?;
        let maintenance = self
            .state_store
            .load_credential_maintenance(account_id)
            .await
            .map_err(map_state_error)?;
        let current_maintenance = maintenance
            .as_ref()
            .filter(|record| record.credential_generation == generation);
        if let Some(record) = current_maintenance {
            if matches!(
                record.state,
                CredentialMaintenanceState::ReauthRequired
                    | CredentialMaintenanceState::Unrefreshable
            ) {
                return Ok(None);
            }
            if let Some(deadline) = record.next_attempt_unix_seconds {
                return Ok(Some(deadline));
            }
        }
        let (_, bundle) = self
            .read_active_bundle(account_id, account.provider())
            .await?;
        Ok(Some(credential_renewal_due_at(
            &bundle,
            current_maintenance,
            now_unix_seconds,
        )))
    }

    /// Gives a rejected access generation one coordinated renewal opportunity.
    pub async fn recover_unauthorized_credentials(
        &self,
        account_id: &AccountId,
        expected_provider: Provider,
        rejected_generation: u64,
    ) -> Result<(ResolvedProviderCredential, bool), CredentialResolverError> {
        self.read_active_bundle(account_id, expected_provider)
            .await?;
        let now_unix_seconds = self.observed_now_unix_seconds()?;
        let (generation, bundle, provider_used) = self
            .renew_credentials(
                account_id,
                expected_provider,
                now_unix_seconds,
                RenewalTrigger::UnauthorizedGeneration(rejected_generation),
            )
            .await?;
        Ok((
            ResolvedProviderCredential::new(
                account_id.clone(),
                bundle.access_token().clone(),
                generation,
            )
            .with_chatgpt_account_id(bundle.chatgpt_account_id()),
            provider_used,
        ))
    }

    /// Records a provider-rejected access generation without attempting credential renewal.
    pub async fn mark_generation_reauth_required(
        &self,
        account_id: &AccountId,
        rejected_generation: u64,
    ) -> Result<bool, CredentialResolverError> {
        self.state_store
            .mark_generation_reauth_required(account_id, rejected_generation)
            .await
            .map_err(map_state_error)
    }

    fn observed_now_unix_seconds(&self) -> Result<u64, CredentialResolverError> {
        self.fixed_now_unix_seconds.map(Ok).unwrap_or_else(|| {
            current_unix_seconds().map_err(|_| CredentialResolverError::RefreshUnavailable)
        })
    }

    async fn renew_credentials(
        &self,
        account_id: &AccountId,
        expected_provider: Provider,
        now_unix_seconds: u64,
        trigger: RenewalTrigger,
    ) -> Result<(u64, CredentialBundle, bool), CredentialResolverError> {
        let mut current_trigger = trigger;
        for attempt in 0..2 {
            let owned_resolver = self.clone();
            let owned_account_id = account_id.clone();
            let owned_expected_provider = expected_provider;
            let task = self
                .refresh_tasks
                .spawn(|waiting_lock_cancellation| async move {
                    let lease = owned_resolver.refresh_leases.lease_for(&owned_account_id);
                    let _guard = tokio::select! {
                        biased;
                        () = waiting_lock_cancellation.cancelled() => {
                            return Err(CredentialResolverError::RenewalAdmissionClosed);
                        }
                        guard = lease.lock() => guard,
                    };
                    owned_resolver
                        .renew_bundle_under_lock(
                            &owned_account_id,
                            owned_expected_provider,
                            now_unix_seconds,
                            current_trigger,
                            waiting_lock_cancellation,
                        )
                        .await
                })?;
            let (generation, bundle, provider_used) = task
                .await
                .map_err(|_| CredentialResolverError::RefreshUnavailable)??;
            let completed_now_unix_seconds =
                self.observed_now_unix_seconds()?.max(now_unix_seconds);
            if !self.bundle_is_expired(&bundle, completed_now_unix_seconds) {
                return Ok((generation, bundle, provider_used));
            }
            if provider_used || attempt == 1 {
                return Err(CredentialResolverError::RefreshUnavailable);
            }
            current_trigger = RenewalTrigger::ExpiredAccess;
        }
        Err(CredentialResolverError::RefreshUnavailable)
    }

    async fn read_active_bundle(
        &self,
        account_id: &AccountId,
        expected_provider: Provider,
    ) -> Result<(u64, CredentialBundle), CredentialResolverError> {
        let account = self
            .state_store
            .load_account(account_id)
            .await
            .map_err(map_state_error)?
            .ok_or(CredentialResolverError::AccountUnavailable)?;
        if account.provider() != expected_provider {
            return Err(CredentialResolverError::AccountProviderMismatch);
        }
        if account.status() != AccountStatus::Enabled {
            return Err(CredentialResolverError::AccountIneligible);
        }
        let provider = account.provider();
        let active_generation = account
            .active_credential_generation()
            .ok_or(CredentialResolverError::AccountIneligible)?;
        let bundle_key = provider_credential_bundle_key(provider, account_id, active_generation)
            .map_err(map_secret_error)?;
        let secret_store = self.secret_store.clone();
        let bundle_join = tokio::task::spawn_blocking(move || {
            let secret = secret_store
                .read_secret(&bundle_key)
                .map_err(map_secret_error)?;
            CredentialBundle::from_secret_string(provider, secret).map_err(map_secret_error)
        })
        .await;
        let bundle_result = match bundle_join {
            Ok(result) => result,
            Err(_) => {
                if let Ok(now_unix_seconds) = self.observed_now_unix_seconds() {
                    self.record_local_failure(account_id, active_generation, now_unix_seconds)
                        .await;
                }
                return Err(CredentialResolverError::SecretUnavailable);
            }
        };
        let bundle = match bundle_result {
            Ok(bundle) => bundle,
            Err(error @ CredentialResolverError::CredentialStoreUnavailable) => {
                return Err(error);
            }
            Err(error) => {
                if let Ok(now_unix_seconds) = self.observed_now_unix_seconds() {
                    self.record_local_failure(account_id, active_generation, now_unix_seconds)
                        .await;
                }
                return Err(error);
            }
        };

        Ok((active_generation, bundle))
    }

    fn bundle_is_expired(&self, bundle: &CredentialBundle, now_unix_seconds: u64) -> bool {
        bundle
            .expires_unix_seconds()
            .is_some_and(|expires| expires <= now_unix_seconds)
    }

    async fn record_local_failure(
        &self,
        account_id: &AccountId,
        current_generation: u64,
        now_unix_seconds: u64,
    ) {
        if self
            .state_store
            .record_pre_provider_local_failure(account_id, current_generation, now_unix_seconds)
            .await
            .is_err()
        {
            tracing::warn!("credential local failure health could not be persisted");
        }
    }

    async fn prune_obsolete_generations(
        &self,
        account_id: &AccountId,
        provider: Provider,
        active_generation: u64,
    ) -> Result<(), CredentialResolverError> {
        let secret_store = self.secret_store.clone();
        let account_id = account_id.clone();
        tokio::task::spawn_blocking(move || {
            secret_store.prune_obsolete_generations(provider, &account_id, active_generation)
        })
        .await
        .map_err(|_| CredentialResolverError::SecretUnavailable)?
        .map(|_| ())
        .map_err(map_secret_error)
    }

    async fn renew_bundle_under_lock(
        &self,
        account_id: &AccountId,
        expected_provider: Provider,
        now_unix_seconds: u64,
        trigger: RenewalTrigger,
        waiting_lock_cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<(u64, CredentialBundle, bool), CredentialResolverError> {
        let database_path = self.state_store.database_path().to_path_buf();
        let account_for_lock = account_id.clone();
        #[cfg(test)]
        let file_lock_acquired_observer = self.file_lock_acquired_observer.clone();
        let file_lock_acquisition = tokio::task::spawn_blocking(move || {
            // This closure only acquires the existing file lock. If cancellation
            // wins while it blocks, its eventual owned guard is dropped with the
            // detached result and cannot reach claim or provider work.
            let result = AccountCredentialLock::acquire(&database_path, &account_for_lock);
            #[cfg(test)]
            if result.is_ok()
                && let Some(observer) = file_lock_acquired_observer
            {
                let _ = observer.send(());
            }
            result
        });
        let file_lock_result = tokio::select! {
            biased;
            () = waiting_lock_cancellation.cancelled() => {
                return Err(CredentialResolverError::RenewalAdmissionClosed);
            }
            result = file_lock_acquisition => result,
        };
        let mut file_lock = match file_lock_result {
            Ok(Ok(file_lock)) => file_lock,
            Ok(Err(_)) | Err(_) => {
                if let Ok(Some(account)) = self.state_store.load_account(account_id).await
                    && let Some(generation) = account.active_credential_generation()
                {
                    self.record_local_failure(account_id, generation, now_unix_seconds)
                        .await;
                }
                return Err(CredentialResolverError::RefreshUnavailable);
            }
        };
        let (current_generation, bundle) = self
            .read_active_bundle(account_id, expected_provider)
            .await?;
        let provider = bundle.provider();
        let now_unix_seconds = self.observed_now_unix_seconds()?.max(now_unix_seconds);
        if let RenewalTrigger::UnauthorizedGeneration(rejected_generation) = trigger
            && current_generation > rejected_generation
        {
            return Ok((current_generation, bundle, false));
        }
        let mut maintenance = match self
            .state_store
            .load_credential_maintenance(account_id)
            .await
        {
            Ok(maintenance) => maintenance,
            Err(error) => {
                self.record_local_failure(account_id, current_generation, now_unix_seconds)
                    .await;
                return Err(map_state_error(error));
            }
        };
        let active_claim = maintenance
            .as_ref()
            .filter(|record| {
                record.credential_generation == current_generation
                    && record.state == CredentialMaintenanceState::InProgress
            })
            .cloned();
        if let Some(record) = active_claim {
            let purpose = record
                .claim_purpose
                .ok_or(CredentialResolverError::RefreshUnavailable)?;
            let successor = record
                .claimed_successor_generation
                .ok_or(CredentialResolverError::RefreshUnavailable)?;
            let claimed_key = provider_credential_bundle_key(provider, account_id, successor)
                .map_err(map_secret_error)?;
            let secret_store = self.secret_store.clone();
            let claimed_secret =
                tokio::task::spawn_blocking(move || secret_store.read_secret(&claimed_key))
                    .await
                    .map_err(|_| CredentialResolverError::SecretUnavailable)?;
            match (purpose, claimed_secret) {
                (ClaimPurpose::Refresh | ClaimPurpose::Login, Ok(secret)) => {
                    let staged = CredentialBundle::from_secret_string(provider, secret)
                        .map_err(map_secret_error)?;
                    let activated = self
                        .state_store
                        .activate_claimed_credential_generation(
                            account_id,
                            provider,
                            purpose,
                            current_generation,
                            successor,
                            now_unix_seconds,
                        )
                        .await
                        .map_err(map_state_error)?;
                    if activated {
                        if self
                            .prune_obsolete_generations(account_id, provider, current_generation)
                            .await
                            .is_err()
                        {
                            tracing::warn!(
                                "obsolete credential generations could not be pruned after recovered activation"
                            );
                        }
                        return Ok((successor, staged, false));
                    }
                    return Err(CredentialResolverError::RefreshUnavailable);
                }
                (ClaimPurpose::Refresh, Err(error)) if secret_is_missing(&error) => {
                    self.state_store
                        .finish_credential_refresh_claim(
                            account_id,
                            provider,
                            current_generation,
                            successor,
                            CredentialRefreshClaimDisposition::ReauthRequired {
                                failure_class: CredentialFailureClass::RotationCommitFailed,
                            },
                        )
                        .await
                        .map_err(map_state_error)?;
                    return Err(CredentialResolverError::RefreshUnavailable);
                }
                (ClaimPurpose::Login, Err(error)) if secret_is_missing(&error) => {
                    let released = self
                        .state_store
                        .release_stale_login_credential_claim(
                            account_id,
                            provider,
                            now_unix_seconds,
                            LOGIN_CREDENTIAL_CLAIM_TIMEOUT_SECONDS,
                        )
                        .await
                        .map_err(map_state_error)?;
                    if !released {
                        return Err(CredentialResolverError::RefreshUnavailable);
                    }
                    maintenance = self
                        .state_store
                        .load_credential_maintenance(account_id)
                        .await
                        .map_err(map_state_error)?;
                }
                (_, Err(error)) => return Err(map_secret_error(error)),
            }
        }
        if let Some(record) = maintenance
            .as_ref()
            .filter(|record| record.credential_generation == current_generation)
            && (matches!(
                record.state,
                CredentialMaintenanceState::ReauthRequired
                    | CredentialMaintenanceState::Unrefreshable
            ) || record
                .next_attempt_unix_seconds
                .is_some_and(|deadline| deadline > now_unix_seconds))
        {
            return Err(CredentialResolverError::RefreshUnavailable);
        }
        let current_maintenance = maintenance
            .as_ref()
            .filter(|record| record.credential_generation == current_generation);
        let elapsed_retry_is_due = current_maintenance.is_some_and(|record| {
            record.state == CredentialMaintenanceState::Retrying
                && record
                    .next_attempt_unix_seconds
                    .is_some_and(|deadline| deadline <= now_unix_seconds)
        });
        let renewal_due = match trigger {
            RenewalTrigger::ExpiredAccess => self.bundle_is_expired(&bundle, now_unix_seconds),
            RenewalTrigger::Proactive => {
                elapsed_retry_is_due
                    || credential_renewal_is_due(&bundle, current_maintenance, now_unix_seconds)
            }
            RenewalTrigger::UnauthorizedGeneration(_) => true,
        };
        if !renewal_due {
            return Ok((current_generation, bundle, false));
        }
        let Some(refresh_token) = bundle.refresh_token().cloned() else {
            self.state_store
                .mark_credential_unrefreshable(account_id, provider, current_generation)
                .await
                .map_err(map_state_error)?;
            return Err(CredentialResolverError::RefreshUnavailable);
        };
        let secret_store = self.secret_store.clone();
        let account_for_slot = account_id.clone();
        let successor_result = tokio::task::spawn_blocking(move || {
            first_unused_account_credential_generation(
                &secret_store,
                provider,
                &account_for_slot,
                current_generation,
            )
        })
        .await;
        let successor_generation = match successor_result {
            Ok(Ok(generation)) => generation,
            Ok(Err(error)) => {
                self.record_local_failure(account_id, current_generation, now_unix_seconds)
                    .await;
                return Err(map_secret_error(error));
            }
            Err(_) => {
                self.record_local_failure(account_id, current_generation, now_unix_seconds)
                    .await;
                return Err(CredentialResolverError::SecretUnavailable);
            }
        };
        let claim_result = self
            .state_store
            .claim_credential_refresh(
                account_id,
                provider,
                ClaimPurpose::Refresh,
                current_generation,
                successor_generation,
                now_unix_seconds,
            )
            .await;
        let claimed = match claim_result {
            Ok(claimed) => claimed,
            Err(error) => {
                self.record_local_failure(account_id, current_generation, now_unix_seconds)
                    .await;
                return Err(map_state_error(error));
            }
        };
        if !claimed {
            return Err(CredentialResolverError::RefreshUnavailable);
        }
        let refresh_client = self.refresh_client.clone();
        let account_id_for_refresh = account_id.clone();
        let (returned_lock, provider_result) = tokio::task::spawn_blocking(move || {
            let result = refresh_client.refresh_provider_credentials(
                provider,
                &account_id_for_refresh,
                &refresh_token,
            );
            (file_lock, result)
        })
        .await
        .map_err(|_| CredentialResolverError::RefreshUnavailable)?;
        file_lock = returned_lock;
        let mut refreshed = match provider_result {
            Ok(refreshed) => refreshed,
            Err(failure) => {
                let previous_failures = maintenance
                    .as_ref()
                    .filter(|record| record.credential_generation == current_generation)
                    .map_or(0, |record| record.consecutive_failures);
                let bounded_backoff = 60_u64
                    .saturating_mul(1_u64 << previous_failures.min(5))
                    .min(30 * 60);
                let retry_deadline = failure.confirmed_unspent.then(|| {
                    now_unix_seconds.saturating_add(
                        failure
                            .retry_after_seconds
                            .unwrap_or(0)
                            .max(bounded_backoff),
                    )
                });
                let disposition_started = std::time::Instant::now();
                loop {
                    match self
                        .state_store
                        .finish_credential_refresh_claim(
                            account_id,
                            provider,
                            current_generation,
                            successor_generation,
                            if let Some(next_attempt_unix_seconds) = retry_deadline {
                                CredentialRefreshClaimDisposition::Retrying {
                                    failure_class: failure.failure_class,
                                    next_attempt_unix_seconds,
                                }
                            } else {
                                CredentialRefreshClaimDisposition::ReauthRequired {
                                    failure_class: failure.failure_class,
                                }
                            },
                        )
                        .await
                    {
                        Ok(true) => break,
                        Ok(false) => return Err(CredentialResolverError::RefreshUnavailable),
                        Err(_) if disposition_started.elapsed() < Duration::from_secs(30) => {
                            tokio::time::sleep(Duration::from_millis(250)).await;
                        }
                        Err(_) => return Err(CredentialResolverError::RefreshUnavailable),
                    }
                }
                return Err(CredentialResolverError::RefreshUnavailable);
            }
        };
        if refreshed.chatgpt_account_id().is_none()
            && let Some(chatgpt_account_id) = bundle.chatgpt_account_id()
        {
            refreshed = refreshed.with_openai_chatgpt_account_id(chatgpt_account_id.to_owned());
        }
        let refreshed_key =
            provider_credential_bundle_key(provider, account_id, successor_generation)
                .map_err(map_secret_error)?;
        let refreshed_secret = refreshed.to_secret_string().map_err(map_secret_error)?;
        let commit_started = std::time::Instant::now();
        let mut secret_saved = false;
        loop {
            if !secret_saved {
                let secret_store = self.secret_store.clone();
                let key_for_write = refreshed_key.clone();
                let secret_for_write = refreshed_secret.clone();
                let expected_bundle = refreshed.clone();
                let (returned_lock, write_result) = tokio::task::spawn_blocking(move || {
                    let result = secret_store
                        .write_staged(&key_for_write, &secret_for_write)
                        .and_then(|()| secret_store.read_secret(&key_for_write))
                        .and_then(|stored_secret| {
                            CredentialBundle::from_secret_string(provider, stored_secret)
                        })
                        .and_then(|stored_bundle| {
                            if stored_bundle == expected_bundle {
                                Ok(())
                            } else {
                                Err(SecretStoreError::InvalidSecretPayload {
                                    message: "staged credential verification failed".to_owned(),
                                })
                            }
                        });
                    (file_lock, result)
                })
                .await
                .map_err(|_| CredentialResolverError::SecretUnavailable)?;
                file_lock = returned_lock;
                secret_saved = write_result.is_ok();
            }
            if secret_saved {
                match self
                    .state_store
                    .activate_claimed_credential_generation(
                        account_id,
                        provider,
                        ClaimPurpose::Refresh,
                        current_generation,
                        successor_generation,
                        now_unix_seconds,
                    )
                    .await
                {
                    Ok(true) => break,
                    Ok(false) => return Err(CredentialResolverError::RefreshUnavailable),
                    Err(_) => {}
                }
            }
            if commit_started.elapsed() >= Duration::from_secs(30) {
                return Err(CredentialResolverError::RefreshUnavailable);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        if self
            .prune_obsolete_generations(account_id, provider, current_generation)
            .await
            .is_err()
        {
            tracing::warn!("obsolete credential generations could not be pruned after refresh");
        }
        drop(file_lock);

        Ok((successor_generation, refreshed, true))
    }
}

fn secret_is_missing(error: &SecretStoreError) -> bool {
    matches!(error, SecretStoreError::Filesystem { source, .. } if source.kind() == std::io::ErrorKind::NotFound)
}

pub(crate) fn credential_renewal_is_due(
    bundle: &CredentialBundle,
    maintenance: Option<&CredentialMaintenanceRecord>,
    now_unix_seconds: u64,
) -> bool {
    now_unix_seconds >= credential_renewal_due_at(bundle, maintenance, now_unix_seconds)
}

fn credential_renewal_due_at(
    bundle: &CredentialBundle,
    maintenance: Option<&CredentialMaintenanceRecord>,
    now_unix_seconds: u64,
) -> u64 {
    let Some(last_success) = maintenance.and_then(|record| record.last_success_unix_seconds) else {
        return now_unix_seconds;
    };
    let age_deadline = last_success.saturating_add(4 * 60 * 60);
    let Some(expires_unix_seconds) = bundle.expires_unix_seconds() else {
        return age_deadline;
    };
    let known_lifetime = expires_unix_seconds.saturating_sub(last_success);
    let renewal_lead = if known_lifetime <= 30 * 60 {
        known_lifetime / 2
    } else {
        30 * 60
    };
    age_deadline.min(expires_unix_seconds.saturating_sub(renewal_lead))
}
