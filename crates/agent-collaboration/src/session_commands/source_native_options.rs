//! Convert the configured launch's narrow CLI option set into its typed wire value.
use super::source_ssh_protocol::{SourceNativeOptions, SourceRejection};
use std::ffi::OsString;

pub(super) fn parse(arguments: &[OsString]) -> Result<SourceNativeOptions, SourceRejection> {
    let mut options = SourceNativeOptions::default();
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        let text = argument
            .to_str()
            .ok_or(SourceRejection::ConflictingArguments)?;
        match text {
            "--no-alt-screen" if !options.no_alt_screen => options.no_alt_screen = true,
            "--model" | "-m" => {
                let value = arguments
                    .next()
                    .and_then(|value| value.to_str())
                    .ok_or(SourceRejection::ConflictingArguments)?;
                set_once(&mut options.model, value)?;
            }
            "--config" | "-c" => {
                let value = arguments
                    .next()
                    .and_then(|value| value.to_str())
                    .ok_or(SourceRejection::ConflictingArguments)?;
                parse_assignment(&mut options, value)?;
            }
            _ => {
                if let Some(value) = text.strip_prefix("--model=") {
                    set_once(&mut options.model, value)?;
                } else if let Some(value) = text.strip_prefix("--config=") {
                    parse_assignment(&mut options, value)?;
                } else if let Some(value) =
                    text.strip_prefix("-m").filter(|value| !value.is_empty())
                {
                    set_once(&mut options.model, value.strip_prefix('=').unwrap_or(value))?;
                } else if let Some(value) =
                    text.strip_prefix("-c").filter(|value| !value.is_empty())
                {
                    parse_assignment(&mut options, value.strip_prefix('=').unwrap_or(value))?;
                } else {
                    return Err(SourceRejection::ConflictingArguments);
                }
            }
        }
    }
    options.validate()?;
    Ok(options)
}

fn set_once(slot: &mut Option<String>, value: &str) -> Result<(), SourceRejection> {
    if slot.is_some() {
        return Err(SourceRejection::ConflictingArguments);
    }
    *slot = Some(value.to_owned());
    Ok(())
}

fn parse_assignment(options: &mut SourceNativeOptions, value: &str) -> Result<(), SourceRejection> {
    let document: toml::Table = value
        .parse()
        .map_err(|_| SourceRejection::ConflictingArguments)?;
    if document.len() != 1 {
        return Err(SourceRejection::ConflictingArguments);
    }
    let Some((key, value)) = document.iter().next() else {
        return Err(SourceRejection::ConflictingArguments);
    };
    let value = value
        .as_str()
        .ok_or(SourceRejection::ConflictingArguments)?;
    match key.as_str() {
        "model" => set_once(&mut options.model, value),
        "model_reasoning_effort" => set_once(&mut options.reasoning_effort, value),
        _ => Err(SourceRejection::ConflictingArguments),
    }
}
