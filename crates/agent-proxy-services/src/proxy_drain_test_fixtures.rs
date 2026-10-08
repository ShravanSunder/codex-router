use crate::proxy_role_test_fixtures::*;
use crate::*;
use codex_router_auth::resolver::{
    CredentialRefreshClient, CredentialRefreshFailure, NoopCredentialRefreshClient,
};
use codex_router_core::{ids::AccountId, provider::Provider, redaction::SecretString};
use codex_router_keeper_protocol::PrepareMode;
use codex_router_secret_store::{
    SecretStore,
    account_tokens::{AccountCredentialBundle, provider_credential_bundle_key},
    credential_bundle::CredentialBundle,
};
use codex_router_state::{
    account::{AccountRecord, AccountStatus},
    sqlite::AsyncSqliteStateStore,
};
use std::{
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

#[derive(Clone, Copy)]
pub(super) enum HeldProducer {
    Upkeep,
    Quota,
}
#[derive(Clone)]
struct HeldProvider {
    entered: tokio::sync::mpsc::UnboundedSender<Provider>,
    release: Arc<Mutex<mpsc::Receiver<()>>>,
}
impl HeldProvider {
    fn await_release(&self, provider: Provider) {
        self.entered
            .send(provider)
            .expect("readiness registered before entry");
        self.release
            .lock()
            .expect("external provider gate")
            .recv_timeout(Duration::from_secs(90))
            .expect("provider fixture released");
    }
}
impl CredentialRefreshClient for HeldProvider {
    fn refresh_credentials(
        &self,
        _account: &AccountId,
        _token: &SecretString,
    ) -> Result<AccountCredentialBundle, CredentialRefreshFailure> {
        self.await_release(Provider::Openai);
        Ok(AccountCredentialBundle::imported_codex_auth(
            "drain-successor-access",
            Some("drain-successor-refresh".to_owned()),
        )
        .with_expires_unix_seconds(
            codex_router_auth::resolver::current_unix_seconds().expect("fixture UTC clock") + 3_600,
        ))
    }
    fn refresh_provider_credentials(
        &self,
        provider: Provider,
        account: &AccountId,
        token: &SecretString,
    ) -> Result<CredentialBundle, CredentialRefreshFailure> {
        match provider {
            Provider::Openai => self
                .refresh_credentials(account, token)
                .map(CredentialBundle::OpenAi),
            Provider::Claude => {
                self.await_release(provider);
                CredentialBundle::new_claude(SecretString::new("drain-claude-successor-access"),SecretString::new("drain-claude-successor-refresh"),codex_router_auth::resolver::current_unix_seconds().expect("fixture UTC clock")+3_600)
                    .map_err(|_|CredentialRefreshFailure::ambiguous(codex_router_state::credential_maintenance::CredentialFailureClass::MalformedResponse))
            }
        }
    }
}
pub(super) struct ControlledQuotaEndpoint;
impl quota::QuotaRefreshProvider for ControlledQuotaEndpoint {
    async fn fetch_quota(
        &self,
        _request: quota::QuotaRefreshProviderRequest,
    ) -> Result<quota::QuotaRefreshProviderResponse, quota::QuotaRefreshError> {
        // External quota egress only: actual resolver and state writes stay real.
        Err(quota::QuotaRefreshError::ProviderStatus { status: 503 })
    }
}
pub(super) struct HeldRoleFixture {
    pub root: tempfile::TempDir,
    pub state: AsyncSqliteStateStore,
    pub account: AccountId,
    pub role: ProxyRoleRuntime,
    pub release: mpsc::Sender<()>,
}
pub(super) async fn held_role(provider: Provider, producer: HeldProducer) -> HeldRoleFixture {
    held_role_at(
        provider,
        producer,
        usize::MAX,
        "http://127.0.0.1:1/v1".to_owned(),
    )
    .await
}
pub(super) async fn held_role_at(
    provider: Provider,
    producer: HeldProducer,
    limit: usize,
    upstream: String,
) -> HeldRoleFixture {
    let root = tempfile::tempdir().expect("isolated drain root");
    drop(
        prepare(root.path(), PrepareMode::Fresh)
            .await
            .expect("secret prerequisites"),
    );
    let state = AsyncSqliteStateStore::open(&root.path().join("state.sqlite"))
        .await
        .expect("real DB owner");
    let account = AccountId::new("drain-held-account").expect("literal account");
    state
        .upsert_account(
            &AccountRecord::new(
                provider,
                account.clone(),
                "drain held",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        )
        .await
        .expect("real generation one account");
    let secrets = codex_router_secret_store::test_support::open_encrypted_credential_store(
        root.path().join("secrets"),
    )
    .expect("external Keychain fixture");
    let bundle = match provider {
        Provider::Openai => CredentialBundle::OpenAi(
            AccountCredentialBundle::imported_codex_auth(
                "drain-expired-access",
                Some("drain-refresh".to_owned()),
            )
            .with_expires_unix_seconds(900),
        ),
        Provider::Claude => CredentialBundle::new_claude(
            SecretString::new("drain-claude-access"),
            SecretString::new("drain-claude-refresh"),
            900,
        )
        .expect("literal Claude bundle"),
    };
    secrets
        .write_secret(
            &provider_credential_bundle_key(provider, &account, 1).expect("provider key"),
            &bundle.to_secret_string().expect("valid bundle"),
        )
        .expect("real expired credentials");
    if matches!(producer, HeldProducer::Quota) {
        // The real proactive owner honors this existing retry deadline at its fixture clock 0.
        // Quota's actual clock is later and therefore admits the actual held rotation.
        assert!(
            state
                .record_pre_provider_local_failure(&account, 1, 0)
                .await
                .expect("real cooldown history")
        );
    }
    let (entered, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let (release, held) = mpsc::channel();
    let client = HeldProvider {
        entered,
        release: Arc::new(Mutex::new(held)),
    };
    let listener = held_listener().await;
    let mut config = role_config(root.path(), listener.tcp_address().expect("real address"));
    config.max_connections = limit;
    config.core = codex_router_proxy::server::LoopbackRouterRuntimeConfig::new_tokenless(
        config.core.bind_address(),
        codex_router_proxy::upstream::UpstreamEndpoint::new(upstream).expect("isolated upstream"),
        root.path().join("state.sqlite"),
        root.path().join("secrets"),
    )
    .with_quota_clock(1_000, 60);
    config.quota_refresh = match producer {
        HeldProducer::Upkeep => ProxyQuotaRefreshPolicy::Disabled,
        HeldProducer::Quota => ProxyQuotaRefreshPolicy::Enabled,
    };
    let prepared = ProxyRoleRuntime::prepare(
        config,
        PrepareMode::Fresh,
        listener,
        codex_router_descriptor_boundary::DescriptorGate::global(),
    )
    .await
    .expect("real prepared role");
    let upkeep_client = client.clone();
    let mut role=prepared.activate_with_worker_starts(
        move |path,credentials,supervisor| async move {
            match producer {
                HeldProducer::Upkeep=>credential_upkeep_worker::start_background_credential_upkeep_worker_with_client_and_clock(path,credentials,supervisor,upkeep_client,||1_000).await,
                HeldProducer::Quota=>credential_upkeep_worker::start_background_credential_upkeep_worker_with_client_and_clock(path,credentials,supervisor,NoopCredentialRefreshClient,||0).await,
            }
        },
        move |path,secret_root,credentials,base_url,interval,_notifier,supervisor| async move {
            let resolver=credential_runtime::AsyncCliCredentialResolver::open_with_refresh_client(&path,credentials,client,supervisor).await?;
            Ok(quota::start_background_quota_refresh_worker_with_reporter(path,secret_root,base_url,resolver,ControlledQuotaEndpoint,
                quota::BackgroundQuotaRefreshRuntime::new(||1_000,|_diagnostic|{},interval)).await)
        },|_|{},|_|Ok(()),
    ).await.expect("real workers/serving owners");
    let entry = tokio::time::timeout(Duration::from_secs(2), observed.recv()).await;
    if entry.is_err() {
        eprintln!(
            "fixture entry failure provider={provider:?} maintenance={:?}",
            state.load_credential_maintenance(&account).await
        );
        role.shutdown()
            .await
            .expect("same acquired owners joined on fixture setup refusal");
    }
    assert_eq!(entry.expect("bounded provider entry"), Some(provider));
    HeldRoleFixture {
        root,
        state,
        account,
        role,
        release,
    }
}
