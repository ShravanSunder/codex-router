# AGENTS.md

## Local Debug Boundaries

- Never stop, restart, or replace the production Codex router process unless
  the user explicitly says: "replace the production Codex router process".
  Installing a binary does not mean replacing the running process.

- Codex session state is normal Codex state. `codex-router sessions` reads
  `$HOME/.codex/state_5.sqlite` and `$HOME/.codex/sessions/*.jsonl` read-only;
  it must not redirect to a repo-local fake Codex home in debug builds.
- Router-owned runtime state is separate. Debug `cargo run -p codex-router-cli`
  defaults router state/secrets to `$HOME/.codex-router-debug`; installed or
  home-default runs use `$HOME/.codex-router`.
- The debug Codex profile lives in normal Codex home as
  `$HOME/.codex/codex-router-debug.config.toml` and points Codex at the debug
  router port. Keep this profile/config separate from router-owned state.
