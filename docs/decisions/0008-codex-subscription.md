# 0008: OpenAI Codex, signed in with ChatGPT

- Status: proposed
- Date: 2026-09-28

## Context

[0002](0002-openai-websocket-only.md) limits tau-agent to OpenAI's
Responses API over WebSocket, with API keys only. Many users pay for
ChatGPT Plus or Pro instead of API usage. pi serves them with its
`openai-codex` provider: the same Responses protocol, on ChatGPT's
Codex endpoint, signed in with the user's ChatGPT account.

Our WebSocket layer was ported from that provider
(`openai-codex-responses.ts`). Only the endpoint, the credentials and a
few headers differ.

## Decision

- **Codex is a second endpoint of the same client.** `OpenAi::codex`
  builds a client whose connector reaches
  `wss://chatgpt.com/backend-api/codex/responses`. The pool, lanes,
  continuation and delta rule are shared with the API endpoint.
- **Credentials are OAuth, ported from pi.** `tau_ai::codex` has the
  browser flow (PKCE, callback on `localhost:1455`, or a pasted redirect
  URL) and the device-code flow, with pi's client id. It can also read
  the credentials the Codex CLI saved in `~/.codex/auth.json`.
- **The connector refreshes the sign-in.** Before each connection it
  refreshes an access token that expires within five minutes; clones of
  a client share one refresh. A callback can persist new credentials.
- **Requests follow pi's Codex shape.** The upgrade sends
  `chatgpt-account-id` (read from the token), `originator: tau` and
  `OpenAI-Beta: responses_websockets=2026-02-06`. Every session has
  instructions (a default when the agent has none) and no
  `max_output_tokens`. `store` stays `false`, as for every request.
- **One lane per connection.** Codex rejects the `stream_id` that lets
  lanes share a connection, so a Codex client opens a connection per
  run and routes frames by connection.
- **No second HTTP stack.** The three token requests go over a small
  HTTPS client on the rustls setup the WebSocket already uses.
- **Credentials live in `$XDG_CONFIG_HOME/tau/codex.json`**, mode 0600,
  in pi's field names. Where to keep them stays the caller's choice.
- **Codex models are priced like their API versions**, as pi prices
  them, so limits and cost reports keep working. The subscription bills
  differently; the numbers are an equivalent, not a charge.

## Consequences

- 0002's "API keys only" becomes "API keys, or a ChatGPT sign-in for
  Codex". Everything else in 0002 holds.
- The Codex endpoint is not a public API. Its headers and required
  fields may change without notice; pi is the reference to follow.
- Signing in needs a browser or a second device. A headless server uses
  the device-code flow or imports the Codex CLI's credentials.
- Usage limits of the subscription surface as model errors (429 with
  `usage_limit_reached`), handled by the existing retry policy.
- Details: [`../reference/codex.md`](../reference/codex.md).
