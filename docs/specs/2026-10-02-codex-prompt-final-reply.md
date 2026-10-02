# Codex prompt results carry the final reply, not a transcript

Date: 2026-10-02. Owner direction: replace the buffered Codex prompt transcript with the agent's final reply; history is read on demand. Decisions marked **(owner)** come from the owner; **(design)** marks choices this spec makes, each with its reason.

## 1. Problem

A Codex prompt through the CLI (`agent-collaboration conversation prompt --json`) or MCP (`conversation_prompt`, `conversation_create` with a prompt) returns one settlement:

```text
CodexPrompt { updates: Vec<Value>, permissionRequired, result }
```

`updates` is every ACP `session/update` the agent streamed during the turn: each assistant text delta (roughly a word), each tool call, each plan step. To produce it, `collaboration-client` keeps every update in `BoundedPromptUpdates` (`acp_conversation.rs:19-50`), capped at 1024 updates or 64 MiB, and keeps session-setup updates in `pending_updates` with the same 1024 cap (`acp_conversation.rs:570-576`).

Ordinary turns exceed 1024 updates: a reply of about a thousand words, or a few hundred tool calls. The client then returns `protocolViolation: ACP prompt updates overflow` with an unknown effect, while the agent keeps working and its reply is lost to the caller. On 2026-10-02 this lost a design review and an implementation report, and an unknown effect invites a duplicate retry.

The buffer exists only to return the transcript inline. Callers use the final reply and the stop reason; nobody needed the transcript, and the full history already lives in the agent's own thread.

**Claude and Cursor prompts already work the way this spec proposes.** Their settlement is `ProviderPrompt { output: Available { text } | Unavailable { reason } }`, where `text` is the turn's assistant text (`acp-client-runtime/src/provider_prompt_observation.rs:306-319`, bounded by `MAX_PROMPT_OUTPUT_BYTES` = 1 MiB, the `MessageText` limit). Codex is the outlier.

## 2. Decision

```text
before  CodexPrompt { updates: [ …every streamed update… ], permissionRequired, result }
after   CodexPrompt { output: Available { text } | Unavailable { reason }, permissionRequired, result }
```

1. **Final reply (owner).** A Codex prompt's settlement reports the turn's assistant text in `output`, using the provider shape. `updates` is removed from `CodexPrompt`, `ExistingConversationPromptResult`, and `ConversationCreatePromptResult`. Hard cutover; no compatibility field.
2. **Text rule (design).** `text` is the concatenation of every `agent_message_chunk` text block in the turn, in order, exactly the rule the provider path uses. Reason: one meaning of "the reply" across Codex, Claude, and Cursor. Non-text content and every other update kind are not retained. A turn with no assistant text reports `Available { text: None }`, as providers do.
3. **Size (design).** `text` is a `MessageText` (1 MiB). A turn whose text exceeds it still settles with its real stop reason and reports `output: Unavailable { reason: outputLimitExceeded }`, a new `ConversationOutputUnavailableReason`. Reason: the turn succeeded, so the settlement must say so; failing a completed turn is the defect this spec removes. The caller reads the reply from history. The provider path currently fails the operation instead (`PromptOutputLimitExceeded`); aligning it is out of scope and recorded as a follow-up.
4. **No retention of streamed updates (design).** The aggregated prompt paths keep only the growing text string and the permission flag. Session-setup updates (for example history replayed on `session/load`) are not buffered for these paths. The update-count caps are removed; the only bound is the `text` byte limit. The CLI's streaming record path (`emit_record`, which prints each `ConversationEvent` as it arrives) is unchanged and still sees every update.
5. **History on demand (owner).** A caller that needs more than the final reply reads the thread's history through the existing session reads, not through the prompt result.

## 3. Ownership

| Concern | Owner | Change |
| --- | --- | --- |
| Result types | `crates/collaboration-client/src/conversation_contract.rs`, `conversation_operation_result.rs` | replace `updates` with `output` on the Codex prompt results and `CodexPrompt` |
| Accumulation | `crates/collaboration-client/src/acp_conversation.rs` | replace `BoundedPromptUpdates` with a text accumulator implementing §2.2–2.3; stop retaining setup updates on the aggregated paths |
| Settlement mapping | `crates/collaboration-client/src/conversation_session_operations.rs` | map the accumulated output into `CodexPrompt` |
| Unavailable reason | `crates/collaboration-protocol/src/provider_conversation_contract.rs` | add `outputLimitExceeded` |
| Surfaces | `agent-collaboration` CLI `--json`, `collaboration-mcp` tools | follow the types; regenerate the MCP golden success-schema snapshot |

## 4. Proof

| Obligation | Proof |
| --- | --- |
| Long turns settle with their full reply | integration test (`collaboration-client/tests/acp_conversation_flow.rs`): a fake agent streams more than 1024 text chunks and many tool calls; the result is `Completed` with the full concatenated text |
| Setup replay cannot fail a prompt | a fake agent replays more than 1024 history updates during `session/load`, then answers; the prompt settles with the new turn's text only |
| Text rule matches providers | unit tests: chunk order, non-text ignored, no text → `Available { text: None }` |
| Over-limit text keeps the turn's outcome | unit/integration: text beyond 1 MiB → real stop reason and `Unavailable { reason: outputLimitExceeded }` |
| Wire shape | CLI `--json` and MCP results contain `output` and no `updates`; MCP golden snapshot refreshed; existing MCP schema-validation test green |
| Streaming path unchanged | existing `emit_record` tests stay green |
| Gates | `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, affected crate tests, workspace tests once |

## 5. Risks

- **Contract change.** Anything parsing `updates` from a Codex prompt result breaks; the repository's own tests are the only consumers found (the agent-router skills tell agents not to treat provider updates as a transport). The release note names the change.
- **Version skew.** An older client reading a newer result rejects `output` under `deny_unknown_fields`; a newer client reading an older Host never sees `updates` because the client computes the result. Same cutover as every protocol change: upgrade, restart the Host, then agent sessions.
- **Lost detail.** Callers lose tool-call detail in the result; the thread history keeps it.

## 6. Out of scope

Aligning the provider path's over-limit behaviour with §2.3; a streaming JSON mode for the aggregated CLI path; the Router-side `Detached` behaviour.
