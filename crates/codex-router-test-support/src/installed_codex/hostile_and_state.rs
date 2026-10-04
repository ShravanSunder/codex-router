/// Runs a hostile local no-token smoke and verifies upstream remains untouched.
pub fn run_hostile_no_token_smoke() -> Result<(), String> {
    let smoke_root = SmokeTempRoot::new("hostile-no-token")?;
    let router_root = smoke_root.path().join("router");
    let state_path = router_root.join("state.sqlite");
    let secret_root = router_root.join("secrets");
    fs::create_dir_all(&router_root).map_err(|error| {
        format!(
            "failed to create hostile smoke router root {}: {error}",
            router_root.display()
        )
    })?;

    let upstream = MockNoConnectionUpstream::start(Duration::from_secs(3))?;
    let seed = seed_router_state(&state_path, &secret_root)?;
    let router_port = reserve_loopback_port()?;
    let audit_path = router_root.join("audit").join("events.jsonl");
    let router_process = start_router_process(
        router_port,
        state_path,
        secret_root,
        Some(seed.local_token),
        format!("http://{}/v1", upstream.address()),
        audit_path,
    )?;

    send_hostile_no_token_websocket(router_port)?;
    let _router_process = router_process.stop("hostile no-token router process")?;
    let upstream_connection_count = upstream.join()?;
    if upstream_connection_count != 0 {
        return Err(format!(
            "hostile no-token smoke reached upstream {upstream_connection_count} time(s)"
        ));
    }

    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SmokeQuotaStatus {
    table: String,
    plain: String,
    json: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SmokeSeed {
    local_token_assignment: String,
    local_token: String,
    expected_account_label: String,
    expected_account_tag: String,
    expected_upstream_token: String,
    routable_upstream_tokens: Vec<String>,
    quota_status: SmokeQuotaStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SmokeAccountFixture {
    account_id: &'static str,
    label: &'static str,
    upstream_token: &'static str,
    short_remaining: u32,
    short_reset: u64,
    weekly_remaining: u32,
    weekly_reset: u64,
    weekly_status: SelectorQuotaWindowStatus,
}

const SMOKE_ACCOUNT_FIXTURES: &[SmokeAccountFixture] = &[
    SmokeAccountFixture {
        account_id: "acct_askluna",
        label: "askluna",
        upstream_token: "installed-smoke-askluna-token",
        short_remaining: 100,
        short_reset: 17_900,
        weekly_remaining: 0,
        weekly_reset: 130_600,
        weekly_status: SelectorQuotaWindowStatus::Ineligible,
    },
    SmokeAccountFixture {
        account_id: "acct_matches",
        label: "matches",
        upstream_token: "installed-smoke-matches-token",
        short_remaining: 91,
        short_reset: 16_000,
        weekly_remaining: 54,
        weekly_reset: 525_000,
        weekly_status: SelectorQuotaWindowStatus::Eligible,
    },
    SmokeAccountFixture {
        account_id: "acct_ssdev",
        label: "ssdev",
        upstream_token: "installed-smoke-ssdev-token",
        short_remaining: 100,
        short_reset: 15_000,
        weekly_remaining: 16,
        weekly_reset: 120_000,
        weekly_status: SelectorQuotaWindowStatus::Eligible,
    },
];

const QUOTA_RECONNECT_PRIMARY: SmokeAccountFixture = SmokeAccountFixture {
    account_id: "acct_quota_primary",
    label: "quota-primary",
    upstream_token: "installed-quota-primary-token",
    short_remaining: 100,
    short_reset: 17_900,
    weekly_remaining: 100,
    weekly_reset: 525_000,
    weekly_status: SelectorQuotaWindowStatus::Eligible,
};

const QUOTA_RECONNECT_FALLBACK: SmokeAccountFixture = SmokeAccountFixture {
    account_id: "acct_quota_fallback",
    label: "quota-fallback",
    upstream_token: "installed-quota-fallback-token",
    short_remaining: 80,
    short_reset: 17_900,
    weekly_remaining: 80,
    weekly_reset: 525_000,
    weekly_status: SelectorQuotaWindowStatus::Eligible,
};

// Far-idle ordering selects the earliest reset before remaining headroom.
// The single-client reconnect and floor journeys require primary first.
const QUOTA_RECONNECT_PRIMARY_FOR_INITIAL_ADMISSION: SmokeAccountFixture = SmokeAccountFixture {
    weekly_reset: 500_000,
    ..QUOTA_RECONNECT_PRIMARY
};

const SMOKE_SELECTOR_STALE_AFTER_SECONDS: u64 = 300;

fn seed_router_state(state_path: &Path, secret_root: &Path) -> Result<SmokeSeed, String> {
    let state = SqliteStateStore::open(state_path)
        .map_err(|error| format!("failed to open smoke SQLite state: {error}"))?;
    let secrets =
        codex_router_secret_store::test_support::open_encrypted_credential_store(secret_root)
            .map_err(|error| format!("failed to open smoke secret store: {error}"))?;
    let token_service = LocalRouterTokenService::new(secrets.clone());
    let local_token = token_service
        .rotate()
        .map_err(|error| format!("failed to rotate smoke local token: {error}"))?;
    let local_token_assignment = export_token_assignment(
        "CODEX_ROUTER_TOKEN",
        local_token.token().expose_secret(),
        Shell::Posix,
    );
    let exported_token = parse_posix_token_assignment(&local_token_assignment)?;

    disable_accounts_outside_fixtures(&state, SMOKE_ACCOUNT_FIXTURES, "three-websocket")?;
    reset_fixture_route_band_state(state_path, SMOKE_ACCOUNT_FIXTURES, "three-websocket")?;
    for fixture in SMOKE_ACCOUNT_FIXTURES {
        seed_smoke_account(&state, &secrets, *fixture)?;
    }

    let quota_status = capture_quota_status(state_path)?;
    let selected_account = selected_account_from_status_json(&quota_status.json)?;
    let selected_fixture = SMOKE_ACCOUNT_FIXTURES
        .iter()
        .find(|fixture| fixture.label == selected_account.safe_label)
        .ok_or_else(|| {
            format!(
                "quota status selected unknown smoke account: {}",
                selected_account.safe_label
            )
        })?;

    Ok(SmokeSeed {
        local_token_assignment,
        local_token: exported_token,
        expected_account_label: selected_fixture.label.to_owned(),
        expected_account_tag: selected_account.account_hash,
        expected_upstream_token: selected_fixture.upstream_token.to_owned(),
        routable_upstream_tokens: SMOKE_ACCOUNT_FIXTURES
            .iter()
            .filter(|fixture| fixture.weekly_status == SelectorQuotaWindowStatus::Eligible)
            .map(|fixture| fixture.upstream_token.to_owned())
            .collect(),
        quota_status,
    })
}

fn seed_quota_reconnect_router_state(
    state_path: &Path,
    secret_root: &Path,
    primary_fixture: SmokeAccountFixture,
) -> Result<(), String> {
    let state = SqliteStateStore::open(state_path)
        .map_err(|error| format!("failed to open quota reconnect SQLite state: {error}"))?;
    let secrets =
        codex_router_secret_store::test_support::open_encrypted_credential_store(secret_root)
            .map_err(|error| format!("failed to open quota reconnect secret store: {error}"))?;
    disable_accounts_outside_fixtures(
        &state,
        &[primary_fixture, QUOTA_RECONNECT_FALLBACK],
        "quota reconnect",
    )?;
    reset_fixture_route_band_state(
        state_path,
        &[primary_fixture, QUOTA_RECONNECT_FALLBACK],
        "quota reconnect",
    )?;
    seed_smoke_account(&state, &secrets, primary_fixture)?;
    seed_smoke_account(&state, &secrets, QUOTA_RECONNECT_FALLBACK)?;
    Ok(())
}

fn seed_s8_overlap_quota_router_state(
    state_path: &Path,
    secret_root: &Path,
) -> Result<SmokeSeed, String> {
    seed_quota_reconnect_router_state(state_path, secret_root, QUOTA_RECONNECT_PRIMARY)?;
    let secrets =
        codex_router_secret_store::test_support::open_encrypted_credential_store(secret_root)
            .map_err(|error| format!("failed to open S8 overlap quota secret store: {error}"))?;
    let token_service = LocalRouterTokenService::new(secrets);
    let local_token = token_service
        .rotate()
        .map_err(|error| format!("failed to rotate S8 overlap quota local token: {error}"))?;
    let local_token_assignment = export_token_assignment(
        "CODEX_ROUTER_TOKEN",
        local_token.token().expose_secret(),
        Shell::Posix,
    );
    let exported_token = parse_posix_token_assignment(&local_token_assignment)?;
    let quota_status = capture_quota_status(state_path)?;
    let selected_account = selected_account_from_status_json(&quota_status.json)?;

    Ok(SmokeSeed {
        local_token_assignment,
        local_token: exported_token,
        expected_account_label: selected_account.safe_label,
        expected_account_tag: selected_account.account_hash,
        expected_upstream_token: QUOTA_RECONNECT_PRIMARY.upstream_token.to_owned(),
        routable_upstream_tokens: vec![
            QUOTA_RECONNECT_PRIMARY.upstream_token.to_owned(),
            QUOTA_RECONNECT_FALLBACK.upstream_token.to_owned(),
        ],
        quota_status,
    })
}

fn reset_fixture_route_band_state(
    state_path: &Path,
    fixtures: &[SmokeAccountFixture],
    scenario_label: &str,
) -> Result<(), String> {
    let mut command = Command::new("python3");
    command
        .arg("-c")
        .arg(
            r#"
import sqlite3
import sys

database_path = sys.argv[1]
account_ids = sys.argv[2:]
connection = sqlite3.connect(database_path, timeout=0)
try:
    for account_id in account_ids:
        connection.execute(
            "DELETE FROM route_band_account_states WHERE account_id = ? AND route_band = 'responses'",
            (account_id,),
        )
    connection.commit()
finally:
    connection.close()
"#,
        )
        .arg(state_path);
    for account_fixture in fixtures {
        command.arg(account_fixture.account_id);
    }
    let output = command
        .output()
        .map_err(|error| format!("failed to run {scenario_label} state reset helper: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to reset {scenario_label} route-band state in {}: status={} stderr={}",
            state_path.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

fn disable_accounts_outside_fixtures(
    state: &SqliteStateStore,
    allowed_fixtures: &[SmokeAccountFixture],
    scenario_label: &str,
) -> Result<(), String> {
    for account in AccountStateRepository::list_accounts(state)
        .map_err(|error| format!("failed to list accounts for {scenario_label} fixture: {error}"))?
    {
        if allowed_fixtures
            .iter()
            .any(|fixture| fixture.account_id == account.account_id().as_str())
        {
            continue;
        }
        let mut disabled_account = AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            account.account_id().clone(),
            account.label().to_owned(),
            AccountStatus::Disabled,
        );
        if let Some(active_generation) = account.active_credential_generation() {
            disabled_account =
                disabled_account.with_active_credential_generation(active_generation);
        }
        AccountStateRepository::upsert_account(state, &disabled_account).map_err(|error| {
            format!(
                "failed to disable non-{scenario_label} account {}: {error}",
                account.label()
            )
        })?;
    }
    Ok(())
}

fn seed_smoke_account(
    state: &SqliteStateStore,
    secrets: &EncryptedCredentialStore,
    fixture: SmokeAccountFixture,
) -> Result<(), String> {
    let account_id = account_id(fixture.account_id)?;
    let observed_unix_seconds = timestamp_seconds();
    let short_reset_unix_seconds = observed_unix_seconds.saturating_add(fixture.short_reset);
    let weekly_reset_unix_seconds = observed_unix_seconds.saturating_add(fixture.weekly_reset);
    let stale_after_unix_seconds =
        observed_unix_seconds.saturating_add(SMOKE_SELECTOR_STALE_AFTER_SECONDS);
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        fixture.label,
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    AccountStateRepository::upsert_account(state, &account)
        .map_err(|error| format!("failed to seed smoke account {}: {error}", fixture.label))?;
    let snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(observed_unix_seconds)
            .with_route_band("responses", fixture.short_remaining)
            .with_reset_unix_seconds(short_reset_unix_seconds);
    QuotaSnapshotRepository::upsert_snapshot(state, &snapshot).map_err(|error| {
        format!(
            "failed to seed smoke quota snapshot for {}: {error}",
            fixture.label
        )
    })?;
    let short_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        "responses",
        18_000,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(fixture.short_remaining)
    .with_reset_unix_seconds(short_reset_unix_seconds)
    .with_effective(true)
    .with_observed_unix_seconds(observed_unix_seconds);
    let weekly_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        "responses",
        604_800,
        fixture.weekly_status,
    )
    .with_remaining_headroom(fixture.weekly_remaining)
    .with_reset_unix_seconds(weekly_reset_unix_seconds)
    .with_observed_unix_seconds(observed_unix_seconds);
    SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
        state,
        &account_id,
        "responses",
        &[short_window, weekly_window],
        observed_unix_seconds,
        stale_after_unix_seconds,
    )
    .map_err(|error| {
        format!(
            "failed to seed fresh smoke selector windows for {}: {error}",
            fixture.label
        )
    })?;
    let credential_key = openai_account_credential_bundle_key(&account_id, 1)
        .map_err(|error| format!("failed to build account credential key: {error}"))?;
    let credential_bundle =
        AccountCredentialBundle::imported_codex_auth(fixture.upstream_token, None)
            .to_secret_string()
            .map_err(|error| format!("failed to serialize smoke credential bundle: {error}"))?;
    secrets
        .write_secret(&credential_key, &credential_bundle)
        .map_err(|error| format!("failed to write smoke credential bundle: {error}"))?;
    let legacy_token_key = upstream_access_token_key(&account_id)
        .map_err(|error| format!("failed to build upstream token key: {error}"))?;
    secrets
        .write_secret(
            &legacy_token_key,
            &SecretString::new(fixture.upstream_token.to_owned()),
        )
        .map_err(|error| format!("failed to write smoke upstream token: {error}"))?;

    Ok(())
}

fn capture_quota_status(state_path: &Path) -> Result<SmokeQuotaStatus, String> {
    let router_root = state_path
        .parent()
        .ok_or_else(|| "state path had no router root parent".to_owned())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("quota status runtime unavailable: {error}"))?;
    Ok(SmokeQuotaStatus {
        table: run_quota_status(&runtime, router_root, "table")?,
        plain: run_quota_status(&runtime, router_root, "plain")?,
        json: run_quota_status(&runtime, router_root, "json")?,
    })
}

fn run_quota_status(
    runtime: &tokio::runtime::Runtime,
    router_root: &Path,
    format: &str,
) -> Result<String, String> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    runtime
        .block_on(run_with_io_async(
            vec![
                OsString::from("codex-router"),
                OsString::from("quota"),
                OsString::from("status"),
                OsString::from("--router-root"),
                router_root.as_os_str().to_owned(),
                OsString::from("--no-refresh"),
                OsString::from("--format"),
                OsString::from(format),
                OsString::from("--now-unix-seconds"),
                OsString::from("1030"),
            ],
            &CliContext::new(Vec::new()),
            &mut stdout,
            &mut stderr,
        ))
        .map_err(|error| {
            format!(
                "quota status {format} failed: {error}; stderr={}",
                String::from_utf8_lossy(&stderr)
            )
        })?;
    if !stderr.is_empty() {
        return Err(format!(
            "quota status {format} wrote stderr: {}",
            String::from_utf8_lossy(&stderr)
        ));
    }
    String::from_utf8(stdout)
        .map_err(|error| format!("quota status {format} was not UTF-8: {error}"))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SelectedQuotaStatusAccount {
    safe_label: String,
    account_hash: String,
}

fn selected_account_from_status_json(payload: &str) -> Result<SelectedQuotaStatusAccount, String> {
    let value: Value = serde_json::from_str(payload)
        .map_err(|error| format!("quota status json was invalid: {error}"))?;
    value
        .get("accounts")
        .and_then(Value::as_array)
        .and_then(|accounts| {
            accounts.iter().find_map(|account| {
                if !account
                    .get("preferred_next")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    return None;
                }
                Some(SelectedQuotaStatusAccount {
                    safe_label: account
                        .get("safe_account_label")
                        .and_then(Value::as_str)?
                        .to_owned(),
                    account_hash: account
                        .get("account_hash")
                        .and_then(Value::as_str)?
                        .to_owned(),
                })
            })
        })
        .ok_or_else(|| "quota status json did not include preferred account label".to_owned())
}

fn smoke_account_label_from_upstream_token(token: &str) -> Option<&'static str> {
    SMOKE_ACCOUNT_FIXTURES
        .iter()
        .find(|fixture| fixture.upstream_token == token)
        .map(|fixture| fixture.label)
}
