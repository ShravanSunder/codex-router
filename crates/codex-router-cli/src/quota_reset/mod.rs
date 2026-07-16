//! Interactive, live-only usage-limit reset workflow.

use std::io::Write;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_router_secret_store::model::SecretStoreError;
use codex_router_state::sqlite::StateStoreError;
use thiserror::Error;

use crate::presentation::quota_reset::ResetConfirmationOutcome;
use crate::presentation::quota_reset::confirm_prepared_reset;
use crate::presentation::quota_reset::select_reset_account;

use self::credentials::load_read_only_reset_credential;
use self::credentials::load_reset_account_choices;
use self::domain::ResetEligibilityRefusal;
use self::orchestration::ConsumeAfterConfirmationOutcome;
use self::orchestration::PrepareResetOutcome;
use self::orchestration::consume_after_live_revalidation;
use self::orchestration::prepare_guarded_reset;
use self::provider::HttpLiveQuotaResetProvider;
use self::provider::LiveResetAccountAuth;

pub(crate) mod credentials;
pub(crate) mod domain;
pub(crate) mod orchestration;
pub(crate) mod provider;
pub(crate) mod service;
pub(crate) mod supervisor;
pub(crate) mod workflow;

/// Interactive quota-reset failure.
#[derive(Debug, Error)]
pub enum QuotaResetError {
    #[error(transparent)]
    State(#[from] StateStoreError),
    #[error(transparent)]
    Secret(#[from] SecretStoreError),
    #[error("selected account credential is expired; refresh the account before retrying")]
    ExpiredCredential,
    #[error("selected account credential is missing its ChatGPT account identifier")]
    MissingChatGptAccountId,
    #[error("system clock is unavailable for guarded quota reset")]
    ClockUnavailable,
    #[error("read-only credential task failed")]
    CredentialTaskFailed,
    #[error("quota reset provider request failed: {message}")]
    ProviderRequest { message: String },
    #[error("quota reset provider returned HTTP {status}")]
    ProviderStatus { status: u16 },
    #[error("quota reset provider response was unusable: {message}")]
    ProviderResponse { message: String },
    #[error("quota reset requires an interactive terminal")]
    TerminalRequired,
    #[error("no enabled accounts with active credentials are available")]
    NoEligibleAccounts,
    #[error("quota reset terminal interaction failed: {0}")]
    Terminal(std::io::Error),
    #[error("failed to write quota reset result: {0}")]
    Stdout(std::io::Error),
}

pub(crate) async fn run_interactive_quota_reset(
    stdout: &mut impl Write,
    router_root: PathBuf,
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
) -> Result<(), QuotaResetError> {
    if !stdin_is_terminal || !stdout_is_terminal {
        return Err(QuotaResetError::TerminalRequired);
    }
    let accounts = load_reset_account_choices(&router_root.join("state.sqlite")).await?;
    if accounts.is_empty() {
        return Err(QuotaResetError::NoEligibleAccounts);
    }
    let Some(account) = select_reset_account(accounts.clone())
        .await
        .map_err(QuotaResetError::Terminal)?
    else {
        return Ok(());
    };
    writeln!(
        stdout,
        "Checking live weekly usage for {} [{}]...",
        account.label, account.account_tag
    )
    .map_err(QuotaResetError::Stdout)?;
    let credential = load_read_only_reset_credential(
        &router_root.join("secrets"),
        &account,
        current_unix_seconds()?,
    )
    .await?;
    let auth = LiveResetAccountAuth {
        access_token: credential.access_token,
        chatgpt_account_id: credential.chatgpt_account_id,
    };
    let provider = HttpLiveQuotaResetProvider::new()?;
    let prepared = match prepare_guarded_reset(&provider, &auth).await? {
        PrepareResetOutcome::Eligible(prepared) => prepared,
        PrepareResetOutcome::Refused(refusal) => {
            writeln!(stdout, "{}", refusal_message(&account.label, &refusal))
                .map_err(QuotaResetError::Stdout)?;
            writeln!(stdout, "No reset credit was consumed.").map_err(QuotaResetError::Stdout)?;
            return Ok(());
        }
    };
    let confirmation = confirm_prepared_reset(accounts, account.clone(), prepared.clone())
        .await
        .map_err(QuotaResetError::Terminal)?;
    if confirmation != ResetConfirmationOutcome::Confirm {
        writeln!(stdout, "Cancelled. No reset credit was consumed.")
            .map_err(QuotaResetError::Stdout)?;
        return Ok(());
    }
    let response = match consume_after_live_revalidation(&provider, &auth, &prepared).await? {
        ConsumeAfterConfirmationOutcome::Consumed(response) => response,
        ConsumeAfterConfirmationOutcome::Refused(refusal) => {
            writeln!(stdout, "{}", refusal_message(&account.label, &refusal))
                .map_err(QuotaResetError::Stdout)?;
            writeln!(stdout, "No reset credit was consumed.").map_err(QuotaResetError::Stdout)?;
            return Ok(());
        }
    };
    writeln!(
        stdout,
        "Quota reset result: {} ({} windows reset)",
        response.code.as_str(),
        response.windows_reset
    )
    .map_err(QuotaResetError::Stdout)
}

fn refusal_message(account_label: &str, refusal: &ResetEligibilityRefusal) -> String {
    match refusal {
        ResetEligibilityRefusal::WeeklyWindowMissing => {
            format!("Reset refused for {account_label}: live weekly quota is unavailable.")
        }
        ResetEligibilityRefusal::WeeklyRemainingNotBelowOnePercent { remaining_percent } => {
            format!(
                "Reset refused for {account_label}: live weekly remaining is {remaining_percent}%; strictly less than 1% is required."
            )
        }
        ResetEligibilityRefusal::NoAvailableResetCredit => {
            format!("Reset refused for {account_label}: no available reset credit was returned.")
        }
        ResetEligibilityRefusal::SelectedCreditChanged => format!(
            "Reset refused for {account_label}: the earliest-expiring reset credit changed while awaiting confirmation."
        ),
    }
}

fn current_unix_seconds() -> Result<u64, QuotaResetError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_error| QuotaResetError::ClockUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reset_refuses_when_either_terminal_stream_is_not_interactive() {
        for (stdin_is_terminal, stdout_is_terminal) in [(false, true), (true, false)] {
            let mut stdout = Vec::new();
            let error = run_interactive_quota_reset(
                &mut stdout,
                PathBuf::from("path-must-not-be-read"),
                stdin_is_terminal,
                stdout_is_terminal,
            )
            .await
            .expect_err("non-interactive reset must fail before state or network access");

            assert!(matches!(error, QuotaResetError::TerminalRequired));
            assert!(stdout.is_empty());
        }
    }
}
