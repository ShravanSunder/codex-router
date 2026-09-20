# Collaboration through CLI and MCP

Agents and people should be able to use the existing collaboration system through either CLI or MCP without learning two different sets of behavior. This is a foundation for additional clients and providers, not a new orchestration system.

## Who needs this and why

- Shravan needs predictable agent creation and communication, with the requested working directory, model, effort, and access clearly represented.
- Agents using CLI or MCP need discoverable operations, explicit schemas, reusable conversation addresses, and results they can compose programmatically.
- Maintainers need one behavioral implementation so fixes and new capabilities reach both surfaces together.

```text
Choose the Router and endpoint
          |
Start a conversation in the supplied directory
          |
Keep its one thread/session ID
          |
Send, inspect, observe, interrupt, or resume
          |
Use CLI or MCP without changing the meaning of the operation
```

## Authorized needs

All rows below are authorized by the owner in conversation `01a0b425-8085-7ae2-8d92-f0d552404c69`. Priority is owner-selected foundation scope; no ordering among rows is implied. The September 19 correction supersedes earlier assistant proposals for a task system. Shared discussion root: `01a0b741-6a76-7522-ae6c-72631fa7c39c`; correction message: `01a0b994-5c62-7680-87f3-f0a75cd2c6bf`.

| Need | Outcome and reason | Owner basis |
| --- | --- | --- |
| U1 | CLI and MCP expose equivalent collaboration behavior and evolve together, avoiding duplicate implementations and drift. | “improve them in lockstep” |
| U2 | Agents can start and talk to Router-managed Codex app-server conversations through explicit, composable schemas. | “Router managed app server conversations only”; MCP for talking to agents and starting threads |
| U3 | Keep the existing service/endpoint/session address; a Codex thread and session are one conversation with one ID. | “The thread ID and the session ID are the same” |
| U4 | Starting a conversation supplies its existing working directory and harness settings; Router does not manage the assignment or provision its worktree. | “telling the agent to start a specific work tree”; “no extra machinery” |
| U5 | Improve existing collaboration DX, including communication, observation, discovery and existing shared-board operations, using the SDK. | “any agent to use the collaboration system with MCP” and approved code-grounded scope |
| U6 | Keep external ACP providers separate from the Codex CLI/MCP foundation, with only necessary session mapping when that extension is designed. | Separate second PR for Claude Code and Cursor; minimal persistent state discussion |
| U7 | Use a remote-capable HTTP MCP protocol from the start, deployed only on this computer without authentication in V1. | “I don’t want a local stdio adapter”; “No auth for v1 ... It'll be local to the computer for this PR.” |

## Boundary

The foundation includes CLI and a Streamable HTTP MCP server backed by the existing SDK and Router. V1 binds only to loopback, with no authentication. Remote exposure, TLS and authentication are deferred; HTTP avoids a later stdio-to-network protocol replacement. Local service discovery may resolve the actual service ID; explicit addresses must not be silently redirected. A working directory is an input, not a new worktree or task object. Codex remains the owner of conversations, permissions, execution, and conversation persistence.

The following are excluded: task IDs or task registries; assignment completion evaluation; worktree creation or deletion; automatic agent replacement; extra computer/installation identity layers; transcript duplication; harness-native subagent management; skill changes; a code execution sandbox inside Router; new mesh networking or remote authentication; external ACP provider launchers in the foundation.

The term **board thread** refers to the existing message-board discussion and its existing root-message identifier. It is not a second identifier for a Codex conversation. The foundation preserves that existing feature without conflating the two domains.

## Existing foundation and remaining questions

Owner implementation constraint: use established Rust MCP libraries and protocol helpers, meaningful names, and the repository's Rust standards. Reuse protocol/transport implementations rather than hand-writing MCP framing, initialization, streaming or cancellation. This constrains realization without adding a new product capability.

The source already contains `SessionRef`, `ControlClient`, `AcpConversation`, `NativeObservation`, and board/automation SDK operations. CLI handlers still own some caller resolution and request preparation. No collaboration MCP server was found in the inspected source at `74aaa88`.

External-provider capabilities and remote mesh transport are not established by this document. They do not block local Codex collaboration parity. Provider-specific promises belong to the later ACP design.

Continue with [Specification](specification.md) for observable behavior and [Program Design](program-design.md) for realization.
