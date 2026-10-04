# Host-controller checkpoint — 2026-10-04

This checkpoint records the existing host-controller design work on
`host-controller-impl`. Implementation remains held. It does not authorize
implementation, a push, merge, release or production process replacement.

## Deliverables and source of truth

| Artifact | Role |
| --- | --- |
| [Requirements](../specs/2026-09-26-host-controller/requirements.md) | Authorized needs and owner decisions. |
| [Specification](../specs/2026-09-26-host-controller/specification.md) | Observable obligations R1–R19 and required proof V1–V12. |
| [Program Design](../specs/2026-09-26-host-controller/program-design.md) | Keeper, generation, role, handover and evidence realization. |

The latest existing design commit is `95652af5`. Its native-evidence correction
received an independent Opus 5.5 review with a bounded `ready` result. That
result covers the corrected evidence boundary; it does not establish runnable
behavior or resolve the separate lifetime and planning holds.

The evidence correction distinguishes a captured executable identity record
from a freshly observed file identity. Candidate admission verifies the
executable and schema bundle before publication. Services reuse of an
already-current generation validates its captured record and retained bundle
without re-hashing an installed executable path that may have changed or
disappeared. Self-exec phase-one structural validation remains distinct from
phase-two health assessment.

## Existing design checkpoints

| Commit | Scoped result |
| --- | --- |
| `e495b15a` | Corrections for migration authority, native-request settlement, retained image commitment, complete frame-size admission, schema availability and pending-schema preparation. |
| `84a0ec9b` | A reversible handover content cut, explicit absent payload for idle version changes, and broader older-proxy migration proof. |
| `67d531b5` | Native-owned recorded executable identity and schema digest boundaries. |
| `95652af5` | Explicit candidate-admission versus current-generation evidence use. |

These are design-document checkpoints. Keeper/protocol product implementation
has not started. S1's pre-edit admission returned blocked without code,
manifest or lockfile changes. The earlier local implementation plan predates
the evidence corrections and remains historical. A current replacement plan
has not been admitted.

## Blockers and intentional holds

- **Dependency proof availability:** the last pre-migration offline baseline
  failed before compilation because the registry cache could not resolve
  `chrono`. Normal locked checks remained at registry update and were
  cancelled without reaching compilation. The online cause was not established.
  Dependency availability has not been rechecked after migration.
  Subsequent coordination reports attribute the registry problem to sandbox
  permissions and report a successful locked fetch and adapter check in the
  separate fixes worktree after an explicit grant. Those results have not been
  independently verified in this host-controller reporting turn; the historical
  failures above do not establish a current dependency-access failure.
- **Planning:** a replacement immutable plan must carry the corrected native
  owner write surfaces and evidence-use proof against a current governing
  basis. The previous plan is not current execution authority.
- **ProviderLink:** the revised query and provider snapshot contract check
  remains a prerequisite for its implementation slice.
- **Turn lifetime:** outer ACP-router and lazy-route abort paths can bypass
  inner registry cleanup. The lifetime realization and full routing-path proof
  must be reconciled before the dependent turn-handover slice proceeds.
- **Owner hold:** the current request authorizes status reporting and local
  checkpoints of existing owned work. It does not resume feature implementation.

The existing lifetime decision and contract coordination remain outstanding;
this checkpoint makes no new selection or owner request. Network configuration,
credentials and production processes remain unchanged. No stand-in has been
introduced and no required proof gate has been removed or weakened.

## Validation evidence

### Historical, before migration

- Pinned Rust 1.98.1 compiler and Cargo version checks succeeded. Formatter,
  Clippy and rust-src availability was verified; a Clippy validation run was
  not performed.
- Design checks passed for balanced fences, retained V1–V12 and R1–R19 coverage,
  and `git diff --check`. The changed handover sequence was rendered and inspected.
- The focused locked/offline native-integration baseline exited 101 while
  resolving the missing dependency. Normal locked attempts were cancelled with
  exit 130 at registry update; they provide no compilation or network-success
  evidence.
- The retained independent reviewer verified the evidence correction at
  `95652af5`. Required real-path/runtime proof remains unexecuted.

### After migration

- Local read-only checks confirmed the worktree root, branch and existing
  `95652af5` HEAD. The tracked working tree was clean; local work traces remained
  untracked.
- The existing trace, review receipt and historical plan were found through
  relocated repository paths. Supported session inspection reported the retained
  configured `gpt-6.1-sol` model and `high` reasoning effort.
- This documentation checkpoint is checked for local link resolution,
  public-safe content, whitespace and consistency with the recorded state.
  No dependency, compiler, test, runtime or production action is part of this
  checkpoint request.

## Next narrow step when authorized

Verify host-controller dependency proof availability with the pinned toolchain,
resolve the existing contract and lifetime prerequisites, and author the current
immutable implementation plan. Re-admit only its proven, non-overlapping ready
frontier and retain every V1–V12 gate. The current reporting/checkpoint request
does not perform those steps.

## Local checkpoint outcome

The signed local commit attempt exited 128: `1Password: failed to fill whole
buffer`, followed by `fatal: failed to write commit object`. No new checkpoint
commit was created; existing HEAD remains `95652af5`. Signing remains enabled.
This public-safe document is the only staged file. Existing local work traces
remain unstaged, including the implementation contributor's untouched trace.
The configured signer must be restored before retrying the authorized commit.
No push, merge, implementation, build, runtime or production action occurred.
