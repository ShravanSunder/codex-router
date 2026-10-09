# Router selector fork cwd and policy decision brief

Status: queued after D1 source-view and D2 provenance decisions. This is source-grounded design input; it does not authorize implementation, policy changes, auth, or remote access.

## Evidence

- Existing default hosted/local fork launch uses the invoking cwd unless an explicit user argument overrides it: `crates/agent-collaboration/src/session_commands/session_launch_selection.rs:127-155`; `crates/codex-native-integration/src/native_session_launch.rs` fork argument construction.
- The selector Program Design proposes source metadata for a configured-source fork and requires the effective directory/origin to be visible, but leaves preservation versus unification open: `docs/specs/2026-10-03-router-selector/2026-10-03-router-selector-program-design.md:219-221`.
- Specification R9 requires the effective fork directory to be visible and its policy to be settled: `docs/specs/2026-10-03-router-selector/2026-10-03-router-selector-specification.md:54`.
- Existing remote launch projection forwards caller-side policy/model settings; native service identity and permitted remote projection remain unverified: Program Design `:116-149`.

## Options

### A. Preserve default behavior; source-owned cwd for qualified configured sources (recommended)

Keep the current default/local fork cwd semantics unchanged. A configured source may use source-owned cwd metadata only after source identity and policy projection are qualified. The popup shows effective cwd and origin before confirmation. Unqualified source metadata rejects the fork.

### B. Unify all same-source forks on source cwd

Make default/local forks use source metadata too. This gives one rule but changes an existing default behavior and adds a new qualification failure to the current journey.

### C. Disable all remote-source forks until cwd/policy proof exists

Keep default/local behavior and reject configured-source forks. This is safest but does not realize the requested same-source fork in configured machine contexts.

## Recommendation

Prefer A. It preserves U3 while making configured-source behavior truthful and visible. D3 remains an owner choice because the cost of changing default cwd behavior and accepting new failure points is product policy, not a planner detail.
