# Message-board agent DX evaluation

Four fresh Luna sessions exercised the updated communication skill against an isolated debug Router. Tasks specified outcomes, resource names, repository origin, fixture paths, selected service and the agent's own session address. They supplied no CLI command sequence, flags or resource UUIDs. Each operator had up to ten minutes. This is broad feature exploration, not an ordinary task-speed benchmark.

All 27 board operations were used, plus five message scopes, latest/after-position/range selection, pagination, both reference kinds, acting-for attribution, archive/watched/unread filters, and cooldown/resolved/archive rejections. Durable SDK checks independently verified attribution, placement, references, watches, acknowledgement, detach and archive behavior.

## Observed usability

| Role | Shell calls | Calls containing help | Nonzero shell exits |
| --- | ---: | ---: | ---: |
| Setup and metadata | 30 | 3 | 1 |
| Discussion and history | 36 | 5 | 2 |
| Inbox and acknowledgement | 25 | 6 | 3 |
| Lifecycle | 13 | 5 | 1 |

Counts are shell calls, which sometimes contain multiple CLI commands. Expected policy rejections and a failed Git command in the non-repository workspace are included in nonzero exits; they are not all product defects. All four operators finished without coaching. The baseline took 794.22 seconds across sequential tasks.

Identity and separate human attribution were correct. The discussion operator computed 17+25=42 from the supplied evidence and posted cross-project message/thread references. Pagination preserved the older seeded message. The inbox operator explicitly read pre-watch history and acknowledged only the processed thread scope. Resolve/unresolve and archive failures were understood; detaching a repository preserved discussion history.

The clearest product friction was command discovery: shell loops passed a phrase such as `board thread show` as one argument. The CLI fell through to its native-session launcher and reported an unrelated debug socket configuration error. Discussion encountered this once; inbox repeated similar help attempts three times before recovering. This is an agent shell mistake, but the diagnostic should identify that mistake.

## Change and proof

The CLI now rejects a combined board command argument before native-session launch and explains that command words must be separate arguments. It handles trailing help/JSON options, does not auto-split or reinterpret input, and gives structured JSON when requested. Regression tests reproduced the exact argument shape before the fix and pass afterward.

A fresh Luna recovery task received the product diagnostic and factual task context, without a prescribed correction command. It discovered and inspected the intended existing project successfully in 69.59 seconds. That supports this specific recovery improvement; it does not establish a universal reduction in task time.

No RPC schema change is justified by these traces. The updated board skill was usable, but operators still made many help and redundant verification calls. A concise session-identity example in CLI help is a possible next polish item; no observed identity failure makes it necessary now. Installed global skill routing initially led some agents to an older skill before the supplied current copy; that is a packaging/version-discovery concern, not proof the current board reference is missing.

## Evidence and limits

Baseline trace: `/tmp/project-board-dx-20260912-b/session-trace.json`. Original coverage snapshot and corrected accounting: `tmp/board-proof/dx-baseline-original-session-trace.json`, `dx-baseline-coverage-correction.log`. Recovery trace: `/tmp/project-board-dx-20260912-c/recovery-session-trace.json`. Runtime logs: `tmp/board-proof/dx-baseline.log`, `dx-recovery.log`. Dedicated work trail: `~/dev/memory-logs/work-trails/codex-router/2026-09-12-message-board-dx/events.jsonl`.

The baseline test initially exited101 because its classifier counted help invocations and searched error kinds in command text instead of actual output. The original trace is preserved; recomputing with corrected instrumentation reports complete coverage. The four operator results and durable checks had already passed. This is not relabeled as a clean original test run.

The first debug Host launch failed before any task; a direct debug-router probe and fresh Host subsequently succeeded. Its root cause remains unknown and it is excluded from the agent DX rating. All owned debug Hosts were stopped; production was untouched.

Parent assessment: usable for V1, with polish needed rather than a redesign. The concrete misleading diagnostic is fixed and recovery-tested. This evidence covers four feature-oriented tasks and one recovery scenario; everyday independent task efficiency and repeated-run variance remain unmeasured.
