//! Keep bounded diagnostics for a test-owned thread without retaining provider error bodies.
use serde_json::{Value, json};

pub(super) fn for_target(notification: &Value, thread_id: &str) -> Option<Value> {
    if notification.get("method").and_then(Value::as_str) != Some("error")
        || notification
            .pointer("/params/threadId")
            .and_then(Value::as_str)
            != Some(thread_id)
    {
        return None;
    }
    let info = notification.pointer("/params/error/codexErrorInfo");
    let known = [
        "contextWindowExceeded",
        "sessionBudgetExceeded",
        "usageLimitExceeded",
        "rateLimitExceeded",
        "serverOverloaded",
        "cyberPolicy",
        "misalignmentPolicyViolation",
        "httpConnectionFailed",
        "responseStreamConnectionFailed",
        "internalServerError",
        "unauthorized",
        "badRequest",
        "threadRollbackFailed",
        "sandboxError",
        "responseStreamDisconnected",
        "responseTooManyFailedAttempts",
        "activeTurnNotSteerable",
        "other",
    ];
    let kind = known
        .into_iter()
        .find(|kind| {
            info.is_some_and(|info| {
                info.as_str() == Some(*kind)
                    || info
                        .as_object()
                        .is_some_and(|object| object.contains_key(*kind))
            })
        })
        .unwrap_or("unclassified");
    let status = info
        .and_then(|info| info.get(kind))
        .and_then(|details| details.get("httpStatusCode"))
        .and_then(Value::as_u64)
        .filter(|status| (100..=599).contains(status));
    Some(
        json!({"threadId":thread_id,"errorKind":kind,"httpStatusCode":status,
        "willRetry":notification.pointer("/params/willRetry").and_then(Value::as_bool)}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_retry_class_without_provider_body_or_unrelated_thread_data() {
        let message = json!({"method":"error","params":{"threadId":"owned",
            "turnId":"turn","willRetry":true,"error":{"message":"SECRET_CANARY",
            "additionalDetails":"PRIVATE_BODY","codexErrorInfo":{"httpConnectionFailed":{"httpStatusCode":503}}}}});
        assert_eq!(
            for_target(&message, "owned"),
            Some(json!({"threadId":"owned",
            "errorKind":"httpConnectionFailed","httpStatusCode":503,"willRetry":true}))
        );
        assert!(for_target(&message, "another-thread").is_none());
    }

    #[test]
    fn unknown_provider_payload_is_not_copied_into_diagnostics() {
        let message = json!({"method":"error","params":{"threadId":"owned",
            "error":{"codexErrorInfo":{"PRIVATE_KEY":"PRIVATE_VALUE"}},"willRetry":false}});
        assert_eq!(
            for_target(&message, "owned"),
            Some(json!({"threadId":"owned",
            "errorKind":"unclassified","httpStatusCode":null,"willRetry":false}))
        );
    }
}
