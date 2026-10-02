# Codemode inference

`tools.infer({ task = "...", context = ... })` is available inside a Codemode
script. The optional `schema` field is a JSON Schema for the answer. The tool
is nested: the agent model does not see a separate `infer` tool declaration.
Codemode refuses to start a run if another tool already has the reserved name
`infer`.

The call returns `{ ok, value, trace_id, usage, error }`. `value` is the answer
text when there is no schema, or validated JSON when there is one. Failures
return `ok = false`, `value = null`, and a readable `error`. `usage` sums every
reported attempt, including failed and retried responses. `trace_id` identifies
private persisted provenance (see [inference traces](codemode-inference-traces.md)). The tool's output metadata has
`provider_output_limit`, indicating whether the provider accepts the requested
2,048-token output ceiling.

The Codemode card lists each inference as a call. While it runs, the row shows
the latest attempt progress and the card remains open. On completion, the row
shows success or failure and the sum of reported cost across every attempt,
including retries and failed responses. The card's total usage includes Jev
requests and returned inference usage for display. Inference usage is already
charged by the agent loop and is not charged again by Codemode. A row with
unknown final usage labels its reported cost as partial; zero SDK usage is not
proof that the provider charged zero. Live rows use Codemode's own row updates
for inference cost and settle to the same `details.calls` rows as stored runs.

Every call sends one user message containing only its task and explicit JSON
context. It opens an independent session, sends no parent transcript, and
declares no tools. The host run's model and optional reasoning effort are used
by default. `Codemode::with_inference_model` can select another model;
`Codemode::with_inference_limits` sets per-run call, concurrency, deadline,
and reported usage thresholds. Attempts across scripts and retries in the same
run share one budget. The deadline and script cancellation cover admission,
session opening, retries, and answer validation. Cancelling a script drops its
active inference future and releases the concurrency slot.

The 64 KiB answer limit and shared deadline are host-enforced. The provider
output ceiling is sent only if the transport reports support. Current ChatGPT
transport does not support it, so `provider_output_limit: false` is reported
in output metadata and calls continue without a provider output cap. Reported
token and cost thresholds block later admissions, but concurrent attempts can
exceed them. Cost is taken from reported usage; none is estimated or invented.

For schema and byte limits, see [inference request](codemode-inference-request.md).
For budget details, see [inference budget](codemode-inference-budget.md).
