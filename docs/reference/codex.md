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
`previous_response_id` and the delta rule, with one difference: Codex
rejects `stream_id` (`Unsupported parameter: stream_id`), so lanes
cannot share a connection. `CodexConnector::tags_lanes` returns `false`,
`OpenAi::codex` uses `codex_limits()` (one lane and one request in
flight per connection), and the driver routes each frame to the lane of
the connection it came on. Concurrent runs each get their own
connection.

Checked against a ChatGPT Pro account on 2026-09-28: a one-word answer,
and a two-turn run with a tool call continued on the same connection.

## Requests

`OpenAi::session` adjusts the settings of a Codex client:

- `instructions` defaults to `You are a helpful assistant.`; Codex
  requires them.
- `max_output_tokens` is dropped; pi never sends it to Codex.
- `store` is `false`, as for every request.

## Models

`codex::MODELS` lists the models pi offers on Codex, less
`gpt-5.3-codex-spark`, which Codex refuses to ChatGPT accounts ("not
supported when using Codex with a ChatGPT account", checked on
2026-09-28): `gpt-5.5`, `gpt-5.6-{luna,sol,terra}` and
`gpt-6-{astra,sol,luna}`. Their costs come from the API model table, as
pi prices them; the subscription bills differently.

## Reasoning efforts

Each model takes its own efforts, and the server rejects any other
with `unsupported_value`. `data/openai-reasoning-efforts.json` holds
them, probed through both an API key and Codex on 2026-09-28; the two
agree on every model they share. `Model::efforts` carries them, lowest
first:

| Models                                       | Efforts                             |
| -------------------------------------------- | ----------------------------------- |
| `gpt-5`, `gpt-5-mini`, `gpt-5-nano`          | minimal, low, medium, high          |
| `gpt-5.1`                                    | none, low, medium, high             |
| `gpt-5.2` to `gpt-5.5`, and their minis      | none, low, medium, high, xhigh      |
| `gpt-5.6-{luna,sol,terra}`, `gpt-6-{luna,sol}` | none, low, medium, high, xhigh, max |
| `gpt-6-astra`                                | low, medium, high, xhigh, max       |
| the `-pro` models from `gpt-5.2`             | medium, high, xhigh                 |
| `gpt-5-pro`                                  | high                                |
| `o1`, `o3`, `o4-mini` and their variants     | low, medium, high                   |

Codex's own model list (`GET /backend-api/codex/models`) leaves out
`none`, which its server takes, and adds `ultra`, which it never sends:
Codex turns `ultra` into the model's `max` (or its
`multi_agent_reasoning_effort`) and has the model delegate to
sub-agents on its own (`codex-rs/protocol/src/openai_models/reasoning_effort.rs`).
tau offers no `ultra` until it has sub-agents to delegate to.

An `error` frame from the server nests its `code` and `message` under
`error`; pi's tests send them at the top. The stream reads either.

## Not ported

- pi's SSE transport for Codex: tau is WebSocket only (0002).
- `text.verbosity` and `service_tier` defaults pi sets per request.
- pi's per-session WebSocket cache keyed by account: tau's pool already
  keeps connections per client.
