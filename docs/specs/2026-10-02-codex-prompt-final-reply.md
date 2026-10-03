# Codex prompt results carry the final reply, not a transcript

Date: 2026-10-02. Owner direction: replace the buffered Codex prompt transcript with the agent's final reply; history is read on demand. Decisions marked **(owner)** come from the owner; **(design)** marks choices this spec makes, each with its reason.

## 1. Problem

A Codex prompt through the CLI (`agent-collaboration conversation prompt --json`) or MCP (`conversation_prompt`, `conversation_create_and_prompt`) returns one settlement:

```text
CodexPrompt { updates: Vec<Value>, permissionRequired, result }
```

`updates` is every ACP `session/update` the agent streamed during the turn: each assistant text delta (roughly a word), each tool call, each plan step. To produce it, `collaboration-client` keeps every update in `BoundedPromptUpdates` (`acp_conversation.rs:19-50`), capped at 1024 updates or 64 MiB, and keeps session-setup updates in `pending_updates` with the same 1024 cap (`acp_conversation.rs:570-576`).

Ordinary turns exceed 1024 updates: a long reply (update count depends on how the agent chunks text), or a few hundred tool calls. The client then returns `protocolViolation: ACP prompt updates overflow` with an unknown effect, while the agent keeps working and its reply is lost to the caller. On 2026-10-02 this lost a design review and an implementation report, and an unknown effect invites a duplicate retry.

The buffer exists only to return the transcript inline. Callers use the final reply and the stop reason; nobody needed the transcript, and the full history already lives in the agent's own thread.

**Claude and Cursor prompts already work the way this spec proposes.** Their settlement is `ProviderPrompt { output: Available { text } | Unavailable { reason } }`, where `text` is the turn's assistant text (`acp-client-runtime/src/provider_prompt_observation.rs:306-319`, bounded by `MAX_PROMPT_OUTPUT_BYTES` = 1 MiB, the `MessageText` limit). Codex is the outlier.

## 2. Decision

```text
before  CodexPrompt { updates: [ …every streamed update… ], permissionRequired, result }
after   CodexPrompt { output: Available { text } | Unavailable { reason }, permissionRequired, result }
```

Revised 2026-10-02 after the 🦉 Advisor review (Sol 6.1 xhigh, `tmp/codex-final-reply-advisor.md`): the reply is selected in the Codex adapter from native message phases, setup replay is skipped at its source, and history recovery is not promised through MCP yet.

1. **Final reply (owner).** A Codex prompt settlement reports the turn's final reply in `output`, using the provider shape. `updates` is removed from `CodexPrompt`, `ExistingConversationPromptResult`, and `ConversationCreatePromptResult`. Hard cutover; no compatibility field.
2. **Selection in the adapter (design).** ACP `agent_message_chunk` deltas carry no message identity or phase, so the client cannot tell commentary from the answer. The Codex adapter (`codex-acp-adapter`) observes the accepted turn's native `item/completed` notifications whose `item.type` is `agentMessage` and selects:
   - the last completed `agentMessage` with `phase: "final_answer"`;
   - otherwise the last completed `agentMessage` with no phase;
   - never one with `phase: "commentary"`.

   The selected item's complete `text` is the reply; streamed deltas are not appended. The adapter returns it on the `session/prompt` response as `result._meta["codex-router/finalReply"]`, one of `{"kind":"available","text":<string|null>}` or `{"kind":"unavailable","reason":<reason>}`. Streaming `agent_message_chunk` updates are unchanged.
3. **Size and validity (design).** The limit applies to the selected reply only, so long commentary cannot suppress a short answer. A reply over `MessageText`'s 1 MiB reports `unavailable` / `outputLimitExceeded`; a reply `MessageText` rejects for content (C0 controls other than newline and tab) reports `unavailable` / `outputInvalid`. Text is never truncated or stripped. Both are new `ConversationOutputUnavailableReason` variants.
4. **Outcome is independent of output (design).** Output availability never changes the observed terminal outcome. The settlement keeps today's stop-reason normalization (`ConversationEnd`), and `result` keeps the adapter's ACP result, including `stopReason`. A cancelled turn reports whatever reply the adapter selected before cancellation, usually `available` with `text: null`. Rejection, disconnect, and detached behaviour are unchanged.
5. **No retention on aggregate paths (design).** The client's aggregate prompt paths (`create_and_prompt`, `prompt_existing`, and the public CLI/MCP operations built on them) keep no streamed updates: they read the final reply from the prompt response and record `permissionRequired`. `BoundedPromptUpdates` and its count cap go away. The streaming path (`emit_record`) still receives every update, including setup updates after `SessionReady`.
6. **Setup replay skipped at its source (design).** On aggregate paths the client asks the adapter not to replay history, with `_meta["codex-router/replayHistory"]: false` on `session/load` (and on the existing-binding resume it maps to). The adapter then skips `project_history`, whose own 1024-update cap would otherwise still fail long threads. Streaming loads keep replay unchanged; their cap is a follow-up (§6).
7. **History (owner intent, narrowed).** A caller that needs more than the reply reads the thread's turns. Today only the CLI's raw native route offers that (`agent-collaboration native --endpoint codex-local`, then `thread/read` with `includeTurns: true` or paged `thread/turns/list`); `session inspect` excludes turns and `conversation load` returns no history. A bounded Codex history-read operation for CLI and MCP is a follow-up (§6); until it exists, an `outputLimitExceeded` reply is recoverable only through that CLI route.

## 3. Ownership

| Concern | Owner | Change |
| --- | --- | --- |
| Reply selection | `crates/codex-acp-adapter` (`native_prompt_execution.rs`, a new `final_reply_selection.rs`, `prompt_settlement.rs`) | observe the accepted turn's completed `agentMessage` items, select per §2.2, apply §2.3, attach `codex-router/finalReply` to the completed and cancelled prompt responses |
| Replay opt-out | `crates/codex-acp-adapter` (`session_creation.rs`, `session_setup_task.rs`, `history_projection.rs`) | honour `codex-router/replayHistory: false` by skipping `project_history` on cold load and existing-binding resume |
| Result types | `crates/collaboration-client` (`conversation_contract.rs`, `conversation_operation_result.rs`) | replace `updates` with `output` |
| Aggregate paths | `crates/collaboration-client` (`acp_conversation.rs`, `conversation_session_operations.rs`, `conversation_client.rs`) | remove `BoundedPromptUpdates`; send the replay opt-out; map `finalReply` into `output` |
| Unavailable reasons | `crates/collaboration-protocol/src/provider_conversation_contract.rs` | add `outputLimitExceeded` and `outputInvalid` |
| Surfaces | `agent-collaboration` CLI `--json`, `collaboration-mcp` (`conversation_prompt`, `conversation_create_and_prompt`) | follow the types; refresh the MCP golden success-schema snapshot |

## 4. Proof

| Obligation | Proof |
| --- | --- |
| Selection rule | adapter unit tests: commentary → tool activity → `final_answer` returns the final answer only; no phase selected when no `final_answer`; commentary never selected; completed text without any deltas; the last of several final answers wins |
| Limit applies to the selected reply | adapter test: large commentary followed by a small final answer returns the answer; a final answer at exactly 1 MiB is available, one byte over is `outputLimitExceeded`; a C0 control character gives `outputInvalid` |
| Long turns settle | integration through the real adapter: a turn with more than 1024 deltas and tool updates settles `Completed` with the selected reply (the old overflow regression) |
| Long-thread load | integration through the real adapter: prompting an existing thread with more than 1024 historical messages, on both cold load and existing-binding resume, settles with the new reply; a streaming load still receives replay |
| Outcome independence | over-limit and invalid replies keep `Completed`; cancellation keeps its stop reason and interruption metadata |
| Public surfaces | CLI `conversation prompt --json` and MCP `conversation_prompt` / `conversation_create_and_prompt` return `output` and no `updates`; MCP golden snapshot refreshed; strict decoding of the new reasons; Control schema validation of `conversation/operationWait` with the new reasons |
| Streaming path unchanged | existing `emit_record` and setup-update streaming tests stay green |
| Gates | `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, affected crate tests, workspace tests once |

## 5. Risks

- **Contract change.** `updates` disappears from Codex prompt results; the only consumers found are this repository's tests (the agent-router skills tell agents not to treat provider updates as a transport). The release note names the change.
- **Version skew.** The aggregate result is built in the CLI/MCP process from the adapter's prompt response. A new client against an older Host gets no `codex-router/finalReply` and must report the output as unavailable rather than inventing one; an old client against a new Host still buffers updates as today. The Control schema digest changes with the new reasons, and an older strict reader rejects them. Production replacement stays a separate owner decision.
- **Lost detail.** Tool-call detail is no longer in the result; native history keeps it, reachable today only through the CLI native route.

## 6. Out of scope (follow-ups)

- A bounded Codex history-read operation for CLI and MCP.
- The adapter's 1024-update replay cap for *streaming* loads.
- Aligning the provider path's over-limit behaviour (it fails the operation) and its all-chunks text rule with §2.2–2.3.
- A streaming JSON mode for the aggregate CLI path.
