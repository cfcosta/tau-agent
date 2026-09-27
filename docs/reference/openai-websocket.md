# OpenAI Responses over WebSocket

Source: OpenAI's WebSocket mode guide,
<https://developers.openai.com/api/docs/guides/websocket-mode>, as read
on 2026-09-26. Re-check it before M1 starts; the mode is recent.

## Connection

- **Endpoint:** `wss://api.openai.com/v1/responses`.
- **Authentication:** `Authorization: Bearer $OPENAI_API_KEY`, sent on
  the upgrade request.
- **Lifetime:** at most 60 minutes per connection. tau-agent rotates at
  55 minutes, which is the same margin pi uses.

## Client message

Each turn is one `response.create` message. Its body is the normal
Responses create body, with these differences:

- **Forbidden fields:** `stream` and `background`.
- **`stream_id`** is optional: 1–256 characters from `[A-Za-z0-9_.-]`.
  Omit it to use the default lane.
- **`previous_response_id`** continues from a response the connection
  still holds.
- **`store`:** tau-agent always sends `false`.
- **`generate: false`** runs a warm-up: it prepares state and returns a
  response id without producing output.

```json
{
  "type": "response.create",
  "stream_id": "run_01J…",
  "model": "gpt-5.5",
  "store": false,
  "instructions": "…",
  "tools": [ … ],
  "include": ["reasoning.encrypted_content"],
  "previous_response_id": "resp_abc",
  "input": [
    { "type": "function_call_output", "call_id": "call_xyz", "output": "…" }
  ]
}
```

## Server events

Streaming events use the same `response.*` types as the SSE transport:

- `response.output_text.delta`
- the reasoning and function-call argument deltas
- `response.in_progress`, sent mid-stream
- one terminal event: `response.completed`, `response.failed` or
  `response.incomplete`

Events on a named lane carry its `stream_id`, and events on the default
lane omit it. Errors look like this:

```json
{
  "type": "error",
  "code": "previous_response_not_found",
  "message": "Previous response with id 'resp_abc' not found.",
  "param": "previous_response_id"
}
```

## Limits and lanes

| Limit                              | Value                                              | tau-agent behaviour                                                                               |
| ---------------------------------- | -------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| In-flight responses per connection | 16, across all lanes                               | The pool holds a semaphore per connection. A 17th concurrent run is placed on another connection. |
| Named `stream_id`s per connection  | 32 (the default lane doesn't count)                | Past 32 the pool opens another connection. The error is `websocket_stream_limit_reached`.         |
| Ordering                           | FIFO within one `stream_id`; lanes run in parallel | One run = one lane. Parallel runs never queue behind each other.                                  |
| Connection age                     | 60 min                                             | The pool rotates at 55 min. The error is `websocket_connection_limit_reached`.                    |

## Continuation state

- The connection keeps recent responses in memory. Continuing from a
  response it still holds is fast.
- With `store: false` there is no persisted fallback. An id the
  connection no longer holds fails with `previous_response_not_found`.
- The in-memory cache is evicted after an error and when the connection
  closes.
- Because nothing is written to disk, this mode works with Zero Data
  Retention.

## The delta rule

Ported from pi's `getCachedWebSocketInputDelta`
(`packages/ai/src/api/openai-codex-responses.ts:1438`).

After each `response.completed` on a lane, the lane records three things:

- `body_sans_input`: the full request, minus `input` and
  `previous_response_id`.
- `baseline`: the request's input items, followed by the response's
  output items. Tool outputs are excluded, because the next request
  supplies them.
- `response_id`.

For the next request on that lane, send only
`input[baseline.len()..]` with `previous_response_id = response_id`,
but only if both of these hold:

1. the new request equals `body_sans_input` apart from `input`; and
2. `input[..baseline.len()] == baseline`.

Otherwise send the full input and no `previous_response_id`.

Events that force a full resend:

- a fork's first turn;
- the first turn after a compaction;
- any change to instructions, tools or reasoning settings (these are
  fixed per run, so this should never happen);
- a reconnect.

## Recovery ladder

| Condition                                    | Action                                                                                                                                          |
| -------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| `previous_response_not_found`                | Clear the lane's continuation, then resend in full on the same connection.                                                                      |
| `websocket_connection_limit_reached`         | Reconnect, then resend in full.                                                                                                                 |
| Connection lost before any event was emitted | Reconnect, then resend in full. This counts as one retry attempt.                                                                               |
| Connection lost mid-stream                   | Emit an `error` event with `stopReason = error`. The agent's retry policy decides what happens next.                                            |
| Cancel                                       | Close the lane's continuation. If the socket has other live lanes, keep it open; otherwise close it. The next turn on that run resends in full. |

## Retries are invisible to the caller

A recovery that resends on the same lane or a new connection does not
start a new stream for the caller. The caller sees exactly one `start`
event and no `error` event for the turn. An `error` event appears only
when recovery gives up.

## Usage on abort

OpenAI reports usage only in the terminal event. A turn that is
cancelled before its terminal event therefore records zero usage. Limits
and cost undercount such turns. This is accepted, and it is documented
in `Outcome::usage`.

## Pool statistics

The pool keeps counters, readable as a `PoolStats` snapshot per pool and
per lane:

| Counter              | Meaning                                              |
| -------------------- | ---------------------------------------------------- |
| `full_requests`      | requests sent with the full input                    |
| `delta_requests`     | requests sent as a delta with `previous_response_id` |
| `last_delta_items`   | number of input items in the last delta request      |
| `connections_opened` | sockets opened, including reconnects and rotations   |
| `connections_reused` | requests placed on an existing socket                |
| `recoveries`         | recovery-ladder steps taken, by condition            |

These counters are part of the API. Tests use them as an oracle, and
users use them to measure the delta hit rate. pi keeps the same counters
(`openai-codex-responses.ts:899`).

## Warm-up

`generate: false` with the run's instructions and tools returns a
response id and produces no output. The first real turn then continues
from that id. This pays off for agents that run many times with the same
setup. It is optional and set per agent.

- `Agent::warmup(true)` warms each run's session before its first turn.
  The warm-up is the run's body with no input and `generate: false`.
- The delta rule ignores `generate` when it compares bodies, so the first
  turn goes as a delta carrying its whole input, continuing from the
  warm-up's id.
- A warm-up's usage counts toward the run. A failed warm-up is ignored,
  and the first turn goes in full.

## Things pi does that tau-agent keeps

- It requests `reasoning.encrypted_content`, so full resends can replay
  reasoning when `store` is `false`.
- Tool-call ids are `call_id|item_id`.
- The idle timer and the maximum age are tracked per connection:
  - a connection with no lanes for 5 minutes is closed (pi's cache
    lifetime); one that still has lanes stays, since it holds their
    continuations, and rotation bounds its age;
  - a connection with requests in flight that receives nothing for 5
    minutes (pi's idle timeout) is presumed dead and handled as lost: a
    request with no output yet is resent, pi's "idle before the first
    event" case, and one with output fails.
    Both are `pool::Limits` fields (`idle_timeout`, `stall_timeout`).
