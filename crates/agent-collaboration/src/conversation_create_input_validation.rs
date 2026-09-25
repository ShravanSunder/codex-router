use super::CreateArguments;

pub(super) struct CreateInputValidationFailure {
    pub(super) kind: &'static str,
    pub(super) exit_code: i32,
    pub(super) message: String,
}

pub(super) fn validate_create_endpoint_options(
    arguments: &CreateArguments,
) -> Result<(), CreateInputValidationFailure> {
    let endpoint_id = requested_endpoint_id(&arguments.endpoint);
    if endpoint_id.as_deref() == Some("codex-local") {
        let mut missing_fields = Vec::new();
        if arguments.model.is_none() {
            missing_fields.push("--model");
        }
        if arguments.effort.is_none() {
            missing_fields.push("--effort");
        }
        if !missing_fields.is_empty() {
            let example = corrected_codex_create_example(&missing_fields);
            return Err(CreateInputValidationFailure {
                kind: "invalidField",
                exit_code: 2,
                message: format!(
                    "Codex endpoints require {}; example: {example}",
                    missing_fields.join(" and ")
                ),
            });
        }
    } else if endpoint_id
        .as_deref()
        .is_some_and(|endpoint| matches!(endpoint, "claude-local" | "cursor-local"))
    {
        let mut supplied_fields = Vec::new();
        if arguments.model.is_some() {
            supplied_fields.push("--model");
        }
        if arguments.effort.is_some() {
            supplied_fields.push("--effort");
        }
        if !supplied_fields.is_empty() {
            return Err(CreateInputValidationFailure {
                kind: "unsupportedCapability",
                exit_code: 4,
                message: format!(
                    "{} is rejected for provider endpoint {}; omit these fields",
                    supplied_fields.join(" and "),
                    endpoint_id.as_deref().unwrap_or_default()
                ),
            });
        }
    }
    Ok(())
}

fn requested_endpoint_id(endpoint: &str) -> Option<String> {
    if endpoint.trim_start().starts_with('{') {
        serde_json::from_str::<serde_json::Value>(endpoint)
            .ok()?
            .pointer("/endpointId")?
            .as_str()
            .map(str::to_owned)
    } else {
        Some(endpoint.to_owned())
    }
}

fn corrected_codex_create_example(missing_fields: &[&str]) -> String {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let mut command = vec!["agent-collaboration".to_owned()];
    command.extend(arguments.iter().map(|argument| shell_quote(argument)));
    for (flag, placeholder) in [("--model", "<model>"), ("--effort", "<low|medium|high>")] {
        if missing_fields.contains(&flag) {
            command.push(flag.to_owned());
            command.push(placeholder.to_owned());
        }
    }
    command.join(" ")
}

fn shell_quote(argument: &str) -> String {
    format!("'{}'", argument.replace('\'', "'\\''"))
}
