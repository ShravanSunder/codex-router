# Session rename and scoped list: find a session again by scope and name

Date: 2026-09-16. Status: owner-accepted design, ready to implement after `2026-09-15-thread-model-choice.md`. Author: Fable design session on the owner's behalf. Owner decisions are marked **(owner)**; decisions taken on the owner's standing rules and awaiting explicit confirmation are marked **(owner, assumed)**. The implementer builds from this document and does not author design artifacts. Fable is final reviewer.

## 1. Problem

An orchestrator that spawned a reviewer, or a human who opened "another session here", cannot find that session again. The Control inventory that agents call (`agent-collaboration sessions list`) returns up to 100 rows with no scope, source, or name filter, so discovery is a scratch-file dump plus `jq` on `workingDirectory`. Titles are derived from the first user message ("Greet user", or blank for children), and nothing on the collaboration CLI can set a name. The human picker (`codex-router sessions --list`) already scopes by checkout, repository, and source and already prefers an explicit name; agents are not sent there and should not be, because the Control CLI is the schema'd, endpoint-bound surface the skill teaches.

## 2. Ground truth this design rests on

- Both list surfaces share one engine: `StoredThreadQuery` in `codex-native-integration/src/stored_thread_query.rs`, a read-only keyset query over Codex's `state_5.sqlite` `threads` table. It supports root `Any | Checkout(path)`, provider, source `All | Interactive | Subagents`, sort, and a cursor. The selected columns include `cwd`, `model_provider`, `model`, `source`, `thread_source`, `git_branch`, `git_origin_url`, `name`, `title`, `preview`, `first_user_message`. The table also carries `reasoning_effort` (Codex migration `0020_threads_model_reasoning_effort.sql`), which Router does not select yet (verified by the implementer 2026-09-16).
- The Control handler `collaboration-service/src/session_inventory_dispatch.rs::stored_page` calls that engine with root `Any`, provider `Any`, source `All`, sort `Updated`, because `NativeSessionListParams` (`collaboration-protocol/src/native_session_catalog.rs`) carries only `endpoint`, `view`, `page_size`, `cursor`. Its row already prefers `name` over `title` but folds both into one `title` field; `NativeSessionSummary` is `target`, `title`, `working_directory`, `observation`.
- Repository identity is resolved client-side in `collaboration-client/src/session_catalog/repository.rs` (`discover_repository_identity`: normalized origin URL, live worktree roots, repository basename, fallback cwd). The SQL side only matches paths and origin.
- Codex app-server v2 has `thread/name/set { threadId, name }`, which writes `threads.name` and emits `thread/name/updated`. Router does not call it today. It works on a stored thread that is not loaded: the handler calls `ThreadManager::update_thread_metadata`, which falls through to the thread store when the thread is cold (`codex-rs/app-server/src/request_processors/thread_processor.rs:1818-1842`, `codex-rs/core/src/thread_manager.rs:897-928`; verified by the implementer 2026-09-16).
- Codex `thread/list` accepts `cwd`, `source_kinds`, `search_term`, `model_providers`; Router reads the state DB directly for the stored view and does not need it.

## 3. Domain model

| Term | Meaning |
|---|---|
| Session | one Codex thread reachable through an endpoint, addressed by `SessionRef` |
| Name | the explicit persisted thread name (`threads.name`), set by a person or agent; the identity a human uses |
| Title | the derived label Codex stores from the first user message (`threads.title`); never an identity |
| Scope | the path predicate a listing is bounded to: `cwd`, `checkout`, `repo`, or `any` |
| Source | `interactive` (a person, ACP, or a Router dispatch drove the thread) or `subagents` (native spawned children); `all` for both. A Sidekick started through Router is `interactive`; it is a top-level persistent session, never a subagent **(owner)** |
| Query | a case-insensitive substring matched against Name first, then Title |

Invariants:

1. A listing states its Scope explicitly. There is no default **(owner, assumed: the board's no-defaults rule)**. Missing scope is a validation error naming the four flags.
2. Scope predicates are resolved on the client from the given path, exactly as the human catalog resolves them: `--cwd` matches that directory; `--checkout` matches the worktree root containing it; `--repo` matches every live worktree root and the normalized origin of that repository; `--any` matches everything. The service receives resolved predicates, not a working directory, and stays a pure catalog query.
3. A row carries `name` (explicit, nullable) and `title` (derived) as separate fields. A consumer that wants one display label takes `name` then `title`; Router never merges them.
4. A row carries `source`, `model`, `reasoningEffort`, `gitBranch`, `workingDirectory`, `updatedAt`, `idleSeconds`, read from the stored columns, and `status` (`idle`, `busy` with `turnId`, or `stored` when the thread is not loaded) so an owner can see what an implementer is doing from one listing **(owner)**. `reasoningEffort` is absent only when the stored column is NULL; it is never guessed.
5. Rename sets Name through the native runtime. Router never writes Codex's database.
6. Any authorized sender may rename any session on the endpoint **(owner, assumed)**. Rename is a session command; it is not part of `board thread create` or any board operation.
7. A rename result reports the Name the runtime echoes, not the one requested; a mismatch is a refusal with both values.
8. Query matching is Name then Title, case-insensitive substring, applied in SQL so pagination stays correct.

## 4. CLI

```text
agent-collaboration sessions list --endpoint <id> --view stored
  (--cwd <path> | --checkout <path> | --repo <path> | --any)
  --source <interactive|subagents|all>
  [--query <text>] [--page-size <1..100>] [--cursor <c>] --json

agent-collaboration session rename --endpoint <id> --session <id> --name <text> --json
```

`--source` is required **(owner, assumed)**; `all` is a stated choice, not a default. `--name` is 1 to 120 Unicode scalar values after trimming, emoji included (the owner names Sidekicks with emoji so they stand out in listings); empty, control characters, or newlines are validation errors. Views `loaded` and `active` accept the same scope and source flags and apply them to the runtime page.

Row shape (`sessions[]`):

```json
{"target":{"endpoint":{…},"sessionId":"…"},"name":"headless-tools review","title":"Greet user",
 "source":"subagents","model":"gpt-6-astra","reasoningEffort":"medium","gitBranch":"feat/x",
 "workingDirectory":"/…","observation":{"kind":"stored","updatedAt":"…"},"idleSeconds":412}
```

Rename result: `{"target":…,"name":"headless-tools review","previousName":null}`.

## 5. Native and protocol mapping

- `NativeSessionListParams` gains `scope: Any | Cwd{path} | Checkout{root} | Repo{live_roots, normalized_origin, basename}`, `source`, `query: Option<String>`. Serde-tagged enum; `deny_unknown_fields` stays. Bump the Control schema document.
- `stored_page` maps `scope` to `StoredThreadRoot` (extend it with `Repo` or compose the existing path and origin predicates), `source` to `StoredThreadSource`, and pushes the query predicate into `stored_thread_page_query`. Row decoding reads `name`, `title`, `source`/`thread_source`, `model`, `git_branch` from the already-selected columns and adds `reasoning_effort` to the select list.
- `NativeSessionSummary` becomes the row above. Hard cutover: the old `title`-only shape is removed, and `crates/agent-collaboration/src/session_commands/picker_runtime_inventory.rs` plus the lifecycle stored-observation recorder are updated in the same change.
- Rename: one Control request `NativeSessionRename { target, name }` dispatched like other native operations through `native_control_dispatch.rs`; the service calls `thread/name/set` through the admitted backend. No resume is needed for a cold thread, and the service never starts a turn.
- `idleSeconds` is computed at the service from `updated_at_ms` and the service clock, once, for every row.

## 6. Skill reference

`agent-skills/agent-collaboration/references/session-messaging.md`: discovery uses the scoped list, never a dump plus `jq`; rename a working session right after identifying it; if several rows remain after scope and query, inspect them, do not ask the person for a title. The ai-tools copy is Astra's change after this lands.

## 7. Proof

- Validation: missing scope, two scopes, missing source, empty name, name over 120 characters each fail naming the flag before any Control call.
- Catalog tests (extend the existing `session_catalog` and `stored_thread_query` tests): cwd, checkout, repo (live roots plus origin), any; source filtering; query matches name before title and is case-insensitive; keyset pagination stays gap-free with a query applied.
- Service tests: `stored_page` passes every predicate through; rows carry split `name` and `title`; `reasoningEffort` is absent when unknown.
- Rename: native path test sends `thread/name/set` with exactly `threadId` and `name`; echo mismatch refused; live proof renames a real stored session and the next scoped list shows the name.
- Repo checks: fmt, clippy, tests, `git diff --check`; Control schema document regenerated.

## 7a. Picker resume restores the session's model and effort **(owner, 2026-09-17)**

The human picker (`agent-sessions`, and `agent-collaboration sessions` launch paths) resumes a selected session with the model and reasoning effort that session last ran with. Today it launches `codex --profile codex-router resume <id>`, and the TUI applies the profile defaults, which the owner described as "switches to default, confusing, hard to remember." The stored record already carries `model` and `reasoningEffort` (§3 invariant 4). On resume and fork of a selected record the launch adds `-c model="<model>"` and `-c model_reasoning_effort="<effort>"` before `resume`/`fork`, each only when the row value is present and only when the caller's own Codex arguments do not already set that key (`-m`/`--model`, or a `-c model=…` / `-c model_reasoning_effort=…` override); caller arguments win. Dry-run output shows the injected arguments. A row without a stored model or effort launches as before. This amends §8's "no change to the human picker" for this one behavior; the picker binary is `agent-sessions` from the `agent-collaboration` crate (the retired `agent-sessions` 0.1.19 install predates it).

## 8. Boundaries

- No change to the human `codex-router sessions --list` picker beyond sharing extended query code.
- No board involvement; no automatic naming of children at spawn.
- No Codex database writes; no new storage.
- Second PR of the stack, after model choice; dated changelog entry; no merge, install, or restart.
