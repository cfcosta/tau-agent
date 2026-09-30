# 0011: Sign in with ChatGPT, for plan usage on the public API

- Status: accepted. Supersedes [0008](0008-codex-subscription.md).
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

## What the live check found

The probe (`cargo run -p tau-ai --example chatgpt_probe`) ran on
2026-09-29 with a Plus/Pro account:

- `wss://api.openai.com/v1/responses` takes the plan token as a bearer
  token: 101, a completed response, and a second one continuing with
  `previous_response_id` on the same connection. tau stays WebSocket
  only; 0002 needs no exception.
- Plain top-level function tools work, over HTTP and WebSocket; the
  `namespace` and `additional_tools` forms work too. No reshaping.
- Refresh works and rotates; `earliest_refresh_at` comes as Unix
  seconds.
- `stream_id` was not checked: the plan client uses one lane per
  connection. Since [0012](0012-chatgpt-sign-in-only.md) that is the
  only client, and tau sends no `stream_id` at all.

## Consequences

- The Codex connector and its headers, the Codex CLI's client id, the
  device-code flow, `~/.codex/auth.json` import, `codex.json` and the
  `codex.rate_limits` usage windows are gone. tau no longer reads
  `~/.config/tau/codex.json`; users sign in again.
- `OpenAi::chatgpt` sends the plan token to `api.openai.com`, stripping
  the fields the route rejects (`max_output_tokens`, `temperature`,
  `prompt_cache_retention`, …); tau sends no `system` items.
- Plan errors carry a `Refusal`; only "retry later" retries. tau-ui
  says what to do instead. There is no other way of paying to switch
  to: [0012](0012-chatgpt-sign-in-only.md) removed API keys.
- The plan's models come from tau's model table, the newest of each
  plan family (`tau_ai::model::plan_models`), not from `/v1/models`,
  which leaves out models the plan runs; `/v1/models` is called once
  per sign-in only to learn whether the account is eligible. The static
  Codex list is gone.
- Signing in needs a browser on the same machine (the callback is on
  `127.0.0.1`). A remote host signs in locally and copies its record,
  keeping its own host id. There is no device-code flow.
- OpenAI does not notify tau when a user disconnects it; a refresh or
  request that fails says so.
- Details: [`../reference/chatgpt-sign-in.md`](../reference/chatgpt-sign-in.md).
