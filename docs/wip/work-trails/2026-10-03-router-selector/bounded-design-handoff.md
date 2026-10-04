# Bounded selector design handoff

Current source head: `7a8cbd8943e6fb2dac23bcde7069203925003918`, branch `router-selector-design`. All product sources remain untouched; the existing design artifacts are captured in signed local-only checkpoint `a0a94f2c24c77e0979e1268ed8108ceda7f01fc9`. A subsequent trail checkpoint records validation and delivery state. Use `git log -- docs/specs/2026-10-03-router-selector docs/wip/work-trails/2026-10-03-router-selector` for actual checkpoint commits; commit history records the actual local checkpoint boundary; no push or merge occurred. This is the requested **partial, reviewable design with explicit gaps**, not executable readiness, independent acceptance, a completed implementation plan or a proved tool loop. The owner-directed bound stops research and further Advisor/reviewer calls here.

## Current artifacts

- [Requirements](../../../specs/2026-10-03-router-selector/2026-10-03-router-selector-requirements.md): U1–U7, settled machine execution meaning, TUI refinement and explicit fork extension.
- [Specification](../../../specs/2026-10-03-router-selector/2026-10-03-router-selector-specification.md): E1–E8, R1–R10, source/destination/default/cancel/unsupported contracts. R10/F2 source browsing is a derived proposal, not an accepted binding.
- [Program Design](../../../specs/2026-10-03-router-selector/2026-10-03-router-selector-program-design.md): JSONC preference shape, NEW-only action, source-affine fork proposal, typed source contexts, ownership/flow/state/proof tables and gaps.
- [Trace](main.md): source findings and relay/receipt dispositions; private coordination references are withheld from this repository copy.

The two project-local illustrations show NEW machine selection and symmetric same-source fork popups. They are proposals, not screenshots/runtime proof; the fork image still omits effective cwd and is explicitly incomplete.

## Substantive shape

Agent Sessions retains Enter/resume and Alt+Enter/fork bindings. Start new leads to configured machine choices; current/default remains available, explicit local mode remains local and direct unnamed NEW remains today's default. Fork popup fixes the source and shows same-source destination first. Other machines are disabled without portability proof. No global Router switch, merged catalog, state-copy/migration, new gateway/auth mechanism or production action is introduced.

A proposed F2 one-machine source view is the Lead's supporting realization for selecting actual sessions on configured machines. It uses existing read-only inventory, not caller-local ID resolution. Its scope/interface/key remain unconfirmed and are among the open questions.

## Existing Advisor conclusions

The sole native Advisor remains `[private coordination id withheld]`. All invoked existing-session print results settled process exit 0, exact session ID, sole modelUsage `claude-opus-5-5`, no permission denials. `xhigh` and exact display title were requested; saved effort/title are not independently observed. No new agent sessions/reviewers were created. Supported results and packets live in ignored `tmp/router-selector/`; no transcript/session-file reads.

Earlier concrete findings corrected and checked by this Advisor: caller policy forwarding versus inferred Host-default-only authority; missing MCP exposure/auth/tool-visibility contract; attachment-time/creation-time service binding; unrequested local directory profiles; NEW's input/cancel/late-result/local-mode behavior. The latest correction figure mismatches (Prepared versus handoff; whole-picker exit versus Esc/back) were corrected by the Lead. These corrections are advice/author evidence, not a formal independent design review.

The latest fork check says the extension is conditionally coherent but not build-ready. It identifies four substantive unresolved seams:

1. **Provenance/default behavior:** current default refresh stamps local rows with a hosted-looking reference; that is not historical Router proof. Distinguish observed source identity from attribution without rewriting local records. Mandatory inspection would change default fork failure behavior; that tradeoff remains open. Remove local bare-ID model/metadata fallbacks for configured sources.
2. **Stored view/endpoint:** Stored pages and cursors have no live generation. Exact source endpoint selection and source-consistency rules must be specified; Loaded/Active generation checks cannot be reused unchanged.
3. **Loader/source view:** dispatch owns the existing default-bound record-loader closure. Its source-parameterized request/result boundary needs design. Active configured-source refresh is proposed as explicit, not inherited three-second remote polling. F2 is a derived surface requiring confirmation.
4. **Fork cwd:** current default uses invoking cwd; proposed remote context uses source cwd. Show effective cwd before fork and settle preservation versus unification. Current picture does not yet show it.

Additional bounded UX gaps: provider-row popup versus inert behavior; fork/F2 behavior for missing/empty/invalid registry and local mode; explicit return to caller default source; non-F2 access fallback. No cross-machine portability proof exists, so disabled cross-machine fork is intentional, not an unanswered implementation step.

## Remote and proof gaps

Actual remote MCP exposure/credential presentation/tool visibility, native service/endpoint binding through thread/start or thread/fork, and permitted native caller-policy projection remain unestablished. Upstream 0.160.0 supports ws/wss and env-name token projection, but native metadata has no Router service identity/epoch. Matching names/health/model traffic do not close those gaps. There is no remote runtime or state-portability proof.

Document-only verification: final inline Python validation exit 0; 3 documents, 6 internal links, 6 image embeds, 10 requirement rows, 10 corresponding trace rows and one valid example configuration. `git diff --check` exit 0. Asset pixels inspected and relative embeds exist. Browser policy rejected local file preview; no bypass/workaround, so rendered Mermaid and document placement remain unverified. No Cargo/toolchain/test-suite/runtime/network probe attempted.

Formal review classification is `review-required / matched-risk` for both Specification and Program Design: multiple interacting contracts, source/trust/failure/interface changes. No fresh reviewer was commissioned under the no-new-sessions bound. Structural-realization confirmation is not obtained; it is not implied by the settled placement or popup request. No `locally-ready`/acceptance claim. Phase result remains evidence-blocked/partial on the listed interfaces/proofs; scope ends at this bounded handoff.

Final artifact SHA-256: Requirements `7adae758016be4c61aeff31aa10a4ced598605007c5eb9d9281cb3a0fb88a0fa`; Specification `38e90eeb30bfb5a2959bfc132a1f62368786c683c0cd2fbad857b1852b72ef99`; Program Design `f96c417793cff549f31d94ace8e1bd858d85653f067385c27dbf8b97c7b92e00`.


## Local checkpoint boundary

The owner authorized local checkpoint commits after relocation, preserving every design/implementation hold. Push and merge remain unauthorized. Private coordination IDs and absolute workspace paths are withheld from the public-safe trace and handoff; their originals remain in ignored local scratch storage. [Current document validation](checkpoint-validation-2026-10-04.json) is separate from the historical design receipt and supplies no implementation or runtime proof.
