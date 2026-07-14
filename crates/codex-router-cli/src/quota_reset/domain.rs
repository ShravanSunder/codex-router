//! Pure eligibility and reset-credit selection rules.

/// Provider-reported reset credit used by the guarded reset workflow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LiveResetCredit {
    pub(crate) id: String,
    pub(crate) status: String,
    pub(crate) expires_unix_seconds: Option<i64>,
    pub(crate) expires_at: Option<String>,
    pub(crate) title: Option<String>,
}

/// Why a selected account cannot consume a reset credit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ResetEligibilityRefusal {
    WeeklyWindowMissing,
    WeeklyRemainingNotBelowOnePercent { remaining_percent: u32 },
    NoAvailableResetCredit,
    SelectedCreditChanged,
}

/// Returns the earliest-expiring available credit after enforcing the live weekly guard.
pub(crate) fn select_guarded_reset_credit(
    weekly_remaining_percent: Option<u32>,
    credits: &[LiveResetCredit],
) -> Result<&LiveResetCredit, ResetEligibilityRefusal> {
    let weekly_remaining_percent =
        weekly_remaining_percent.ok_or(ResetEligibilityRefusal::WeeklyWindowMissing)?;
    if weekly_remaining_percent >= 1 {
        return Err(ResetEligibilityRefusal::WeeklyRemainingNotBelowOnePercent {
            remaining_percent: weekly_remaining_percent,
        });
    }

    credits
        .iter()
        .filter(|credit| credit.status == "available")
        .min_by_key(|credit| (credit.expires_unix_seconds.unwrap_or(i64::MAX), &credit.id))
        .ok_or(ResetEligibilityRefusal::NoAvailableResetCredit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_weekly_guard_refuses_missing_one_and_higher_percentages() {
        let credit = available_credit("credit-a", Some(100));

        assert_eq!(
            select_guarded_reset_credit(None, std::slice::from_ref(&credit)),
            Err(ResetEligibilityRefusal::WeeklyWindowMissing)
        );
        assert_eq!(
            select_guarded_reset_credit(Some(1), std::slice::from_ref(&credit)),
            Err(ResetEligibilityRefusal::WeeklyRemainingNotBelowOnePercent {
                remaining_percent: 1
            })
        );
        assert_eq!(
            select_guarded_reset_credit(Some(75), std::slice::from_ref(&credit)),
            Err(ResetEligibilityRefusal::WeeklyRemainingNotBelowOnePercent {
                remaining_percent: 75
            })
        );
    }

    #[test]
    fn zero_percent_selects_earliest_expiring_available_credit() {
        let credits = vec![
            available_credit("never", None),
            available_credit("later", Some(200)),
            LiveResetCredit {
                id: "redeemed".to_owned(),
                status: "redeemed".to_owned(),
                expires_unix_seconds: Some(50),
                expires_at: Some("1970-01-01T00:00:50Z".to_owned()),
                title: None,
            },
            available_credit("earliest", Some(100)),
        ];

        let selected = select_guarded_reset_credit(Some(0), &credits)
            .unwrap_or_else(|error| panic!("zero percent should be eligible: {error:?}"));

        assert_eq!(selected.id, "earliest");
    }

    #[test]
    fn zero_percent_without_available_credit_fails_closed() {
        let credits = vec![LiveResetCredit {
            id: "redeemed".to_owned(),
            status: "redeemed".to_owned(),
            expires_unix_seconds: Some(50),
            expires_at: Some("1970-01-01T00:00:50Z".to_owned()),
            title: None,
        }];

        assert_eq!(
            select_guarded_reset_credit(Some(0), &credits),
            Err(ResetEligibilityRefusal::NoAvailableResetCredit)
        );
    }

    fn available_credit(id: &str, expires_unix_seconds: Option<i64>) -> LiveResetCredit {
        LiveResetCredit {
            id: id.to_owned(),
            status: "available".to_owned(),
            expires_unix_seconds,
            expires_at: expires_unix_seconds.map(|value| format!("unix-{value}")),
            title: None,
        }
    }
}
