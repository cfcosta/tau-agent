# Codemode inference budget

`tau_codemode_host::inference_budget` provides a host-side budget for
inference attempts. Create one `Arc<Budget>` at the start of an agent run and
share it across every script and retry in that run. The budget does not itself
call a provider or charge `PluginCtx`.

The plugin restores a stored run's call count and reported usage from its
terminal inference traces. A reserved attempt without a final SDK report
blocks further same-run admissions, even when partial usage was reported or
opening the provider failed. A fork has a fresh
allowance; inherited parent traces do not spend it. The monotonic deadline is
new for each activation, while call and reported-usage allowances persist.

```rust
use tau_codemode_host::{
    CancellationToken,
    inference_budget::{Budget, Limits},
};

let budget = Budget::new(Limits::default())?;
let cancel = CancellationToken::new();
let mut permit = budget.admit(&cancel).await?;
// Perform one inference attempt while holding the permit.
permit.report(&usage);
drop(permit);
let snapshot = budget.snapshot();
```

Defaults are 16 attempts, 4 concurrent attempts, a 60-second deadline,
100,000 reported tokens, and US$1.00 in reported cost. `max_calls = 0`
disables all attempts. Concurrency must be positive and fit Tokio's semaphore;
the timeout must make a representable deadline, and an optional cost limit
must be finite and positive. Token and cost limits can be disabled with `None`.

Admission waits for a semaphore slot, subject to cancellation and the shared
deadline. Immediately after the slot is acquired, it checks cancellation,
deadline, call count, reported tokens, and reported cost together with the
counter increment. Waiting requests, cancelled requests, expired requests,
and requests rejected by a limit do not consume a call. Once admitted, an
attempt remains counted even if it fails or is cancelled. Dropping its permit
releases the slot. Each permit adds reported `Usage` at most once through
`report`; if an attempt produces no usage report, it adds none.

Token accounting uses `input + output + cache_read + cache_write`, matching
the agent's run limit. Cost accounting uses `Usage.cost.total`. A threshold
blocks later admissions when reported usage reaches it. This is a **soft**
budget: concurrent in-flight attempts can report after admission and cause
the final total to exceed a token or dollar threshold. `snapshot()` returns
the admitted call count, current in-flight count, and accumulated reported
usage from one locked state.
