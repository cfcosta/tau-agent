# tau-constitution

The interface half of tau-constitution, the plugin that checks a run's
tool calls and final answer against a repository's rules with Jev. This
crate holds what every interface needs: the rules and how they are
checked for shape, the records the checks publish, and the Constitution
page that edits rules and reviews what they flagged. The checks
themselves, and the database the rules live in, are in
`tau-constitution-host`.

## What it provides

| Item                                   | What it is                                                        |
| -------------------------------------- | ----------------------------------------------------------------- |
| `NAME`                                 | `"tau-constitution"`, the name in events, reports and records     |
| `Constitution`                         | A repository's rules, `on_error` and `max_holds`                  |
| `Rule`, `Target`                       | One rule: id, text, where it applies, `review` and `block`        |
| `OnError`                              | What happens when Jev gives no answer: `Allow` or `Block`         |
| `StoredConstitution`, `StoredRule`     | The rules as the database keeps them: plain values                |
| `RuleError`                            | Why a rule does not check out, worded for the UI                  |
| `Record`                               | What the checks publish, one variant per kind                     |
| `Check`, `Score`, `Verdict`, `Failure` | A check's scores, a decision about one rule, and a failed check   |
| `VerdictKind`                          | `Blocked`, `Flagged` or `Held`                                    |
| `Trial`                                | A rule tried on a past call or answer, with Jev's score           |
| `ConstitutionUi`                       | The `UiPlugin`: the rules page, call badges, notes, the inspector |
| `ui::Rules`, `ui::Data`                | A repository's rules for its page, and what a person reviewed     |
| `ui::Act`                              | What the page asks the host half: add, update, remove, try, reset |
| `ui::State`, `ui::Stats`               | A run's folded records, and what its checks did                   |

A rule applies to tool arguments or to the final answer. `edit.newText`
means every `newText` in an `edit` call's arguments, however deep;
`final answer` means the run's last message. `Constitution::add` gives a
new rule the next free id (`R1`, `R2`, ...), and `from_stored` checks
every rule read back: an id, text, at least one target, and thresholds
between 0 and 1 with `review` at most `block`.

`Record` has one variant per `kind`: `checked`, `blocked`, `flagged`,
`held`, `error`, and `starting`, which the interface folds as a run starts
and is never stored.

## How it fits

This is the interface half (decisions 0006, 0017 and 0030). Its partner,
`tau-constitution-host`, runs the checks with Jev and keeps the rules in
the plugin's own SQLite file. The host half re-exports the rule and
record types from here.

It builds on `tau-agent` (to read stored records), `tau-ui-plugin` and
`tau-ui-kit`, and on gpui.

Used by `tau-constitution-host`, by `tau-ui-remote`, which lists
`ConstitutionUi` among its plugins so a phone draws checks without
linking the host half, and by `tau-ui`.

## Usage

Building and narrowing a constitution:

```rust
use serde_json::json;
use tau_constitution::{Constitution, rules::{DEFAULT_BLOCK, DEFAULT_REVIEW}};

let mut constitution = Constitution::default();
let id = constitution.add(
    "Library code returns errors. No unwrap or expect outside tests.",
    &["edit.newText".into(), "write.content".into()],
    DEFAULT_REVIEW,
    DEFAULT_BLOCK,
)?;

let args = json!({ "path": "src/lib.rs", "content": "x.unwrap()" });
let applicable = constitution.for_call("write", &args);
assert_eq!(applicable[0].0.id, id);
```

`for_call` returns each rule that applies with only the argument fields it
names, which is what Jev is shown.

## Testing

```sh
cargo nextest run --release -p tau-constitution
```

The unit tests in `rules.rs` cover reading rules back and the errors bad
ones give. The checks, the records and the page are tested in
`tau-constitution-host`.

## Further reading

- [docs/reference/constitution.md](../../../docs/reference/constitution.md)
- [docs/reference/plugins.md](../../../docs/reference/plugins.md), section
  "`tau-constitution`: rules checked on specific calls"
- [ADR 0017: plugins bring their UI](../../../docs/decisions/0017-plugins-bring-their-ui.md)
- [ADR 0030: host halves are crates](../../../docs/decisions/0030-host-halves-are-crates.md)
