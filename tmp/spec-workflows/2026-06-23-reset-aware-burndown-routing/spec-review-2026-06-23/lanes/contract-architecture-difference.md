# Lane: Contract, Architecture, And Spec Difference

Status: answered
Verdict: needs revision

Accepted candidate findings:

- The spec does not define the adapter/shared-contract boundary between state DTOs and pure assessment inputs.
- The mixed-window status collapse rule is missing.
- Empty-window and no-effective-window accounts are not classified.

Required revision:

- Name who owns `SelectorQuotaInput -> QuotaWindowFact` adaptation.
- State whether `codex-router-selection` may depend on state DTOs.
- Define collapse rules for mixed eligible/stale/unknown/ineligible windows.
- Decide whether empty relevant windows are unknown fallback, blocked, or excluded.
