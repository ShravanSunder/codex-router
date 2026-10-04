# Router fixes — Requirements

The owner commissioned this milestone on Sunclaw on 2026-10-03. The affected users are the owner, agents collaborating through Router, CLI users, and maintainers relying on test and release evidence. The backlog is an allegation to re-derive from current main, not proof that every item remains broken. A non-reproducing item closes only with current evidence.

The owner assigns correctness first. Test reliability investigation may run in parallel. Each feature group has one PR unless repository constraints force another grouping. Implementation follows source-grounded design, independent review and a Lead-authored plan; the Lead retains design and acceptance, contributors return implementation and evidence. The milestone excludes the other machine's codex-prompt-final-reply change and its reserved version 0.1.63.

## Authorized needs

Every row below is authorized by the owner's milestone commission. Priority is owner-assigned where explicitly given, otherwise unranked within its group. Why the outcome matters is stated without selecting a mechanism.

| ID | Need and desired outcome | Why / consumers | Priority |
| --- | --- | --- | --- |
| U1 | A provider crashing just after startup is reported with the correct failure kind. | Agents need an accurate recovery action and cause. | HIGH; correctness first |
| U2 | Exact turn interruption works without a protocol violation. | The owner and agents need to stop the selected turn safely. | Correctness first |
| U3 | Managed Codex ask-the-user questions reach their existing approver and can receive an answer. | An agent must not hang on an invisible question. | Correctness first |
| U4 | Floor reconnect preserves the upstream Close as well as the client reconnect signal. | Both peers need orderly shutdown; completion order must not abort required cleanup. | Correctness first |
| U5 | An ambiguous Claude session rejection says why it cannot be delivered. | Users need to disambiguate terminals rather than guess or resend. | Correctness first |
| U6 | Every shortened push preview ends with an ellipsis and shows the remaining count with its unit, such as `…" (+1167 more chars)`. | The owner must recognize missing text on every shared push surface. | Owner report, 2026-10-03 |
| U7 | Raw Claude peer-registry error text does not leak into rejection messages. | Users need safe, actionable errors rather than internal text. | Error messages |
| U8 | `wake send` errors follow the established error format. | CLI users need consistent failure and next-step guidance. | Error messages |
| U9 | Credential renewal retains the local lock/file error instead of discarding its cause. | Maintainers must distinguish local access failure from provider renewal failure. | Error messages |
| U10 | The quota-exhaustion half-second wait has reliable behavioral proof. | Test success must not depend on elapsed wall time. | Test reliability |
| U11 | The owned restart test reliably completes and leaves no fixture child running. | Maintainers need repeatable tests without leaked processes. | Test reliability |
| U12 | Re-derive and repair the SQLite-lock and wakeup-first-fire flakes. | Test failures must reflect behavior rather than fixture races. | Test reliability |
| U13 | The Claude launch-target fixture has bounded waits. | A failed fixture must terminate with useful evidence. | Test reliability |
| U14 | Audit paused-time patterns and repair admitted reliability defects. | Timer proof must establish the relevant behavior under the runtime used. | Test reliability |
| U15 | Source guards establish the intended checks; HTTP-streaming G-25 covers the owning split code rather than websocket.rs alone. | File moves must not manufacture false-green structural proof. | Test reliability |
| U16 | Re-derive the six proof-matrix rows reported failing on main and repair confirmed defects without weakening gates. | Maintainers need current completion evidence. | Test reliability |
| U17 | Provide `codex-router account rename`. | The owner needs to change a display label while retaining the account. | Small feature |
| U18 | Provide schedule display names. | The owner needs readable schedule identity on display surfaces. | Small feature |
| U19 | The quota screen works below 24 terminal rows. | The owner needs usable information and navigation on short terminals. | Polish |
| U20 | Measure credits' database contention under load. | Evidence should determine whether contention merits further work. | Measurement; no speculative architecture change |
| U21 | Evaluate whether pinned log module names from the LUNA-407 split should remain. | Maintainers need intentional observability and module boundaries. | Evaluation; unresolved choice |
| U22 | Design Router as the main dispatcher for non-native agents so board-only blocker posts do not strand work. | Leads need to be woken when a contributor is blocked. | ai-tools; design-only |

## Follow-ups admitted after #123

The owner's 2026-10-03 continuation adds these review follow-ups to this backlog after #123 landed at `7a8cbd89` as 0.1.63. Investigation is authorized for every row; where product meaning is unsettled, the row authorizes settling that decision rather than an inferred implementation. [The current backlog](../../wip/2026-10-03-router-fixes-backlog.md) records source/reproduction status separately from these needs. #123 itself remains the other Lead's shipped work.

| ID | Need or investigation | Why / consumers | Authority and priority |
| --- | --- | --- | --- |
| U23 | Make public `conversation load` / `conversation create` handle long history; re-derive the reported failure beyond 1,024 messages. | Agents need to open an existing long-running thread successfully. | Owner-admitted O1; primary new follow-up. |
| U24 | Correct cancellation before prompt dispatch to retain truthful output availability, including `available` / `text: null` where the existing spec requires it. | Callers must distinguish no reply from old-Host output loss. | Owner-admitted O2; existing final-reply specification is the rail. |
| U25 | Settle malformed/unknown final-reply metadata handling while preserving truthful turn outcome/effect. | A completed turn must not become an unknown-effect claim solely because output metadata cannot be decoded. | Owner-admitted O3; future-reason policy remains unsettled. |
| U26 | Re-derive full-thread download when replay is skipped and design bounded history reads. | Long threads must not consume resources up to a frame limit merely to resume. | Owner-admitted O4 and spec §6; no guessed new limit. |
| U27 | Remove duplicate execution of the aggregate prompt-output suite after verifying its target/module inclusion. | Maintainers need one effective proof execution per scenario. | Owner-admitted O5; proof strength preserved. |
| U28 | Re-derive the 1,024-frame adapter queue overflow when a client stalls mid-turn and settle the intended bounded streaming behavior. | Output backpressure must have deliberate completion/failure semantics. | Owner-admitted O6 and spec §6; policy remains to design. |
| U29 | Align provider over-limit and text-selection behavior with the admitted common output contract where required by the final-reply follow-up. | CLI/MCP callers need predictable outcomes across supported providers. | Owner-admitted spec §6; precise cross-provider obligation remains to settle. |
| U30 | Re-derive and repair the Host remote-control deadline test's CI timeout. | Readiness proof must retain separate native/remote deadlines and be reliable under CI. | Owner-admitted CI flake; test reliability. |

The recurring WebSocket close race in #123 CI strengthens U4's evidence and is not a duplicate requirement. The shared Question extension for U3 (native Other/free text and secret-input metadata through the existing broker) was explicitly selected by the owner on 2026-10-03. All runtime proof uses an isolated debug profile/state/socket/port setup, with Remote Control disabled by default; no production endpoint, credential, Keychain or Desktop validation is authorized.

## Folder-migration admission (2026-10-04)

**U31:** A caller must be able to move the same eligible managed conversation to an explicitly requested new working directory through the supported Router path, preserving session identity and existing permission/configuration/active-turn safeguards. The owner admitted this correctness item after migration source evidence exposed the missing native cwd propagation. Native support and precise failure behavior remain to verify before a ready design.

Migration-reported diagnostics, idle-client lifetime, branch-metadata provenance and public load-access propagation are admitted for investigation as U32–U35 in the backlog. They do not authorize inferred retention mechanisms, trust bypasses or access-policy changes.

## Urgent global-harness correction (2026-10-04)

**U36:** Default agent launches use globally installed harnesses, including Claude. Adapter packages may provide ACP transport, but they must not silently substitute an SDK-bundled native runtime. A missing global harness yields an actionable failure with no bundled fallback. Preserve provider protocol behavior and existing conversation identities. The observed ACPX review launcher and Router startup are distinct owning boundaries; correcting one does not establish the other. Authentication causes remain independent of executable provenance. No authentication/security changes, unrecognized installs, production restart, push, merge or release are authorized by this correction.

## Queued UUID restore (2026-10-04)

**U37 UUID restore:** A canonical positional UUID or `--id UUID` may infer the provider only from one valid candidate in the explicitly configured/current Router-machine scope. Preserve full SessionRef provenance and explicit provider selection; collisions require an explicit choice and missing candidates produce actionable errors. Names, fuzzy matching and cross-machine discovery remain outside this slice. Queue implementation behind U36's global-harness correction.

## Operational limits

Use the owner's requested `wt` worktree. Keep production Router running on both machines. Exercise unreleased code only through tests, cargo run, or an isolated debug Host. Do not cargo-install Homebrew executables or edit the owner's dotfiles. Follow repository SQLite validation and signing rules. Do not expose secrets, secret references, credential paths or account metadata in public artifacts. No destructive Git commands, directory deletion or unrelated cleanup.

User-facing changes require the next unused workspace version and matching Cargo.lock before merge, green required CI, independent review, repository-compliant tag/release and Homebrew proof. Recheck main at release time. Publishing/installing never authorizes production replacement. The owner authorizes the scoped merge, tag and release extension after those gates.

## Unresolved hypotheses

The historical detailed notes were lost. Reproduction and current-owner evidence remain to be established for each backlog row. This document authorizes outcomes and investigation; it does not assert root causes, close rows, authorize a broader recovery mechanism, or settle U21's choice. U22 permits design only in the separate ai-tools repository.
