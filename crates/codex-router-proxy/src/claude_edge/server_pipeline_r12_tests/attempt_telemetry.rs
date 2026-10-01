use super::super::log_claude_attempt_selection;
use super::super::log_claude_attempts_completed;
use super::account_id;
use crate::account_selection::SelectedAccountDecision;
use crate::http_sse::redacted_account_hash;
use crate::test_log_capture::capture_log_output;

#[test]
fn claude_attempt_logs_have_structured_redacted_attempt_fields() {
    let account_id = account_id("private-account@example.test");
    let pinned = SelectedAccountDecision::new(account_id.clone(), "prompt_cache_account_affinity");
    let fresh = SelectedAccountDecision::new(account_id.clone(), "preferred_quota");
    let expected_account_hash = redacted_account_hash(&account_id);
    let rendered = capture_log_output(|| {
        log_claude_attempt_selection(77, 1, &pinned);
        log_claude_attempt_selection(77, 2, &fresh);
        log_claude_attempts_completed(77, 2, "credential_rejected");
    });

    assert!(rendered.contains("request.sequence=77"), "{rendered}");
    assert!(rendered.contains("attempt.number=1"), "{rendered}");
    assert!(rendered.contains("attempt.number=2"), "{rendered}");
    assert!(rendered.contains("account.hash"), "{rendered}");
    assert!(rendered.contains(&expected_account_hash), "{rendered}");
    assert!(rendered.contains("selection.mode=\"pin\""), "{rendered}");
    assert!(rendered.contains("selection.mode=\"fresh\""), "{rendered}");
    assert!(
        rendered.contains("attempt_2.failure_class=\"credential_rejected\""),
        "{rendered}"
    );
    assert!(!rendered.contains(account_id.as_str()), "{rendered}");
    assert!(!rendered.contains("oauth-secret-token"), "{rendered}");
    assert!(!rendered.contains("user prompt sentinel"), "{rendered}");
}
