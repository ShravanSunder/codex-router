//! Request-local effect evidence; never a replay or delivery store.
use serde_json::{Value, json};

pub(crate) struct MessageEffects {
    pub id: Value,
    pub resume: &'static str,
    pub submission: &'static str,
    pub correlation: Option<String>,
}
impl MessageEffects {
    pub fn new(id: Value) -> Self {
        Self {
            id,
            resume: "notRequested",
            submission: "notDispatched",
            correlation: None,
        }
    }
    pub fn failure(&self, kind: &str, stage: &str) -> Value {
        let mut data = json!({"kind":kind,"stage":stage,"message":"Message operation failed",
            "effects":{"resume":self.resume,"submission":self.submission}});
        if let (Some(fields), Some(id)) = (data.as_object_mut(), &self.correlation) {
            fields.insert("clientUserMessageId".to_owned(), json!(id));
        }
        json!({"jsonrpc":"2.0","id":self.id,"error":{"code":-32050,"message":"Message operation failed","data":data}})
    }
    pub fn native_rejection(&self, stage: &str, code: i64, native: Option<&Value>) -> Value {
        let (reason, next_action) = classify_native_rejection(code, native);
        let message = if reason == "heldByAnotherClient" {
            "Message it from the Codex client that holds it."
        } else {
            "Message operation failed"
        };
        let mut data = json!({"kind":"nativeRejected","stage":stage,"message":message,
            "reason":reason,"nextAction":next_action,"clientCode":code,
            "effects":{"resume":self.resume,"submission":self.submission}});
        if reason == "unknown"
            && let Some(fields) = data.as_object_mut()
        {
            fields.insert("nativeCode".into(), json!(code));
        }
        if let (Some(fields), Some(id)) = (data.as_object_mut(), &self.correlation) {
            fields.insert("clientUserMessageId".to_owned(), json!(id));
        }
        json!({"jsonrpc":"2.0","id":self.id,"error":{"code":-32050,"message":"Message operation failed","data":data}})
    }
}

/// The closed rejection reason set and its corrective action, shared by every
/// native path so one refusal reads the same whichever call produced it.
pub(crate) fn classify_native_rejection(
    code: i64,
    native: Option<&Value>,
) -> (&'static str, &'static str) {
    let native_message = native
        .and_then(|value| value.get("message"))
        .and_then(Value::as_str);
    if code == -32600
        && native_message.is_some_and(|message| {
            message
                .strip_prefix("thread ")
                .and_then(|thread| thread.strip_suffix(" already has an active writer"))
                .is_some_and(|thread| {
                    !thread.is_empty() && !thread.chars().any(char::is_whitespace)
                })
        })
    {
        return ("heldByAnotherClient", "messageFromHoldingCodexClient");
    }
    let description = native
        .map(Value::to_string)
        .unwrap_or_default()
        .to_lowercase();
    if description.contains("subagent") || description.contains("child") {
        ("childThread", "inspectTarget")
    } else if description.contains("busy") || description.contains("active turn") {
        ("busy", "useDeliverySteer")
    } else if description.contains("resume") {
        ("notResumable", "inspectTarget")
    } else if description.contains("permission") || code == -32001 {
        ("permissionDenied", "requestApproval")
    } else if code == -32601 || description.contains("unsupported") {
        ("unsupportedCapability", "correctRequest")
    } else {
        ("unknown", "retryLater")
    }
}

#[cfg(test)]
mod tests {
    use super::{MessageEffects, classify_native_rejection};
    use serde_json::json;

    #[test]
    fn native_rejections_have_closed_reasons_and_corrective_actions() {
        let effects = MessageEffects::new(json!(1));
        for (code, native, reason, action) in [
            (
                -32600,
                json!({"message":"thread 00000000-0000-4000-8000-000000000001 already has an active writer"}),
                "heldByAnotherClient",
                "messageFromHoldingCodexClient",
            ),
            (
                -32000,
                json!({"message":"target is a spawned child subagent"}),
                "childThread",
                "inspectTarget",
            ),
            (
                -32000,
                json!({"message":"thread has an active turn and is busy"}),
                "busy",
                "useDeliverySteer",
            ),
            (
                -32000,
                json!({"message":"thread cannot resume"}),
                "notResumable",
                "inspectTarget",
            ),
            (
                -32001,
                json!({"message":"denied"}),
                "permissionDenied",
                "requestApproval",
            ),
            (
                -32601,
                json!({"message":"missing method"}),
                "unsupportedCapability",
                "correctRequest",
            ),
            (
                -32099,
                json!({"message":"provider refused"}),
                "unknown",
                "retryLater",
            ),
        ] {
            let rejection = effects.native_rejection("submit", code, Some(&native));
            assert_eq!(
                rejection.pointer("/error/data/reason"),
                Some(&json!(reason))
            );
            assert_eq!(
                rejection.pointer("/error/data/nextAction"),
                Some(&json!(action))
            );
            if reason == "unknown" {
                assert_eq!(
                    rejection.pointer("/error/data/nativeCode"),
                    Some(&json!(code))
                );
            } else {
                assert!(rejection.pointer("/error/data/nativeCode").is_none());
            }
        }
    }

    #[test]
    fn only_the_exact_invalid_request_writer_lock_error_names_another_client() {
        for (code, message, expected) in [
            (
                -32601,
                "thread target already has an active writer",
                ("unsupportedCapability", "correctRequest"),
            ),
            (
                -32600,
                "thread target already has an active writer; retry",
                ("unknown", "retryLater"),
            ),
            (
                -32600,
                "thread  already has an active writer",
                ("unknown", "retryLater"),
            ),
            (-32600, "thread target not found", ("unknown", "retryLater")),
        ] {
            assert_eq!(
                classify_native_rejection(code, Some(&json!({"message":message}))),
                expected,
                "unrelated native error: {message}"
            );
        }
    }
}
