use std::collections::BTreeMap;

use codex_router_core::attempt_outcome::AttemptOutcome;
use codex_router_core::attempt_outcome::PassThroughReason;
use codex_router_core::route_profile::WindowKind;
use serde::Deserialize;
use serde_json::Value;

use super::classify;
use crate::claude_edge::forward::ErrorBodyEvidence;
use crate::headers::Header;
use crate::headers::HeaderCollection;

#[derive(Deserialize)]
struct ResponseFixture {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Value,
}

fn classify_fixture(fixture_json: &str) -> AttemptOutcome {
    let fixture: ResponseFixture = serde_json::from_str(fixture_json)
        .unwrap_or_else(|error| panic!("response fixture must be valid JSON: {error}"));
    let headers = HeaderCollection::new(
        fixture
            .headers
            .into_iter()
            .map(|(name, value)| Header::new(name, value))
            .collect(),
    );
    let body = serde_json::to_vec(&fixture.body)
        .unwrap_or_else(|error| panic!("fixture body must serialize: {error}"));

    classify(
        fixture.status,
        &headers,
        ErrorBodyEvidence {
            prefix: &body,
            complete: true,
        },
    )
}

#[test]
fn claude_classifier_exhausts_shared_five_hour_representative_window() {
    assert_eq!(
        classify_fixture(include_str!(
            "../../../tests/fixtures/claude/shared_five_hour_rejection.json"
        )),
        AttemptOutcome::SharedWindowExhausted {
            windows: vec![WindowKind::FiveHour],
            resets: vec![Some(1_764_554_400)],
        }
    );
}

#[test]
fn claude_classifier_exhausts_shared_weekly_representative_window() {
    assert_eq!(
        classify_fixture(include_str!(
            "../../../tests/fixtures/claude/shared_weekly_rejection.json"
        )),
        AttemptOutcome::SharedWindowExhausted {
            windows: vec![WindowKind::Weekly],
            resets: vec![Some(1_764_615_600)],
        }
    );
}

#[test]
fn claude_classifier_rejects_expired_oauth_credential() {
    assert_eq!(
        classify_fixture(include_str!(
            "../../../tests/fixtures/claude/oauth_credential_rejection.json"
        )),
        AttemptOutcome::CredentialRejected
    );
}

#[test]
fn claude_classifier_passes_through_capability_header_401() {
    assert_eq!(
        classify_fixture(include_str!(
            "../../../tests/fixtures/claude/capability_authentication_401.json"
        )),
        AttemptOutcome::PassThrough(PassThroughReason::RequestRejected)
    );
}

#[test]
fn claude_classifier_passes_through_overage_only_rejection() {
    assert_eq!(
        classify_fixture(include_str!(
            "../../../tests/fixtures/claude/overage_only_rejection.json"
        )),
        AttemptOutcome::PassThrough(PassThroughReason::ModelOrOverageLimit)
    );
}

#[test]
fn claude_classifier_passes_through_model_only_rejection() {
    assert_eq!(
        classify_fixture(include_str!(
            "../../../tests/fixtures/claude/model_only_rejection.json"
        )),
        AttemptOutcome::PassThrough(PassThroughReason::ModelOrOverageLimit)
    );
}

#[test]
fn claude_classifier_passes_through_bare_429() {
    assert_eq!(
        classify_fixture(include_str!("../../../tests/fixtures/claude/bare_429.json")),
        AttemptOutcome::PassThrough(PassThroughReason::UnattributedLimit)
    );
}

#[test]
fn claude_classifier_requires_complete_explicit_oauth_evidence() {
    let fixture: ResponseFixture = serde_json::from_str(include_str!(
        "../../../tests/fixtures/claude/oauth_credential_rejection.json"
    ))
    .unwrap_or_else(|error| panic!("response fixture must be valid JSON: {error}"));
    let headers = HeaderCollection::new(
        fixture
            .headers
            .into_iter()
            .map(|(name, value)| Header::new(name, value))
            .collect(),
    );
    let body = serde_json::to_vec(&fixture.body)
        .unwrap_or_else(|error| panic!("fixture body must serialize: {error}"));

    assert_eq!(
        classify(
            fixture.status,
            &headers,
            ErrorBodyEvidence {
                prefix: &body,
                complete: false,
            },
        ),
        AttemptOutcome::PassThrough(PassThroughReason::RequestRejected)
    );
}

#[test]
fn claude_classifier_passes_through_conflicting_complete_credential_and_window_evidence() {
    let mut quota_fixture: ResponseFixture = serde_json::from_str(include_str!(
        "../../../tests/fixtures/claude/shared_five_hour_rejection.json"
    ))
    .unwrap_or_else(|error| panic!("quota fixture must be valid JSON: {error}"));
    let credential_fixture: ResponseFixture = serde_json::from_str(include_str!(
        "../../../tests/fixtures/claude/oauth_credential_rejection.json"
    ))
    .unwrap_or_else(|error| panic!("credential fixture must be valid JSON: {error}"));
    quota_fixture.body = credential_fixture.body;

    let headers = HeaderCollection::new(
        quota_fixture
            .headers
            .into_iter()
            .map(|(name, value)| Header::new(name, value))
            .collect(),
    );
    let body = serde_json::to_vec(&quota_fixture.body)
        .unwrap_or_else(|error| panic!("fixture body must serialize: {error}"));

    assert_eq!(
        classify(
            quota_fixture.status,
            &headers,
            ErrorBodyEvidence {
                prefix: &body,
                complete: true,
            },
        ),
        AttemptOutcome::PassThrough(PassThroughReason::MalformedEvidence)
    );
}

#[test]
fn claude_classifier_requires_unified_rejection_and_shared_representative_claim() {
    let fixture: ResponseFixture = serde_json::from_str(include_str!(
        "../../../tests/fixtures/claude/shared_five_hour_rejection.json"
    ))
    .unwrap_or_else(|error| panic!("response fixture must be valid JSON: {error}"));

    for (header_name, header_value) in [
        ("anthropic-ratelimit-unified-status", "allowed"),
        (
            "anthropic-ratelimit-unified-representative-claim",
            "unknown_window",
        ),
    ] {
        let mut headers = fixture.headers.clone();
        headers.insert(header_name.to_owned(), header_value.to_owned());
        let headers = HeaderCollection::new(
            headers
                .into_iter()
                .map(|(name, value)| Header::new(name, value))
                .collect(),
        );
        let body = serde_json::to_vec(&fixture.body)
            .unwrap_or_else(|error| panic!("fixture body must serialize: {error}"));

        assert_eq!(
            classify(
                fixture.status,
                &headers,
                ErrorBodyEvidence {
                    prefix: &body,
                    complete: true,
                },
            ),
            AttemptOutcome::PassThrough(PassThroughReason::MalformedEvidence)
        );
    }
}
