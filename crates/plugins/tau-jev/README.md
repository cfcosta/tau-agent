# tau-jev

A client for Jev, TypeSafe's System One model, for tau plugins. Jev does
not write text. It takes a state (text or JSON) and a set of typed
questions about it, and answers each with a probability, a choice or a
score. Several plugins ask Jev for small, cheap decisions, so the client
is one crate they share instead of code copied into each.

## What it provides

| Item                       | What it is                                                           |
| -------------------------- | -------------------------------------------------------------------- |
| `Jev`                      | The trait plugins depend on: `ask(&Request) -> Result<Response, _>`  |
| `TypeSafe`                 | The HTTPS client: `new(key)` or `from_env()`; clones share a pool    |
| `Request`                  | A state and its questions; `Request::new(state).question(id, q)`     |
| `Question`                 | `noul` (yes or no), `choice` (one of a set), `score` (levels)        |
| `NoulCriteria`             | What counts as yes and as no for a `Noul`                            |
| `Response`                 | The answers by question id, read with `noul`, `choice` and `score`   |
| `Response::usage`          | Tokens and cost as a `tau_ai` `Usage`, for `PluginCtx::charge`       |
| `Answer`, `JevUsage`       | The raw answers and token counts                                     |
| `JevError`                 | Transport, status, malformed, missing, wrong-kind or out-of-range    |
| `fake::FakeJev`            | A `Jev` that answers from a function and records requests, for tests |
| `fake::response`           | Builds a `Response` billed one input token per request byte          |
| `api_key`, `MissingApiKey` | Reading a key; the error when it is missing or blank                 |
| `API_KEY_VAR`              | `TYPESAFE_API_KEY`                                                   |
| `DEFAULT_MODEL`            | `jev-latest`                                                         |
| `SYSTEM_ONE_URL`           | `https://api.typesafe.ai/v1/systemone`                               |
| `PRICE_PER_MILLION_INPUT`  | US dollars per million input tokens; output tokens are free          |

`TypeSafe::model`, `url` and `retry` change the model, the endpoint (for a
proxy or a test server) and the retry policy.

Answers are checked when read. A missing answer, an answer of the wrong
kind, or a probability outside `[0, 1]` is an error, never a default.

`TypeSafe` retries status 429, 503 and 529 under tau-ai's `RetryPolicy`,
honoring `retry-after`, and fails at once on anything else. Each attempt
times out after 30 seconds. TLS is rustls with Mozilla's roots, as for
OpenAI. Its `Debug` never shows the key, transport errors leave the URL
out, and error statuses drop the body, which can echo the request.

## How it fits

`tau-jev` is a plain library, not a plugin: it has no UI and no host
half. It lives in `crates/plugins` because only plugins use it (decision
0006). It builds on `tau-ai` for `Usage`, retries and TLS.

Plugins take an `Arc<dyn Jev>`. In tau's interface, `tau-ui` makes one
`TypeSafe` from the TypeSafe key, wraps it to count requests and cost for
the Plugins screen, and puts it in each run's services, where host halves
find it. Used by
`tau-constitution-host`, `tau-goal`, `tau-reasoning`,
`tau-fast-compaction`, `tau-codemode-host`, `tau-luau-plugins-host`,
`tau-ui` and `crates/evals/tau-output-pruning-eval`.

## Usage

```rust
use tau_jev::{Jev, Question, Request, TypeSafe};

let jev = TypeSafe::from_env()?; // reads TYPESAFE_API_KEY
let request = Request::new("cargo nextest: 214 passed, 0 failed")
    .question("green", Question::noul("Did every test pass?"))
    .question(
        "tone",
        Question::choice(
            "How does the run read?",
            [("calm", "nothing to fix"), ("alarm", "something broke")],
        ),
    );
let response = jev.ask(&request).await?;
let passed = response.noul("green")? >= 0.7;
let (tone, confidence) = response.choice("tone")?;
ctx.charge(&response.usage()); // inside a plugin
```

In tests, answer every yes/no question with a fixed probability:

```rust
use tau_jev::fake::FakeJev;

let jev = FakeJev::nouls(|_id| 0.9);
// ... run the plugin ...
assert_eq!(jev.requests().len(), 1);
```

The client does not count tokens. TypeSafe's limits (64k tokens per
request, 32k for the state plus the longest question) are the caller's to
respect.

## Testing

```sh
cargo nextest run --release -p tau-jev
```

The tests run `TypeSafe` against a local server that plays back scripted
responses: round trips, retries under the policy, malformed and
wrong-kind answers, and that the key stays secret. No key or network is
needed. Answer checking is also a `cargo mutants` target (see the testing
reference).

## Further reading

- [docs/reference/plugins.md](../../../docs/reference/plugins.md), section
  "The shared Jev client: `tau-jev`"
- [docs/reference/testing.md](../../../docs/reference/testing.md), mutation
  testing
- [ADR 0006: plugin crates](../../../docs/decisions/0006-plugin-crates.md)
- [TypeSafe's documentation](https://docs.typesafe.ai)
