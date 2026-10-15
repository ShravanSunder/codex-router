# Compact storyboard validation

The [current gallery](storyboard.html) has 36 actual compiled fixture screenshots: sessions/quota ×120×36,76×30,48×24 ×six states. The [manifest](help-image-manifest.json) includes six cleanup receipts and exact image/source/tool hashes. All36 embedded and copied PNGs match those receipts and were inspected. The [93-image 0.1.95 archive](storyboard-0.1.95-archived.html) is separate historical proof.

## Current correction snapshot

Captured on 2026-10-15 with version 0.1.96, base commit d448337c plus uncommitted review corrections. The manifest identifies source snapshot `903a19b5b39192487a35c9dd968a1aeb14322f066a7d749649a9bae4e3d7f468`. The published PR head is separate; these results do not establish its CI or readiness.

| Gate | Observed result | Scope |
| --- | --- | --- |
| CLI library |478 passed,0 failed,4 ignored; exit0 |Includes keychain-test-support, legacy Help, Details and original narrow-column tests |
| Compiled terminal suite |19 passed,0 failed; exit0 |Raw legacy Help, provider/disabled filters, paste, terminal restoration and sealed loopback workflows |
| Current capture contracts |Expected RED0/1, then GREEN2/0 |Literal original-column identity cues; full identity remains required in Details |
| Fresh compact capture |1 passed,0 failed; exit0 |Six journeys,36 PNGs individually inspected,18 rejected blank/wrong-state/unapplied images; zero provider requests |
| Strict workspace/all-target Clippy |exit0 |After the capture-owner correction |
| Formatting, whitespace, Rust size |exit0;1910 Rust files |After the capture-owner correction |
| Quota component measurements |24 cases,15 samples each; exit0 |Worst observed p95 63.167ms at160×40,1000 accounts,Details; debug MockTerminal journey, not release or physical-terminal latency |

The first full CLI run had one stale full-name expectation (477/1/4); correcting that test preserved the original narrow columns and quota assertions. The first full PTY run had an options-close wait failure (18/1). Adding diagnostics without changing events, assertions or timeouts was followed by an isolated1/0 and full19/0. That intermittent failure remains unexplained; no cancellation defect is claimed fixed. All original failures and later results are retained for the same affected recheck.

The self-contained HTML embeds exactly the36 receipt-bound PNGs; static integrity and JavaScript syntax checks pass. Browser controls remain unverified after the earlier file-URL access denial. The prior pre-correction gallery remains preserved in the immutable full-review corpus; it is not relabeled as current proof.

## Cancellation readiness qualification

The later controlled component case held the existing Inspected snapshot after Cancel. It observed the `Inspect` body while the action owner still offered Review/Cancel; an early Escape issued another Cancel. Only after the Browse acknowledgement did the Close action appear, and Escape then restored Filters. This demonstrates an unsafe observation marker in the earlier test; it does not establish the exact timing of the original failed attempt or a product cancellation defect.

The two existing tests now wait for the Browse-owned Close action. Events, timeouts, final Filters, zero-POST, read-only and terminal-cleanup assertions are unchanged. The controlled witness passed1/0; the affected component group passed3/0; the compiled cancellation case passed1/0; and the full PTY suite passed19/0, all exit0. Strict workspace/all-target Clippy, formatting, Rust size and whitespace checks passed. This was test-only work: the application, capture fixture and all36 images are unchanged, so capture was not repeated. The earlier18/1 failure remains retained. The same reviewer is checking this bounded proof correction; acceptance of the four original fixes and current pictures is already recorded.

## Prior reviewed snapshot (before corrections)

Version 0.1.96, base commit d448337c plus uncommitted owner-feedback snapshot e110634b (preserved in the immutable full-review corpus). This evidence predates the current QHELP/QDETAIL/QSTATIC/DOC1 corrections. It does not establish those corrected contracts or current published-head CI.

| Gate | Observed result | Scope |
| --- | --- | --- |
| Session library |348 passed,0 failed,3 ignored |Full raw results; later two test-only checked-prefix repairs separately covered |
| CLI library |474 passed,0 failed,4 ignored |Includes keychain-test-support fixture feature |
| Shared dialog library |12 passed,0 failed,1 existing benchmark ignored |Component/render/input scope |
| Affected checked-prefix tests |6 sessions and3 quota passed |Not additional unique scenarios |
| Compiled terminal suite |18 passed,0 failed,0 ignored |Includes full-ID/control-byte paste and terminal restoration |
| Prior compact capture |1 passed,0 failed;36 PNGs and18 negative image checks |Six synthetic-input compiled journeys; zero provider requests |
| Strict workspace/all-target Clippy |exit0 |Separate from ordinary test-build linker warning |
| Formatting, whitespace, Rust size |exit0;1910 Rust files |Snapshot-specific |

Private locked/offline builds used one compiler job. Library/test fixtures and the compiled child do not contact a live vendor. The ordinary CLI test build emitted a linker warning; the later strict check passed separately. Historical counts 335/461/5/17 belong to earlier source and are not these snapshot results.

## Prior component measurements

Exact successful debug test executables measured16 session cases and24 quota cases,15 samples each. Worst observed whole-component journey p95 was 106.566ms for sessions and 59.809ms for quota, at 160×40 with 1000 records and Details. These include state/input/rendering work; they are not release paint timings, before/after speedup or vendor/native latency. Benchmarks do not assert time-based correctness.

## Current entry and limits

Use the [compact capture instructions](../../../crates/codex-router-quota-reset-test-harness/tests/quota_storyboard_capture_test/PROVIDER_FILTERS.md). The current runner selects only compiled_compact_current_storyboard_exports_both_real_tuis; the old full runner/ignored captures and their old shortcut oracles remain historical.

The original independent review returned needs-revision for legacy quota Ctrl+/, unsupported F4-close Help text, narrow static-row allocation and stale proof labels. Local corrections and their affected tests/captures are now recorded above. The same reviewer accepted those four fixes and the current-image/documentation increments. The later cancellation-readiness proof qualification above remains under affected recheck; no whole-PR readiness is inferred.

Local/SSH terminal paste gestures, live provider metadata/quota, browser controls, actual configured NEW/resume/fork, current publication/stack/CI/body and normal native artifact qualification remain separate. Configured action guards do not establish successful source-affine execution. Nothing here authorizes merge, release, install or production replacement.
