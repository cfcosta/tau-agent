# tau-constitution-host

Checks a tau agent's tool calls and final answers against rules, asking
Jev how likely each applicable rule is broken. This is tau-constitution's
host half: the plugin that runs the checks, the plugin's own SQLite
database of each repository's rules, and the `HostHalf` tau's interface
registers.

## What it provides

| Item                   | What it is                                                                 |
| ---------------------- | -------------------------------------------------------------------------- |
| `ConstitutionPlugin`   | The `Plugin`. `new(jev, constitution)` for fixed rules, `live(jev, rules)` |
| `Live`                 | Rules that can change while runs use them: `get`, `set`                    |
| `try_rule`             | Asks Jev what a rule makes of past calls and answers, running nothing      |
| `db::Db`               | The plugin's database: `open(path)`, or `memory()` for tests               |
| `db::load`, `db::save` | A repository's `Constitution`, read and checked, or saved                  |
| `ConstitutionError`    | A constitution that could not be loaded or saved, or is not valid          |
| `ConstitutionHost`     | The `HostHalf`: rules per repository, the page's acts, the catalog entry   |
| `Host`                 | What it keeps on the host: the database and each repository's `Live`       |
| `demo`                 | The rules tau-ui's demo shows (`demo` feature)                             |

It also re-exports the rule and record types from `tau-constitution`
(`Constitution`, `Rule`, `Target`, `Record`, `Verdict`, `NAME` and the
rest), by the paths the host half has always used.

What a check does:

- Before a tool call, every rule that names one of the call's arguments
  is asked about in one Jev request, one `Noul` per rule. Jev sees only
  those fields, as the model wrote them, never tool output.
- At or past a rule's `block` probability the call is refused, and the
  model gets the rule, quoted, so it can fix the call. Between `review`
  and `block`, the call runs and is flagged for a person.
- Before the run stops, rules on the final answer are checked the same
  way. A broken one sends the answer back, up to `max_holds` times.
- When Jev gives no answer, `on_error` decides: `allow` (the default)
  or `block`.
- Every check and decision is reported and recorded with the run, and
  Jev's cost is charged to it.

## How it fits

This is the host half (decision 0030). Its partner, `tau-constitution`, is
the interface half: the rule and record shapes and the Constitution page.

It builds on `tau-agent` (the `Plugin` traits), `tau-jev` (the checks),
`tau-ai`, `tau-ui-plugin` (`HostHalf`) and `sqlx`. The database keeps its
own migrations in `migrations/` and its query metadata in `.sqlx/`, apart
from tau's store.

Used by `tau-ui`, which registers `ConstitutionHost` with the `demo`
feature. In the app the database is `constitution.db` in the plugin's
directory, rules are keyed by the repository's checkout, and the plugin
is added to a run only when its services hold a Jev (a TypeSafe key is
saved). Rules that cannot be read fail the run at start; they are never
skipped.

## Usage

```rust
use std::sync::Arc;
use tau_constitution_host::{Constitution, ConstitutionPlugin, Live};
use tau_jev::{Jev, TypeSafe};

let jev: Arc<dyn Jev> = Arc::new(TypeSafe::from_env()?);

let mut rules = Constitution::default();
rules.add(
    "Never force-push.",
    &["bash.command".into()],
    0.3,
    0.8,
)?;

// Rules that can change between tool calls of a running agent.
let live = Live::new(rules);
let agent = agent.plugin(ConstitutionPlugin::live(jev, live.clone()));
// Later: live.set(edited);
```

To keep rules across sessions, open a `db::Db` and use `db::load` and
`db::save` with a repository key.

## Features

| Feature | What it adds                                     |
| ------- | ------------------------------------------------ |
| `demo`  | The `demo` module: the rules tau-ui's demo shows |

## Testing

```sh
cargo nextest run --release -p tau-constitution-host
```

The tests run real agent runs with `tau-testing`'s `ScriptedModel`,
`tau_jev::fake::FakeJev` and an in-memory database (`Db::memory`), so no
TypeSafe key or network is needed. Some are property tests
(`hegeltest`): which fields a rule is shown, `on_error` wherever Jev
fails, a constitution coming back from the database as saved, and the
page's fold of the records.

The query macros read the committed `.sqlx/` metadata, so no
`DATABASE_URL` is needed. After changing a query or a migration,
regenerate it as `docs/reference/storage.md` describes, against a
database with both tau-store-sqlite's schema and this crate's:

```sh
sqlx database setup --source crates/tau-store-sqlite/migrations
sqlx migrate run --ignore-missing --source crates/plugins/tau-constitution-host/migrations
(cd crates/plugins/tau-constitution-host && cargo sqlx prepare)
```

## Further reading

- [docs/reference/constitution.md](../../../docs/reference/constitution.md)
- [docs/reference/plugins.md](../../../docs/reference/plugins.md), section
  "`tau-constitution`: rules checked on specific calls"
- [docs/reference/storage.md](../../../docs/reference/storage.md), a
  plugin's own database
- [ADR 0006: plugin crates](../../../docs/decisions/0006-plugin-crates.md)
- [ADR 0030: host halves are crates](../../../docs/decisions/0030-host-halves-are-crates.md)
