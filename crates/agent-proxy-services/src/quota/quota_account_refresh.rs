use super::*;
use codex_router_auth::resolver::ResolvedProviderCredential;
pub(super) struct AccountQuotaRefresh<'a, R, P, W> {
    pub(super) stdout: &'a mut W,
    pub(super) quota_history_state: &'a AsyncSqliteStateStore,
    pub(super) account: &'a AccountRecord,
    pub(super) base_url: &'a str,
    pub(super) credential_resolver: &'a R,
    pub(super) quota_provider: &'a P,
    pub(super) observed_unix_seconds: u64,
    pub(super) refresh_interval_seconds: u64,
    pub(super) weekly_quota_floor: Option<u32>,
    pub(super) weekly_floor_observer: Option<&'a dyn WeeklyQuotaFloorIntentObserver>,
    pub(super) resolved: &'a mut ResolvedProviderCredential,
    pub(super) refreshed_count: &'a mut u64,
    pub(super) failed_count: &'a mut u64,
    pub(super) committed_responses_generations: &'a mut HashMap<AccountId, u64>,
}
