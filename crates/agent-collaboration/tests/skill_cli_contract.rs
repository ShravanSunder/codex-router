//! The canonical skill may never document a command or flag the CLI does not have.
//!
//! `agent-skills/agent-collaboration/` is the text agents follow verbatim. A flag that
//! drifts out of the CLI turns into an invented argument at the other end, so every
//! invocation the skill prints is replayed here against the real `--help` surface.
#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const CLI_NAME: &str = "agent-collaboration";

    /// Closed flag vocabularies the skill states in prose or examples.
    ///
    /// Hard-coded rather than scraped so a reworded sentence cannot silently drop a
    /// value. Sources: `references/message-board.md` (`--role`, `--lifetime`,
    /// `--deliver`), `references/session-messaging.md` (`--access`, `--source`).
    const CLOSED_FLAG_VALUES: &[(&str, &[&str])] = &[
        (
            "--role",
            &[
                "orchestrator",
                "implementer",
                "advisor",
                "reviewer",
                "participant",
            ],
        ),
        ("--lifetime", &["short", "long"]),
        ("--deliver", &["stdout", "session"]),
        ("--access", &["write-restricted", "workspace-write"]),
        ("--source", &["interactive", "subagents", "all"]),
    ];

    #[derive(Clone, Debug)]
    struct SkillInvocation {
        file: String,
        line_number: usize,
        line: String,
        subcommand_path: Vec<String>,
        flags: Vec<String>,
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum TokenShape {
        /// A bare word that may extend the subcommand path.
        Word,
        /// A flag, a shell variable, a placeholder, or a quoted value.
        Opaque,
    }

    fn skill_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../agent-skills/agent-collaboration")
    }

    fn skill_markdown_files() -> Vec<PathBuf> {
        let root = skill_root();
        let mut files = vec![root.join("SKILL.md")];
        let mut references = std::fs::read_dir(root.join("references"))
            .unwrap_or_else(|error| panic!("read references: {error}"))
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
            .collect::<Vec<_>>();
        references.sort();
        files.append(&mut references);
        files
    }

    /// Every shell line the skill prints: fenced `sh` blocks plus inline code spans.
    ///
    /// Backslash continuations inside a fenced block are joined so a wrapped example is
    /// checked as the one invocation it is.
    fn command_lines(contents: &str) -> Vec<(usize, String)> {
        let mut lines = Vec::new();
        let mut fence_language: Option<String> = None;
        let mut pending: Option<(usize, String)> = None;
        for (index, raw_line) in contents.lines().enumerate() {
            let line_number = index + 1;
            if let Some(rest) = raw_line.trim_start().strip_prefix("```") {
                if fence_language.is_some() {
                    if let Some(pending) = pending.take() {
                        lines.push(pending);
                    }
                    fence_language = None;
                } else {
                    fence_language = Some(rest.trim().to_owned());
                }
                continue;
            }
            if fence_language.as_deref() == Some("sh") {
                let trimmed = raw_line.trim();
                let (body, continues) = match trimmed.strip_suffix('\\') {
                    Some(body) => (body.trim_end(), true),
                    None => (trimmed, false),
                };
                let (start, mut joined) = pending
                    .take()
                    .unwrap_or_else(|| (line_number, String::new()));
                if !joined.is_empty() {
                    joined.push(' ');
                }
                joined.push_str(body);
                if continues {
                    pending = Some((start, joined));
                } else {
                    lines.push((start, joined));
                }
                continue;
            }
            for span in inline_code_spans(raw_line) {
                lines.push((line_number, span));
            }
        }
        if let Some(pending) = pending {
            lines.push(pending);
        }
        lines
    }

    fn inline_code_spans(line: &str) -> Vec<String> {
        let mut spans = Vec::new();
        let mut current: Option<String> = None;
        for character in line.chars() {
            match (character, current.as_mut()) {
                ('`', None) => current = Some(String::new()),
                ('`', Some(_)) => {
                    if let Some(span) = current.take() {
                        spans.push(span);
                    }
                }
                (character, Some(span)) => span.push(character),
                _ => {}
            }
        }
        spans
    }

    /// Splits a shell line into tokens, treating quoted strings and `$VARS` as opaque.
    fn split_shell_tokens(line: &str) -> Vec<(String, TokenShape)> {
        let mut tokens = Vec::new();
        let mut current = String::new();
        let mut shape = TokenShape::Word;
        let mut quote: Option<char> = None;
        for character in line.chars() {
            match quote {
                Some(open) if character == open => {
                    quote = None;
                    shape = TokenShape::Opaque;
                }
                Some(_) => current.push(character),
                None => match character {
                    '"' | '\'' => {
                        quote = Some(character);
                        shape = TokenShape::Opaque;
                    }
                    character if character.is_whitespace() => {
                        if !current.is_empty() || shape == TokenShape::Opaque {
                            tokens.push((std::mem::take(&mut current), shape));
                            shape = TokenShape::Word;
                        }
                    }
                    character => {
                        if matches!(character, '-' | '$' | '<') && current.is_empty() {
                            shape = TokenShape::Opaque;
                        }
                        current.push(character);
                    }
                },
            }
        }
        if !current.is_empty() || shape == TokenShape::Opaque {
            tokens.push((current, shape));
        }
        tokens
    }

    fn parse_invocation(file: &str, line_number: usize, line: &str) -> Option<SkillInvocation> {
        let tokens = split_shell_tokens(line);
        let (first, first_shape) = tokens.first()?;
        if first != CLI_NAME || *first_shape != TokenShape::Word {
            return None;
        }
        let rest = &tokens[1..];
        let subcommand_path = rest
            .iter()
            .take_while(|(token, shape)| {
                *shape == TokenShape::Word && !token.is_empty() && !token.starts_with('-')
            })
            .map(|(token, _)| token.clone())
            .collect::<Vec<_>>();
        let flags = rest
            .iter()
            .filter(|(token, shape)| *shape == TokenShape::Opaque && token.starts_with("--"))
            .map(|(token, _)| {
                token
                    .split_once('=')
                    .map_or(token.clone(), |(flag, _)| flag.to_owned())
            })
            .filter(|flag| flag.len() > 2)
            .collect::<Vec<_>>();
        Some(SkillInvocation {
            file: file.to_owned(),
            line_number,
            line: line.to_owned(),
            subcommand_path,
            flags,
        })
    }

    fn skill_invocations() -> Vec<SkillInvocation> {
        let mut invocations = Vec::new();
        for path in skill_markdown_files() {
            let contents = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            let file = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("unknown")
                .to_owned();
            for (line_number, line) in command_lines(&contents) {
                if let Some(invocation) = parse_invocation(&file, line_number, &line) {
                    invocations.push(invocation);
                }
            }
        }
        invocations
    }

    fn help_text(subcommand_path: &[String]) -> (bool, String) {
        let output = Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
            .args(subcommand_path)
            .arg("--help")
            .output()
            .unwrap_or_else(|error| panic!("run CLI help: {error}"));
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push('\n');
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        (output.status.success(), text)
    }

    /// A flag counts as present only as a whole word, so `--to` never matches `--topic-id`.
    fn help_documents_flag(help: &str, flag: &str) -> bool {
        help.match_indices(flag).any(|(index, _)| {
            let after = help
                .get(index + flag.len()..)
                .and_then(|rest| rest.chars().next());
            matches!(
                after,
                None | Some(' ' | '\n' | '\t' | '=' | '<' | ',' | ']')
            )
        })
    }

    #[test]
    fn canonical_skill_documents_only_commands_and_flags_the_cli_has() {
        // Arrange
        let invocations = skill_invocations();
        assert!(
            invocations.len() >= 20,
            "the skill parser found only {} invocations; the extractor is broken",
            invocations.len()
        );
        let mut flags_by_path: BTreeMap<Vec<String>, BTreeSet<String>> = BTreeMap::new();
        for invocation in &invocations {
            flags_by_path
                .entry(invocation.subcommand_path.clone())
                .or_default()
                .extend(invocation.flags.iter().cloned());
        }

        // Act
        let mut failures = Vec::new();
        let mut help_by_path = BTreeMap::new();
        for (subcommand_path, flags) in &flags_by_path {
            let (succeeded, help) = help_text(subcommand_path);
            if !succeeded {
                failures.push(format!(
                    "`{CLI_NAME} {} --help` failed; the skill documents a subcommand the CLI does not have",
                    subcommand_path.join(" ")
                ));
                continue;
            }
            for flag in flags {
                if !help_documents_flag(&help, flag) {
                    let source = invocations
                        .iter()
                        .find(|invocation| {
                            &invocation.subcommand_path == subcommand_path
                                && invocation.flags.contains(flag)
                        })
                        .map(|invocation| {
                            format!(
                                "{}:{}: {}",
                                invocation.file, invocation.line_number, invocation.line
                            )
                        })
                        .unwrap_or_default();
                    failures.push(format!(
                        "`{CLI_NAME} {}` does not accept `{flag}`\n      documented at {source}",
                        subcommand_path.join(" ")
                    ));
                }
            }
            help_by_path.insert(subcommand_path.clone(), help);
        }

        // Act: closed vocabularies, only where this revision of the skill uses the flag.
        let mut undocumented_closed_flags = Vec::new();
        for (flag, values) in CLOSED_FLAG_VALUES {
            let paths = flags_by_path
                .iter()
                .filter(|(_, flags)| flags.contains(*flag))
                .map(|(path, _)| path.clone())
                .collect::<Vec<_>>();
            if paths.is_empty() {
                undocumented_closed_flags.push(*flag);
                continue;
            }
            for path in paths {
                let Some(help) = help_by_path.get(&path) else {
                    continue;
                };
                for value in *values {
                    if !help.contains(value) {
                        failures.push(format!(
                            "`{CLI_NAME} {}` help does not offer `{flag} {value}`",
                            path.join(" ")
                        ));
                    }
                }
            }
        }

        // Assert
        println!(
            "checked {} skill invocations across {} subcommand paths",
            invocations.len(),
            flags_by_path.len()
        );
        if !undocumented_closed_flags.is_empty() {
            println!(
                "closed flags absent from this skill revision, not checked: {}",
                undocumented_closed_flags.join(", ")
            );
        }
        assert!(
            failures.is_empty(),
            "the canonical skill documents surface the CLI does not have:\n  - {}",
            failures.join("\n  - ")
        );
    }

    #[test]
    fn flag_detection_matches_whole_words_and_rejects_absent_flags() {
        // Arrange: real help for a real path, so the detector is proved against it.
        let (succeeded, help) = help_text(&["board".to_owned(), "thread".to_owned()]);

        // Assert
        assert!(succeeded, "`{CLI_NAME} board thread --help` must succeed");
        assert!(help_documents_flag(&help, "--help"));
        assert!(
            !help_documents_flag(&help, "--not-a-real-flag"),
            "a fabricated flag must not be reported as documented"
        );
        // A prefix must never satisfy a longer flag, or the pin would pass on drift.
        assert!(!help_documents_flag("--topic-id <ID>", "--to"));
        assert!(help_documents_flag("--to <ADDRESS>", "--to"));
        assert!(help_documents_flag("use --json\n", "--json"));
    }

    #[test]
    fn the_extractor_reads_fenced_blocks_inline_spans_and_continuations() {
        // Arrange
        let markdown = "Run `agent-collaboration board --help` first.\n\n             ```sh\n             agent-collaboration board message post --placement thread \\\n               --root-message-id \"$ROOT_ID\" --actor self --json\n             ```\n\n             ```json\n             agent-collaboration board fake --ignored\n             ```\n";

        // Act
        let invocations = command_lines(markdown)
            .into_iter()
            .filter_map(|(line_number, line)| parse_invocation("fixture.md", line_number, &line))
            .collect::<Vec<_>>();

        // Assert
        assert_eq!(invocations.len(), 2, "the json fence must be ignored");
        assert_eq!(invocations[0].subcommand_path, ["board"]);
        assert_eq!(invocations[0].flags, ["--help"]);
        assert_eq!(
            invocations[1].subcommand_path,
            ["board", "message", "post"],
            "the subcommand path stops at the first flag"
        );
        assert_eq!(
            invocations[1].flags,
            ["--placement", "--root-message-id", "--actor", "--json"],
            "a backslash continuation is one invocation"
        );
    }
}
