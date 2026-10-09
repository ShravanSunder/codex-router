//! Public native error metadata only; backend messages and details remain private.
use serde::Serialize;
use serde_json::Value;

const STRING_CLASSES: &[&str] = &[
    "contextWindowExceeded",
    "sessionBudgetExceeded",
    "usageLimitExceeded",
    "rateLimitExceeded",
    "flexUnavailable",
    "serverOverloaded",
    "cyberPolicy",
    "misalignmentPolicyViolation",
    "tooManyDenials",
    "internalServerError",
    "unauthorized",
    "badRequest",
    "threadRollbackFailed",
    "sandboxError",
    "other",
];
const STATUS_CLASSES: &[&str] = &[
    "httpConnectionFailed",
    "responseStreamConnectionFailed",
    "responseStreamDisconnected",
    "responseTooManyFailedAttempts",
];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedTurnErrorDiagnostic {
    kind: &'static str,
    classification: Option<&'static str>,
    http_status_code: Option<u16>,
    will_retry: Option<bool>,
}

impl OwnedTurnErrorDiagnostic {
    pub fn from_parameters(parameters: &Value) -> Self {
        let error_info = parameters.pointer("/error/codexErrorInfo");
        let mut diagnostic = Self {
            kind: "ownedTurnErrorObserved",
            classification: error_info.and_then(Value::as_str).and_then(|name| {
                STRING_CLASSES
                    .iter()
                    .chain(STATUS_CLASSES)
                    .copied()
                    .find(|known| *known == name)
            }),
            http_status_code: None,
            will_retry: parameters.get("willRetry").and_then(Value::as_bool),
        };
        if let Some(fields) = error_info.and_then(Value::as_object)
            && fields.len() == 1
            && let Some(classification) = STATUS_CLASSES
                .iter()
                .copied()
                .find(|known| fields.contains_key(*known))
        {
            diagnostic.classification = Some(classification);
            diagnostic.http_status_code = fields[classification]
                .get("httpStatusCode")
                .and_then(Value::as_u64)
                .filter(|status| (100..=599).contains(status))
                .and_then(|status| u16::try_from(status).ok());
        }
        diagnostic
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn diagnostics_preserve_known_class_status_and_retry_without_backend_text() {
        for &classification in STATUS_CLASSES {
            let parameters = json!({"willRetry":true,"error":{
                "message":"private-backend-canary", "additionalDetails":"private-details-canary",
                "codexErrorInfo":{classification:{"httpStatusCode":503,"private":"private-nested-canary"}}
            }});
            let serialized =
                serde_json::to_value(OwnedTurnErrorDiagnostic::from_parameters(&parameters))
                    .unwrap();
            assert_eq!(
                serialized,
                json!({"kind":"ownedTurnErrorObserved","classification":classification,"httpStatusCode":503,"willRetry":true})
            );
            assert!(!serialized.to_string().contains("canary"));
        }
        for classification in STRING_CLASSES {
            let parameters = json!({"willRetry":false,"error":{"codexErrorInfo":classification}});
            let serialized =
                serde_json::to_value(OwnedTurnErrorDiagnostic::from_parameters(&parameters))
                    .unwrap();
            assert_eq!(serialized["classification"], *classification);
            assert_eq!(serialized["willRetry"], false);
            assert!(serialized["httpStatusCode"].is_null());
        }
    }

    #[test]
    fn unknown_or_malformed_metadata_never_becomes_a_status_or_private_classification() {
        for status in [
            json!(null),
            json!("401 private-canary"),
            json!(-1),
            json!(0),
            json!(99),
            json!(600),
            json!(65536),
        ] {
            let parameters = json!({"error":{"codexErrorInfo":{"responseStreamDisconnected":{"httpStatusCode":status}}}});
            let serialized =
                serde_json::to_value(OwnedTurnErrorDiagnostic::from_parameters(&parameters))
                    .unwrap();
            assert!(serialized["httpStatusCode"].is_null());
        }
        for error_info in [
            json!("private-class-canary"),
            json!({"private-class-canary":{"httpStatusCode":401}}),
            json!({"responseStreamDisconnected":{"httpStatusCode":401},"private-class-canary":{}}),
        ] {
            let parameters =
                json!({"willRetry":"private-retry-canary","error":{"codexErrorInfo":error_info}});
            let serialized =
                serde_json::to_value(OwnedTurnErrorDiagnostic::from_parameters(&parameters))
                    .unwrap();
            assert!(serialized["classification"].is_null());
            assert!(serialized["httpStatusCode"].is_null());
            assert!(serialized["willRetry"].is_null());
            assert!(!serialized.to_string().contains("canary"));
        }
    }
}
