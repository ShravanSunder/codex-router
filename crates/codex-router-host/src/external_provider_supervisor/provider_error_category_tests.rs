//! Independent closed-category/public-failure oracles; all peer payloads are synthetic.
use super::{
    ExternalProviderRuntime, ExternalProviderRuntimeError, prompt_runtime_failure, runtime_failure,
};
use collaboration_protocol::{
    ConversationOperationFailureKind as FailureKind,
    ConversationOperationFailureStage as FailureStage, OperationId, ProviderOperationEffect,
};
use serde_json::{Value, json};
use std::{fmt, path::PathBuf};
use tracing_subscriber::{filter::LevelFilter, prelude::*};

#[path = "provider_error_trace_capture.rs"]
mod trace_capture;
use trace_capture::{
    DiagnosticField, PROVIDER_ERROR_TARGET, StructuredProviderTrace, TraceCaptureError,
};

type TestResult<TValue = ()> = Result<TValue, Box<dyn std::error::Error + Send + Sync>>;
const RAW_SENTINEL: &str = "TEST_PRIVATE_ERROR_MESSAGE_SENTINEL";
const DETAIL_SENTINEL: &str = "TEST_PRIVATE_ERROR_DETAIL_SENTINEL";
const PROMPT_SENTINEL: &str = "TEST_PRIVATE_PROMPT_SENTINEL";
const PATH_SENTINEL: &str = "/synthetic/private/provider-category-path";

// These input/expected pairs are literal contract oracles, not production enum exports.
const KNOWN_KINDS: [(&str, &str); 16] = [
    ("authentication_failed", "authentication_failed"),
    ("oauth_org_not_allowed", "oauth_org_not_allowed"),
    ("account_on_hold", "account_on_hold"),
    ("verification_required", "verification_required"),
    ("billing_error", "billing_error"),
    ("rate_limit", "rate_limit"),
    ("overloaded", "overloaded"),
    ("invalid_request", "invalid_request"),
    ("model_not_found", "model_not_found"),
    ("server_error", "server_error"),
    ("unknown", "unknown"),
    ("max_output_tokens", "max_output_tokens"),
    ("cloud_credential_error", "cloud_credential_error"),
    ("worker_shutdown", "worker_shutdown"),
    ("transport_lost", "transport_lost"),
    ("no_result", "no_result"),
];

#[test]
fn all_reported_kinds_keep_generic_public_failure_and_closed_trace() -> TestResult {
    for (input_kind, expected_kind) in KNOWN_KINDS {
        let trace = StructuredProviderTrace::new(expected_kind);
        let subscriber = tracing_subscriber::registry().with(trace.clone());
        let error = tracing::subscriber::with_default(subscriber, || {
            acp_client_runtime::acp_operation_error_for_test(synthetic_error(
                -32603,
                Some(kind_data(input_kind)),
            ))
        });
        check_public_failure(error, &trace, generic_failure(), expected_kind)?;
    }
    Ok(())
}

#[test]
fn malformed_missing_nested_case_and_unknown_kinds_are_unclassified() -> TestResult {
    let malformed_data = [
        None,
        Some(Value::Null),
        Some(json!(true)),
        Some(json!(37)),
        Some(json!(2.5)),
        Some(json!("authentication_failed")),
        Some(json!([{"errorKind": "authentication_failed"}])),
        Some(json!({})),
        Some(json!({"errorKind": null})),
        Some(json!({"errorKind": true})),
        Some(json!({"errorKind": 7})),
        Some(json!({"errorKind": ["authentication_failed"]})),
        Some(json!({"errorKind": {"value": "authentication_failed"}})),
        Some(json!({"details": {"errorKind": "authentication_failed"}})),
        Some(json!({"ErrorKind": "authentication_failed"})),
        Some(json!({"errorKind": "Authentication_failed"})),
        Some(json!({"errorKind": "AUTHENTICATION_FAILED"})),
        Some(json!({"errorKind": " authentication_failed"})),
        Some(json!({"errorKind": "authentication_failed "})),
        Some(json!({"errorKind": "prefix_authentication_failed"})),
        Some(json!({"errorKind": "authentication_failed/synthetic-path"})),
        Some(json!({"errorKind": "future_provider_kind"})),
        Some(json!({"errorKind": DETAIL_SENTINEL, "details": "authentication_failed"})),
        Some(json!({"details": DETAIL_SENTINEL, "prompt": PROMPT_SENTINEL, "path": PATH_SENTINEL})),
    ];
    for data in malformed_data {
        let trace = StructuredProviderTrace::new("Unclassified");
        let subscriber = tracing_subscriber::registry().with(trace.clone());
        let error = tracing::subscriber::with_default(subscriber, || {
            acp_client_runtime::acp_operation_error_for_test(synthetic_error(-32603, data))
        });
        check_public_failure(error, &trace, generic_failure(), "Unclassified")?;
    }
    Ok(())
}

#[test]
fn all_six_existing_code_mappings_keep_closed_category_and_reference() -> TestResult {
    for (code, kind, stage, effect, explanation) in [
        (
            -32000,
            FailureKind::AuthenticationRequired,
            FailureStage::Binding,
            ProviderOperationEffect::None,
            "provider authentication is required",
        ),
        (
            -32002,
            FailureKind::NotFound,
            FailureStage::Binding,
            ProviderOperationEffect::None,
            "provider resource was not found",
        ),
        (
            -32601,
            FailureKind::UnsupportedCapability,
            FailureStage::Validation,
            ProviderOperationEffect::None,
            "provider ACP method is unsupported",
        ),
        (
            -32602,
            FailureKind::InvalidRequest,
            FailureStage::Validation,
            ProviderOperationEffect::None,
            "provider ACP parameters are invalid",
        ),
        (
            -32800,
            FailureKind::ProviderRejected,
            FailureStage::Settlement,
            ProviderOperationEffect::Unknown,
            "provider ACP request was cancelled",
        ),
        (
            -32603,
            FailureKind::ProviderRejected,
            FailureStage::Settlement,
            ProviderOperationEffect::Unknown,
            "provider rejected the operation after dispatch",
        ),
    ] {
        let trace = StructuredProviderTrace::new("authentication_failed");
        let subscriber = tracing_subscriber::registry().with(trace.clone());
        let error = tracing::subscriber::with_default(subscriber, || {
            acp_client_runtime::acp_operation_error_for_test(synthetic_error(
                code,
                Some(kind_data("authentication_failed")),
            ))
        });
        check_public_failure(
            error,
            &trace,
            ExpectedPublicFailure {
                code: i64::from(code),
                kind,
                stage,
                effect,
                explanation,
            },
            "authentication_failed",
        )?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn scripted_acp_error_categories_reach_real_runtime_and_host_projection() -> TestResult {
    for (input_kind, expected_kind) in KNOWN_KINDS
        .into_iter()
        .chain([("AUTHENTICATION_FAILED", "Unclassified")])
    {
        let trace = StructuredProviderTrace::new(expected_kind);
        let subscriber = tracing_subscriber::registry().with(trace.clone());
        let dispatch = tracing::Dispatch::new(subscriber);
        // The actual ACP actor tasks share this test's one runtime thread.
        let _capture_guard = tracing::dispatcher::set_default(&dispatch);
        let fixture =
            crate::external_provider_runtime::acp_scripted_fixture::AcpFixtureScript::new()
                .expect_request("initialize", "initialize", json!({"protocolVersion": 1}))
                .respond(
                    "initialize",
                    json!({"protocolVersion": 1, "agentCapabilities": {},
                "agentInfo": {"name": "category-fixture", "version": "1"}}),
                )
                .expect_request("create", "session/new", json!({}))
                .respond("create", json!({"sessionId": "fixture-session"}))
                .expect_request(
                    "prompt",
                    "session/prompt",
                    json!({"sessionId": "fixture-session"}),
                )
                .respond_error_with_data("prompt", -32603, kind_data(input_kind))
                .launch();
        let runtime = ExternalProviderRuntime::initialize(fixture).await?;
        let result = async {
            runtime.create_session(PathBuf::from("/tmp")).await?;
            let error = match runtime
                .prompt("fixture-session".to_owned(), PROMPT_SENTINEL.to_owned())
                .await
            {
                Err(error) => error,
                Ok(_) => return Err("scripted ACP prompt unexpectedly succeeded".into()),
            };
            check_public_failure(error, &trace, generic_failure(), expected_kind)
        }
        .await;
        finish_wire_case(&runtime, result).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn scripted_load_not_found_keeps_session_mapping_and_safe_category() -> TestResult {
    let trace = StructuredProviderTrace::new("authentication_failed");
    let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(trace.clone()));
    let _capture_guard = tracing::dispatcher::set_default(&dispatch);
    let fixture = crate::external_provider_runtime::acp_scripted_fixture::AcpFixtureScript::new()
        .expect_request("initialize", "initialize", json!({"protocolVersion": 1}))
        .respond(
            "initialize",
            json!({"protocolVersion": 1, "agentCapabilities": {"loadSession": true},
            "agentInfo": {"name": "category-load-fixture", "version": "1"}}),
        )
        .expect_request(
            "load",
            "session/load",
            json!({"sessionId": "missing-session"}),
        )
        .respond_error_with_data("load", -32002, kind_data("authentication_failed"))
        .launch();
    let runtime = ExternalProviderRuntime::initialize(fixture).await?;
    let result = async {
        let error = match runtime.load_session("missing-session".to_owned(), PathBuf::from("/tmp")).await {
            Err(error) => error,
            Ok(_) => return Err("scripted missing-session load unexpectedly succeeded".into()),
        };
        require(matches!(&error, ExternalProviderRuntimeError::ProviderSessionNotFound { code: -32002, .. }),
            "load path lost its ProviderSessionNotFound specialization")?;
        check_public_failure(error, &trace, ExpectedPublicFailure {
            code: -32002, kind: FailureKind::ProviderSessionNotFound, stage: FailureStage::Binding,
            effect: ProviderOperationEffect::None,
            explanation: "this session never started a turn and did not survive the provider restart; create a new conversation",
        }, "authentication_failed")
    }.await;
    finish_wire_case(&runtime, result).await
}

#[test]
fn incoming_transport_closed_short_circuits_category_emission() -> TestResult {
    let trace = StructuredProviderTrace::new("authentication_failed");
    let subscriber = tracing_subscriber::registry().with(trace.clone());
    let error = tracing::subscriber::with_default(subscriber, || {
        acp_client_runtime::acp_operation_error_for_test(synthetic_error(
            -32603,
            Some(json!({
                "reason": "incoming_transport_closed", "method": "session/prompt",
                "errorKind": "authentication_failed", "details": DETAIL_SENTINEL,
            })),
        ))
    });
    require(
        matches!(&error, ExternalProviderRuntimeError::TransportFailure),
        "incoming transport EOF was reclassified as a provider category",
    )?;
    require(
        trace.is_empty()?,
        "transport EOF emitted a reported-error-category event",
    )?;
    let failure = prompt_runtime_failure(OperationId::generate(), None, error);
    require(
        failure.kind == FailureKind::OutcomeUnknown
            && failure.stage == FailureStage::Settlement
            && failure.effect == ProviderOperationEffect::Unknown
            && failure.provider_code.is_none(),
        "transport EOF public failure contract changed",
    )
}

#[test]
fn disabled_debug_target_emits_no_safe_category_without_changing_error() -> TestResult {
    let trace = StructuredProviderTrace::new("authentication_failed");
    let subscriber = tracing_subscriber::registry()
        .with(trace.clone())
        .with(LevelFilter::WARN);
    let error = tracing::subscriber::with_default(subscriber, || {
        acp_client_runtime::acp_operation_error_for_test(synthetic_error(
            -32603,
            Some(kind_data("authentication_failed")),
        ))
    });
    require(
        trace.is_empty()?,
        "disabled DEBUG target emitted a safe category event",
    )?;
    require(
        matches!(
            error,
            ExternalProviderRuntimeError::ProviderRejected { code: -32603, .. }
        ),
        "debug filtering changed the runtime error",
    )
}

#[test]
fn structured_capture_rejects_any_extra_safe_event_field() -> TestResult {
    let trace = StructuredProviderTrace::new("Unclassified");
    tracing::subscriber::with_default(tracing_subscriber::registry().with(trace.clone()), || {
        tracing::debug!(target: PROVIDER_ERROR_TARGET, {
            event.name = "codex_router.acp.error_diagnostic", provider_code = -32603_i64,
            reported_error_kind = "Unclassified",
            correlation_id = "019f0000-0000-7000-8000-000000000001",
            unexpected_private_details = DETAIL_SENTINEL,
        }, "Observed ACP reported error category");
    });
    require(
        trace.single() == Err(TraceCaptureError::UnexpectedField),
        "safe-event visitor silently ignored a new private field",
    )
}

#[test]
fn structured_capture_requires_i64_provider_code() -> TestResult {
    let trace = StructuredProviderTrace::new("Unclassified");
    tracing::subscriber::with_default(tracing_subscriber::registry().with(trace.clone()), || {
        tracing::debug!(target: PROVIDER_ERROR_TARGET, {
            event.name = "codex_router.acp.error_diagnostic", provider_code = "-32603",
            reported_error_kind = "Unclassified",
            correlation_id = "019f0000-0000-7000-8000-000000000001",
        }, "Observed ACP reported error category");
    });
    require(
        trace.single() == Err(TraceCaptureError::InvalidField(DiagnosticField::Code)),
        "safe-event visitor accepted a non-i64 provider code",
    )
}

fn synthetic_error(code: i32, data: Option<Value>) -> agent_client_protocol::Error {
    let mut error = agent_client_protocol::Error::new(code, RAW_SENTINEL);
    error.message = format!("authentication_failed {RAW_SENTINEL}");
    error.data = data;
    error
}
fn kind_data(kind: &str) -> Value {
    json!({"errorKind": kind, "details": DETAIL_SENTINEL,
        "nested": {"prompt": PROMPT_SENTINEL, "path": PATH_SENTINEL}})
}
struct ExpectedPublicFailure {
    code: i64,
    kind: FailureKind,
    stage: FailureStage,
    effect: ProviderOperationEffect,
    explanation: &'static str,
}
fn generic_failure() -> ExpectedPublicFailure {
    ExpectedPublicFailure {
        code: -32603,
        kind: FailureKind::ProviderRejected,
        stage: FailureStage::Settlement,
        effect: ProviderOperationEffect::Unknown,
        explanation: "provider rejected the operation after dispatch",
    }
}
fn check_public_failure(
    error: ExternalProviderRuntimeError,
    trace: &StructuredProviderTrace,
    expected: ExpectedPublicFailure,
    expected_kind: &'static str,
) -> TestResult {
    let (code, reference) = match &error {
        ExternalProviderRuntimeError::AuthenticationRequired {
            code,
            correlation_id,
        }
        | ExternalProviderRuntimeError::ProviderSessionNotFound {
            code,
            correlation_id,
        }
        | ExternalProviderRuntimeError::ResourceNotFound {
            code,
            correlation_id,
        }
        | ExternalProviderRuntimeError::UnsupportedMethod {
            code,
            correlation_id,
        }
        | ExternalProviderRuntimeError::InvalidParams {
            code,
            correlation_id,
        }
        | ExternalProviderRuntimeError::RequestCancelled {
            code,
            correlation_id,
        }
        | ExternalProviderRuntimeError::ProviderRejected {
            code,
            correlation_id,
        } => (*code, correlation_id.to_string()),
        _ => return Err("expected a correlated coded ACP failure".into()),
    };
    require(
        code == expected.code,
        "runtime error lost the exact safe provider code",
    )?;
    let runtime_message = error.to_string();
    let operation_id = OperationId::generate();
    let failure = runtime_failure(operation_id.clone(), None, error);
    require(
        failure.kind == expected.kind
            && failure.stage == expected.stage
            && failure.effect == expected.effect
            && failure.provider_code == Some(expected.code)
            && failure.operation_id == Some(operation_id),
        "closed Host failure contract changed",
    )?;
    let message = String::from(failure.message);
    require(
        message
            == format!(
                "{} (provider code {}; reference {})",
                expected.explanation, expected.code, reference
            ),
        "Host failure was not the exact safe explanation/code/reference",
    )?;
    for sentinel in [
        RAW_SENTINEL,
        DETAIL_SENTINEL,
        PROMPT_SENTINEL,
        PATH_SENTINEL,
        "private provider text",
        "secret sentinel",
    ] {
        require(
            !runtime_message.contains(sentinel) && !message.contains(sentinel),
            "synthetic private payload escaped into runtime or Host failure",
        )?;
    }
    // Old source reaches and verifies the real Host projection before failing the new event oracle.
    let observed = trace.single()?;
    require(
        observed.reported_error_kind == expected_kind && observed.provider_code == expected.code,
        "structured diagnostic lost its literal category or exact safe code",
    )?;
    require(
        uuid::Uuid::parse_str(&reference)? == observed.correlation_id,
        "structured diagnostic lost exact runtime correlation",
    )?;
    Ok(())
}
fn require(condition: bool, message: &'static str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(std::io::Error::other(message).into())
    }
}
async fn finish_wire_case(runtime: &ExternalProviderRuntime, result: TestResult) -> TestResult {
    runtime.shutdown().await;
    match (result, runtime.shutdown_failed()) {
        (Ok(()), false) => Ok(()),
        (Err(error), false) => Err(error),
        (Ok(()), true) => Err("scripted ACP runtime shutdown failed".into()),
        (Err(error), true) => Err(Box::new(WireCaseShutdownFailure { scenario: error })),
    }
}
#[derive(Debug)]
struct WireCaseShutdownFailure {
    scenario: Box<dyn std::error::Error + Send + Sync>,
}
impl fmt::Display for WireCaseShutdownFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("scripted ACP scenario and awaited runtime shutdown both failed")
    }
}
impl std::error::Error for WireCaseShutdownFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.scenario.as_ref())
    }
}
