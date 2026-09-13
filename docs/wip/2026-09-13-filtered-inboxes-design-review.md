# Filtered inboxes design working review

## Current state

Target: general-domain. Source head: `6af9ddfe39955a2cea4c43e400e92d3104a6b0ee`.
Three separate draft artifacts are under `docs/specs/2026-09-13-filtered-inboxes-and-search/`.
No source implementation or runtime proof is claimed.

Parent author self-review read all three drafts and the baseline artifacts and checked the current inbox, history, watch/post, summary, SDK, Control, and test seams. F1–F9 have contract/proof homes; the owner resolved F9 as explicit selection on every fetch, with no saved defaults. Search matching and archive behavior are now owner-confirmed; latest cursor behavior now follows the owner-directed ordinary-pagination simplification. The prior conditional proposals have been resolved in the drafts; final structural confirmation and formal readiness review are not claimed. Structural-realization confirmation is pending. Independent readiness review has not run.

Specification-only and program-only classification: `review-required`, forced by owner review request and also matched by public-contract, pagination/state, and cross-component changes. No prior review coverage exists. The requested advisor consult below is candidate guidance on drafts, not a claim of completed formal three-artifact acceptance.

Plain-text diagrams were inspected as shown for scope separation, owner/call direction, state, failure, and return paths. Relative link existence and whitespace are checked by the parent. Remaining concrete contract gaps are in the Specification proposals section; no runtime tests are warranted for this documentation-only turn.

## Job graph

1. Parent: Requirements and Specification draft; closes at complete author read and authorized F-row comparison.
2. Parent: bounded Program Design; depends on 1, preserves open specification choices rather than selecting them.
3. Parent: self-review; depends on 1–2, checks links, scope, source and proof boundaries.
4. Advisor: inspect three drafts and source; depends on 3. Read-only; may run alongside parent link/status checks that do not edit the reviewed drafts. Returns candidate findings. Closes only at parent verification of cited anchors.
5. Parent: disposition and bounded corrections; depends on 4. Unmade product meaning remains an owner decision.

## Advisor ledger

- Relationship: inbox-design-advisor / Advisor; assignment `2026-09-13-inbox-design-advice`.
- Continuity: retain one advisor for clarifications/corrections across these three documents, rather than resending their contents to new sessions.
- Route: Frontier / OpenAI Astra / native `gpt-6-astra` at `high`; fresh history.
- Launcher: native spawn_agent, fork_turns=none. agent-collaboration skill inspected: native child coordination is outside its Router communication scope; no Router message, wake, or production action is needed to launch this native model.
- Permission: workspace read-only; do not edit any repository file. Scratch under tmp only.
- Status: assignment receipt complete; runtime identity `/root/inbox_design_advisor`, model `gpt-6-astra`, effort `high`, history none. Parent verified the three candidate anchors. No advisor writes occurred.
- Expected return: complete/partial/blocked, candidate findings with file/line evidence, consequence and smallest correction; bound to relationship, assignment, three draft paths and source head.
- Parent verification: read cited source/artifact anchors, compare worktree, classify each candidate accepted/rejected/unverified, preserve explicit decision gaps.
- Budget: no extra token-budget allocation requested; compact packet and bounded result, no parent history.

## Document checks

Parent Python link/fence/whitespace check: all three files passed, 17 relative links resolved, 0 failures, exit 0. `git diff --check` exited 0. Working tree contains only the three new design files and this working review note. Source behavior/tests were not modified or executed.

The cooldown source check confirms current admission uses actual actor/board, wall-clock elapsed milliseconds and upward-rounded retry seconds; 60 seconds is the newly authorized duration, not an already-implemented claim. Existing references are loaded as target metadata and do not expand message placement. Current project summary is project-local; new cross-project watched-thread inclusion must not rewrite that summary's meaning implicitly.

## Advisor receipt and parent reduction

Assignment `2026-09-13-inbox-design-advice`, relationship inbox-design-advisor, target three drafts, source head above: complete candidate advice. Advisor read all targets and baseline Requirements/Specification and relevant source; no edits, builds/tests, dispatches or Router messages. Parent checked worktree scope and independently opened the record/decoder/schema anchors.

1. **Accepted: missing located thread-state representation.** `board_messages.rs` ThreadStateChanged has root/topic but no board/project; the inbox decoder cannot infer out-of-project location from a page filter. Existing board_activity already stores project/board/topic. Parent clarified C5 location for both message/state activity, specified projectId on message activity and projectId/boardId on thread-state activity, store-owned ancestry validation, latest message project context, and cross-project state-event proof. Route: spec-design -> program-design. Semantic change stays within F2/C5's already-required actual location; no new state or service.
2. **Accepted as an open policy gap: archive during search continuation.** Parent compared baseline cursor-after-archive contract and current search proposal. Added a concrete conditional proposal: current-state filtering, archived hits removed, cursor remains valid; explicit archived scopes still require inclusion flag. Conditional query/proof realization added. Route: caller, then spec-design -> program-design. This does not claim owner approval.
3. **Accepted as an already recorded owner gap: F9 saved defaults.** Pending asynchronous owner question; no schema chosen and no F9 completion claim. Route: caller. No additional question about cardinality or summaries before this first decision.

Advisor otherwise found attention separation, first-use behavior, cooldown ownership and lack of unnecessary services consistent. Those are candidate source/design observations, not executed runtime evidence. Parent inspection supports them. This was the requested single advisor consult, not formal readiness acceptance. No second review requested solely for freshness.

## Owner resolution: request-only selection

Owner: “Pass the selection on every fetch; no saved defaults.” Updated Requirements F9, Specification R9/C9 and V7, and Program Design request ownership/proof. Removed the saved-default alternative and any proposed persistence. Scope is mandatory on fresh and continuation requests; missing scope fails, concurrent scopes remain independent, existing initialization/bookmarks/watches remain persisted. This is a user-authorized semantic update after the advisor receipt; the earlier receipt is not claimed as review of C9. Search and latest membership proposals remain open. No reviewer rerun merely for freshness.

## Owner resolutions: search and archive

Owner selected simple literal substring matching (ASCII case-insensitive), strict location filters, and newest-first message results, then confirmed explicit archived inclusion with cursor validity while newly archived hits disappear. Updated F5/F6 decision basis, C6/C7, matching/archive proof, and direct-query realization. Project discovery itself has no archived state. Latest watch-change pagination remains pending. These are authorized semantic updates after the advisor receipt, not a claim of new independent review.

## Owner-directed simplification: ordinary latest pagination

Owner rejected overengineering and then said “ok continue.” Removed the proposed watch-set digest and forced restart. Specification and Program Design now use fixed-upper-bound, last-position continuation with current watch eligibility; pages do not revisit passed positions and fresh latest reads pick up newly eligible messages there. This is an explicit limitation, not a complete-membership snapshot claim. Unread remains future-only for new watches. Updated V3 and the structural proof row. No new state, worker, migration, or source implementation. This supersedes the earlier tentative restart answer, which the owner immediately questioned.

## Current design review for plan-only entry

Planning target is plan-only; no source implementation, publication, or tracking-provider changes. The plan-implementation admission rule requires completed design review. Run one whole-mode reviewer followed by scope-defense dispel; no chunks because all three documents fit one coherent review, no executable-proof challenge because documents claim proposed seams only, no focused lanes without a specific residual. Earlier advisor guidance is not reused as formal coverage.

Assignment `2026-09-13-current-inbox-review`: native Delegate `/root/inbox_design_review`, `gpt-6-astra`, high, history none, workspace read-only. Full targets plus baseline Requirements/Specification and exact owner decisions supplied; source head remains `6af9ddfe39955a2cea4c43e400e92d3104a6b0ee`. Scope includes F1–F9 with request-only filters, settled search/archive policies, and ordinary pagination. Structural confirmation basis is owner's direction to keep the existing path/simple pagination and continue. Review packet treats any actual remaining structure decision as a gap, not implied approval.

While the reviewer reads, parent inspects existing CI commands, SQLx preparation, and real debug proof entrypoints for a future plan. No plan is created before admission. No build or test is run during design.

### Completed current three-artifact review

Mode: three-artifact-design. Covered targets: the current Requirements, Specification, and Program Design in `docs/specs/2026-09-13-filtered-inboxes-and-search/`. Governing coverage: baseline Requirements/Specification, current source head above, F1–F9 and subsequent explicit owner choices. Structural scope: existing Control/store ownership, explicit per-request selection, simple direct search and ordinary pagination; no extra persistence or processes.

- Mode-complete `/root/inbox_design_review`: complete, candidate ready, F1–F9 covered, no substantive findings or missing semantic decisions.
- Dispel `/root/inbox_scope_check`: complete, no candidates to classify, all material components/contracts mapped to F/C or baseline obligations, no over-delivery or owner decisions.
- Both used native Astra high with no inherited history and read-only authority. Parent checked unchanged target/source worktree and verified the cited inbox query, summary predicate, client uncertainty mapping, existing cursor path and real proof seams. No source edits or executable proof claimed by either reviewer.
- Chunks: not needed for this coherent target set. Proof-challenge: not applicable; artifacts cite required future proof, not successful executable claims. Focused lanes: no concrete unresolved residual after parent reduction.
- Accepted/rejected/contested/unverified findings: none. Current semantic coverage has no review gap. Parent result: **ready for scoped plan-only work**; implementation and runtime proof remain future obligations. No remediation required in this formal round.

This result covers the final ordinary-pagination design, unlike the earlier advisor receipt. No product-code implementation, PR, release, or production process action is authorized by this review.
