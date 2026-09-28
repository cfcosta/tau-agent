# OpenAI Codex

- Status: implemented in `tau_ai::codex`
- Source: pi `packages/ai/src/auth/oauth/openai-codex.ts` and
  `packages/ai/src/api/openai-codex-responses.ts` (commit `2b0a123`)
- Decision: [0008](../decisions/0008-codex-subscription.md)

## Using it

```rust
use tau_agent::agent::Agent;
use tau_ai::{client::OpenAi, codex::{CodexAuth, CodexCredentials}};

let path = CodexCredentials::default_path().expect("a home directory");
let client = OpenAi::codex(CodexAuth::from_file(path)?);
let agent = Agent::new(client).model("gpt-5.5");
```

Sign in first:

```sh
cargo run -p tau-ai --example codex_login            # browser
cargo run -p tau-ai --example codex_login -- device  # device code
cargo run -p tau-ai --example codex_login -- import  # ~/.codex/auth.json
```

## Signing in

| Step      | Browser flow                                               | Device-code flow                                     |
| --------- | ---------------------------------------------------------- | ---------------------------------------------------- |
| Start     | `BrowserLogin::start("tau")`: PKCE verifier, state         | `DeviceLogin::start()`: POST `deviceauth/usercode`   |
| User      | opens `login.url`                                          | enters `user_code` at `auth.openai.com/codex/device` |
| Come back | `localhost:1455/auth/callback?code&state`, or a pasted URL | polls `deviceauth/token` every `interval` s          |
| Exchange  | POST `/oauth/token`, `authorization_code` grant            | same, with the returned code and verifier            |

- Client id: `app_EMoamEEZ73f0CkXaXp7hrann`. Scope:
  `openid profile email offline_access`.
- The authorize URL also sets `id_token_add_organizations`,
  `codex_cli_simplified_flow` and `originator`.
- The callback checks the path and the state. Other paths get a 404 and
  the server keeps waiting; a wrong state fails the sign-in.
- Pasted input may be the redirect URL, `code#state`, a query string or
  the bare code. A pasted state must match.
- Device polling treats 403, 404 and `deviceauth_authorization_pending`
  as pending, adds 5 s on `slow_down`, and gives up after 15 minutes.

## Credentials

```rust
pub struct CodexCredentials {
    pub access: String,
    pub refresh: String,
    pub expires: u64, // ms since the epoch
    pub account_id: String,
}
```

- `account_id` is `chatgpt_account_id` under the
  `https://api.openai.com/auth` claim of the access token. The token's
  signature is not checked; the server checks it.
- Saved as JSON with pi's field names (`access`, `refresh`, `expires`, `accountId`) at
  `$XDG_CONFIG_HOME/tau/codex.json`, falling back to
  `~/.config/tau/codex.json`, mode 0600.
- `from_codex_cli` reads `tokens.{access_token,refresh_token,account_id}`
  and takes the expiry from the token's `exp`.
- `Debug` never prints tokens.

## Refresh

- `CodexAuth` shares one set of credentials between every connection of
  a client and its clones.
- `CodexConnector::connect` calls `ensure_fresh` first: a token that
  expires within five minutes is refreshed with a `refresh_token`
  grant. Concurrent connections wait on one refresh.
- `CodexAuth::on_refresh` receives new credentials, to persist them.
  `CodexAuth::from_file` saves them back to the file they came from.
- A failed refresh fails the connection; the pool reports it like any
  connection error.

## The connection

| Header                              | Value                                           |
| ----------------------------------- | ----------------------------------------------- |
| URL                                 | `wss://chatgpt.com/backend-api/codex/responses` |
| `Authorization`                     | `Bearer <access>` (marked sensitive)            |
| `chatgpt-account-id`                | from the credentials                            |
| `originator`                        | `tau`                                           |
| `OpenAI-Beta`                       | `responses_websockets=2026-02-06`               |
| `User-Agent`                        | `tau/<version>`                                 |
| `session-id`, `x-client-request-id` | a random id per connection                      |

Everything after the upgrade is the protocol in
[`openai-websocket.md`](openai-websocket.md): `response.create` frames,
`previous_response_id`, the delta rule and the 16-in-flight limit.

## Requests

`OpenAi::session` adjusts the settings of a Codex client:

- `instructions` defaults to `You are a helpful assistant.`; Codex
  requires them.
- `max_output_tokens` is dropped; pi never sends it to Codex.
- `store` is `false`, as for every request.

## Models

`codex::MODELS` lists the models pi offers on Codex: `gpt-5.5`,
`gpt-5.3-codex-spark`, `gpt-5.6-{luna,sol,terra}` and
`gpt-6-{astra,sol,luna}`. Their costs come from the API model table, as
pi prices them; the subscription bills differently.

## Not ported

- pi's SSE transport for Codex: tau is WebSocket only (0002).
- `text.verbosity` and `service_tier` defaults pi sets per request.
- pi's per-session WebSocket cache keyed by account: tau's pool already
  keeps connections per client.
