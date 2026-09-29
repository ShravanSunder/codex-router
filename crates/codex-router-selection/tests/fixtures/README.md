# OpenAI legacy selector oracle

`openai_legacy_oracle.json` is the golden output from the OpenAI selector at parent
commit `05b0be91c7fbdc42c0ed429d5518755b9112c674` (PR1, immediately before PR3).
The generator is kept in `generate_openai_legacy_oracle.rs`; it was copied into the
parent checkout's `crates/codex-router-selection/examples/` directory and run
against that commit, so none of the new selector code produced these expected
values.

The matrix captures both long-window reserve boundaries (10% headroom and 25
pressure), the short-window survival guard with and without a usable peer,
`HeldFloorSwitch`, unknown and stale evidence, a configured hard floor, initial
admission, and far-idle priority before and after the first reservation. Each
snapshot records the selected pool, preferred account, weighted candidates, and
per-account availability, freshness, exclusions, evidence/reason labels, weight,
and priority flags.

To regenerate from a clean checkout of the recorded parent commit:

```sh
git worktree add --detach /private/tmp/codex-router-pr3-parent-oracle 05b0be91c7fbdc42c0ed429d5518755b9112c674
mkdir -p /private/tmp/codex-router-pr3-parent-oracle/crates/codex-router-selection/examples
cp crates/codex-router-selection/tests/fixtures/generate_openai_legacy_oracle.rs /private/tmp/codex-router-pr3-parent-oracle/crates/codex-router-selection/examples/openai_legacy_oracle.rs
CARGO_TARGET_DIR=/private/tmp/codex-router-pr3-oracle-target cargo run --manifest-path /private/tmp/codex-router-pr3-parent-oracle/Cargo.toml -p codex-router-selection --example openai_legacy_oracle --quiet > crates/codex-router-selection/tests/fixtures/openai_legacy_oracle.json
```
