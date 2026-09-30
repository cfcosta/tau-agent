# Sign in with ChatGPT

- Status: sign-in, tokens, sign-out and model list implemented in
  `tau_ai::chatgpt`; inference still goes through
  [Codex](codex.md) until the live probe settles the transport.
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
let token = chatgpt.inference_token(&signed_in.account).await?;
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
| Inference   | `POST https://api.openai.com/v1/responses` (bearer token)     |
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
first when it expires within five minutes. `inference_token` also
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

## The WebSocket connector

`ChatGptConnector` implements the transport's `Connector` for one
account: before each connection it gets the inference token
(refreshing it), then upgrades `wss://api.openai.com/v1/responses` with
it as the bearer token. Nothing uses it yet but the probe.

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
  authorization URL, the callback checks, error classification and the
  record's round trip.

## Not done yet

- Inference over this route: the transport stage, after the probe.
- tau-ui's account picker and the Usage settings link.
- A device-code flow: OpenAI documents none for this route.
