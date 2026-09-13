use crate::proof_context::ProofResult;
use collaboration_client::protocol::SessionRef;
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Write, os::unix::fs::OpenOptionsExt, path::Path};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OperatorTrace {
    role: &'static str,
    session: SessionRef,
    turns: Vec<Value>,
}

impl OperatorTrace {
    pub(super) fn new(role: &'static str, session: SessionRef, turns: Vec<Value>) -> Self {
        Self {
            role,
            session,
            turns,
        }
    }
}

const REQUIRED_OPERATIONS: &[(&str, &str)] = &[
    ("board/projectCreate", " board project create "),
    ("board/projectUpdate", " board project update "),
    ("board/projectShow", " board project show "),
    ("board/projectList", " board project list "),
    ("board/repositoryAttach", " board repository attach "),
    ("board/repositoryDetach", " board repository detach "),
    ("board/repositoryList", " board repository list "),
    ("board/create", " board create "),
    ("board/update", " board update "),
    ("board/show", " board show "),
    ("board/list", " board list "),
    ("board/archive", " board archive "),
    ("board/topicCreate", " board topic create "),
    ("board/topicUpdate", " board topic update "),
    ("board/topicList", " board topic list "),
    ("board/messagePost", " board message post "),
    ("board/messageShow", " board message show "),
    ("board/messageList", " board message list "),
    ("board/threadShow", " board thread show "),
    ("board/threadResolve", " board thread resolve "),
    ("board/threadUnresolve", " board thread unresolve "),
    ("board/threadWatch", " board thread watch "),
    ("board/threadUnwatch", " board thread unwatch "),
    ("board/threadList", " board thread list "),
    ("board/inboxFetch", " board inbox fetch "),
    ("board/inboxAcknowledge", " board inbox acknowledge "),
    ("board/inboxProjects", " board inbox projects "),
];

const REQUIRED_READ_VARIATIONS: &[(&str, &str)] = &[
    ("messageList.scope.topic", " --scope topic "),
    ("messageList.scope.thread", " --scope thread "),
    ("messageList.scope.board", " --scope board "),
    ("messageList.scope.project", " --scope project "),
    ("messageList.scope.allProjects", " --scope all-projects "),
    ("messageList.selection.latest", " --selection latest "),
    (
        "messageList.selection.afterPosition",
        " --selection after-position ",
    ),
    ("messageList.selection.range", " --selection range "),
    ("pagination.limit", " --limit "),
    ("pagination.cursor", " --cursor "),
    ("reference.message", " --reference-message "),
    ("reference.thread", " --reference-thread "),
    ("attribution.actingFor", " --acting-for "),
    ("boards.includeArchived", " --include-archived "),
    ("threads.watchedOnly", " --watched-only "),
    ("inbox.unreadOnly", " --unread-only "),
    ("failure.topLevelMessageCooldown", "toplevelmessagecooldown"),
    ("failure.threadResolved", "threadresolved"),
    ("failure.archivedBoard", "archivedboard"),
];

pub(super) fn observed_coverage(traces: &[OperatorTrace]) -> BTreeMap<String, usize> {
    let command_items = traces
        .iter()
        .flat_map(|trace| &trace.turns)
        .filter_map(|turn| turn.get("items").and_then(Value::as_array))
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("commandExecution"))
        .collect::<Vec<_>>();
    REQUIRED_OPERATIONS
        .iter()
        .map(|(name, pattern)| {
            let count = command_items
                .iter()
                .flat_map(|item| executable_board_invocations(item))
                .filter(|invocation| invocation.contains(pattern))
                .count();
            ((*name).to_owned(), count)
        })
        .chain(REQUIRED_READ_VARIATIONS.iter().map(|(name, pattern)| {
            let count = if name.starts_with("failure.") {
                command_items
                    .iter()
                    .filter_map(|item| item.get("aggregatedOutput"))
                    .map(all_strings)
                    .filter(|output| output.contains(pattern))
                    .count()
            } else {
                command_items
                    .iter()
                    .flat_map(|item| executable_board_invocations(item))
                    .filter(|invocation| invocation.contains(pattern))
                    .count()
            };
            ((*name).to_owned(), count)
        }))
        .collect()
}

pub(super) fn require_complete_operation_coverage(
    traces: &[OperatorTrace],
) -> ProofResult<BTreeMap<String, usize>> {
    let coverage = observed_coverage(traces);
    let missing = coverage
        .iter()
        .filter_map(|(operation, count)| (*count == 0).then_some(operation.as_str()))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "actual Luna command traces omitted required board coverage: {}",
            missing.join(", ")
        )
        .into());
    }
    Ok(coverage)
}

pub(super) fn write_session_trace(
    root: &Path,
    traces: &[OperatorTrace],
    coverage: &BTreeMap<String, usize>,
) -> ProofResult<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(root.join("session-trace.json"))?;
    serde_json::to_writer_pretty(
        &mut file,
        &json!({"operators":traces,"actualCommandCoverage":coverage}),
    )?;
    writeln!(file)?;
    Ok(())
}

fn all_strings(value: &Value) -> String {
    match value {
        Value::String(text) => format!(" {text} "),
        Value::Array(values) => values.iter().map(all_strings).collect(),
        Value::Object(values) => values.values().map(all_strings).collect(),
        _ => String::new(),
    }
    .to_lowercase()
}

fn executable_board_invocations(item: &Value) -> Vec<String> {
    let Some(command) = item.get("command") else {
        return Vec::new();
    };
    all_strings(command)
        .lines()
        .map(str::trim)
        .filter(|line| line.contains(" board "))
        .filter(|line| !line.contains(" --help") && !line.ends_with(" -h"))
        .map(|line| format!(" {line} "))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_commands_are_not_operation_evidence() {
        let item = json!({
            "command":"/bin/zsh -lc 'agent-collaboration board project create --help\nagent-collaboration board project create --name Actual'"
        });
        let invocations = executable_board_invocations(&item);
        assert_eq!(invocations.len(), 1);
        assert!(
            invocations
                .first()
                .is_some_and(|invocation| invocation.contains("board project create --name actual"))
        );
    }

    #[test]
    fn failure_evidence_comes_from_command_output() {
        let command = json!({"aggregatedOutput":"{\"error\":{\"kind\":\"threadResolved\"}}"});
        assert!(
            command
                .get("aggregatedOutput")
                .is_some_and(|output| all_strings(output).contains("threadresolved"))
        );
        assert!(
            command
                .get("command")
                .is_none_or(|value| !all_strings(value).contains("threadresolved"))
        );
    }
}
