//! Guarded reset preparation and consumption orchestration.

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use super::QuotaResetError;
use super::domain::ResetEligibilityRefusal;
use super::domain::select_guarded_reset_credit;
#[cfg(test)]
use super::provider::ConsumeResetCreditCode;
use super::provider::ConsumeResetCreditResponse;
use super::provider::LiveQuotaResetProvider;
use super::provider::LiveResetAccountAuth;

static IDEMPOTENCY_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PrepareResetOutcome {
    Refused(ResetEligibilityRefusal),
    Eligible(PreparedReset),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ConsumeAfterConfirmationOutcome {
    Refused(ResetEligibilityRefusal),
    Consumed(ConsumeResetCreditResponse),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparedReset {
    pub(crate) weekly_remaining_percent: u32,
    pub(crate) credit_id: String,
    pub(crate) credit_title: Option<String>,
    pub(crate) expires_unix_seconds: Option<i64>,
    pub(crate) expires_at: Option<String>,
    redeem_request_id: String,
}

#[cfg(test)]
impl PreparedReset {
    pub(crate) fn for_test(
        credit_id: impl Into<String>,
        expires_unix_seconds: Option<i64>,
    ) -> Self {
        Self {
            weekly_remaining_percent: 0,
            credit_id: credit_id.into(),
            credit_title: None,
            expires_unix_seconds,
            expires_at: expires_unix_seconds.map(|value| format!("unix-{value}")),
            redeem_request_id: "test-redeem-request".to_owned(),
        }
    }
}

pub(crate) async fn prepare_guarded_reset<P: LiveQuotaResetProvider>(
    provider: &P,
    auth: &LiveResetAccountAuth,
) -> Result<PrepareResetOutcome, QuotaResetError> {
    let weekly_remaining_percent = provider.fetch_weekly_remaining_percent(auth).await?;
    if weekly_remaining_percent.is_none_or(|remaining_percent| remaining_percent >= 1) {
        let refusal = match weekly_remaining_percent {
            Some(remaining_percent) => {
                ResetEligibilityRefusal::WeeklyRemainingNotBelowOnePercent { remaining_percent }
            }
            None => ResetEligibilityRefusal::WeeklyWindowMissing,
        };
        return Ok(PrepareResetOutcome::Refused(refusal));
    }

    let credits = provider.fetch_reset_credits(auth).await?;
    let selected = match select_guarded_reset_credit(weekly_remaining_percent, &credits) {
        Ok(selected) => selected,
        Err(refusal) => return Ok(PrepareResetOutcome::Refused(refusal)),
    };
    Ok(PrepareResetOutcome::Eligible(PreparedReset {
        weekly_remaining_percent: weekly_remaining_percent.unwrap_or(0),
        credit_id: selected.id.clone(),
        credit_title: selected.title.clone(),
        expires_unix_seconds: selected.expires_unix_seconds,
        expires_at: selected.expires_at.clone(),
        redeem_request_id: new_redeem_request_id()?,
    }))
}

pub(crate) async fn consume_prepared_reset<P: LiveQuotaResetProvider>(
    provider: &P,
    auth: &LiveResetAccountAuth,
    prepared: &PreparedReset,
) -> Result<ConsumeResetCreditResponse, QuotaResetError> {
    provider
        .consume_reset_credit(auth, &prepared.credit_id, &prepared.redeem_request_id)
        .await
}

pub(crate) async fn revalidate_live_weekly_guard<P: LiveQuotaResetProvider>(
    provider: &P,
    auth: &LiveResetAccountAuth,
) -> Result<Result<(), ResetEligibilityRefusal>, QuotaResetError> {
    let remaining_percent = provider.fetch_weekly_remaining_percent(auth).await?;
    Ok(match remaining_percent {
        Some(remaining_percent) if remaining_percent < 1 => Ok(()),
        Some(remaining_percent) => {
            Err(ResetEligibilityRefusal::WeeklyRemainingNotBelowOnePercent { remaining_percent })
        }
        None => Err(ResetEligibilityRefusal::WeeklyWindowMissing),
    })
}

pub(crate) async fn consume_after_live_revalidation<P: LiveQuotaResetProvider>(
    provider: &P,
    auth: &LiveResetAccountAuth,
    prepared: &PreparedReset,
) -> Result<ConsumeAfterConfirmationOutcome, QuotaResetError> {
    if let Err(refusal) = revalidate_live_weekly_guard(provider, auth).await? {
        return Ok(ConsumeAfterConfirmationOutcome::Refused(refusal));
    }
    let credits = provider.fetch_reset_credits(auth).await?;
    let selected = match select_guarded_reset_credit(Some(0), &credits) {
        Ok(selected) => selected,
        Err(refusal) => return Ok(ConsumeAfterConfirmationOutcome::Refused(refusal)),
    };
    if selected.id != prepared.credit_id {
        return Ok(ConsumeAfterConfirmationOutcome::Refused(
            ResetEligibilityRefusal::SelectedCreditChanged,
        ));
    }
    consume_prepared_reset(provider, auth, prepared)
        .await
        .map(ConsumeAfterConfirmationOutcome::Consumed)
}

fn new_redeem_request_id() -> Result<String, QuotaResetError> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_error| QuotaResetError::ClockUnavailable)?
        .as_nanos();
    let counter = IDEMPOTENCY_COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(format!(
        "codex-router-{}-{nanos}-{counter}",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use codex_router_core::redaction::SecretString;

    use super::*;
    use crate::quota_reset::domain::LiveResetCredit;

    struct FakeProvider {
        weekly_remaining_percent: Option<u32>,
        credits: Vec<LiveResetCredit>,
        calls: Mutex<Vec<&'static str>>,
        consume_arguments: Mutex<Vec<(String, String)>>,
    }

    impl LiveQuotaResetProvider for FakeProvider {
        async fn fetch_weekly_remaining_percent(
            &self,
            _auth: &LiveResetAccountAuth,
        ) -> Result<Option<u32>, QuotaResetError> {
            self.record("usage");
            Ok(self.weekly_remaining_percent)
        }

        async fn fetch_reset_credits(
            &self,
            _auth: &LiveResetAccountAuth,
        ) -> Result<Vec<LiveResetCredit>, QuotaResetError> {
            self.record("credits");
            Ok(self.credits.clone())
        }

        async fn consume_reset_credit(
            &self,
            _auth: &LiveResetAccountAuth,
            credit_id: &str,
            redeem_request_id: &str,
        ) -> Result<ConsumeResetCreditResponse, QuotaResetError> {
            self.record("consume");
            self.consume_arguments
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push((credit_id.to_owned(), redeem_request_id.to_owned()));
            Ok(ConsumeResetCreditResponse {
                code: ConsumeResetCreditCode::Reset,
                windows_reset: 2,
            })
        }
    }

    impl FakeProvider {
        fn record(&self, call: &'static str) {
            self.calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(call);
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }
    }

    #[tokio::test]
    async fn ineligible_weekly_usage_never_fetches_credits_or_consumes() {
        for remaining_percent in [1, 50, 100] {
            let provider = fake_provider(Some(remaining_percent));

            let outcome = prepare_guarded_reset(&provider, &auth())
                .await
                .unwrap_or_else(|error| panic!("guard should return refusal: {error}"));

            assert!(matches!(outcome, PrepareResetOutcome::Refused(_)));
            assert_eq!(provider.calls(), vec!["usage"]);
        }
    }

    #[tokio::test]
    async fn eligible_preparation_stops_before_consume_until_separately_confirmed() {
        let provider = fake_provider(Some(0));

        let outcome = prepare_guarded_reset(&provider, &auth())
            .await
            .unwrap_or_else(|error| panic!("eligible reset should prepare: {error}"));
        let PrepareResetOutcome::Eligible(prepared) = outcome else {
            panic!("zero percent with a credit should be eligible");
        };

        assert_eq!(prepared.credit_id, "credit-early");
        assert_eq!(provider.calls(), vec!["usage", "credits"]);
    }

    #[tokio::test]
    async fn post_confirmation_revalidation_refuses_before_consume_when_weekly_changed() {
        let provider = fake_provider(Some(1));

        let prepared = PreparedReset::for_test("credit-a", Some(100));
        let revalidation = consume_after_live_revalidation(&provider, &auth(), &prepared)
            .await
            .unwrap_or_else(|error| panic!("live revalidation should return refusal: {error}"));

        assert!(matches!(
            revalidation,
            ConsumeAfterConfirmationOutcome::Refused(
                ResetEligibilityRefusal::WeeklyRemainingNotBelowOnePercent {
                    remaining_percent: 1
                }
            )
        ));
        assert_eq!(provider.calls(), vec!["usage"]);
    }

    #[tokio::test]
    async fn confirmed_reset_revalidates_usage_and_credit_before_exact_consume() {
        let provider = fake_provider(Some(0));
        let prepared = match prepare_guarded_reset(&provider, &auth())
            .await
            .unwrap_or_else(|error| panic!("preparation should succeed: {error}"))
        {
            PrepareResetOutcome::Eligible(prepared) => prepared,
            PrepareResetOutcome::Refused(refusal) => {
                panic!("preparation should not refuse: {refusal:?}")
            }
        };

        let outcome = consume_after_live_revalidation(&provider, &auth(), &prepared)
            .await
            .unwrap_or_else(|error| panic!("confirmed consume should succeed: {error}"));

        assert!(matches!(
            outcome,
            ConsumeAfterConfirmationOutcome::Consumed(_)
        ));
        assert_eq!(
            provider.calls(),
            vec!["usage", "credits", "usage", "credits", "consume"]
        );
        let arguments = provider
            .consume_arguments
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some((credit_id, redeem_request_id)) = arguments.first() else {
            panic!("consume arguments should be recorded");
        };
        assert_eq!(credit_id, "credit-early");
        assert!(!redeem_request_id.is_empty());
    }

    #[tokio::test]
    async fn changed_earliest_credit_after_confirmation_refuses_without_consume() {
        let provider = fake_provider(Some(0));
        let prepared = PreparedReset::for_test("credit-stale", Some(200));

        let outcome = consume_after_live_revalidation(&provider, &auth(), &prepared)
            .await
            .unwrap_or_else(|error| panic!("changed credit should refuse: {error}"));

        assert_eq!(
            outcome,
            ConsumeAfterConfirmationOutcome::Refused(
                ResetEligibilityRefusal::SelectedCreditChanged
            )
        );
        assert_eq!(provider.calls(), vec!["usage", "credits"]);
    }

    fn fake_provider(weekly_remaining_percent: Option<u32>) -> FakeProvider {
        FakeProvider {
            weekly_remaining_percent,
            credits: vec![LiveResetCredit {
                id: "credit-early".to_owned(),
                status: "available".to_owned(),
                expires_unix_seconds: Some(100),
                expires_at: Some("1970-01-01T00:01:40Z".to_owned()),
                title: Some("Weekly reset".to_owned()),
            }],
            calls: Mutex::new(Vec::new()),
            consume_arguments: Mutex::new(Vec::new()),
        }
    }

    fn auth() -> LiveResetAccountAuth {
        LiveResetAccountAuth {
            access_token: SecretString::new("test-token"),
            chatgpt_account_id: "test-account".to_owned(),
        }
    }
}
