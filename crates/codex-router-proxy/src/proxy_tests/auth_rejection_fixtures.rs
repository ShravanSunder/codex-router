use super::*;
use codex_router_state::credential_maintenance::{ClaimPurpose, CredentialMaintenanceRecord};

pub(super) struct AuthRejectionFixture {
    _temp_dir: ProxyTestTempDir,
    pub(super) database_path: PathBuf,
    pub(super) secret_path: PathBuf,
    pub(super) primary: AccountRecord,
    pub(super) fallback: AccountRecord,
}

impl AuthRejectionFixture {
    pub(super) fn new(label: &str) -> Self {
        let temp_dir = ProxyTestTempDir::new(label);
        let database_path = temp_dir.path().join("state.sqlite");
        let secret_path = temp_dir.path().join("secrets");
        let state = must_ok(SqliteStateStore::open(&database_path));
        let secrets = must_ok(
            codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_path),
        );
        let primary = AccountRecord::new(
            Provider::Openai,
            account_id("auth-primary"),
            "primary",
            AccountStatus::Enabled,
        );
        let fallback = AccountRecord::new(
            Provider::Openai,
            account_id("auth-fallback"),
            "fallback",
            AccountStatus::Enabled,
        );
        persist_account_with_snapshot_and_token(&state, &secrets, &primary, 90, "primary-token");
        persist_account_with_snapshot_and_token(&state, &secrets, &fallback, 80, "fallback-token");
        for (account, token, account_header) in [
            (&primary, "primary-token", "synthetic-openai-primary"),
            (&fallback, "fallback-token", "synthetic-openai-fallback"),
        ] {
            let bundle = AccountCredentialBundle::imported_codex_auth(token, None)
                .with_expires_unix_seconds(4_000_000_000)
                .with_chatgpt_account_id(account_header);
            must_ok(secrets.write_secret(
                &must_ok(openai_account_credential_bundle_key(
                    account.account_id(),
                    1,
                )),
                &must_ok(bundle.to_secret_string()),
            ));
        }
        Self {
            _temp_dir: temp_dir,
            database_path,
            secret_path,
            primary,
            fallback,
        }
    }

    pub(super) fn start(&self, upstream_address: std::net::SocketAddr) -> LoopbackRouterRuntime {
        let config = LoopbackRouterRuntimeConfig::new(
            must_ok(LoopbackBindAddress::new("127.0.0.1", 0)),
            must_ok(UpstreamEndpoint::new(format!(
                "http://{upstream_address}/v1"
            ))),
            self.database_path.clone(),
            self.secret_path.clone(),
            LocalRouterTokenRecord::new(
                SecretString::new("current-token"),
                TokenGeneration::new(1),
            ),
        )
        .with_quota_clock(1_030, 60);
        must_ok(LoopbackRouterRuntime::start_for_test(config))
    }

    pub(super) fn maintenance(&self) -> Option<CredentialMaintenanceRecord> {
        let runtime = must_ok(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build(),
        );
        runtime.block_on(async {
            let state = must_ok(AsyncSqliteStateStore::open(&self.database_path).await);
            let record = must_ok(
                state
                    .load_credential_maintenance(self.primary.account_id())
                    .await,
            );
            must_ok(state.close().await);
            record
        })
    }

    pub(super) fn credential_metadata(
        &self,
    ) -> Vec<(AccountRecord, Option<CredentialMaintenanceRecord>)> {
        let runtime = must_ok(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build(),
        );
        runtime.block_on(async {
            let state = must_ok(AsyncSqliteStateStore::open(&self.database_path).await);
            let mut metadata = Vec::new();
            for account in must_ok(state.list_accounts().await) {
                let maintenance = must_ok(
                    state
                        .load_credential_maintenance(account.account_id())
                        .await,
                );
                metadata.push((account, maintenance));
            }
            must_ok(state.close().await);
            metadata
        })
    }

    pub(super) fn claim_successor(&self) -> CredentialMaintenanceRecord {
        self.claim_account_successor(self.primary.account_id())
    }

    pub(super) fn claim_account_successor(
        &self,
        account_id: &AccountId,
    ) -> CredentialMaintenanceRecord {
        let runtime = must_ok(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build(),
        );
        runtime.block_on(async {
            let state = must_ok(AsyncSqliteStateStore::open(&self.database_path).await);
            assert!(must_ok(
                state
                    .claim_credential_refresh(
                        account_id,
                        Provider::Openai,
                        ClaimPurpose::Refresh,
                        1,
                        2,
                        1_000
                    )
                    .await
            ));
            let record = must_ok(state.load_credential_maintenance(account_id).await)
                .expect("claim should exist");
            must_ok(state.close().await);
            record
        })
    }

    pub(super) fn persist_hard_owner(&self, response_id: &str) {
        let state = must_ok(SqliteStateStore::open(&self.database_path));
        let secrets = must_ok(
            codex_router_secret_store::test_support::open_encrypted_credential_store(
                &self.secret_path,
            ),
        );
        let affinity_secret = must_ok(load_or_create_router_affinity_hash_secret(&secrets));
        must_ok(persist_previous_response_owner(
            &state,
            response_id,
            affinity_secret.secret(),
            self.primary.account_id(),
        ));
    }
}

pub(super) fn accept_upstream_before_deadline(listener: &TcpListener) -> Option<TcpStream> {
    let listener = listener
        .try_clone()
        .expect("upstream listener should clone");
    listener
        .set_nonblocking(true)
        .expect("async listener should be nonblocking");
    let runtime = must_ok(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build(),
    );
    runtime.block_on(async {
        let listener =
            tokio::net::TcpListener::from_std(listener).expect("async listener should register");
        let Ok(accepted) =
            tokio::time::timeout(Duration::from_millis(250), listener.accept()).await
        else {
            return None;
        };
        let (stream, _) = accepted.expect("upstream should accept");
        let stream = stream.into_std().expect("upstream stream should convert");
        stream
            .set_nonblocking(false)
            .expect("upstream read should be blocking");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("upstream read should be bounded");
        Some(stream)
    })
}

pub(super) fn authorization_from_request(request: &str) -> String {
    request
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("authorization")
                .then(|| value.trim().to_owned())
        })
        .expect("upstream must have authorization")
}

pub(super) fn exact_http_request_body(request: &str) -> Vec<u8> {
    let (headers, body) = request
        .split_once("\r\n\r\n")
        .expect("request headers should end");
    if !headers
        .lines()
        .any(|line| line.eq_ignore_ascii_case("transfer-encoding: chunked"))
    {
        return body.as_bytes().to_vec();
    }
    let mut remaining = body.as_bytes();
    let mut decoded = Vec::new();
    loop {
        let line_end = remaining
            .windows(2)
            .position(|window| window == b"\r\n")
            .expect("chunk size should end");
        let size = usize::from_str_radix(
            std::str::from_utf8(&remaining[..line_end]).expect("chunk size should be utf8"),
            16,
        )
        .expect("chunk size should be hexadecimal");
        remaining = &remaining[line_end + 2..];
        if size == 0 {
            break;
        }
        decoded.extend_from_slice(&remaining[..size]);
        assert_eq!(&remaining[size..size + 2], b"\r\n");
        remaining = &remaining[size + 2..];
    }
    decoded
}

pub(super) fn write_http_response(stream: &mut TcpStream, status: u16, body: &[u8]) {
    let content_type = if body.starts_with(b"event:") {
        "text/event-stream"
    } else {
        "application/json"
    };
    write!(
        stream,
        "HTTP/1.1 {status} synthetic\r\nConnection: close\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .expect("response headers should write");
    stream.write_all(body).expect("response body should write");
}
