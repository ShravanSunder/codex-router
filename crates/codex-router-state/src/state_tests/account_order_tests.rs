use super::*;

#[test]
fn state_repository_lists_accounts_in_selector_stable_order() {
    let temp_dir = TestTempDir::new("list_accounts");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("state store should open and migrate: {error}"),
    };
    let beta_id = account_id("acct_beta");
    let alpha_id = account_id("acct_alpha");
    let disabled_id = account_id("acct_disabled");
    let beta = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        beta_id,
        "beta",
        AccountStatus::Enabled,
    );
    let alpha = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        alpha_id,
        "alpha",
        AccountStatus::Enabled,
    );
    let disabled = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        disabled_id,
        "disabled",
        AccountStatus::Disabled,
    );

    if let Err(error) = AccountStateRepository::upsert_account(&store, &beta) {
        panic!("beta account should persist: {error}");
    }
    if let Err(error) = AccountStateRepository::upsert_account(&store, &alpha) {
        panic!("alpha account should persist: {error}");
    }
    if let Err(error) = AccountStateRepository::upsert_account(&store, &disabled) {
        panic!("disabled account should persist: {error}");
    }

    let accounts = match AccountStateRepository::list_accounts(&store) {
        Ok(accounts) => accounts,
        Err(error) => panic!("account repository should list accounts: {error}"),
    };

    assert_eq!(accounts, vec![alpha, beta, disabled]);
}
