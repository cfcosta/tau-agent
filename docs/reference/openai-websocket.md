# OpenAI Responses over WebSocket

Source: OpenAI's WebSocket mode guide,
<https://developers.openai.com/api/docs/guides/websocket-mode>, as read
on 2026-09-26. Re-check it before M1 starts; the mode is recent.

## Connection

- **Endpoint:** `wss://api.openai.com/v1/responses`.
- **Authentication:** `Authorization: Bearer <token>`, sent on the
  upgrade request. tau's token is always a ChatGPT plan's access token
  from Sign in with ChatGPT ([`chatgpt-sign-in.md`](chatgpt-sign-in.md)),
  refreshed before each connection. tau takes no API key
  ([0012](../decisions/0012-chatgpt-sign-in-only.md)).
- **Lifetime:** at most 60 minutes per connection. tau-agent rotates at
  55 minutes, which is the same margin pi uses.

## Client message

Each turn is one `response.create` message. Its body is the normal
Responses create body, with these differences:

- **Forbidden fields:** `stream` and `background`.
- **`stream_id`** is optional: 1–256 characters from `[A-Za-z0-9_.-]`.
  Omit it to use the default lane. tau never sends it: the plan route
  may not take it, so each run uses the default lane of a connection
  of its own.
- **`previous_response_id`** continues from a response the connection
  still holds.
- **`store`:** tau-agent always sends `false`.
- **`generate: false`** runs a warm-up: it prepares state and returns a
  response id without producing output.

```json
{
  "type": "response.create",
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
lane omit it. tau uses only the default lane, so a frame belongs to the
run of the connection it came on. Errors look like this:

```json
{
  "type": "error",
  "code": "previous_response_not_found",
  "message": "Previous response with id 'resp_abc' not found.",
  "param": "previous_response_id"
}
```

## Limits and lanes

| Limit                              | Value                                              | tau-agent behaviour                                                                                                                |
| ---------------------------------- | -------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| In-flight responses per connection | 16, across all lanes                               | One response in flight per connection. A run's lane takes a connection by conversation affinity (see "Connection pool").           |
| Named `stream_id`s per connection  | 32 (the default lane doesn't count)                | Not reached: tau sends no `stream_id`, so `websocket_stream_limit_reached` never comes and has no handling.                        |
| Ordering                           | FIFO within one `stream_id`; lanes run in parallel | One run = one lane, on one connection at a time. Parallel runs never queue behind each other.                                      |
| Connection age                     | 60 min                                             | The pool rotates at 55 min. The error is `websocket_connection_limit_reached`.                                                     |

## Connection pool

`tau_ai::ws::proto::pool`. A lane is one run's requests. It belongs to
a conversation, a path of work, given as an `Affinity`: the path (in
tau, the run id, which is also the `prompt_cache_key`) and, for a fork
whose first response has not completed, the parent's path. A
connection remembers the path it serves, the path it served before,
and the continuation its last lane left.

A lane takes a connection when it opens, and when it has none and
sends. In this order:

1. an idle connection of its own path that no other lane holds;
2. for a fork's first request, an idle connection of its parent's path.
   If the parent's live lane holds it, that lane gives it up and takes
   another connection when it next sends; the connection serves the
   fork from then on (a handoff);
3. if a connection of 1 or 2 has a response in flight, that connection
   once it is idle, waiting at most `affinity_wait` (5 s);
4. a free connection (no lane holds it): one that served the lane's
   path before (such as the one a fork took over, given back when the
   fork's run ended), else one that serves no path, else the least
   recently used;
5. a new connection.

A lane keeps its connection while its run lives. When it closes, the
connection keeps its continuation: the next lane of the same path
continues from it by delta when its input extends the baseline. A free
connection that serves a path stays open until rotation (55 minutes);
one that serves no path closes after `idle_timeout` (5 minutes). At
most `max_idle` (8) connections stay free: past it, the least recently
used close, those that serve no path first. Rotation and a lost
connection leave an idle lane without a connection until it sends.
`PoolStats` counts `own_connection` placements, `handoffs` and
`waits`.

## Prompt cache

On this route the prompt cache lives on the connection. Measured on
2026-10-03 with gpt-5.5 and a ChatGPT token, prompts of about 4k
tokens, with `crates/tau-ai/examples/cache_probe.rs`:

| Request                                                                  | Cached                                   |
| ------------------------------------------------------------------------ | ---------------------------------------- |
| Same connection that served the prefix; same key, another key or none    | 85–86% (15 of 16 cases)                  |
| Same, as a delta (`previous_response_id`) or as a full resend            | 85–86%                                   |
| Same connection, `prompt_cache_key` changed mid-chain (a delta is taken) | 85%                                      |
| A new connection; same key, another key or none                          | about 40% of requests, at random; else 0 |
| Same connection, one tool fewer                                          | 0%                                       |
| Same connection, effort low → medium                                     | 0%                                       |
| Same connection, text added at the end of the instructions               | the prefix before it                     |

Re-run on 2026-10-03 (`--handoff`): the source connection read 85–86%
on every request; three new connections read 0%, 85% and 0% (same key,
another key, none); a new one after the source closed read 85%.

- A connection continues only from its most recent response:
  `previous_response_id` of an older one fails with
  `previous_response_not_found`.
- `prompt_cache_key` does not route to a cache on this route; tau sends
  it (each run's path) all the same.
- So tau keeps a conversation on its connection (see "Connection
  pool"), gives every run of a repository the same tools and
  instructions, and keeps a long conversation's effort (tau-reasoning,
  [plugins.md](plugins.md)). See
  [ADR 0022](../decisions/0022-the-prompt-cache-follows-the-connection.md).

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

- `fields`: the full request, minus `input`, `previous_response_id`
  and `generate`.
- `baseline`: the request's input items, followed by the response's
  output items. Tool outputs are excluded, because the next request
  supplies them.
- `response_id`.

For the next request on that lane, send only
`input[baseline.len()..]` with `previous_response_id = response_id`,
but only if both of these hold:

1. the new request's fields equal `fields`; and
2. `input[..baseline.len()] == baseline`.

Otherwise send the full input and no `previous_response_id`.

A change of reasoning effort is a change of fields, so it resends in
full. Continuing across one would not save anything: measured on
2026-09-29 over the Codex backend's WebSocket (a 3.6k-token prompt, two
requests per chain), the server accepts a `previous_response_id` at a
new effort but reports 0 cached tokens, against 2.8k–3.5k at the same
effort. It held for `gpt-6-luna`, `gpt-6-sol`, `gpt-6-astra` and
`gpt-5.6-terra`, in both directions (low→high, high→low, low→medium,
medium→high). A full resend at the new effort also caches nothing.

The check is cheap even for a long transcript, because nothing in it is
copied:

- A request holds its fields and each input item behind an `Arc`. A
  session builds its fields once, and its `InputCache` converts only
  the messages that changed since the last turn, so an unchanged
  message yields the very same item `Arc`s.
- `Arc<Value>` equality compares pointers before contents, so the prefix
  check touches the baseline's items by pointer. Only the last
  response's output items, built separately by the lane and the
  session, are compared by value.
- The lane records its baseline by cloning `Arc`s, and the connection
  task, not the driver every lane shares, serializes the frame.

Events that force a full resend:

- a fork's first turn (its `prompt_cache_key` differs from its
  parent's);
- the first turn after a compaction;
- any change to instructions, tools or reasoning settings (these are
  fixed per run, so this should never happen);
- a reconnect.

## Recovery ladder

| Condition                                    | Action                                                                                                                                      |
| -------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| `previous_response_not_found`                | Clear the lane's continuation, then resend in full on the same connection.                                                                  |
| `websocket_connection_limit_reached`         | Reconnect, then resend in full.                                                                                                             |
| Connection lost before any event was emitted | Reconnect, then resend in full. This counts as one retry attempt.                                                                           |
| Connection lost mid-stream                   | Emit an `error` event with `stopReason = error`. The agent's retry policy decides what happens next.                                        |
| Cancel                                       | Close the lane's continuation and keep the connection; the driver skips the cancelled response's tail on it. The next turn resends in full. |

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
| `connections_reused` | lanes placed on an open socket another lane left     |
| `own_connection`     | lanes placed on a connection of their own path       |
| `handoffs`           | forks placed on their parent's connection            |
| `waits`              | lanes that waited for their path's busy connection   |
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
  - a free connection that serves no conversation is closed after 5
    minutes (pi's cache lifetime); one that serves a conversation, or
    still has its lane, stays, since it holds that conversation's
    cache and continuation, and rotation bounds its age;
  - a connection with requests in flight that receives nothing for 5
    minutes (pi's idle timeout) is presumed dead and handled as lost: a
    request with no output yet is resent, pi's "idle before the first
    event" case, and one with output fails.
    Both are `pool::Limits` fields (`idle_timeout`, `stall_timeout`).
