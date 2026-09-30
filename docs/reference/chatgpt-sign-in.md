# Sign in with ChatGPT

- Status: implemented. `tau_ai::chatgpt` signs in, keeps tokens, signs
  out and lists models; `OpenAi::chatgpt` runs inference on the plan
  over `wss://api.openai.com/v1/responses`; tau-ui signs in, switches
  accounts and shows the plan's state. It replaces the Codex backend
  route (decision 0008, superseded), which is gone.
- Source: OpenAI, "ChatGPT plan usage for open-source apps",
  <https://developers.openai.com/siwc/token-sharing-open-source>, read
  on 2026-09-29.
- Decision: [0011](../decisions/0011-sign-in-with-chatgpt.md)

## Using it

```rust
use tau_ai::chatgpt::{ChatGpt, Loopback, Store};

let chatgpt = ChatGpt::new(Store::open_default()?);
let loopback = Loopback::bind().await?; // before the browser opens
let sign_in = chatgpt.start_sign_in(None, loopback.redirect_uri(), false)?;
open(sign_in.url()); // may carry id_token_hint: never log it
let callback = loopback.wait(&sign_in).await?; // or sign_in.callback(pasted)
let signed_in = chatgpt.finish_sign_in(&sign_in, &callback).await?;
let client = tau_ai::client::OpenAi::chatgpt(chatgpt, signed_in.account);
```

## Endpoints

| What        | URL                                                           |
| ----------- | ------------------------------------------------------------- |
| Authorize   | `https://auth.openai.com/api/accounts/authorize`              |
| Token       | `https://auth.openai.com/api/accounts/oauth/token`            |
| Discovery   | `https://auth.openai.com/.well-known/openid-configuration`    |
| JWKS        | the discovery document's `jwks_uri`                           |
| Revocation  | the discovery document's `revocation_endpoint`                |
| Models      | `GET https://api.openai.com/v1/models`                        |
| Inference   | `wss://api.openai.com/v1/responses` (bearer token; tau's)     |
| Issuer      | `https://auth.openai.com`                                     |
| `resource`  | `https://api.openai.com/v1`, on authorize, exchange, refresh |

## The host id

`Store::host_id` makes a `urn:uuid:` id from a random UUIDv4 the first
time it is asked for, saves it in `host.json`, and never replaces it.
Racing processes agree: each writes a temporary file and hard-links it
into place; the loser reads the winner's. It is opaque, not a
credential, and sent as `ext_agent_host_id` on every authorization.
`HostId::parse` also accepts the JWK-thumbprint and `did:key:` forms.

## Signing in

`ChatGpt::start_sign_in(account, redirect_uri, ask_consent)` builds the
authorization URL with a fresh `state`, `nonce` and PKCE verifier:

| Parameter               | First sign-in          | Returning (`account` given)     |
| ----------------------- | ---------------------- | ------------------------------- |
| `client_id`             | `dynamic_agent_client` | the saved issued id             |
| `agent_name_hint`       | `tau`                  | omitted                         |
| `ext_agent_host_id`     | this host's id         | this host's id                  |
| `id_token_hint`         | omitted                | the saved ID token, if any      |
| `login_hint`            | omitted                | the saved email, if any         |
| `response_type`         | `code`                 | `code`                          |
| `redirect_uri` | `http://127.0.0.1:<port>/auth/callback` | same |
| `scope` | `openid profile email offline_access resource.invoke chatgpt.tokens.use.direct` | same |
| `resource` | `https://api.openai.com/v1` | same |
| `state`, `nonce` | 32 random bytes, base64url | same |
| `code_challenge(_method)` | S256 of the verifier | same |
| `prompt` | `consent` only when `ask_consent` (enabling plan usage after a decline) | same |

`Loopback::bind` listens on `127.0.0.1:1455`, or any free port if that
is taken; only the port may vary. It answers other paths with a 404 and
keeps waiting. `SignIn::callback` takes the redirect URL, a request
target or a query string, so a pasted URL works the same.

The callback checks, in order:

1. `state` must be this attempt's, even on an error
   (`StateMismatch`).
2. `error=access_denied` stops (`ConsentDeclined`); other errors are
   `Authorization`.
3. A `code` must be there (`NoCode`).
4. A new registration must return an issued `client_id` that is not
   `dynamic_agent_client` (`RegistrationIncomplete`). A returning one
   may omit it, but a different one is refused (`ClientMismatch`).

`ChatGpt::finish_sign_in` then:

1. POSTs the `authorization_code` grant (form, no secret) with the
   issued client id, the code, the verifier, the same `redirect_uri`
   and `resource`. `invalid_grant` is `CodeRejected`: start again.
2. Validates the ID token: RS256/384/512 or ES256 only, signature
   against the JWKS (fetched again once for an unknown `kid`), `iss`,
   `aud` = the issued client id (`azp` when there are several), `exp`
   (60 s leeway), `nonce`. `sub` is the identity.
3. On a returning sign-in, refuses another `sub` (`IdentityMismatch`).
4. Takes the scopes from the token response (the callback's if the
   response has none). Without `chatgpt.tokens.use.direct` the sign-in
   is saved with `PlanUsage::Disabled`.
5. Saves the record, then makes it the active account. Nothing is
   saved when any step fails.

## Storage

```text
$XDG_CONFIG_HOME/tau/chatgpt/          0700 (~/.config when unset)
  host.json                            {"ext_agent_host_id": "urn:uuid:…"}
  active                               the active account's id
  accounts/<account id>.json           one credential record, 0600
  accounts/<account id>.lock           locked while refreshing or signing out
```

The account id is the issued client id plus a hash of the subject
(`oaiapp_…-1a2b3c4d5e6f`), so two registrations with one email stay
apart. Every file is written to a temporary owner-only file, synced and
renamed into place. A record has OpenAI's fields plus three of tau's:

```json
{
  "label": "user@example.com",
  "email": "user@example.com",
  "issuer": "https://auth.openai.com",
  "subject": "<sub>",
  "client_id": "oaiapp_…",
  "ext_agent_host_id": "urn:uuid:…",
  "id_token": "…",
  "access_token": "…",
  "refresh_token": "…",
  "token_type": "Bearer",
  "expires_in": 3600,
  "expires_at": 1790003600,
  "earliest_refresh_at": null,
  "scopes": ["chatgpt.tokens.use.direct", "email", "offline_access", "openid", "profile", "resource.invoke"],
  "saved_at": "2026-09-29T12:00:00Z"
}
```

- `label` is the email, with the client id's tail added when another
  account already has that label.
- `expires_at` is `saved_at` + `expires_in`, in Unix seconds.
- `Debug` never prints a token. Nothing logs one.

`Credentials::status` is `SignedIn(PlanUsage)` while tokens are saved,
and `SignedOut` after sign-out or a dead refresh token.

## Tokens

`ChatGpt::access_token` returns the saved access token, refreshing it
first when it has expired, or when it expires within five minutes and
the token response's `earliest_refresh_at` (Unix seconds) has passed:
OpenAI asks not to refresh earlier unless the token has expired. `inference_token` also
refuses a sign-in without plan usage (`PlanUsageDisabled`).

A refresh takes the process's turn mutex and the account's lock file,
reads the record again (another process may have refreshed it), and
only then POSTs `grant_type=refresh_token` with the issued client id,
the refresh token and `resource`, without `scope`. On success it
replaces the access token, the refresh token, the expiry and the
scopes together. A new ID token replaces the hint only if it validates
as the same account.

| Refresh error                                                                                                                       | What happens                                                  |
| ----------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------- |
| `invalid_grant`, `invalid_refresh_token`, `token_expired`, `refresh_token_expired`, `refresh_token_invalidated`, `refresh_token_reused` | access and refresh tokens cleared; `SignInRequired`         |
| `invalid_client`                                                                                                                    | `InvalidClient`; tokens kept                                  |
| a network failure or 5xx                                                                                                            | `Network` / `OAuth`, recovery `RetryLater`; tokens kept       |

## Signing out

`ChatGpt::sign_out` holds the same turn and lock, POSTs
`token=<refresh token>`, `token_type_hint=refresh_token` and the client
id to the discovered `revocation_endpoint`, and retries a network
failure or 5xx up to four times (1 s, 2 s, 4 s). Any 2xx is success.
Then it clears the access, refresh and ID tokens. It returns
`Revocation::Confirmed`, `NothingToRevoke`, or `Unconfirmed { reason }`:
tell the user they can disconnect tau in ChatGPT Settings. The record,
its client id and the host id stay for the next sign-in.

## Models

`ChatGpt::models` sends `GET /v1/models` with the access token and keeps
the `models` entries with `visibility: "list"`, in the server's order,
as `ModelInfo { slug, display_name }`.

## Errors

`ChatGptError::recovery` and `ApiError::recovery` return a `Recovery`:

| Returned                                                                   | Recovery          |
| -------------------------------------------------------------------------- | ----------------- |
| `subscription_sharing_usage_limit_exceeded` (429)                          | `UsageLimit`      |
| `subscription_sharing_usage_unavailable`, `…_user_unavailable` (503)       | `RetryLater`      |
| `subscription_sharing_user_not_eligible` (403)                             | `Restricted`      |
| `chatpass_v2_scope_not_authorized`, `…_invalid_authorization_context` (403) | `Restricted`     |
| `subscription_sharing_unsupported_capability` (400), `…_route_not_supported` (403) | `FixRequest` |
| `subscription_sharing_invalid_user` (401)                                  | `SignInAgain`     |
| `{"detail": …}` with 401 / 403 / 503                                       | `SignInAgain` / `Restricted` / `RetryLater` |
| no documented code: 401 / 403 / 408, 409, 429, 5xx / other 4xx             | `SignInAgain` / `Restricted` / `RetryLater` / `FixRequest` |
| a sign-in without plan usage                                               | `EnablePlanUsage` |
| `invalid_client`, storage failures                                         | `FixClient`       |

A documented code decides whatever the status. `ApiError` keeps the
status, the body verbatim, its shape (`Structured`, `Detail`, `Other`)
and `x-request-id`. `Recovery::of_code` classifies the code of a
`response.failed` event the same way.

## Inference

`OpenAi::chatgpt(chatgpt, account)` is the plan client. Its
`ChatGptConnector` gets the inference token before each connection
(refreshing it; clones of one `ChatGpt` share a refresh, and the lock
file makes other processes wait) and upgrades
`wss://api.openai.com/v1/responses` with only `Authorization: Bearer`
and `User-Agent`. Checked live on 2026-09-29 with a Plus/Pro account:
the upgrade answers 101, a request completes, a second one continuing
with `previous_response_id` on the same connection completes, and plain
top-level `{"type": "function"}` tools work (so do `namespace` and
`additional_tools`). tau stays WebSocket only (0002).

What the route asks, and how tau keeps it:

- `store: false` on every request: tau never sends anything else.
- No `background`, `conversation`, `max_output_tokens`, `max_tool_calls`,
  `metadata`, `moderation`, `multi_agent`, `prompt`,
  `prompt_cache_retention`, `safety_identifier`, `temperature`,
  `top_logprobs`, `top_p`, `truncation` or `user`
  (`chatgpt::UNSUPPORTED_FIELDS`): a plan session strips them from its
  fields whatever its settings say.
- No `system` input items: instructions go in `instructions`, and
  tau's input never has system items.
- Continuation only within the connection that produced the response:
  that is how lanes work already; a lost connection means a full resend.
- `stream_id`: unverified on this route, so the plan client takes one
  lane per connection (`single_lane_limits`, `tags_lanes() == false`).

### Refusals and retries

A failure the server or the sign-in explains is a `tau_ai::refusal::Refusal`
(recovery, HTTP status, code, `x-request-id`, body verbatim):

- **At the upgrade.** An HTTP status instead of 101 (the body and
  `x-request-id` come with it) goes through `ApiError::recovery`. A
  sign-in that cannot give a token (`SignInRequired`,
  `PlanUsageDisabled`, …) is a refusal too; a network failure is not.
- **Mid-stream.** A `response.failed` or `error` frame with a documented
  code (a usage limit can arrive after streaming began).

The retry policy follows `Recovery::class`: only `RetryLater` retries,
with the run's bounded backoff; `retry::classify` maps every
`subscription_sharing_*` and `chatpass_v2_*` code that way whatever the
status. A refusal that is not `RetryLater` fails the waiting requests
at once, with no transparent reconnect, so a usage limit never loops. A
temporary one gets the usual single reconnect, and the run's error is
the refusal's message. The pool opens a connection for a lane moved off
a lost one only when a request needs it, so nothing reconnects behind
idle lanes. `OpenAi::refusal()` keeps the latest refusal until a
response completes; tau-ui reads it when a run ends in an error.

Nothing moves a run to another way of paying after a plan error.

## tau-ui

- Runs use the active account's plan when it is signed in with plan
  usage, else the saved API key (`accounts::Credentials::access`).
- The Models screen lists every saved account (label, state, the
  active one marked): "Continue with ChatGPT" / "Add account", switch,
  "Sign out" (revokes; says so when OpenAI did not confirm), "Manage
  usage", and for a sign-in without plan usage "Enable ChatGPT plan use"
  (signs in again with `prompt=consent`) beside the API-key option.
- Onboarding's model step: "Continue with ChatGPT" opens the browser;
  while it waits, the page can be opened again or the redirect pasted.
- Once, after the first sign-in with plan usage: "You're using your
  ChatGPT plan", with "Got it" (saved in `models.json`).
- Under the composer while runs use the plan: "Using ChatGPT plan ·
  Manage usage".
- A run stopped by a usage limit: "Usage limit reached — Review your
  plan or this app's limit in ChatGPT settings." with "Manage usage";
  a dead sign-in asks to sign in again; a sign-in without plan usage
  offers to enable it or add a key; a restriction shows its message.
- The picker lists `models()` for the active account (display name,
  server order), listed again on a switch, without prices.
- "Manage usage" opens `USAGE_SETTINGS_URL`
  (`https://chatgpt.com/#settings/Usage`): the docs name the page but
  not its address, so this is a guess kept in one constant.

## The live probe

`crates/tau-ai/examples/chatgpt_probe.rs`, run by hand:

| Command      | What it does                                                                                   |
| ------------ | ---------------------------------------------------------------------------------------------- |
| `sign-in`    | binds the loopback, prints and opens the URL, waits (or takes a pasted URL), saves the record. `--new` registers another account; `--consent` asks for plan usage again; `--port N` |
| `accounts`   | the saved accounts, their status and scopes                                                    |
| `models`     | `GET /v1/models`                                                                               |
| `http`       | one streamed `POST /v1/responses` (`store: false`, `stream: true`), events until the terminal one |
| `http-tools` | the same with a top-level function tool, one in a `namespace`, and one in an `additional_tools` input item |
| `ws`         | the same request over `wss://api.openai.com/v1/responses`, then a second continuing with `previous_response_id` on the same connection |
| `ws-tools`   | the three tool shapes over one WebSocket                                                       |
| `refresh`    | a refresh now: new expiry, whether the refresh token rotated                                   |
| `sign-out`   | revokes and clears                                                                             |

It prints statuses, request ids, error bodies and events verbatim, and
never a token.

## Tests

- `crates/tau-ai/tests/chatgpt.rs`: the real client against
  `FakeChatGpt` (`tau-testing`) in turmoil. The fake registers clients,
  binds codes to their client, redirect URI, resource and PKCE
  challenge, signs RS256 ID tokens with a test key its JWKS publishes,
  rotates refresh tokens (reuse fails with `refresh_token_reused`), and
  records the rules a client broke.
- `crates/tau-ai/tests/chatgpt_properties.rs`: Hegel properties for the
  authorization URL, the callback checks, error classification, the
  retry policy's agreement with it, and the record's round trip.
- `crates/tau-ai/tests/chatgpt_transport.rs`: a sign-in on `FakeChatGpt`,
  then the plan client on `FakeOpenAi` (which can refuse upgrades with
  a status and body, and records each upgrade's `Authorization`): the
  bearer is refreshed before the connection, clones share one refresh,
  unsupported fields never go out (Hegel), a usage limit at the upgrade
  or mid-stream stops without retry, a temporary refusal is retryable
  with its details, a dead sign-in and a sign-in without plan usage
  connect nothing.
- tau-ui: `plan_usage` and `models` unit tests, and workspace tests for
  the notice, the composer line, the alerts and the account picker.

## Reasoning efforts

Each model takes its own efforts, and the server rejects any other
with `unsupported_value`. `data/openai-reasoning-efforts.json` holds
them, probed through both an API key and ChatGPT's Codex backend on
2026-09-28; the two agree on every model they share. `Model::efforts` carries them, lowest
first:

| Models                                         | Efforts                             |
| ---------------------------------------------- | ----------------------------------- |
| `gpt-5`, `gpt-5-mini`, `gpt-5-nano`            | minimal, low, medium, high          |
| `gpt-5.1`                                      | none, low, medium, high             |
| `gpt-5.2` to `gpt-5.5`, and their minis        | none, low, medium, high, xhigh      |
| `gpt-5.6-{luna,sol,terra}`, `gpt-6-{luna,sol}` | none, low, medium, high, xhigh, max |
| `gpt-6-astra`                                  | low, medium, high, xhigh, max       |
| the `-pro` models from `gpt-5.2`               | medium, high, xhigh                 |
| `gpt-5-pro`                                    | high                                |
| `o1`, `o3`, `o4-mini` and their variants       | low, medium, high                   |

The Codex backend's own model list (`GET /backend-api/codex/models`) leaves out
`none`, which its server takes, and adds `ultra`, which it never sends:
Codex turns `ultra` into the model's `max` (or its
`multi_agent_reasoning_effort`) and has the model delegate to
sub-agents on its own (`codex-rs/protocol/src/openai_models/reasoning_effort.rs`).
tau offers no `ultra` until it has sub-agents to delegate to.

An `error` frame from the server nests its `code` and `message` under
`error`; pi's tests send them at the top. The stream reads either.

## Not done yet

- `stream_id` on the plan route: one lane per connection until checked.
- A device-code flow: OpenAI documents none for this route.
