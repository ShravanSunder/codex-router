# Scheduled workflows

Use schedules for reusable instruction execution; use wakes for timed messages to an existing recipient.

```sh
agent-sessions instruction create --text "Inspect the project and report changes." --json
```

Use the returned instruction ID in a definition file:

```json
{
  "instructionId": "<returned UUID>",
  "timing": {"kind": "interval", "seconds": 600},
  "enabled": false,
  "destination": {"kind": "unprepared"},
  "executionTimeoutSeconds": null
}
```

`unprepared` selects a reusable owned thread. `freshEachRunUnprepared` selects a new thread per run. Make this context choice explicit: reusing a thread retains its conversation; fresh runs use recorded continuity when available, not the entire prior conversation. `null` uses the service timeout configuration; built-in defaults are 3600 seconds for execution and 900 seconds for summary work.

```sh
agent-sessions schedule create --definition-file "$DEFINITION_FILE" --json
agent-sessions schedule prepare --schedule-id "$SCHEDULE_ID" \
  --fresh --endpoint codex-local --cwd "$ABSOLUTE_WORKSPACE" --json
agent-sessions schedule enable --schedule-id "$SCHEDULE_ID" --json
agent-sessions run list --schedule-id "$SCHEDULE_ID" --json
agent-sessions run show --run-id "$RUN_ID" --json
```

Preparation also supports explicit adoption (`--existing` exact address) or forking (`--fork-from` with `--through-turn`); consult `schedule prepare --help`. Preparation does not enable the schedule. One schedule owns a bound thread; do not adopt a thread belonging to another schedule.

A schedule allows one active run; overlapping triggers coalesce into waiting work. Disabling stops future triggers but preserves already waiting/active runs. It is not a stop command. Admitted runs retain captured instructions/configuration; definition edits apply to future admissions. Use returned `changeId` for schedule updates and `revisionId` for instruction updates rather than guessing stale-edit tokens.

A worker outcome and its summary outcome are separate. `summaryBlocked` does not mean worker failure. Inspect the reason before `run summary-retry`; use `summary-skip` only when deliberately accepting missing continuity. `finished` alone is not success: inspect the worker outcome.

Export/import moves a schedule definition and portable continuity, not a live native session. Imports are disabled and need destination preparation. Existing schedule UUIDs require explicit `--overwrite`; preserve IDs and inspect the destination before overwriting. Use subcommand help for these less-common operations.
