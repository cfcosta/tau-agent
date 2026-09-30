# 0011: Sign in with ChatGPT, for plan usage on the public API

- Status: proposed. Supersedes [0008](0008-codex-subscription.md) once
  a live check confirms the transport (see "Open before accepting").
- Date: 2026-09-29

## Context

[0008](0008-codex-subscription.md) serves ChatGPT Plus and Pro users
through pi's `openai-codex` provider: the Codex CLI's OAuth client id,
and ChatGPT's private `chatgpt.com/backend-api/codex` endpoint. Neither
is a public API. Both can change without notice, and the client id is
not ours.

On 2026-09-29 OpenAI released "ChatGPT plan usage for open-source apps"
as part of Sign in with ChatGPT
(<https://developers.openai.com/siwc/token-sharing-open-source>). It
lets an open-source app register its own OAuth client per user, with no
client secret and no partner key, and spend the user's plan on the
public Responses API at `https://api.openai.com/v1/responses`. Users
see the app, and can limit or disconnect it, in ChatGPT Settings →
Usage.

## Decision

- **Sign in the documented way.** `tau_ai::chatgpt` follows OpenAI's
  flow exactly: a stable `ext_agent_host_id` per host; a first sign-in
  with `client_id=dynamic_agent_client` and `agent_name_hint=tau`;
  later ones with the issued client id and `id_token_hint`/`login_hint`;
  PKCE, `state` and `nonce` per attempt; a loopback callback on
  `127.0.0.1` (port 1455 preferred), or a pasted redirect URL.
- **Validate everything before saving.** The ID token is checked
  against OpenAI's JWKS (issuer, audience = the issued client id,
  expiry, nonce); a returning sign-in must be the same `sub`. The
  granted scopes decide plan usage: without
  `chatgpt.tokens.use.direct` the account is signed in but must not
  infer, a state the UI can show.
- **One record per registration.** Credentials live in
  `$XDG_CONFIG_HOME/tau/chatgpt/`: a host id, the active account, and
  one owner-only file per (issued client id, subject), written
  atomically. Signing out revokes the refresh token, then clears the
  tokens but keeps the registration and the host id.
- **Refreshes take turns.** Refresh tokens rotate, so refreshes of one
  account are serialized within a process (a mutex) and across
  processes (a lock file), and each re-reads the record under the lock.
- **Failures carry a recovery.** Every error maps to one of: retry
  with backoff, pause and link to Usage settings, sign in again, enable
  plan usage, restricted (explain, do not loop), fix the request, fix
  the client. The documented `subscription_sharing_*` and
  `chatpass_v2_*` codes and the `{"detail": …}` admission bodies are
  classified as the docs say. Plan-usage errors never fall back to
  another way of paying.
- **No second HTTP stack.** `tau_ai::http` is the small HTTP/1.1 client
  the Codex sign-in already used, generalized: any method, headers,
  streamed bodies, and a `Dialer` so tests run it over turmoil.

## Open before accepting

A live probe (`cargo run -p tau-ai --example chatgpt_probe`) answers what
the docs leave open:

- Does `wss://api.openai.com/v1/responses` accept the plan token? The
  docs describe HTTP with `stream: true`, and mention WebSocket
  continuation only in passing. [0002](0002-openai-websocket-only.md)
  makes tau WebSocket only; if the route is HTTP only, 0002 needs an
  exception.
- Do plain top-level function tools work, or must tools be grouped in
  a `namespace` or sent as `additional_tools` input items, as the
  preview limitations suggest? tau's tool list is plain functions today.
- Does `stream_id` (several lanes per connection) work there?

## Consequences

- Once accepted: the Codex connector, `codex.json` and the pi client id
  go; the transport sends the plan token to `api.openai.com`; tau-ui
  signs in through `tau_ai::chatgpt`; 0008 is superseded.
- The route rejects fields tau may send today (`max_output_tokens`,
  `temperature`, `prompt_cache_retention`, …) and `system` message
  items. The transport stage must strip them for plan requests.
- `/v1/models` lists the account's models; the static Codex list goes.
- Signing in needs a browser on the same machine (the callback is on
  `127.0.0.1`). A remote host signs in locally and copies its record,
  keeping its own host id. There is no device-code flow.
- OpenAI does not notify tau when a user disconnects it; a refresh or
  request that fails says so.
- Details: [`../reference/chatgpt-sign-in.md`](../reference/chatgpt-sign-in.md).
