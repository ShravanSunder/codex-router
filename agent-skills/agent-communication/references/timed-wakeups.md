# Timed wake-ups

A wake-up is a timed message, with the same recipient, sender, human-input, content, and delivery options as `message send`.

```sh
agent-sessions wake send --to "$RECIPIENT_ADDRESS" --from "$SENDER_ADDRESS" \
  --text "Check progress and report blockers." --every 10m --for 2h --json
```

Choose timing deliberately: `--after 10m` for a one-shot delay, `--at` for a UTC RFC3339 instant, `--every` for an anchored interval, or `--cron` with an explicit `--timezone`. Cron has five fields: minute, hour, day, month, weekday. Repeating reminders should have the lifetime requested by the user (`--for` or `--until`), rather than silently continuing forever.

Wait intervals (`--after`, `--every`) have two legal regimes. Stay under the provider prompt-cache TTL — 29 minutes as the default ceiling — so a resumed session stays warm. Or use a real calendar schedule: day-scale or longer (`--every 1d`, `--cron` plus `--timezone`). Mid-range waits such as 45 minutes are bad cache management: the cache is already cold, but you are not on a schedule. Do not pick them unless the recipient is Mini, where a cold resume is cheap. `--for` / `--until` is assignment lifetime, not the wait interval; a two-hour watch can still fire every 10 minutes.

Creation returns after durable saving, not native acceptance. Add `--wait-until-first-fire` only when the caller wants to block until the first firing is recorded. It does not wait for acceptance, completion, or a reply. Pause, cancellation, or expiry before firing are not successful firing outcomes. A disconnected waiter does not imply the wake was cancelled.

```sh
agent-sessions wake show --wakeup-id "$WAKE_ID" --json
agent-sessions delivery list --wakeup-id "$WAKE_ID" --json
agent-sessions delivery show --delivery-id "$DELIVERY_ID" --json
```

Pause/cancel discard undispatched reminders; accepted native input cannot be recalled. Resume keeps the original timing and expiry, without replaying paused ticks. Missed repeating ticks coalesce rather than creating a burst.

For recoverable creation, supply a UUIDv7 `--operation-id` and retain it. Recover a lost response using that identity instead of creating another wake. See [receipt recovery](receipt-recovery.md).
