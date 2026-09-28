# Agent loop

The loop is inherited from pi's `agentLoop`
(`packages/agent/src/agent-loop.ts`). The ordering guarantees below are
part of tau-agent's contract. Where tau-agent intentionally differs from
pi, a note says so.

## One run

0. Before the run: store it as `running`, start each plugin in
   registration order (`Plugin::start` may change the input, the
   context before it, the instructions and the reasoning effort), then
   open the session with the resulting settings. A plugin that fails to
   start fails the run, which is stored as `failed`
   ([plugins.md](plugins.md)).
1. Emit `RunStart`. Append the input as a user message, after any
   context the plugins added.
2. **Turn loop:**
   1. Emit `TurnStart`.
   2. Build the request from the run's fixed instructions and tools, the
      transcript, and `text.format` if the run is typed. Send it on the
      run's lane.
   3. Stream the response. Each `AssistantEvent` is wrapped as a
      `RunEvent::Message*` and delivered to hooks and subscribers, in
      order and awaited.
   4. Stop reason `error` or `aborted`: emit `TurnEnd` and `RunEnd`,
      then finish the run with that stop reason. Retryable errors are
      retried first (see Retries).
   5. Stop reason `length` while tool calls are pending: fail every
      pending tool call with an error result asking the model to issue
      it again with smaller arguments. Then continue.
   6. Run the tool calls (see Tool execution).
   7. Persist the turn's messages and usage in one transaction.
   8. Check limits and cancellation.
   9. Emit `TurnEnd`.
   10. Drain the steering queue into the transcript.
   11. No tool calls and no steering: ask each plugin's `before_stop`,
       in order. The first that continues adds its text as a user
       message and emits `Continued`, up to `Limits::max_continuations`
       times (default 3) per run. A plugin error there is reported as
       `PluginError` and counts as letting the run stop.
   12. Offer the context to each plugin's `rewrite_context`, in order
       (the order they were added). The first rewrite is checked, stored as a
       `context` entry with its messages, and becomes the transcript;
       the next request goes in full.
   13. Loop while the model called tools, steering added messages, or a
       plugin continued the run.
3. Emit `RunEnd`. Set the run's status and result. Then call each
   plugin's `finish`, before the outcome is returned.

## Tool execution

- **Preparation is sequential and follows source order.** For each call:
  1. Look the tool up. An unknown tool yields an immediate error result.
  2. Run `prepare_arguments`, if the tool defines it.
  3. Run the coercion pass, then JSON-schema validation. A failure
     yields an error result carrying the validation message.
  4. Run the `before_tool` hooks in registration order:
     - the first hook that blocks wins;
     - a hook that returns an error also blocks the call;
     - mutated arguments are validated again. **Deliberate difference
       from pi:** pi runs mutated arguments without validating them again,
       and a test pins that (`agent-loop.test.ts:480`). tau-agent
       validates them, so a hook cannot pass the tool arguments that
       the tool's schema rejects.
- **Execution is parallel** across the prepared calls. A tool whose
  `execution_mode()` is `Sequential` makes the whole batch sequential.
- **Events:** `ToolStart` in source order; `ToolUpdate` while a tool
  runs; `ToolEnd` in completion order.
- **Result messages** are appended in source order.
- **Errors:** a tool that returns `Err` produces a result with
  `is_error = true`. Its text is the error message.
- **After each tool:** `after_tool` hooks may patch the output field by
  field.
- **Updates after completion:** a tool that sends updates after its
  future has resolved has those updates ignored.

## Coercion before validation

tau-agent reproduces pi's `validateToolArguments`
(`packages/ai/src/utils/validation.ts:317`):

1. Clone the arguments.
2. `null` on an optional property counts as absent.
3. Lenient conversion as typebox `Value.Convert` does it:
   - a string that parses as a number becomes a number where the schema
     says `number` or `integer`;
   - `"true"` and `"false"` become booleans;
   - a single value becomes a one-element array where the schema says
     `array`.
4. Validate against the compiled schema.

Without the coercion pass, tau-agent would reject tool calls that pi
accepts today.

## Steering

`Run::steer(msg)` queues a user message. The loop drains the queue after
the current tool batch, or at once if no batch is running. The message
is appended before the next request. pi's default is one message per
drain, and tau-agent keeps that default.

## Cancellation

- Each run owns a `CancellationToken`. Children created through
  `as_tool` get a child token.
- The provider stream is dropped inside `select!`.
- Tools receive the token and must observe it. The loop never drops
  their futures.
- **Deliberate difference from pi:** when a run is cancelled mid-batch,
  every tool call that has no result yet gets a synthetic error result
  ("cancelled"). The transcript never holds orphaned tool calls. pi
  leaves those calls without results (`agent-loop.ts:572`) and repairs
  them later.

## Retries

- **Retryable:** OpenAI error codes and HTTP statuses for overload, rate
  limits, 5xx and timeouts, plus transport errors that happen before the
  first event.
- **Not retryable:** quota and billing errors. They fail at once.
- **Backoff:** exponential with full jitter, capped at 60 s. The
  defaults are 3 attempts and a 2 s base; `Agent::retry(RetryPolicy)`
  changes them.
- **Where the class comes from:** every failed response's `Error` event
  carries a `retry::Class`, set where the failure is known (the stream
  processor classifies an OpenAI error by its code, type and status; a
  socket that closes before any output is retryable, one that closes
  after is not). The loop never reads the message text to decide.
- **What the loop does:** a failed response that is retried is neither
  stored nor counted as a turn. Each new attempt is announced with a
  `RunEvent::Retry { turn, attempt, delay, error }` inside the turn. A
  cancel during the backoff ends the run as cancelled. The compaction
  summary request uses the same policy.
- **Context overflow** (`context_length_exceeded`): offer the context to
  the plugins' `rewrite_context`, in order; after a rewrite,
  retry once. With no rewrite, the run fails, and the plugins' errors
  join its error. See [`compaction.md`](compaction.md).

## Events

```rust
pub enum RunEvent {
    RunStart   { run: RunId, parent: Option<RunId>, agent: Arc<str> },
    TurnStart  { run: RunId, turn: u32 },
    TextDelta  { run: RunId, parent: Option<RunId>, delta: String },
    ThinkingDelta { run: RunId, delta: String },
    ToolCallDelta { run: RunId, call_id: String, json_fragment: String },
    ToolStart  { run: RunId, call_id: String, tool: Arc<str>, args: Value },
    ToolUpdate { run: RunId, call_id: String, partial: Arc<ToolOutput> },
    ToolEnd    { run: RunId, call_id: String, output: Arc<ToolOutput>, is_error: bool },
    TurnEnd    { run: RunId, turn: u32, usage: Usage },
    ContextRewritten { run: RunId, plugin: Arc<str>, tokens_before: u64, tokens_after: u64 },  // between turns, or in an overflowing turn
    Retry      { run: RunId, turn: u32, attempt: u32, delay: Duration, error: String },
    Continued  { run: RunId, plugin: Arc<str>, message: String },  // between turns
    PluginError { run: RunId, plugin: Arc<str>, message: String },
    RunEnd     { run: RunId, parent: Option<RunId>, stop: StopReason, cost: f64 },
}
```

Events from a child run carry `parent`, so one subscriber can follow a
whole workflow tree.
