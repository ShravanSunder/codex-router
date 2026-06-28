# Account Selection TDD Scenario Spec

Date: 2026-06-28
Status: draft for review; TDD scenario contract

## Product Intent

`codex-router` should maximize usable weekly quota across configured OAuth
accounts while minimizing Codex downtime. Individual account quota exhaustion is
router-owned safety behavior: Codex should not see one account's provider
`usage_limit_reached` while another configured account can serve. Codex should
see a router-level all-accounts-exhausted response only when every configured
account is drained or unusable.

The selector therefore optimizes for perishable quota usage, not for the
healthiest absolute account. Quota that resets soon should be used before quota
that resets later, as long as the soon-reset account can accept work without
creating immediate churn or blocking active long-running work.

This spec narrows the test contract for account selection. It complements
`docs/specs/2026-06-27-account-quota-burn-rate-selection.md` and turns the
selection requirements into scenario-shaped TDD fixtures.

## Normative Companion Inputs

This scenario spec is not standalone. The implementation plan must load and
apply these normative sections from
`docs/specs/2026-06-27-account-quota-burn-rate-selection.md`:

```text
R0. V1 Policy Constants
  weekly_survival_safety_buffer_basis_points
  short_survival_safety_buffer_basis_points
  short_near_reset_threshold_seconds
  same_pool_reset_tolerance_seconds
  same_pool_survival_margin_tolerance_basis_points
  active_session_imbalance_threshold
  usage_limit_suspect_ttl_seconds
  active_session_rollup_bucket_seconds

R1. Strict Next Account
  runtime/proxy owns authoritative live active-session truth
  SQLx owns durable mirror, event history, and rollups
  CLI must label stale/unavailable active-session state

R2-R5. Selection math
  weekly survival, 5h guard, active sessions as measurement not cost,
  same-reset burn-rate estimation, basis-point arithmetic, and confidence

R6-R9. Account blocks, affinity, Codex-safe containment, and SQLx domains
  usage-limit state, reconnect containment, bounded WebSocket error-envelope
  parsing, active_session_events, active_session_rollups, and route-band state

R10. CLI and observability
  human/JSON output and scrubbed telemetry requirements
```

All scenario tests in this file must use those defaults unless a row explicitly
overrides a policy input. The only new policy knob introduced by this scenario
spec is the controlled-drain runway horizon.

## Requirements

### R1. Full-Matrix Scenario Inputs

Every account-selection test that claims to validate account choice must include
the full routing matrix for every account:

```text
AccountSelectionScenario
  now_unix_seconds
  route_band
  starts_to_simulate
  accounts[]
    account_id
    enabled
    has_active_credential
    usage_limit_state
    current_active_sessions
    five_hour_window
      status
      remaining_basis_points
      reset_in_seconds
      burn_basis_points_per_hour
      burn_confidence
      projected_exhaustion_in_seconds
    weekly_window
      status
      remaining_basis_points
      reset_in_seconds
      burn_basis_points_per_hour
      burn_confidence
      projected_exhaustion_in_seconds
  expected
    selected_sequence[]
    final_active_sessions[]
    account_states[]
    reason_codes[]
```

Tests that omit either the 5h window, weekly window, burn rate, reset time,
current active sessions, or exhaustion state are not account-selection tests.
They may be lower-level unit tests, but they cannot prove account routing
behavior.

### R2. Mutating Multi-Start Harness

Scenario tests must simulate session starts. A scenario with `starts_to_simulate
= 5` does not call the selector five times with static inputs. It must mutate
the selected account's active-session count after every start:

```text
start 1
  assess full matrix
  assert selected account
  selected.current_active_sessions += 1

start 2
  assess full matrix with mutated active sessions
  assert selected account
  selected.current_active_sessions += 1

repeat through starts_to_simulate
```

This is the minimum proof that active-session balancing is real. Static loops
that repeatedly select the same account without mutating active sessions are
invalid for this requirement.

The mutation simulates runtime/proxy authority over live reservations. Pure
selector tests mutate the in-memory scenario input. SQLx projection tests prove
the durable mirror, lifecycle events, and rollups can produce the same selector
input. Proxy tests prove the runtime reservation lifecycle is mirrored into
SQLx without making SQLx the authoritative live source.

### R3. Burn Rate And Exhaustion Projection Are First-Class Inputs

The selector must consider:

- observed burn rate for the 5h window;
- observed burn rate for the weekly window;
- burn confidence for each window;
- projected exhaustion time after adding one new session;
- reset time for each window;
- current active session count.

Burn rates are represented in basis points per hour. Sub-percent burn rates
must not round away. For example, `0.39%/h` is `39` basis points per hour, not
`0%/h`.

Projected exhaustion is the candidate projection after adding one new session
to the candidate account. Tests may provide explicit
`projected_exhaustion_in_seconds` values for boundary rows. When tests derive
the value, they must use the 2026-06-27 spec's basis-point math:

```text
projected_active_session_count = current_active_sessions + 1

if active-session history is sufficient:
  projected_burn_basis_points_per_hour =
    per_session_burn_basis_points_per_hour * projected_active_session_count

else if quota history is sufficient:
  projected_burn_basis_points_per_hour =
    aggregate_account_burn_basis_points_per_hour

projected_exhaustion_in_seconds =
  remaining_basis_points / projected_burn_basis_points_per_hour * 3600
```

When division is inexact, selector math rounds projected burn up for safety.
Display rounding must not affect selector comparisons.

### R4. Drain Pool Before Far-Reset Reserve

The selector forms a drain pool from accounts with the nearest weekly reset that
can still accept new work. Accounts with far later weekly resets are reserve.
Pool membership inherits the 2026-06-27 spec defaults:
`same_pool_reset_tolerance_seconds = 7_200` and
`same_pool_survival_margin_tolerance_basis_points = 500`.

The drain pool wins even when a reserve account has more absolute weekly
headroom, because soon-reset quota is perishable. The reserve pool is used when
the drain pool is hard-blocked, exhausted, fails the 5h guard, lacks
credentials, is stale/unknown with known alternatives, or is projected to create
unsafe immediate churn.

For v1 scenario tests, "can still accept new work" means one of:

- the account survives to its weekly reset after adding one projected session;
- the account is a controlled-drain candidate with projected runway at or above
  `controlled_drain_min_runway_seconds`; or
- all alternatives are worse or unavailable, in which case the expected behavior
  must be explicit in the scenario row.

The scenario fixture must expose `controlled_drain_min_runway_seconds` as a
policy input. The recommended initial value is fifteen minutes for reactive
reconnect safety tests and six hours for long-running work placement tests. A
test must name which horizon it is exercising.

### R5. Balance Active Sessions Inside The Drain Pool

Inside the same useful weekly reset pool, new sessions should spread across
accounts according to current active session count and projected burn. The
selector must not keep sending all new work to one account while a comparable
same-pool account has fewer active sessions.

### R6. Exhaustion Safety Net

If an upstream WebSocket account emits a recognized quota-exhaustion envelope
and another account can serve, the router must:

1. mark the exhausted account blocked/suspect for the route band in SQLx state;
2. retire/release that account's active reservation;
3. send Codex the source-backed `websocket_connection_limit_reached` reconnect
   signal;
4. ensure the old socket does not forward more work to the exhausted account;
5. reselect on reconnect from the remaining usable accounts.

If all accounts are exhausted or unusable, the router must return a router-level
all-accounts-exhausted response. It must not leak one account's raw provider
quota body.

The reconnect signal is allowed only after the router successfully records the
account exhaustion state and verifies at least one alternative account can
serve. If the router cannot record exhaustion state or cannot verify the
alternative account set, Codex must receive a router-level
quota-state-unavailable safety response, not a raw provider quota body and not
the reconnect signal.

Quota-error detection is bounded to complete WebSocket text messages that parse
as Responses provider error envelopes with explicit account-exhaustion
`error.code` or `error.type` values. Binary frames, malformed JSON, non-error
JSON, prompt/tool/message payloads, deltas, and arbitrary JSON containing quota
words are pass-through messages for quota purposes.

## Boundary / Separability Map

```text
test scenario fixture
  owns: full account-selection input matrix, expected sequence
  exposes: pure selector inputs and SQLx projection fixtures

        feeds
        ▼

codex-router-selection
  owns: deterministic account choice, reason codes, account states
  does not own: SQLite, WebSocket, Codex payload semantics

        compared with
        ▼

codex-router-state SQLx projection
  owns: quota windows, quota history, active-session counts,
        active-session rollups, usage-limit state
  exposes: same pure selector input shape

        exercised by
        ▼

codex-router-proxy
  owns: runtime account selection, exhaustion containment,
        reconnect signal, active reservation lifecycle
  does not own: prompt/tool/message payload semantics

        displayed by
        ▼

codex-router quota
  owns: human and JSON explanation of the same selector result
```

## Test Fixture Shape

The pure selector test harness should use a readable fixture builder rather than
ad hoc per-test construction. This shape is the authoritative minimum fixture
contract. The compact scenario tables below are a coverage index only; a test
does not satisfy this spec unless its executable fixture includes every required
input and expected output below.

```rust
struct AccountSelectionScenario {
    name: &'static str,
    now_unix_seconds: i64,
    route_band: &'static str,
    starts_to_simulate: u32,
    policy: AccountSelectionPolicyFixture,
    accounts: Vec<AccountFixture>,
    expected: ExpectedSelectionFixture,
}

struct AccountSelectionPolicyFixture {
    weekly_survival_safety_buffer_basis_points: u32,
    short_survival_safety_buffer_basis_points: u32,
    short_near_reset_threshold_seconds: u64,
    same_pool_reset_tolerance_seconds: u64,
    same_pool_survival_margin_tolerance_basis_points: u32,
    active_session_imbalance_threshold: u32,
    usage_limit_suspect_ttl_seconds: u64,
    active_session_rollup_bucket_seconds: u64,
    controlled_drain_min_runway_seconds: u64,
}

struct AccountFixture {
    id: &'static str,
    enabled: bool,
    has_active_credential: bool,
    usage_limit_state: UsageLimitFixtureState,
    current_active_sessions: u32,
    expected_pool_role: ExpectedPoolRole,
    five_hour: WindowFixture,
    weekly: WindowFixture,
}

struct WindowFixture {
    status: QuotaWindowStatus,
    remaining_basis_points: u32,
    reset_in_seconds: u64,
    burn_basis_points_per_hour: Option<u32>,
    confidence: QuotaRunRateConfidence,
    projection: ProjectionFixture,
}

enum ProjectionFixture {
    ExplicitPerStart {
        projected_exhaustion_after_start_seconds: Vec<Option<u64>>,
    },
    DerivedFromPerSessionBurn {
        per_session_burn_basis_points_per_hour: u32,
        aggregate_burn_basis_points_per_hour: u32,
        active_session_history_sufficient: bool,
    },
    DerivedFromAggregateBurn,
    NoBurnObserved,
}

struct ProjectionTraceExpectation {
    account_id: &'static str,
    window: &'static str,
    projected_exhaustion_in_seconds: Option<u64>,
    projected_exhaustion_after_each_start: Vec<Option<u64>>,
}

struct ExpectedSelectionFixture {
    selected_sequence: Vec<&'static str>,
    final_active_sessions: Vec<(&'static str, u32)>,
    account_states: Vec<(&'static str, ExpectedAccountState)>,
    reason_codes: Vec<(&'static str, &'static str)>,
    projection_trace: Vec<ProjectionTraceExpectation>,
    all_accounts_exhausted: bool,
    reconnect_signal_expected: bool,
    quota_state_unavailable_expected: bool,
}
```

The fixture may derive `projected_exhaustion_in_seconds` from remaining
basis-points and burn rate, but tests that target boundary math should set the
projection explicitly.

Every multi-start fixture must name the exact selected sequence and a projection
trace for rows where projection changes the selected account. Expected values
such as `A/B`, `A or B`, `according to active balance`, or `until gap closes`
are useful prose, but they are not executable assertions and cannot be counted
as TDD proof.

For projection-driven rows, the test must either:

```text
provide ExplicitPerStart projected exhaustion values for each start
```

or:

```text
provide per-session burn, aggregate burn, and history-sufficiency fields so the
test can recompute projection after every current_active_sessions mutation
```

Hardcoded selected sequences without replayable projection inputs do not satisfy
this spec.

## TUI Scenario Matrix

```text
┌────┬──────────────────────┬──────────────────────────┬──────────────────────┬────────────────────┐
│ id │ situation            │ account A                │ account B            │ expected           │
├────┼──────────────────────┼──────────────────────────┼──────────────────────┼────────────────────┤
│ 1A │ one safe account     │ 5h safe, weekly survives │ none                 │ A                  │
│ 1B │ one 5h unsafe        │ 5h runs out before reset │ none                 │ no account         │
│ 1C │ one weekly unsafe    │ weekly runout before     │ none                 │ A only as          │
│    │                      │ reset, enough runway     │                      │ controlled drain   │
│ 1D │ one exhausted        │ usage-limit active       │ none                 │ all exhausted      │
│ 1E │ one unknown          │ unknown quota            │ none                 │ unknown fallback   │
└────┴──────────────────────┴──────────────────────────┴──────────────────────┴────────────────────┘
```

```text
┌────┬──────────────────────┬──────────────────────────┬──────────────────────┬────────────────────┐
│ id │ situation            │ account A                │ account B            │ expected           │
├────┼──────────────────────┼──────────────────────────┼──────────────────────┼────────────────────┤
│ 2A │ same pool balance    │ reset 24h, active 3      │ reset 25h, active 0  │ B                  │
│ 2B │ soon reset wins      │ reset 24h, survives      │ reset 5d, healthier  │ A                  │
│ 2C │ soon reset unsafe    │ reset 24h, 5h guard fail │ reset 5d, safe       │ B                  │
│ 2D │ known beats unknown  │ unknown burn/confidence  │ known survivor       │ B                  │
│ 2E │ exhausted skipped    │ usage-limit active       │ safe                 │ B                  │
└────┴──────────────────────┴──────────────────────────┴──────────────────────┴────────────────────┘
```

```text
┌────┬──────────────────────┬────────────────┬────────────────┬────────────────┬───────────────────┐
│ id │ situation            │ account A      │ account B      │ account C      │ expected          │
├────┼──────────────────────┼────────────────┼────────────────┼────────────────┼───────────────────┤
│ 3A │ drain pool balance   │ reset 24h,     │ reset 25h,     │ reset 4d,      │ B, A/B, B/A; C   │
│    │ before reserve       │ active 1       │ active 0       │ reserve        │ held             │
│ 3B │ all near reset unsafe│ 5h guard fail  │ weekly runout  │ reset 4d safe  │ C                │
│ 3C │ provider exhaustion  │ selected then  │ safe           │ safe           │ reconnect then   │
│    │ with alternatives    │ usage-limit    │                │                │ B/C              │
│ 3D │ all exhausted        │ usage-limit    │ usage-limit    │ usage-limit    │ router all       │
│    │                      │ active         │ active         │ active         │ exhausted        │
│ 3E │ real low weekly pool │ 4%, reset 22h, │ 8%, reset 24h, │ 26%, reset 84h │ B; C reserve     │
│    │                      │ active 1       │ active 0       │ active 1       │                   │
└────┴──────────────────────┴────────────────┴────────────────┴────────────────┴───────────────────┘
```

## Required Scenario Cases

### S1. One-Account Suite

The one-account suite proves that the selector behavior is defined even before
balancing and reserve behavior enter the picture.

```text
┌─────┬─────┬────────┬──────────────┬────────────┬────────┬──────────────┐
│ id  │ 5h  │ weekly │ burn         │ active     │ state  │ expected     │
├─────┼─────┼────────┼──────────────┼────────────┼────────┼──────────────┤
│ S1a │ ok  │ safe   │ low/normal   │ 0          │ usable │ select A     │
│ S1b │ bad │ safe   │ 5h exhausts  │ 0          │ usable │ no account   │
│ S1c │ ok  │ unsafe │ runout 10h   │ 0          │ usable │ controlled   │
│     │     │        │ reset 24h    │            │        │ drain if     │
│     │     │        │              │            │        │ runway ok    │
│ S1d │ ok  │ empty  │ any          │ 0          │ usage  │ all exhausted│
│ S1e │ ok  │ unknown│ unknown      │ 0          │ usable │ unknown      │
│ S1f │ ok  │ safe   │ stale        │ 0          │ usable │ stale/held   │
│ S1g │ ok  │ safe   │ zero burn    │ 3          │ usable │ select A     │
└─────┴─────┴────────┴──────────────┴────────────┴────────┴──────────────┘
```

The S1c policy is not allowed to be implicit. The scenario must specify the
controlled-drain horizon and assert whether the account is selectable under that
horizon. A single-account weekly non-survivor with 10h projected runway is
selectable under a 6h long-running horizon and not selectable under a 15h
long-running horizon.

### S2. Two-Account Suite

```text
┌─────┬─────────────────────┬─────────────────────┬─────────────────────┐
│ id  │ account A           │ account B           │ expected            │
├─────┼─────────────────────┼─────────────────────┼─────────────────────┤
│ S2a │ reset 24h, active 3 │ reset 25h, active 0 │ B                   │
│ S2b │ reset 24h, safe     │ reset 5d, healthy   │ A                   │
│ S2c │ reset 24h, unsafe   │ reset 5d, healthy   │ B                   │
│ S2d │ reset 24h, unknown  │ reset 5d, known     │ B                   │
│ S2e │ usage-limit active  │ reset 5d, safe      │ B                   │
│ S2f │ both same pool      │ same pool           │ sequence balances   │
└─────┴─────────────────────┴─────────────────────┴─────────────────────┘
```

### S3. Three-Account Multi-Start Suite

The three-account suite is the primary selector integration suite. Every row
must simulate at least five session starts unless the row is an exhaustion
terminal case.

```text
┌──────┬──────────────────────────┬──────────────────────────┬──────────────────────────┬──────────────────────────────┐
│ id   │ account A                │ account B                │ account C                │ expected starts 1..5        │
├──────┼──────────────────────────┼──────────────────────────┼──────────────────────────┼──────────────────────────────┤
│ S3a  │ 18%, reset 24h, runout   │ 19%, reset 25h, runout   │ 50%, reset 5d, runout    │ A/B alternate by active     │
│      │ 40h, active 0            │ 42h, active 0            │ 10d, active 0            │ count; C held reserve       │
├──────┼──────────────────────────┼──────────────────────────┼──────────────────────────┼──────────────────────────────┤
│ S3b  │ 19%, reset 24h, runout   │ 18%, reset 25h, runout   │ 34%, reset 4d, runout    │ B until active gap closes,  │
│      │ 30h, active 6            │ 29h, active 0            │ 8d, active 0             │ then A/B; C held reserve    │
├──────┼──────────────────────────┼──────────────────────────┼──────────────────────────┼──────────────────────────────┤
│ S3c  │ 4%, reset 22h, runout    │ 8%, reset 24h, runout    │ 26%, reset 84h, runout   │ B first; A only if B        │
│      │ 10h, active 1, retiring  │ 15h, active 0            │ 24h, active 1            │ becomes less safe; C held   │
│      │                          │                          │                          │ until A/B unsafe            │
├──────┼──────────────────────────┼──────────────────────────┼──────────────────────────┼──────────────────────────────┤
│ S3d  │ 30%, reset 20h, runout   │ 40%, reset 22h, runout   │ 90%, reset 6d, runout    │ A/B drain pool; C held even │
│      │ after reset, active 2    │ after reset, active 0    │ after reset, active 0    │ though C is healthiest      │
├──────┼──────────────────────────┼──────────────────────────┼──────────────────────────┼──────────────────────────────┤
│ S3e  │ 20%, reset 24h, 5h       │ 22%, reset 26h, weekly   │ 60%, reset 5d, runout    │ C; both near-reset accounts │
│      │ guard fails, active 0    │ runout 2h, active 0      │ after reset, active 0    │ are unsafe                  │
├──────┼──────────────────────────┼──────────────────────────┼──────────────────────────┼──────────────────────────────┤
│ S3f  │ 40%, reset 24h, normal   │ 42%, reset 25h, stale    │ 65%, reset 5d, normal    │ A; stale/unknown B does not │
│      │ confidence, active 2     │ confidence, active 0     │ confidence, active 0     │ beat known drain account    │
├──────┼──────────────────────────┼──────────────────────────┼──────────────────────────┼──────────────────────────────┤
│ S3g  │ usage-limit active       │ 25%, reset 24h, runout   │ 70%, reset 5d, runout    │ B; A hard-blocked, C held   │
│      │                          │ after reset, active 0    │ after reset, active 0    │ reserve                     │
├──────┼──────────────────────────┼──────────────────────────┼──────────────────────────┼──────────────────────────────┤
│ S3h  │ selected then emits      │ 25%, reset 24h, runout   │ 70%, reset 5d, runout    │ reconnect signal, then B;   │
│      │ usage_limit_reached      │ after reset, active 0    │ after reset, active 0    │ A excluded after mark       │
├──────┼──────────────────────────┼──────────────────────────┼──────────────────────────┼──────────────────────────────┤
│ S3i  │ usage-limit active       │ usage-limit active       │ usage-limit active       │ router all-accounts         │
│      │                          │                          │                          │ exhausted                   │
├──────┼──────────────────────────┼──────────────────────────┼──────────────────────────┼──────────────────────────────┤
│ S3j  │ 20%, reset 24h, runout   │ 21%, reset 24h, runout   │ 80%, reset 3d, runout    │ C only if controlled-drain  │
│      │ 5h, active 0             │ 7h, active 0             │ after reset, active 0    │ horizon is 10h or higher;  │
│      │                          │                          │                          │ otherwise B then A/B       │
├──────┼──────────────────────────┼──────────────────────────┼──────────────────────────┼──────────────────────────────┤
│ S3k  │ 30%, reset 2h, runout    │ 32%, reset 26h, runout   │ 75%, reset 5d, runout    │ A until reset-safe margin   │
│      │ after reset, active 0    │ after reset, active 0    │ after reset, active 0    │ closes; then B, not C       │
├──────┼──────────────────────────┼──────────────────────────┼──────────────────────────┼──────────────────────────────┤
│ S3l  │ 30%, reset crossed and   │ 28%, reset 24h, runout   │ 65%, reset 5d, runout    │ B; A's old segment history  │
│      │ refreshed to 95%         │ after reset, active 0    │ after reset, active 0    │ must not create fake burn   │
├──────┼──────────────────────────┼──────────────────────────┼──────────────────────────┼──────────────────────────────┤
│ S3m  │ 18%, reset 24h, burn     │ 18%, reset 24h, burn     │ 50%, reset 5d, burn      │ B/A balance by projected    │
│      │ 0.8%/h, active 0         │ 0.4%/h, active 1         │ 0.2%/h, active 0         │ runway, not just active     │
├──────┼──────────────────────────┼──────────────────────────┼──────────────────────────┼──────────────────────────────┤
│ S3n  │ 18%, reset 24h, burn     │ 19%, reset 24h, burn     │ 20%, reset 24h, burn     │ spread across A/B/C by      │
│      │ 0.4%/h, active 0         │ 0.4%/h, active 0         │ 0.4%/h, active 0         │ active count; no reserve    │
│      │                          │                          │                          │ when all are same pool      │
└──────┴──────────────────────────┴──────────────────────────┴──────────────────────────┴──────────────────────────────┘
```

Each S3 row must assert:

```text
selected_sequence
final_active_sessions
per-account availability/state
per-account reason code
whether C was held as reserve or entered the drain pool
```

The implementation fixture must make these expected sequences exact:

```text
S3a expected sequence:
  A, B, A, B, A
  final active: A=3, B=2, C=0
  C reason: held_far_reset_reserve

S3b expected sequence:
  B, B, B, B, A
  final active: A=7, B=4, C=0
  C reason: held_far_reset_reserve

S3c expected sequence:
  B, B, B, C, C
  final active: A=1, B=3, C=3
  A reason: retiring_near_zero_quota
  C enters only after B projection drops below controlled-drain runway

S3d expected sequence:
  B, A, B, A, B
  final active: A=4, B=3, C=0
  C reason: held_far_reset_reserve

S3n expected sequence:
  A, B, C, A, B
  final active: A=2, B=2, C=1
  no account is reserve because all three are same effective weekly pool
```

Rows with less specific table prose, such as S3e-S3m, must still be implemented
as full fixtures with exact sequence, final active sessions, per-account state,
reason codes, and pool role before any selector implementation that claims the
row as proof.

Before selector implementation begins, the plan must add an executable-fixture
appendix or fixture source file that expands S1, S2, S3e-S3m, and S5 to the same
quality bar as S4:

```text
full policy
now_unix_seconds
route_band
all account windows
projection mode or explicit per-start projection vector
selected_sequence
final_active_sessions
account_states
reason_codes
pool roles
```

Those rows are required coverage, not optional examples. Implementation may
start with a smaller first red test, but PR-ready proof must include every row.

### S4. Real Observed Low-Weekly Case

This is the scenario that motivated the current discussion. Exact live numbers
can drift, but the fixture shape must remain stable.

```text
┌─────────┬──────────────┬─────────────┬────────────┬────────┬────────────┐
│ account │ weekly left  │ weekly reset│ runout     │ active │ state      │
├─────────┼──────────────┼─────────────┼────────────┼────────┼────────────┤
│ A       │ 4%           │ 22h         │ 10h        │ 1      │ retiring   │
│ B       │ 8%           │ 24h         │ 15h        │ 0      │ drainable  │
│ C       │ 26%          │ 84h         │ 24h        │ 1      │ reserve    │
└─────────┴──────────────┴─────────────┴────────────┴────────┴────────────┘

expected:
  next account: B
  reason: nearest usable weekly drain pool with fewer active sessions
  not expected: C solely because it has more weekly headroom
  not expected: A while B can absorb new work

required executable fixture:
  now_unix_seconds: fixed
  route_band: responses
  starts_to_simulate: 5
  policy: 2026-06-27 defaults plus controlled_drain_min_runway_seconds=21600
  A:
    active=1
    5h remaining=9900bp, reset=4h46m, burn=680bp/h, confidence=normal,
    projection=ExplicitPerStart[14h33m, 14h33m, 14h33m, 14h33m, 14h33m]
    weekly remaining=400bp, reset=22h49m, burn=39bp/h, confidence=normal,
    projection=ExplicitPerStart[10h15m, 10h15m, 10h15m, 10h15m, 10h15m]
    state=retiring_near_zero_quota
  B:
    active=0
    5h remaining=10000bp, reset=4h59m, burn=0bp/h, confidence=insufficient,
    projection=NoBurnObserved
    weekly remaining=800bp, reset=23h56m, burn=53bp/h, confidence=normal,
    projection=ExplicitPerStart[15h05m, 11h18m, 8h42m, 5h28m, 4h12m]
    state=drainable
  C:
    active=1
    5h remaining=9700bp, reset=4h36m, burn=919bp/h, confidence=normal,
    projection=ExplicitPerStart[10h33m, 10h33m, 8h10m, 6h32m, 5h26m]
    weekly remaining=2600bp, reset=84h, burn=105bp/h, confidence=normal,
    projection=ExplicitPerStart[17h20m, 13h, 10h24m, 10h24m, 10h24m]
    state=far_reset_reserve
  expected sequence: B, B, B, C, C
  final active: A=1, B=3, C=3
  projection trace:
    B weekly after each selected start: 15h05m, 11h18m, 8h42m
    C weekly after each selected start: 17h20m, 13h
  reason transition:
    starts 1-3 choose B because B is the nearest usable drain-pool account
    start 4 chooses C because B drops below the 6h controlled-drain runway
    start 5 stays on C because A remains retiring and B remains below runway
```

### S5. Realistic Active-Session Mutation Case

This case proves that the selector sequence changes as active sessions change.

```text
initial:
  A: weekly 18%, reset 24h, burn 0.40%/h, active 0
  B: weekly 19%, reset 25h, burn 0.40%/h, active 0
  C: weekly 50%, reset 5d,  burn 0.20%/h, active 0

expected starts:
  1 -> A
  2 -> B
  3 -> A
  4 -> B
  5 -> A

expected final active:
  A=3
  B=2
  C=0

C enters only if A/B become unsafe under projection; this fixture keeps A/B
safe, so C must remain reserve.
```

### S6. WebSocket Exhaustion Containment Case

```text
initial:
  A selected for WebSocket
  B selectable
  C selectable

upstream A sends:
  {"type":"error","error":{"code":"usage_limit_reached"}}

expected:
  A marked suspect/exhausted in SQLx for responses route band
  A active reservation retired/released
  Codex receives websocket_connection_limit_reached
  old tunnel closes before forwarding more request frames to A
  reconnect selection excludes A
  if B/C unavailable, Codex receives router all-accounts-exhausted instead
  if exhaustion marking or alternative verification fails, Codex receives
  router quota-state-unavailable instead
  client-visible response does not contain usage_limit_reached, account labels,
  provider body text, tokens, prompts, or filesystem paths
  mock upstream capture proves no further client data frames are forwarded to A
```

## Proof Expectations

The implementation plan must translate this spec into these proof layers:

```text
unit:
  pure selector scenario harness with mutating active sessions
  one-account, two-account, three-account scenario suites

integration:
  SQLx projection fixture creates the same pure selector input
  active-session rollups affect burn projection
  usage-limit state excludes accounts before selection

proxy:
  runtime selection uses the same result as the pure selector
  WebSocket usage-limit containment emits reconnect signal when alternatives exist
  all-accounts-exhausted emits router-level exhausted signal
  quota-state-unavailable is emitted if exhaustion marking or alternative
  verification fails
  upstream frame capture proves stale sockets do not receive later work
  client-visible payload assertions prove raw provider quota bodies do not leak

cli:
  codex-router quota displays the same selected account and reason
  human output shows active sessions, burn, reset, runout, and reason codes
  no fake score/transport-cost/headroom-cost output

smoke/e2e:
  installed Codex smoke proves the reconnect path through codex-router on a
  debug port and isolated router state before claiming runtime readiness
  installed codex-router quota proof verifies the user-facing command path and
  installed binary version
  if live auth/quota state is unavailable, the plan must name the blocker and
  preserve lower-layer proof without claiming end-to-end readiness
```

## Review Packet Contract

Any reviewer for this spec must be told the product optimization target before
reviewing individual test rows:

```text
codex-router is an account router.

Optimize for:
  maximize usable weekly quota across all configured accounts
  minimize downtime for 6-15 hour Codex work
  keep Codex from seeing one account's quota exhaustion while another account
  can serve
  use soon-reset quota before far-reset reserve when safe
  balance active sessions inside the same useful reset pool

Do not optimize for:
  healthiest account by absolute remaining quota
  synthetic WebSocket/HTTP cost
  smooth weighted fairness
  minimum score fallback
  per-message WebSocket account switching
  parsing Codex payloads beyond bounded quota error envelopes
```

Reviewers and planners must also load the normative companion inputs from
`docs/specs/2026-06-27-account-quota-burn-rate-selection.md`. This file owns
scenario coverage and executable fixture shape; the 2026-06-27 spec owns the
selector constants, core selection math, active-session history domains, SQLx
state domains, and CLI/OTEL contracts.

Review must explicitly answer:

```text
1. Does every account-selection test carry the full 5h + weekly + burn +
   reset + active-session matrix?
2. Do multi-start tests mutate active sessions after each selected start?
3. Do the three-account cases prove drain-pool usage before far-reset reserve?
4. Do the cases prove active-session spreading inside a same reset pool?
5. Do the cases prove burn-rate and projected exhaustion can override naive
   active-count balancing?
6. Do usage-limit cases prove Codex sees reconnect/all-exhausted behavior, not
   one account's provider quota body?
7. Are all open policy knobs named as knobs instead of hidden in expected rows?
```

## Non-Goals

- No WebSocket-vs-HTTP quota cost.
- No synthetic headroom cost.
- No smooth weighted deficit fallback.
- No selecting weak accounts because of a minimum score.
- No provider payload parsing beyond bounded Responses error-envelope
  classification for quota containment.
- No quota detection from binary frames, malformed JSON, non-error JSON,
  prompt/tool/message payloads, deltas, or arbitrary JSON containing quota
  words.
- No account switch inside one already-open upstream WebSocket except through
  Codex-compatible reconnect after exhaustion containment.

## Open Policy Knob

The remaining policy knob is the controlled-drain runway horizon:

```text
short reactive horizon:
  enough time to mark exhaustion, send reconnect, and reselect without immediate
  churn; recommended starting point 15 minutes

long-running placement horizon:
  enough projected runway for 6-15 hour Codex goals; recommended test values
  must include 6h, 10h, and 15h boundaries
```

Selector tests may classify controlled-drain eligibility before proxy proof is
complete. Runtime activation of controlled drain is allowed only when reconnect
containment proof is green for the route band. That aligns with the product
objective: maximize usable weekly quota while preventing Codex from being
blocked until the entire router account pool is exhausted.
