# Agent communication — Follow-up catalog

## Purpose and governing boundary

This catalog preserves follow-up ideas from the owner discussion. “V2” means work after the two-PR local V1 foundation, potentially as fast-follow PRs; it does not prescribe a major release or one combined delivery. This is an idea inventory, not an implementation plan, reviewed specification, or authorization to expand V1.

Current authority remains the separate [Requirements](./2026-09-05-agent-communication-system-requirements.md), [Specification](./2026-09-05-agent-communication-system-specification.md), and [Program Design](./2026-09-05-agent-communication-system-program-design.md).

V1 provides local Codex communication through native access, Codex-to-ACP adaptation, Session Control, a Rust SDK and descriptive CLI. Its address book and rolling 30-day lifecycle journal record observed thread/server status. Codex owns conversations, turns, queues and native TUI recovery. There is no Sessions supervisor. Swift, TypeScript and Python clients are later deliveries. Product renaming is deferred.

## Domain map

```text
Later callers and workflows
├── chief-of-staff / manager          decides who should do work
├── scheduled activation             decides when to submit
├── durable mailbox                  remembers delivery obligations
└── shared message board             stores findings and discussion
                   │
                   ▼
            public CLI / SDKs
                   │
                   ▼
       V1 communication foundation
       discover · send · observe · interrupt
                   │
                   ▼
       Codex-owned conversation and execution

Later reach: authenticated remote hosts and other runtime integrations
Later movement: explicit online session relocation
```

Lifecycle journal, message board and mailbox are separate domains. An observed status is not an authored finding; an authored finding is not a delivery receipt. None replaces native conversation history.

## Confirmed follow-up capabilities

### Shared message board

**Purpose:** provide a SQLite-backed place where agents explicitly post theories, findings, evidence references, questions and responses, and read other agents' contributions. This supports a coordinator and separate threads researching related work without copying every conversation into one transcript.

**V1 dependency:** endpoint/session references, descriptive CLI and reusable clients. Board operations should have their own public contract; agents should not manipulate a shared SQLite file directly. SQLite is the requested storage direction, not a decided table layout or reason to expose storage internals.

**Meaning still to settle:** board membership and scope (project, task or collaboration); author attribution versus caller-supplied labels; post/reply identities; evidence references and attachments; correction/edit history; subscriptions and cursors; retention; read/write permissions; and behavior when an author session disappears. Candidate records such as boards, posts, replies and verification claims are vocabulary to design, not committed schemas.

**Evidence to obtain in its design:** two independent agents post/read the same board through CLI/SDK; concurrent contributions remain distinguishable; restart preserves committed posts; corrections retain clear provenance; invalid cursors and unauthorized access fail explicitly. A post claiming verification must remain attributable to its author and evidence.

**Exclusions:** storage does not establish consensus, correctness or freedom from reasoning loops. Automatic summary generation, model wake, conflict arbitration and final-answer selection are separate policies. The V1 lifecycle journal's 30-day retention does not automatically apply to board content.

### Scheduled activation

**Purpose:** remember one-time or recurring schedules and invoke an existing communication operation when due. This was accepted as a fast follow rather than a V1 dependency; an external scheduler can call the V1 CLI meanwhile.

**V1 dependency:** explicit destination references, generation-aware submission, acceptance/error semantics and public client calls. Scheduling does not replace Codex's native queue.

**Meaning still to settle:** time zones and daylight-saving behavior; missed runs; overlapping executions; schedule modification/cancellation; budgets; unavailable targets; and crash recovery between dispatch and receipt persistence. Delivery uncertainty cannot be fixed by blindly resubmitting.

**Evidence to obtain:** clock-boundary behavior, restart before/after dispatch, overlap limits and truthful failed/unknown outcomes. It can ship without a durable mailbox if unavailable destinations visibly fail according to its policy.

### Durable mailbox

**Purpose:** accept explicit responsibility for storing information and attempting later delivery, including when a destination is unavailable. This is separate from scheduling and native queued turns.

**V1 dependency:** exact targets and the existing submission boundary. Mailbox acceptance must be a distinct receipt from native acceptance and execution completion.

**Meaning still to settle:** sender/recipient authority, message identity, ordering, duplicate prevention, expiry, retry policy, delivery acknowledgements, cancellation, recovery and storage limits. A correlation ID alone does not provide deduplication. Durable peer request/reply routing and reply-triggered wake are possible additions, not implied mailbox behavior.

**Evidence to obtain:** offline destination, service restart, duplicate attempt and lost acknowledgement cases without false delivered/completed claims. Do not promise exactly-once native execution unless a proven destination contract supports it.

### Additional language clients

**Purpose:** usable Swift, TypeScript and Python SDKs against the same language-independent public protocol; Rust SDK and CLI ship first. Authorized agents can already use the CLI regardless of their harness implementation language.

**V1 dependency:** published schemas, protocol versions and shared conformance scenarios. No Host implementation imports, terminal UI dependencies or mandatory Rust FFI.

**Meaning still to settle:** packaging/distribution, supported asynchronous runtimes and language versions, callback interfaces and ergonomic APIs. Generated types alone are not an SDK.

**Evidence to obtain:** real discovery, calls, event iteration, cancellation, reconnect and uncertain-outcome handling in each language; lossless native numeric IDs and absent/null fidelity. Agent Studio's Rust or Swift mapper is a separate application integration using these clients.

## Longer-term directions requiring their own design

### Remote host communication and discovery

**Purpose:** reach independently owned agent endpoints across the user's Tailscale machines while preserving host-installation, endpoint, session, connection and backend-generation identities.

**V1 dependency:** references separate from locators, capability descriptions and explicit unknown/unavailable outcomes. Communicating remotely does not share an app-server or move a session.

**Open choices and proof:** remote carrier, endpoint verification, principals and authorization, enrollment/revocation, metadata visibility, stale discovery and reconnect. Exercise real allowed/denied two-host calls and disconnection. Tailnet membership alone is not session authority.

**Exploratory mechanisms:** explicitly configured peers, a central directory, gossip membership, or a Cloudflare Durable Object for a directory/mailbox. No option is selected. A Durable Object does not automatically acquire private-tailnet reachability or relocate native state. Choose a mechanism only after defining the consumer and availability requirements.

### Hermes and other runtime integrations

**Purpose:** address Hermes, an agent-vm worker or another ACP-compatible runtime as a destination, with truthful capabilities. V1 instead serves Codex through ACP; an external harness can already be a caller when equipped with client tools.

**V1 dependency:** endpoint-scoped addressing and protocol-specific results rather than a universal Codex-shaped session API.

**Open choices and proof:** who owns each runtime process, whether existing sessions can be loaded/shared, retained connection lifetime, multiple-client coordination, callback/approval ownership and runtime-specific cancellation. ACP alone does not guarantee an app-server-style shared runtime. Real integration proof is required for each named harness; a protocol fixture is insufficient.

**Boundary:** do not silently create a fresh agent subprocess for each caller when callers intend to address the same existing agent. Hosting, attaching and calling are distinct capabilities. Hermes and agent-vm deployment changes remain separately scoped.

### Explicit online session relocation

**Purpose:** a later explicit command transfers a supported conversation to another system, with source and destination online for transfer. Continuous streaming replication is not required.

**V1 dependency:** identity/location separation and honest native persistence semantics.

**Open choices and proof:** runtime-supported export/import, a session transfer blob and its metadata, compatibility checks, history and worktree dependencies, quiescing active work, credential exclusions, transfer integrity, ownership handoff, identity mapping and interrupted-transfer recovery. A metadata server or durable blob store is an option to investigate, not a V1 prerequisite or selected architecture.

**Boundary:** remote attachment is not relocation. Copying a transcript does not prove native resumability. Prove an actual supported export/transfer/resume path before promising portable Codex, Hermes or cross-harness sessions.

### Tool access and caller context

**Purpose:** integrate Tool Portal or a model-visible tool facade and provide harness-specific self-address context where supported. Skills explain the public operations; they do not own hidden delivery state or confer authority.

**V1 dependency:** CLI/SDK, exact session references, capability/error descriptions and explicit return-message semantics. V1 already includes basic skill guidance; this follow-up concerns installation/federation and automatic context integration.

**Open choices and proof:** reliable self-address injection, per-agent tool grants, credential scoping, integration lifecycle, and whether an unavailable direct capability should be delegated to Hermes for reasoning/context. Do not infer self-address from a PID, cwd or newest thread. MCP exposure and Tool Portal deployment are separate integrations.

**Boundary:** calling a tool directly and asking another agent to reason about a task are different operations. Neither implies universal access to Hermes's personal integrations.

### Manager, durable jobs and validation workflows

**Purpose:** an external chief-of-staff client chooses a destination, harness/model, placement and authority, then observes outcomes. It may delegate to Codex roots, Hermes or VM workers. Harness-local subagents remain owned by their harness.

**V1 dependency:** callable operations, observed state and explicit result boundaries. Runtime software detects events; model decisions need not poll continuously.

**Exploratory workflows:** event-driven supervisor activation; bounded overnight jobs; checkpoint/validate/rollback iterations; token/runtime/iteration budgets; independent review; worktree isolation; and human consequence gates. These were architectural lessons, not accepted Router-owned mechanisms. A durable job engine retains objectives and validation state outside model context.

**Open choices and proof:** what warrants model wake, event coalescing, wake-loop prevention, recoverable job state, validation authority and human merge/deploy boundaries. A message-board consensus workflow is a manager policy: contributors can challenge and verify evidence without the board declaring a claim true.

**Boundary:** neither the communication service nor the board becomes a new agent loop, scheduler-by-accident or universal orchestration engine. Worktrees do not supply a security sandbox.

### Agent-loop and tool-execution placement

**Purpose:** eventually choose where the agent loop runs independently of where shell/filesystem/browser/build effects execute; sometimes both run locally, sometimes execution is on a VM or another machine.

**V1 dependency:** distinguish execution environment from session owner in the model.

**Open choices and proof:** supported native remote-execution protocol, capability grants, filesystem/worktree mapping, secrets boundary and remote interruption/failure. Inspect the chosen harness version and prove its actual split; do not generalize one runtime's execution-server support to every ACP endpoint.

**Boundary:** Agent VM owns its isolation substrate. Communication transport does not become a generic tool tunnel or transfer execution authority implicitly.

### Product naming and optional metadata extensions

An overall `agent-router` rename is deferred and optional. It should reflect accepted product responsibilities rather than force a generic runtime platform into V1. Important new files/packages keep the two-/three-word responsibility naming convention; unrelated packages need no speculative rename.

Saved aliases, grouped machine inventories, explicit journal export/reset and longer-lived discovery history are possible metadata extensions. They require concrete consumer needs and identity/retention semantics. They are not prerequisites for V1's address book or authority to change its accepted rolling 30-day retention.

## How to use this catalog

Select one follow-up, establish its observable contract, then design and validate its ownership independently. Scheduling and mailbox can be separate fast follows; the shared board can also stand alone. Remote access, a manager and heterogeneous runtime hosting are not automatic dependencies of any local board or scheduler.

No follow-up here requires a Sessions supervisor, PTY proxy, custom TUI reconnect engine, upstream modification or production process replacement. Future implementation and proof stay separately scoped; current V1 runtime validation remains debug-profile-only.
