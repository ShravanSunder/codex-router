# Collaboration CLI consistency: one address, one envelope, reasons on every refusal

Date: 2026-09-16. Status: owner-accepted design, ready to implement after `2026-09-16-session-rename-and-scoped-list.md`. Author: Fable design session on the owner's behalf. Owner decisions marked **(owner)**; **(owner, assumed)** where taken on standing rules pending explicit confirmation. The implementer builds from this document; Fable is final reviewer.

## 1. Problem

Agents using the CLI on 2026-09-15 and 2026-09-16 failed on the same shapes: two address forms for one session, `jq` paths that differ per command, control bytes in JSON text, native rejections with no reason, thread discovery that needs a project id and a reader before a plain read, and three version numbers (installed CLI, workspace, skill comment) that disagree. Each one produces an invented flag, a wrong `jq`, or a false "Router is down". These are Router defects, not skill wording.

## 2. Ground truth

- `message send` addresses a session with `--to <SessionRef JSON>` (`agent-collaboration/src/message_input_arguments.rs`); `session inspect` and `sessions list` use `--endpoint <id> --session <id>`.
- Result envelopes today: lists return `result.page.records`, inventory returns `result.sessions`, message show returns `result.message`, inspect returns `result.thread`.
- `native_control_dispatch.rs` maps every `NativeConnectionError::Rejected { .. }` to `nativeRejected` with only a `stage`; the DM to a Luna child thread on 2026-09-16 failed this way with "Message operation failed" and no next action.
- `board thread list` requires `--project-id` and `--reader`; there is no read that answers "threads for this repository".
- Board text fields can carry control characters; readers strip them with `tr -d '\000-\011\013-\037'` before `jq`.
- The skill reference states "0.1.23 CLI surface"; installed CLI is 0.1.24; workspace is 0.1.27; results carry no version.

## 3. Contract

1. **One address form.** Every command that targets a session accepts both `--to <SessionRef JSON>` and the pair `--endpoint <id> --session <id>`, parsed by one shared argument module. Giving both, or a pair with one half, is a validation error naming the flags. Applies to `message send`, `session inspect`, `session rename`, `conversation prompt --session|--fork`, `turn interrupt`, `wake send`.
2. **`--actor self` on every board command**, resolved per the participants spec (`CODEX_THREAD_ID` on `codex-local`, `CLAUDE_CODE_SESSION_ID` on `claude-local`, ambiguous when both are set, missing when neither). Verified on 0.1.27 (2026-09-16): `thread join` and `thread listen` resolve `self`; `message post`, `thread watch`, and `thread show --reader` reject it as invalid Identity JSON. Every `--actor` and `--reader` flag on `board` commands shares one resolver. An empty environment variable counts as unset: with both variables present and empty, 0.1.27 wrongly reports ambiguity. Nested subagents pass typed Identity JSON; the refusal for a missing or ambiguous environment names that fallback in `nextAction`.
3. **Reasoned native rejections.** `nativeRejected` carries `reason` from a closed set: `childThread` (target is a spawned child that does not accept direct input), `busy` (active turn and the delivery mode cannot steer), `notResumable`, `permissionDenied`, `unsupportedCapability`, `unknown`; each with a `nextAction` (`inspectTarget`, `useDeliverySteer`, `requestApproval`, `correctRequest`, `retryLater`). `unknown` is allowed only when the native error is genuinely unclassified and includes the native code.
4. **Repository-scoped thread discovery.** `board thread list --repository-path <path> --json` lists threads across every project bound to that repository, newest activity first, with project id, topic id, root id, title, orchestrator holder, and last activity. `--reader` is optional and adds watch status when given. Explicit path, no default **(owner, assumed)**.
5. **One result envelope.** Every list command returns `{"kind":"result","result":{"page":{"records":[…],"nextCursor":…}}}`. Every single read returns `{"kind":"result","result":{"record":{…}}}`. Every mutation returns `{"kind":"result","result":{"record":{…},"effects":{…}}}`. Hard cutover **(owner, assumed)**: `sessions`, `message`, `thread` aliases are removed, and the ai-tools skill references are rewritten by Astra in the same window.
6. **Sanitized text.** Board and message text is validated at the write boundary: C0 control characters other than `\n` and `\t` are rejected with `invalidField` naming `text`. Existing stored rows are escaped on read. `jq` works on raw output.
7. **Versions in every result.** The envelope carries `cliVersion` and `serviceVersion`. The skill reference stops stating a version and instead states the minimum version each feature needs.

## 4. Proof

- Argument tests for every command in item 1: JSON form, pair form, both, half pair.
- `--actor self` resolution tests across the environment states (neither, one, both, empty strings) on every board command that takes `--actor` or `--reader`.
- Rejection mapping tests: each native error class maps to its reason and next action; a live proof sends a DM to a spawned child and receives `childThread` with `inspectTarget`.
- Repository thread list: two projects bound to one repository return one merged page in activity order.
- Envelope: a schema test asserts every command's `--json` output validates against the one envelope; the Control schema document is regenerated.
- Text: write with a control byte is refused; a pre-existing row with one reads back escaped and parses with `jq`.
- Repo checks: fmt, clippy, tests, `git diff --check`.

## 5. Boundaries

- No change to listen, participants, or model-choice semantics.
- No new storage; no migration beyond none.
- Third PR of the stack; dated changelog entry naming every removed field name; no merge, install, or restart.
