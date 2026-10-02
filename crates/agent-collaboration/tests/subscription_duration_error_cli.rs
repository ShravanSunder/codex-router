//! Real-binary validation for JSON-only subscription duration commands.
use serde_json::Value;
use std::process::Command;

const ROOT_MESSAGE_ID: &str = "018f6f67-64d2-7a21-bf9a-8f193f987001";
const HUMAN_ACTOR: &str = r#"{"kind":"human","humanId":"cli-test-human"}"#;

#[test]
fn subscription_duration_errors_name_the_field_constraint_and_next_action() {
    for (arguments, field, constraint) in [
        (
            vec![
                "board",
                "thread",
                "subscribe",
                "--root-message-id",
                ROOT_MESSAGE_ID,
                "--actor",
                HUMAN_ACTOR,
                "--quiet",
                "10x",
                "--json",
            ],
            "--quiet",
            "--quiet requires an integer followed by s, m, h, or d; for example --quiet 2m",
        ),
        (
            vec![
                "board",
                "thread",
                "subscribe",
                "--root-message-id",
                ROOT_MESSAGE_ID,
                "--actor",
                HUMAN_ACTOR,
                "--quiet",
                "31m",
                "--json",
            ],
            "--quiet",
            "--quiet must be between 0 seconds and 30 minutes; for example --quiet 2m",
        ),
        (
            vec![
                "board",
                "thread",
                "subscribe",
                "--root-message-id",
                ROOT_MESSAGE_ID,
                "--actor",
                HUMAN_ACTOR,
                "--cap",
                "10x",
                "--json",
            ],
            "--cap",
            "--cap requires an integer followed by s, m, h, or d; for example --cap 10m",
        ),
        (
            vec![
                "board",
                "thread",
                "subscribe",
                "--root-message-id",
                ROOT_MESSAGE_ID,
                "--actor",
                HUMAN_ACTOR,
                "--cap",
                "61m",
                "--json",
            ],
            "--cap",
            "--cap must be at most 60 minutes; for example --cap 10m",
        ),
        (
            vec![
                "board",
                "thread",
                "subscribe",
                "--root-message-id",
                ROOT_MESSAGE_ID,
                "--actor",
                HUMAN_ACTOR,
                "--for",
                "10x",
                "--json",
            ],
            "--for",
            "--for requires an integer followed by s, m, h, or d; for example --for 24h",
        ),
        (
            vec![
                "board",
                "thread",
                "subscribe",
                "--root-message-id",
                ROOT_MESSAGE_ID,
                "--actor",
                HUMAN_ACTOR,
                "--for",
                "0s",
                "--json",
            ],
            "--for",
            "--for must be greater than zero; for example --for 24h",
        ),
        (
            vec![
                "board",
                "thread",
                "subscribe",
                "--root-message-id",
                ROOT_MESSAGE_ID,
                "--actor",
                HUMAN_ACTOR,
                "--for",
                "8d",
                "--json",
            ],
            "--for",
            "invalid subscription forSeconds: must be between 600 seconds and 604800 seconds; for example --for 24h",
        ),
        (
            vec![
                "board",
                "thread",
                "wait",
                "--actor",
                HUMAN_ACTOR,
                "--max-wait",
                "10x",
                "--json",
            ],
            "--max-wait",
            "--max-wait requires an integer followed by s, m, h, or d; for example --max-wait 10m",
        ),
        (
            vec![
                "board",
                "thread",
                "wait",
                "--actor",
                HUMAN_ACTOR,
                "--max-wait",
                "26m",
                "--json",
            ],
            "--max-wait",
            "--max-wait must be at most 1500 seconds; for example --max-wait 10m",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
            .args(arguments)
            .output()
            .expect("invoke the real agent-collaboration binary");

        assert_eq!(output.status.code(), Some(2));
        assert!(output.stderr.is_empty());
        let response: Value = serde_json::from_slice(&output.stdout).expect("JSON error response");
        assert_eq!(
            response.pointer("/kind"),
            Some(&Value::String("error".to_owned()))
        );
        assert_eq!(
            response.pointer("/error/kind"),
            Some(&Value::String("invalidField".to_owned()))
        );
        assert_eq!(
            response.pointer("/error/field"),
            Some(&Value::String(field.to_owned()))
        );
        assert_eq!(
            response.pointer("/error/constraint"),
            Some(&Value::String(constraint.to_owned()))
        );
        assert_eq!(
            response.pointer("/error/message"),
            Some(&Value::String(constraint.to_owned()))
        );
        assert_eq!(
            response.pointer("/error/nextAction"),
            Some(&Value::String("correctRequest".to_owned()))
        );
    }
}
