# 0012: Sign in with ChatGPT only, no API keys

- Status: accepted. Supersedes the authentication of
  [0002](0002-openai-websocket-only.md).
- Date: 2026-09-29

## Context

Since [0011](0011-sign-in-with-chatgpt.md), tau had two ways to reach
OpenAI: an API key (`OPENAI_API_KEY`, or tau-ui's saved `openai-key`)
and a ChatGPT plan through Sign in with ChatGPT. Both use the same
endpoint, `wss://api.openai.com/v1/responses`, and the same protocol.

Keeping both cost more than the second credential:

- Two billing paths. tau-ui had to say which one a run used, rank them,
  explain what happens when the plan refuses, and price the API-key
  catalog. Evals and live checks had to pick one.
- Two transport shapes. API-key clients named lanes with `stream_id`
  and shared connections (32 lanes, 16 responses in flight); plan
  clients could not, and took one lane per connection. The pool, the
  driver and the fake server carried both.
- Two secrets on disk, each with its own storage rules.

Sign in with ChatGPT is OpenAI's documented route for open-source apps.
Users see tau, and limit or disconnect it, in ChatGPT Settings → Usage.

## Decision

- **Sign in with ChatGPT is the only way in.** `OpenAi::chatgpt` is the
  only product constructor. `OpenAi::new`, `OpenAi::from_env`,
  `API_KEY_VAR`, `MissingApiKey` and the API-key connector are gone.
- **One lane per connection, always.** tau sends no `stream_id`. The
  pool places each run on a connection of its own, with one response
  in flight; frames go to the run of the connection they came on. The
  multi-lane machinery (`Limits::max_lanes`, `max_in_flight`, the
  waiting queue, `stream_id` routing, `websocket_stream_limit_reached`
  handling) is gone.
- **The plan route's request rules stay.** Every session sends
  `store: false` and strips the fields the route rejects
  (`chatgpt::UNSUPPORTED_FIELDS`, including `max_output_tokens`).
- **No fallback, anywhere.** tau-ui has no API-key option: a sign-in
  without plan usage offers "Enable ChatGPT plan use" and nothing else.
  CI, evals and live checks use a saved sign-in in
  `$XDG_CONFIG_HOME/tau/chatgpt/` (`tau_ai::chatgpt::Store`), such as
  `tau-memory-e2e --chatgpt ID`.
- **Old files are left alone.** tau stops reading `openai-key` but does
  not delete it.

## Consequences

- Every user needs a ChatGPT plan with plan usage enabled for tau. Pay
  as you go through the API is not offered.
- A machine without a browser signs in elsewhere and copies its record,
  as 0011 describes. There is no key to paste.
- Costs stay an equivalent computed from the API price table, as in
  [0008](0008-codex-subscription.md) and 0011: the plan bills
  differently.
- Tests need no credential. They run the real client against fake
  servers in turmoil: `tau_testing::fake_chatgpt` for the sign-in and
  `FakeOpenAi` for the WebSocket, which records each upgrade's
  `Authorization`.
- TypeSafe's key (`TYPESAFE_API_KEY`, for Jev) is another service and
  is unchanged.
- If OpenAI confirms that the plan route takes `stream_id`, lane sharing
  can come back as its own decision.
