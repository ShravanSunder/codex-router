# Debug-only live proof boundary

The owner explicitly requires the debug test profile, a previously unused working folder and new threads for every live test run.

- Use only the existing `codex-router-debug` profile for native/model-backed proof. Inspect its nonsecret endpoint selection before launching anything; never use or replace production processes.
- Allocate a new owner-controlled test root with a UUIDv7 suffix under OS temporary storage. Create new working directories beneath that root. Do not point live tools at the product checkout, an existing workspace, or an existing session directory.
- Use a dedicated absolute debug app-server socket under an owner-controlled test directory. Verify debug port/socket/process identities before submission and before stopping any test-owned process. A port collision is a blocker, not authority to reuse or stop the occupant.
- Create new native threads for the scenario and record their returned IDs. Only address or interrupt IDs created by this proof run. No resume of pre-existing user threads.
- Use Luna for all model-backed test agents. Model selection must fail closed rather than fall back.
- Keep normal Codex home and its existing debug profile semantics; a fresh working directory is not a fake CODEX_HOME. If shared-home startup serialization prevents safe concurrent debug operation, report the blocker rather than changing production.
- Automated SQLite/socket fixtures do not invoke Codex or a model and use their own temporary files. They are integration proof, not live-agent acceptance.

The canonical delivery plan remains `tmp/plan-workflows/2026-09-08-run-owned-automation-delivery.md`. This note clarifies the already-required debug isolation gate; it does not authorize production replacement or relax other proof requirements.
