# Sessions and quota dialogs — actual TUI storyboard

Current compiled Source code: `ddfbad481b4e2e2d4e5a1873f47e43af92d711ef`, iocraft 0.9.2. All nine capture journeys passed at 120×36, 76×30 and 48×24. The capture verified 93 positive PNG checkpoints and 21 blank/wrong-scene rejections. The gallery below preserves 40 original PNGs from that run.

These are actual compiled components driven through real PTY keyboard input with synthetic sessions/accounts. They show the UI, not live account quota, native launches, remote forks or model acceptance. Provider requests: zero; fixture state/secrets unchanged; UI child exits and owned cleanup passed. Whole-PR independent review, receiving integration and current-head CI remain separate.

Ctrl+F or F3 opens provider filters. Arrows select, Tab changes focus, Enter applies and Escape cancels. Quota Space toggles disabled-account visibility. F4 opens details; Escape returns to the list. Sessions filter by client provider; quota filters by account provider.

Provider dialogs overlay the list; compact active filter summaries remain visible. Compact screens use full-screen dialogs within explicit margins; wide screens use bounded frames. Fixed actions stay outside the scrolling body. Existing sidecar thresholds remain 140 columns for sessions and 160 for quota; all pictured widths exercise vertical dialogs. Background input, resize and scrolling also have component/event tests.

The phone row allocates two status columns instead of six so the complete fixture identity remains readable alongside status and ages. Very long list metadata and quota detail values still use existing abbreviations; these screenshots do not claim every detail string is untruncated.

## 120x36

### Sessions

All sessions

![All sessions](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/120x36-sessions-all.png)

Choose a session client provider

![Choose a session client provider](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/120x36-sessions-chooser.png)

Codex filter applied

![Codex filter applied](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/120x36-sessions-codex.png)

Details overlay for the selected Codex session

![Details overlay for the selected Codex session](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/120x36-sessions-details.png)

Return to the same filtered list

![Return to the same filtered list](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/120x36-sessions-details-closed.png)

Claude Code filter applied

![Claude Code filter applied](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/120x36-sessions-claude.png)

### Quota

All accounts

![All accounts](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/120x36-quota-all.png)

Choose an account provider and disabled visibility

![Choose an account provider and disabled visibility](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/120x36-quota-chooser.png)

OpenAI filter applied

![OpenAI filter applied](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/120x36-quota-openai.png)

Claude filter with disabled accounts hidden

![Claude filter with disabled accounts hidden](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/120x36-quota-hidden.png)

Details overlay for the selected account

![Details overlay for the selected account](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/120x36-quota-details.png)

Return to the same filtered list

![Return to the same filtered list](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/120x36-quota-details-closed.png)

### Empty provider/disabled intersection

Claude selected with every Claude account disabled and hidden

![Claude selected with every Claude account disabled and hidden](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/120x36-quota-empty.png)

## 76x30

### Sessions

All sessions

![All sessions](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/76x30-sessions-all.png)

Choose a session client provider

![Choose a session client provider](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/76x30-sessions-chooser.png)

Codex filter applied

![Codex filter applied](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/76x30-sessions-codex.png)

Details overlay for the selected Codex session

![Details overlay for the selected Codex session](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/76x30-sessions-details.png)

Return to the same filtered list

![Return to the same filtered list](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/76x30-sessions-details-closed.png)

Claude Code filter applied

![Claude Code filter applied](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/76x30-sessions-claude.png)

### Quota

All accounts

![All accounts](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/76x30-quota-all.png)

Choose an account provider and disabled visibility

![Choose an account provider and disabled visibility](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/76x30-quota-chooser.png)

OpenAI filter applied

![OpenAI filter applied](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/76x30-quota-openai.png)

Claude filter with disabled accounts hidden

![Claude filter with disabled accounts hidden](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/76x30-quota-hidden.png)

Details overlay for the selected account

![Details overlay for the selected account](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/76x30-quota-details.png)

Return to the same filtered list

![Return to the same filtered list](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/76x30-quota-details-closed.png)

### Empty provider/disabled intersection

Claude selected with every Claude account disabled and hidden

![Claude selected with every Claude account disabled and hidden](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/76x30-quota-empty.png)

## 48x24

### Sessions

All sessions

![All sessions](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/48x24-sessions-all.png)

Choose a session client provider

![Choose a session client provider](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/48x24-sessions-chooser.png)

Codex filter applied

![Codex filter applied](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/48x24-sessions-codex.png)

Details overlay for the selected Codex session

![Details overlay for the selected Codex session](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/48x24-sessions-details.png)

Return to the same filtered list

![Return to the same filtered list](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/48x24-sessions-details-closed.png)

Claude Code filter applied

![Claude Code filter applied](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/48x24-sessions-claude.png)

### Quota

All accounts

![All accounts](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/48x24-quota-all.png)

Choose an account provider and disabled visibility

![Choose an account provider and disabled visibility](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/48x24-quota-chooser.png)

OpenAI filter applied

![OpenAI filter applied](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/48x24-quota-openai.png)

Claude filter with disabled accounts hidden

![Claude filter with disabled accounts hidden](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/48x24-quota-hidden.png)

Details overlay for the selected account

![Details overlay for the selected account](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/48x24-quota-details.png)

Return to the same filtered list

![Return to the same filtered list](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/48x24-quota-details-closed.png)

### Empty provider/disabled intersection

Claude selected with every Claude account disabled and hidden

![Claude selected with every Claude account disabled and hidden](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/48x24-quota-empty.png)

## Phone list identity regression

Unclassified session remains distinct from Claude Code and keeps its visible identity

![Unclassified session remains distinct from Claude Code and keeps its visible identity](/Users/shravansunder/code/projects/codex-router.router-selector-design/docs/wip/2026-10-13-dialog-storyboard/images/48x24-sessions-all-bottom.png)

## Evidence and limits

Capture: `captures-run5`; one test passed, zero failed, exit 0, 123.99 seconds. The final receipt validation independently compared all nine receipts, 93 PNG hashes/positive markers, 21 negative marker failures, source/tool/binary hashes, child exits and read-only predicates. No native/account/model action was taken.

Image checks establish scene text and reject blank/wrong scenes; exact font fallback, styled-cell fidelity and deployed Ghostty behavior remain unverified. Current performance measurements describe unoptimized render/event pipelines, not native/network latency or a release benchmark. The original failed captures and their fixes remain preserved in the handoff.
