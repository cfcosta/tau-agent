# Codemode inference traces

Each `tools.infer` call returns an opaque `trace_id`. Its private provenance is
stored as `tau-codemode` plugin records under outer `kind = "inference"`:

1. `phase = "started"` stores the trace ID, owning `RunId`, task, context,
   optional schema, model, and reasoning effort before admission or model work.
2. `phase = "attempt"` reserves a numbered provider attempt before the
   provider can open or respond. Retries each get a reservation and consume a
   call. An open failure has an admitted attempt without a terminal response;
   the provider may still have accepted the request.
3. `phase = "finished"` stores the same ID, selected value or error, bounded
   raw provider text, every attempt's outcome and SDK usage value, summed
   reported usage, and a completion flag. A stored terminal record can have
   `complete = false` when an attempt's final usage is unknown.

Raw output is limited to a UTF-8 prefix of 64 KiB with
`raw_output_truncated = true` when shortened. The `finished` phase means the
trace's terminal record was stored. Its `complete` field means every reserved
attempt received a final SDK report. It does not claim the provider's raw
output is complete.
The selected value and trace ID are the only inference provenance returned to
the main conversation. Private task, context, schema, and raw answer remain in
plugin records. The inspector lists IDs and statuses without displaying those
private fields.

The started and attempt records are stored before their respective work. If
the tool future is dropped or the process crashes before a terminal record,
the inspector reports `interrupted / incomplete`. A failed start write stops
before any provider attempt. A failed terminal write returns an explicit error
with the trace ID and missing-terminal status. Writes are stored before they
are reported to the UI, so a failed terminal write cannot look successful.

The SDK may provide zero-valued default usage when a provider did not report
usage. Only `finished` attempts label their usage `sdk_reported` or
`sdk_zero_or_default`. Cancelled, broken-grammar, no-terminal, interrupted,
and open-failed attempts label final usage `unknown`, even when they contain
nonzero partial `reported_usage`. An open failure does not prove the provider
never accepted the request. The older `no_provider_response` label also
restores as uncertain. Zero is never presented as proof of zero provider
usage. The budget retains every reported usage and cost value exactly once,
including partial values, while unknown final usage blocks later admissions.
`PluginCtx::ask_observed` continues to charge the agent according to its
existing accounting; trace persistence does not charge it again. Partial
stream usage is included only when the SDK observer reported it.

On activation, the plugin reconstructs call count and summed reported usage
from terminal traces owned by **the same** `RunId`. Parent and fork-chain
records remain visible as provenance but do not spend the new run's allowance.
An incomplete attempt has unknown final usage and blocks further same-run
admissions with `incomplete inference budget; use a new run or fork`, including
later admissions in the same activation. A start
without an attempt reservation cannot have reached the provider and does not
consume a call. An inference record with a missing or malformed owner prevents
activation because ownership cannot be established. Malformed same-owner records also prevent activation
instead of silently replenishing allowance. The call, token, and cost
allowances persist for the stored run. Each activation receives a fresh
monotonic deadline; a pause does not replenish the other allowances.

No database schema change is required. Store and module folds ignore inference
records, and older serialized UI states default to an empty inference list.
