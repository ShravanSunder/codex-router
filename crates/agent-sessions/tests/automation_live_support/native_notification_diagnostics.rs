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
    let details = notification
        .pointer("/params/error/additionalDetails")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let hints: Vec<&str> = [
        ("connection refused", "connectionRefused"),
        ("connection reset", "connectionReset"),
        ("closed by server", "serverClosed"),
        ("response.completed", "missingCompletionEvent"),
        ("timed out", "requestTimeout"),
        ("timeout", "timeout"),
        ("websocket", "websocket"),
        ("error sending request", "requestTransportError"),
        ("certificate", "certificateError"),
        ("dns", "dnsError"),
        ("502", "mentions502"),
        ("503", "mentions503"),
        ("401", "mentions401"),
        ("403", "mentions403"),
        ("429", "mentions429"),
        ("decode", "decodeError"),
        ("parse", "parseError"),
    ]
    .into_iter()
    .filter_map(|(needle, hint)| details.contains(needle).then_some(hint))
    .collect();
    Some(
        json!({"threadId":thread_id,"errorKind":kind,"httpStatusCode":status,
        "willRetry":notification.pointer("/params/willRetry").and_then(Value::as_bool),"detailHints":hints}),
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
            "errorKind":"httpConnectionFailed","httpStatusCode":503,"willRetry":true,"detailHints":[]}))
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
            "errorKind":"unclassified","httpStatusCode":null,"willRetry":false,"detailHints":[]}))
        );
    }

    #[test]
    fn underlying_transport_hints_do_not_retain_urls_or_error_bodies() {
        let message = json!({"method":"error","params":{"threadId":"owned","willRetry":true,
            "error":{"codexErrorInfo":"other","additionalDetails":"websocket closed by server before response.completed: PRIVATE_URL PRIVATE_TOKEN"}}});
        assert_eq!(
            for_target(&message, "owned"),
            Some(json!({"threadId":"owned",
            "errorKind":"other","httpStatusCode":null,"willRetry":true,
            "detailHints":["serverClosed","missingCompletionEvent","websocket"]}))
        );
    }
}
