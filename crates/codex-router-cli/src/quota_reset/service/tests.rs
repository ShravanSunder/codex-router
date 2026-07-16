use std::collections::VecDeque;
use std::sync::Mutex;

use codex_router_core::redaction::SecretString;

use super::*;
use crate::quota_reset::domain::ConsumeUnknownReason;
use crate::quota_reset::domain::KnownConsumeOutcome;
use crate::quota_reset::domain::LiveResetCredit;
use crate::quota_reset::domain::validate_credit_inventory;

#[derive(Clone)]
struct FakeAuthority {
    account_id: AccountId,
    generation: ActiveCredentialGeneration,
    fingerprint: u64,
    expires_at: Option<u64>,
}

impl ResetAuthority for FakeAuthority {
    type Fingerprint = u64;

    fn account_id(&self) -> &AccountId {
        &self.account_id
    }
    fn active_credential_generation(&self) -> ActiveCredentialGeneration {
        self.generation
    }
    fn auth(&self) -> LiveResetAccountAuth {
        LiveResetAccountAuth {
            access_token: SecretString::new("fake-token"),
            chatgpt_account_id: "fake-routing".to_owned(),
        }
    }
    fn expires_unix_seconds(&self) -> Option<u64> {
        self.expires_at
    }
    fn fingerprint(&self) -> &Self::Fingerprint {
        &self.fingerprint
    }
}

struct FakeAuthorityReader {
    reads: Mutex<VecDeque<Result<FakeAuthority, RenderSafeFailure>>>,
}

impl ResetAuthorityReader for FakeAuthorityReader {
    type Authority = FakeAuthority;

    async fn read_authority(
        &self,
        _account_id: &AccountId,
        _expected_generation: ActiveCredentialGeneration,
        _now_unix_seconds: u64,
    ) -> Result<Self::Authority, RenderSafeFailure> {
        self.reads
            .lock()
            .expect("reader lock")
            .pop_front()
            .expect("scripted authority")
    }
}

struct FakeProvider {
    usage: Mutex<VecDeque<LiveUsagePortResult>>,
    inventory: Mutex<VecDeque<CreditInventoryPortResult>>,
    consume: Mutex<VecDeque<ConsumePortResult>>,
    ledger: Mutex<Vec<&'static str>>,
    preparation_failure: Mutex<Option<RenderSafeFailure>>,
}

struct FakePreparedConsume;

impl ResetServiceProvider for FakeProvider {
    type PreparedConsume = FakePreparedConsume;

    async fn fetch_usage(&self, _auth: LiveResetAccountAuth) -> LiveUsagePortResult {
        self.ledger.lock().expect("ledger lock").push("usage-get");
        self.usage
            .lock()
            .expect("usage lock")
            .pop_front()
            .expect("scripted usage")
    }

    async fn fetch_inventory(
        &self,
        _auth: LiveResetAccountAuth,
        _now_unix_seconds: i64,
    ) -> CreditInventoryPortResult {
        self.ledger
            .lock()
            .expect("ledger lock")
            .push("inventory-get");
        self.inventory
            .lock()
            .expect("inventory lock")
            .pop_front()
            .expect("scripted inventory")
    }

    fn prepare_consume(
        &self,
        _auth: &LiveResetAccountAuth,
        _selected_credit: &SelectedResetCreditSnapshot,
        _redeem_request_id: &RedeemRequestId,
    ) -> Result<Self::PreparedConsume, RenderSafeFailure> {
        self.ledger.lock().expect("ledger lock").push("prepare");
        match *self.preparation_failure.lock().expect("preparation lock") {
            Some(failure) => Err(failure),
            None => Ok(FakePreparedConsume),
        }
    }

    async fn invoke_prepared(&self, _prepared: Self::PreparedConsume) -> ConsumePortResult {
        self.ledger.lock().expect("ledger lock").push("post");
        self.consume
            .lock()
            .expect("consume lock")
            .pop_front()
            .expect("scripted consume")
    }
}

#[tokio::test]
async fn inspection_runs_both_independent_gets_even_when_usage_is_ineligible() {
    let service = service(
        vec![Ok(authority(7))],
        vec![LiveUsagePortResult::Known(LiveWeeklyUsage::new(4))],
        vec![validated_inventory(vec![credit("earliest", 200)])],
    );
    let inspection = service
        .resolve_inspection_authority(&account_id(), generation(), 100)
        .await
        .expect("authority");

    let (usage, inventory) = tokio::join!(
        service.inspect_usage(inspection.clone()),
        service.inspect_inventory(inspection, 100),
    );

    assert_eq!(usage, LiveUsagePortResult::Known(LiveWeeklyUsage::new(4)));
    assert!(matches!(inventory, CreditInventoryPortResult::Validated(_)));
    assert_eq!(
        *service.provider.ledger.lock().expect("ledger lock"),
        ["usage-get", "inventory-get"]
    );
}

#[tokio::test]
async fn authority_and_selected_credit_changes_refuse_with_zero_posts() {
    for (fresh_authority, revalidated_inventory, expected) in [
        (
            authority(8),
            validated_inventory(vec![credit("earliest", 200)]),
            RenderSafeFailure::CredentialUnavailable,
        ),
        (
            authority(7),
            validated_inventory(vec![credit("replacement", 150), credit("earliest", 200)]),
            RenderSafeFailure::SelectedCreditChanged,
        ),
    ] {
        let service = service(
            vec![Ok(authority(7)), Ok(fresh_authority)],
            vec![usage_zero(), usage_zero()],
            vec![
                validated_inventory(vec![credit("earliest", 200)]),
                revalidated_inventory,
            ],
        );
        let confirmation = inspected_confirmation(&service).await;

        let result = service
            .revalidate(confirmation, 100, redeem_id())
            .await
            .authorization;

        assert_eq!(result.expect_err("revalidation must refuse"), expected);
        assert!(
            !service
                .provider
                .ledger
                .lock()
                .expect("ledger lock")
                .contains(&"post")
        );
    }
}

#[tokio::test]
async fn every_authority_expiry_and_weekly_precommit_refusal_has_zero_posts() {
    let expired_authority = FakeAuthority {
        expires_at: Some(100),
        ..authority(7)
    };
    for (fresh_authority, revalidated_usage, expected) in [
        (
            Err(RenderSafeFailure::AccountUnavailable),
            usage_zero(),
            RenderSafeFailure::AccountUnavailable,
        ),
        (
            Err(RenderSafeFailure::CredentialGenerationChanged),
            usage_zero(),
            RenderSafeFailure::CredentialGenerationChanged,
        ),
        (
            Ok(expired_authority),
            usage_zero(),
            RenderSafeFailure::CredentialExpired,
        ),
        (
            Ok(authority(7)),
            LiveUsagePortResult::Known(LiveWeeklyUsage::new(1)),
            RenderSafeFailure::EligibilityRefused,
        ),
    ] {
        let service = service(
            vec![Ok(authority(7)), fresh_authority],
            vec![usage_zero(), revalidated_usage],
            vec![validated_inventory(vec![credit("earliest", 200)]); 2],
        );
        let confirmation = inspected_confirmation(&service).await;

        let refusal = service
            .revalidate(confirmation, 100, redeem_id())
            .await
            .authorization
            .expect_err("precommit refusal");

        assert_eq!(refusal, expected);
        assert!(
            !service
                .provider
                .ledger
                .lock()
                .expect("ledger lock")
                .contains(&"post")
        );
    }
}

#[tokio::test]
async fn later_credit_only_change_allows_exactly_one_by_value_consume() {
    let service = service(
        vec![Ok(authority(7)), Ok(authority(7))],
        vec![usage_zero(), usage_zero()],
        vec![
            validated_inventory(vec![credit("earliest", 200), credit("later", 300)]),
            validated_inventory(vec![credit("earliest", 200), credit("new-later", 400)]),
        ],
    );
    let confirmation = inspected_confirmation(&service).await;
    let capability = service
        .revalidate(confirmation, 100, redeem_id())
        .await
        .authorization
        .expect("capability");

    let outcome = service.consume(capability).await;

    assert_eq!(
        outcome,
        ConsumePortResult::Known(KnownConsumeOutcome::AlreadyRedeemed)
    );
    assert_eq!(
        service
            .provider
            .ledger
            .lock()
            .expect("ledger lock")
            .iter()
            .filter(|entry| **entry == "post")
            .count(),
        1
    );
}

#[tokio::test]
async fn local_preparation_failure_is_zero_post_and_ambiguous_invocation_is_unknown() {
    let preparation_failure_service = service(
        vec![Ok(authority(7)), Ok(authority(7))],
        vec![usage_zero(), usage_zero()],
        vec![validated_inventory(vec![credit("earliest", 200)]); 2],
    );
    *preparation_failure_service
        .provider
        .preparation_failure
        .lock()
        .expect("preparation lock") = Some(RenderSafeFailure::InvalidResponse);
    let confirmation = inspected_confirmation(&preparation_failure_service).await;
    assert_eq!(
        preparation_failure_service
            .revalidate(confirmation, 100, redeem_id())
            .await
            .authorization
            .expect_err("prep refusal"),
        RenderSafeFailure::InvalidResponse
    );
    assert!(
        !preparation_failure_service
            .provider
            .ledger
            .lock()
            .expect("ledger lock")
            .contains(&"post")
    );

    let unknown_outcome_service = service(
        vec![Ok(authority(7)), Ok(authority(7))],
        vec![usage_zero(), usage_zero()],
        vec![validated_inventory(vec![credit("earliest", 200)]); 2],
    );
    *unknown_outcome_service
        .provider
        .consume
        .lock()
        .expect("consume lock") = VecDeque::from([ConsumePortResult::OutcomeUnknown(
        ConsumeUnknownReason::Transport,
    )]);
    let confirmation = inspected_confirmation(&unknown_outcome_service).await;
    let capability = unknown_outcome_service
        .revalidate(confirmation, 100, redeem_id())
        .await
        .authorization
        .expect("capability");
    assert_eq!(
        unknown_outcome_service.consume(capability).await,
        ConsumePortResult::OutcomeUnknown(ConsumeUnknownReason::Transport)
    );
    assert_eq!(
        unknown_outcome_service
            .provider
            .ledger
            .lock()
            .expect("ledger lock")
            .iter()
            .filter(|entry| **entry == "post")
            .count(),
        1
    );
}

async fn inspected_confirmation(
    service: &ResetWorkflowService<FakeAuthorityReader, FakeProvider>,
) -> ConfirmationAuthority<FakeAuthority> {
    let inspection = service
        .resolve_inspection_authority(&account_id(), generation(), 100)
        .await
        .expect("authority");
    let (usage, inventory) = tokio::join!(
        service.inspect_usage(inspection.clone()),
        service.inspect_inventory(inspection.clone(), 100),
    );
    let LiveUsagePortResult::Known(usage) = usage else {
        panic!("known usage")
    };
    let CreditInventoryPortResult::Validated(inventory) = inventory else {
        panic!("inventory")
    };
    ResetWorkflowService::<FakeAuthorityReader, FakeProvider>::bind_confirmation(
        inspection,
        AttemptGeneration::new(3),
        usage,
        &inventory,
    )
    .expect("confirmation")
}

fn service(
    authorities: Vec<Result<FakeAuthority, RenderSafeFailure>>,
    usage: Vec<LiveUsagePortResult>,
    inventory: Vec<CreditInventoryPortResult>,
) -> ResetWorkflowService<FakeAuthorityReader, FakeProvider> {
    ResetWorkflowService::new(
        FakeAuthorityReader {
            reads: Mutex::new(authorities.into()),
        },
        FakeProvider {
            usage: Mutex::new(usage.into()),
            inventory: Mutex::new(inventory.into()),
            consume: Mutex::new(VecDeque::from([ConsumePortResult::Known(
                KnownConsumeOutcome::AlreadyRedeemed,
            )])),
            ledger: Mutex::new(Vec::new()),
            preparation_failure: Mutex::new(None),
        },
    )
}

fn authority(fingerprint: u64) -> FakeAuthority {
    FakeAuthority {
        account_id: account_id(),
        generation: generation(),
        fingerprint,
        expires_at: Some(500),
    }
}
fn account_id() -> AccountId {
    AccountId::new("acct_service").expect("account id")
}
fn generation() -> ActiveCredentialGeneration {
    ActiveCredentialGeneration::new(2)
}
fn usage_zero() -> LiveUsagePortResult {
    LiveUsagePortResult::Known(LiveWeeklyUsage::new(0))
}
fn redeem_id() -> RedeemRequestId {
    RedeemRequestId::new("redeem-service-test".to_owned()).expect("redeem id")
}
fn credit(id: &str, expires: i64) -> LiveResetCredit {
    LiveResetCredit {
        id: id.to_owned(),
        status: "available".to_owned(),
        expires_unix_seconds: Some(expires),
        expires_at: Some(format!("unix-{expires}")),
        title: Some("Weekly reset".to_owned()),
    }
}
fn validated_inventory(credits: Vec<LiveResetCredit>) -> CreditInventoryPortResult {
    CreditInventoryPortResult::Validated(
        validate_credit_inventory(credits, 100).expect("valid inventory"),
    )
}
