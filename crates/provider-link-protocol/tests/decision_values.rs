use provider_link_protocol::{LinkApprovalOutcome, LinkDecision};
use serde::{Deserialize, Serialize};
use session_event_model::{QuestionAnswerValue, QuestionResponse};
use std::{collections::BTreeMap, fmt::Debug};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn standard_unit_leftovers_match_the_reused_codec_without_a_new_strictness_policy() -> TestResult {
    for (wire, expected) in [
        (
            r#"{"outcome":"cancelled","extra":0}"#,
            LinkApprovalOutcome::Cancelled,
        ),
        (
            r#"{"outcome":"unavailable","extra":0}"#,
            LinkApprovalOutcome::Unavailable,
        ),
    ] {
        if serde_json::from_str::<LinkApprovalOutcome>(wire)? != expected {
            return Err("approval unit leftover behavior diverged from standard serde".into());
        }
    }
    for (wire, expected) in [
        (
            r#"{"decision":"approval","outcome":"cancelled","action":"declined"}"#,
            LinkDecision::Approval(LinkApprovalOutcome::Cancelled),
        ),
        (
            r#"{"decision":"approval","outcome":"unavailable","extra":0}"#,
            LinkDecision::Approval(LinkApprovalOutcome::Unavailable),
        ),
        (
            r#"{"decision":"question","action":"declined","outcome":"cancelled"}"#,
            LinkDecision::Question(QuestionResponse::Declined),
        ),
        (
            r#"{"decision":"question","action":"cancelled","extra":0}"#,
            LinkDecision::Question(QuestionResponse::Cancelled),
        ),
    ] {
        if serde_json::from_str::<LinkDecision>(wire)? != expected {
            return Err("wrapped unit leftover behavior changed the selected decision".into());
        }
    }
    for (wire, expected) in [
        (
            r#"{"action":"declined","outcome":"cancelled"}"#,
            QuestionResponse::Declined,
        ),
        (
            r#"{"action":"cancelled","extra":0}"#,
            QuestionResponse::Cancelled,
        ),
    ] {
        if serde_json::from_str::<QuestionResponse>(wire)? != expected {
            return Err("reused source question unit behavior differs".into());
        }
    }
    Ok(())
}

fn literal_codec<TValue>(value: &TValue, wire: &str) -> TestResult
where
    TValue: Serialize + for<'de> Deserialize<'de> + PartialEq + Debug,
{
    let encoded = serde_json::to_string(value)?;
    let decoded = serde_json::from_str::<TValue>(wire)?;
    if encoded != wire || &decoded != value {
        return Err(format!(
            "literal codec differs: expected={wire}, encoded={encoded}, decoded={decoded:?}"
        )
        .into());
    }
    Ok(())
}

#[test]
fn approval_outcomes_have_distinct_literal_tags_and_exact_selected_bytes() -> TestResult {
    for (outcome, wire) in [
        (
            LinkApprovalOutcome::Selected {
                option_id: "  option/β  ".into(),
                note: Some("  note  ".into()),
            },
            r#"{"outcome":"selected","optionId":"  option/β  ","note":"  note  "}"#,
        ),
        (
            LinkApprovalOutcome::Selected {
                option_id: "".into(),
                note: None,
            },
            r#"{"outcome":"selected","optionId":"","note":null}"#,
        ),
        (
            LinkApprovalOutcome::Selected {
                option_id: "".into(),
                note: Some("".into()),
            },
            r#"{"outcome":"selected","optionId":"","note":""}"#,
        ),
        (LinkApprovalOutcome::Cancelled, r#"{"outcome":"cancelled"}"#),
        (
            LinkApprovalOutcome::Unavailable,
            r#"{"outcome":"unavailable"}"#,
        ),
    ] {
        literal_codec(&outcome, wire)?;
    }
    if LinkApprovalOutcome::Unavailable == LinkApprovalOutcome::Cancelled {
        return Err("unavailable was conflated with cancellation".into());
    }
    Ok(())
}

#[test]
fn selected_missing_note_and_explicit_null_share_the_existing_optional_contract() -> TestResult {
    let expected = LinkApprovalOutcome::Selected {
        option_id: "original id".into(),
        note: None,
    };
    for wire in [
        r#"{"outcome":"selected","optionId":"original id"}"#,
        r#"{"outcome":"selected","optionId":"original id","note":null}"#,
    ] {
        if serde_json::from_str::<LinkApprovalOutcome>(wire)? != expected {
            return Err("optional note absence/null changed the decision".into());
        }
    }
    if serde_json::to_string(&expected)?
        != r#"{"outcome":"selected","optionId":"original id","note":null}"#
    {
        return Err("None note did not serialize as explicit null".into());
    }
    let expected = LinkDecision::Approval(expected);
    for wire in [
        r#"{"decision":"approval","outcome":"selected","optionId":"original id"}"#,
        r#"{"decision":"approval","outcome":"selected","optionId":"original id","note":null}"#,
    ] {
        if serde_json::from_str::<LinkDecision>(wire)? != expected {
            return Err("wrapped optional note changed the decision".into());
        }
    }
    Ok(())
}

#[test]
fn approval_decisions_combine_tags_without_copying_or_collapsing_outcomes() -> TestResult {
    for (decision, wire) in [
        (
            LinkDecision::Approval(LinkApprovalOutcome::Selected {
                option_id: " exact id ".into(),
                note: Some(" yes ".into()),
            }),
            r#"{"decision":"approval","outcome":"selected","optionId":" exact id ","note":" yes "}"#,
        ),
        (
            LinkDecision::Approval(LinkApprovalOutcome::Selected {
                option_id: "".into(),
                note: None,
            }),
            r#"{"decision":"approval","outcome":"selected","optionId":"","note":null}"#,
        ),
        (
            LinkDecision::Approval(LinkApprovalOutcome::Cancelled),
            r#"{"decision":"approval","outcome":"cancelled"}"#,
        ),
        (
            LinkDecision::Approval(LinkApprovalOutcome::Unavailable),
            r#"{"decision":"approval","outcome":"unavailable"}"#,
        ),
    ] {
        literal_codec(&decision, wire)?;
    }
    Ok(())
}

#[test]
fn question_unit_decisions_reuse_the_existing_action_codec() -> TestResult {
    for (response, inner, wire) in [
        (
            QuestionResponse::Declined,
            r#"{"action":"declined"}"#,
            r#"{"decision":"question","action":"declined"}"#,
        ),
        (
            QuestionResponse::Cancelled,
            r#"{"action":"cancelled"}"#,
            r#"{"decision":"question","action":"cancelled"}"#,
        ),
    ] {
        literal_codec(&response, inner)?;
        literal_codec(&LinkDecision::Question(response), wire)?;
    }
    Ok(())
}

#[test]
fn answered_question_preserves_order_and_original_typed_answer_values() -> TestResult {
    let content = BTreeMap::from([
        ("aBoolean".into(), QuestionAnswerValue::Boolean(false)),
        (
            "bNumber".into(),
            QuestionAnswerValue::Number(serde_json::Number::from(-7)),
        ),
        (
            "cSelection".into(),
            QuestionAnswerValue::SelectedOptions {
                selected_option_ids: vec!["b".into(), " a ".into(), "b".into()],
            },
        ),
        ("dText".into(), QuestionAnswerValue::Text(" true ".into())),
    ]);
    let response = QuestionResponse::Answered { content };
    literal_codec(
        &response,
        r#"{"action":"answered","content":{"aBoolean":false,"bNumber":-7,"cSelection":{"selectedOptionIds":["b"," a ","b"]},"dText":" true "}}"#,
    )?;
    literal_codec(
        &LinkDecision::Question(response),
        r#"{"decision":"question","action":"answered","content":{"aBoolean":false,"bNumber":-7,"cSelection":{"selectedOptionIds":["b"," a ","b"]},"dText":" true "}}"#,
    )
}

#[test]
fn answered_question_keeps_fractional_numbers_and_empty_content_without_new_policy() -> TestResult {
    let response = QuestionResponse::Answered {
        content: BTreeMap::from([(
            "number".into(),
            QuestionAnswerValue::Number(
                serde_json::Number::from_f64(1.25).ok_or("finite fixture number absent")?,
            ),
        )]),
    };
    literal_codec(
        &LinkDecision::Question(response),
        r#"{"decision":"question","action":"answered","content":{"number":1.25}}"#,
    )?;
    literal_codec(
        &LinkDecision::Question(QuestionResponse::Answered {
            content: BTreeMap::new(),
        }),
        r#"{"decision":"question","action":"answered","content":{}}"#,
    )
}

#[test]
fn approval_codec_rejects_unknown_tags_missing_required_id_and_wrong_types() -> TestResult {
    for wire in [
        r#"{"outcome":"unknown"}"#,
        r#"{"outcome":"Selected","optionId":"x","note":null}"#,
        r#"{"outcome":"selected","note":null}"#,
        r#"{"outcome":"selected"}"#,
        r#"{"outcome":"selected","optionId":null,"note":null}"#,
        r#"{"outcome":"selected","optionId":7,"note":null}"#,
        r#"{"outcome":"selected","optionId":"x","note":true}"#,
        r#"{"outcome":"selected","option_id":"x","note":null}"#,
        r#"{"outcome":"selected","optionId":"x","note":null,"extra":0}"#,
        r#""cancelled""#,
        "null",
        "0",
    ] {
        if serde_json::from_str::<LinkApprovalOutcome>(wire).is_ok() {
            return Err(format!("approval codec accepted invalid literal {wire}").into());
        }
    }
    Ok(())
}

#[test]
fn decision_codec_rejects_unknown_or_mismatched_discriminants_and_payloads() -> TestResult {
    for wire in [
        r#"{"decision":"unknown"}"#,
        r#"{"decision":"Approval","outcome":"cancelled"}"#,
        r#"{"decision":"approval"}"#,
        r#"{"decision":"approval","outcome":"selected","note":null}"#,
        r#"{"decision":"approval","action":"cancelled"}"#,
        r#"{"decision":"question","outcome":"cancelled"}"#,
        r#"{"decision":"question","action":"selected","optionId":"x"}"#,
        r#"{"decision":"question","action":"answered"}"#,
        r#"{"decision":"question","action":"answered","content":[]}"#,
        r#"{"decision":"question","action":"answered","content":{"choice":{"selectedOptionIds":[7]}}}"#,
        "null",
        "[]",
        r#""approval""#,
    ] {
        if serde_json::from_str::<LinkDecision>(wire).is_ok() {
            return Err(format!("decision codec accepted invalid literal {wire}").into());
        }
    }
    Ok(())
}
