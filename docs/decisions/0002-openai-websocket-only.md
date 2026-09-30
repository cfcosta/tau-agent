# 0002: OpenAI only, Responses over WebSocket

- Status: accepted. Its authentication is superseded by
  [0012](0012-chatgpt-sign-in-only.md).
- Date: 2026-09-26

## Context

pi's LLM layer (`pi-ai`, about 24.5k lines) has three sources of size:

- 10 wire APIs and 42 providers.
- 8 OAuth flows.
- About 30 compatibility flags for OpenAI-compatible servers, plus
  message transformation between providers.

Most of the hard porting work sits in that code.

OpenAI's Responses API has a documented WebSocket mode. OpenAI reports it
as up to about 40% faster end to end for workflows with 20 or more tool
calls, which describes most agent workflows.

The speed comes from the connection, not from token streaming. The
connection keeps recent responses in memory, so each turn sends only the
new input items plus `previous_response_id`.

This works with `store: false`.

## Decision

- **Provider:** OpenAI only.
- **Authentication:** API keys, from `OPENAI_API_KEY` or passed in
  explicitly. Superseded: since [0012](0012-chatgpt-sign-in-only.md)
  the only authentication is a ChatGPT plan's token from Sign in with
  ChatGPT ([0011](0011-sign-in-with-chatgpt.md)); there are no API
  keys.
- **API:** Responses only. Chat Completions has no WebSocket mode.
- **Transport:** WebSocket only. There is no SSE fallback. A failed
  connection surfaces as an `error` event, and the retry policy handles
  it.
- **Continuation:** use `previous_response_id` together with the delta
  rule ported from pi's Codex adapter.

## Consequences

- The LLM layer shrinks to about 3k lines of Rust.
- Instructions and the tool list stay fixed for the life of a run.
  Changing either breaks the continuation chain and forces a full resend.
- Networks that block WebSocket upgrades stop every agent.
- All model and API changes come from a single vendor.
- Authentication changed since: a ChatGPT plan through Sign in with
  ChatGPT ([0011](0011-sign-in-with-chatgpt.md)), on the same endpoint
  and the same WebSocket protocol with the plan's token as the bearer,
  and then that alone, with API keys removed
  ([0012](0012-chatgpt-sign-in-only.md)). The transport stays
  WebSocket only.
- Protocol details: [`../reference/openai-websocket.md`](../reference/openai-websocket.md).
