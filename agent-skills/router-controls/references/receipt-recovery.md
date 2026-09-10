# Receipts and recovery

Keep the identities returned by each operation; not every command returns every identity.

| Evidence | Meaning and next action |
| --- | --- |
| Wake saved | Durable definition exists; inspect firing separately. |
| `wakeFired` | First firing recorded; inspect associated delivery for native acceptance. |
| `nativeInputAccepted` | Harness accepted input; this is not task completion or a reply. |
| Delivery `notDispatched` | No native submission recorded yet. |
| Delivery `dispatching` | Submission in progress; inspect rather than duplicate it. |
| Delivery `knownNotSubmitted` | That attempt did not submit; inspect reason and retry eligibility. |
| Delivery `accepted` | Inspect its native receipt and follow the actual turn if needed. |
| Delivery `outcomeUnknown` | Submission may have happened; reconcile, do not blindly resend. |

Delivery disposition (pending, discarded, failed, accepted, uncertain) and native evidence are separate fields. Read both. Let the service own retries for durable deliveries; manually sending a replacement can duplicate its pending work.

```sh
agent-sessions operation show --operation-id "$OPERATION_ID" --json
agent-sessions delivery show --delivery-id "$DELIVERY_ID" --json
agent-sessions delivery attempts --delivery-id "$DELIVERY_ID" --json
agent-sessions run show --run-id "$RUN_ID" --json
```

Operation IDs support recovery only on commands that expose them. Do not assume immediate `message send` has durable operation replay. Preserve submission/turn IDs when returned. For paginated inspection use the returned cursor.

If a wake wait becomes unavailable, retain its wake ID and inspect its current state. A wait failure is neither proof of cancellation nor proof of non-firing. If execution is stopping, distinguish requested interruption from confirmed cessation. Never report a timeout as proof the native work stopped.

A useful response names the operation, exact recipient, observed stage, returned correlation IDs, and any unresolved outcome. Report unavailable capabilities or service access directly instead of claiming delivery based on CLI help or endpoint discovery.
