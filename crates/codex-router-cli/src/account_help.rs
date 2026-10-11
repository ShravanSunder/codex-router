//! Account help text and the explicit debug-only storage option.

pub(super) const ACCOUNT_HELP_TEXT: &str = "\
codex-router account

commands:
  disable --account <name>  Stop routing to an account while retaining its credentials
  enable --account <name>   Resume routing to an account
  login --provider <openai|claude> --label <name>  Add a provider OAuth account
  list                  Show configured router accounts
  set-weekly-floor      Set or disable one account's weekly quota floor
";

#[cfg(not(debug_assertions))]
const ACCOUNT_LOGIN_BASE_HELP_TEXT: &str = "\
codex-router account login --provider <openai|claude> --label <name>

Adds an OAuth account to router-owned encrypted storage.

options:
  --label <name>         Friendly account name shown in quota and account list
  --provider <name>      OAuth account provider [default: openai]
  OpenAI login displays a device URL and code, then waits for approval.
  Claude login opens the hosted authorization URL and asks you to paste code#state.
";

#[cfg(not(debug_assertions))]
pub(super) const ACCOUNT_LOGIN_HELP_TEXT: &str = ACCOUNT_LOGIN_BASE_HELP_TEXT;

#[cfg(debug_assertions)]
pub(super) const ACCOUNT_LOGIN_HELP_TEXT: &str = "\
codex-router account login --provider <openai|claude> --label <name>

Adds an OAuth account to router-owned storage (encrypted by default).

options:
  --label <name>         Friendly account name shown in quota and account list
  --provider <name>      OAuth account provider [default: openai]
  OpenAI login displays a device URL and code, then waits for approval.
  Claude login opens the hosted authorization URL and asks you to paste code#state.

debug-only options:
  --allow-plaintext-file-secrets  Store pooled credentials in plaintext without Keychain
  Requires --router-root <absolute isolated path>; the root remembers this mode.
";

pub(super) const ACCOUNT_LIST_HELP_TEXT: &str = "\
codex-router account list

Shows configured router accounts.
";

pub(super) const ACCOUNT_ENABLE_HELP_TEXT: &str = "\
codex-router account enable --account <label>

Resumes routing to one account without changing its credentials, quota, or history.
";

pub(super) const ACCOUNT_DISABLE_HELP_TEXT: &str = "\
codex-router account disable --account <label>

Changes one account's routing status without removing its credentials, quota, or history.
";

pub(super) const ACCOUNT_SET_WEEKLY_FLOOR_HELP_TEXT: &str = "\
codex-router account set-weekly-floor --account <label> --percent <0-15>

Sets an integer weekly quota floor for exactly one account label. Zero disables it.
";
