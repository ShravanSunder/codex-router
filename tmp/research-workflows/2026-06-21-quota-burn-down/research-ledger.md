Research Ledger
═══════════════

Question:
How should codex-router compute quota burn-down, pace, projected runout,
and reset-aware routing when an account has short/session/daily and weekly
quota windows?

Mode:
design-input plus implementation validation

Non-goals:
This ledger does not justify retry, timeout, or provider-health gates. It does
not make Codex sessions router-owned. It does not make additional provider
limits participate in routing without a separate route contract.

Sources:
- codex-router local code: current live quota, CLI display, SQLite snapshot,
  and selector implementation.
- Prodex local prior art: quota models, rendering, runtime summaries, pressure
  scoring, and tests.
- OpenAI rate limit docs: multi-dimensional limits and reset/remaining headers.
- Anthropic rate limit docs: active/current restrictive limit display precedent.
- IETF RateLimit header draft: multiple policies and effective-window examples.
- Google SRE workbook: burn-rate and time-to-exhaustion framing.

Lane Summary:
- local-prior-art lane: completed, high confidence. Prodex keeps 5h and weekly
  windows separate, stores separate reset times, and uses reset-aware pressure.
- external-reference lane: completed, medium-high confidence. Primary docs
  support bottleneck constraints, reset/remaining display, and burn-rate
  projection, with caveats around fixed vs rolling windows.

Evidence:
1. Multiple quota windows should not be collapsed to anonymous primary/secondary
   slots for durable logic.
   class: direct observation
   supports/refutes/complicates: supports semantic window labels
   source: Prodex `crates/prodex-quota/src/render/windows.rs` duration labels
   confidence: high

2. Effective headroom should use the bottleneck window when both short and long
   windows are present.
   class: cited source summary
   supports/refutes/complicates: supports min remaining across applicable windows
   source: OpenAI docs say rate limits can be hit across dimensions depending
   on which occurs first; Anthropic exposes the restrictive active limit; IETF
   examples expose the closest limit to reach.
   confidence: high

3. The effective row should carry the bottleneck window reset, pace, and runout.
   class: inference
   supports/refutes/complicates: supports weekly reset visibility
   source: local display requirement plus bottleneck evidence
   confidence: high

4. Pace can be computed as actual used percent minus expected used percent at
   the current point in the reset window.
   class: inference
   supports/refutes/complicates: supports user-facing ahead/behind display
   source: Google SRE burn-rate/time-to-exhaustion framing
   confidence: medium-high

5. Projected runout can be estimated from observed current-window burn rate:
   `now + remaining_percent * elapsed_seconds / used_percent`.
   class: inference
   supports/refutes/complicates: supports "when will I run out"
   source: burn-rate/time-to-exhaustion pattern
   confidence: medium

6. Reset-aware routing should weight eligible accounts by reset urgency after
   eligibility and bottleneck checks. Quota that resets sooner may be preferred
   even with lower headroom, but not if a longer window is already exhausted.
   class: inference
   supports/refutes/complicates: supports reset-aware weighted selection
   source: IETF effective-window examples, Prodex pressure scoring, expiring
   credit prioritization patterns from external research lane
   confidence: medium

7. codex-router's current persisted snapshot shape is still only one
   `remaining_headroom` and one `reset_unix_seconds` per route band. This is
   enough for a first reset-aware selector weight, but not enough for fully
   lossless weekly-vs-short-window routing state.
   class: direct observation
   supports/refutes/complicates: complicates full routing correctness
   source: `crates/codex-router-state/src/quota_snapshot.rs`
   confidence: high

Synthesis:
- supported:
  - show semantic window labels such as 5h, daily, weekly, and monthly
  - show reset-in, pace, and runout per window
  - show effective as the bottleneck window, not an anonymous aggregate
  - use reset urgency in selection weights after eligibility
- refuted:
  - raw remaining percent alone is sufficient for routing
  - primary/secondary slot names are enough to reason about weekly behavior
- complicated:
  - exact runout is only a projection without historical observation samples
  - rolling/token-bucket limits may not restore fully at `reset_at`
  - routing still needs richer persisted per-window state for full fidelity
- unresolved:
  - whether additional provider limits should participate in routing
  - whether codex-router should add observation-history runway later

Recommended Next Workflow:
Implementation review after the local slice passes; then a follow-up spec/plan
for richer persisted per-window quota snapshots if routing needs full fidelity.
