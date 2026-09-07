//! Profile dry-run rendering with secret redaction.
use crate::profile;
use std::io::Write;

pub(super) fn write_profile_preview(
    stdout: &mut impl Write,
    preview: &profile::ProfileDryRun,
) -> Result<(), std::io::Error> {
    if let Some(existing_content) = preview.existing_content() {
        stdout.write_all(b"current:\n")?;
        write_prefixed_redacted_lines(stdout, "< ", existing_content)?;
    } else {
        stdout.write_all(b"current: <missing>\n")?;
    }
    stdout.write_all(b"proposed:\n")?;
    write_prefixed_lines(stdout, "> ", preview.content())
}

fn write_prefixed_redacted_lines(
    stdout: &mut impl Write,
    prefix: &str,
    content: &str,
) -> Result<(), std::io::Error> {
    for line in content.lines() {
        writeln!(stdout, "{prefix}{}", redact_profile_preview_line(line))?;
    }
    Ok(())
}

fn redact_profile_preview_line(line: &str) -> String {
    const SECRET_KEYS: &[&str] = &[
        "authorization",
        "bearer",
        "key",
        "oauth",
        "password",
        "secret",
        "token",
    ];

    let lower = line.to_ascii_lowercase();
    if !SECRET_KEYS.iter().any(|key| lower.contains(key)) {
        return line.to_owned();
    }
    if let Some((key, _value)) = line.split_once('=') {
        return format!("{key}= \"<redacted>\"");
    }
    "<redacted>".to_owned()
}

fn write_prefixed_lines(
    stdout: &mut impl Write,
    prefix: &str,
    content: &str,
) -> Result<(), std::io::Error> {
    for line in content.lines() {
        writeln!(stdout, "{prefix}{line}")?;
    }
    Ok(())
}
