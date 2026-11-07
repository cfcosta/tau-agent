# 0047: What devices edit at once merges, not overwrites

- Status: proposed. Builds on [0013](0013-phones-connect-to-a-running-tau.md)
  (phones steer a running tau) and [0045](0045-everything-is-a-plugin.md)
  ("The interface's state is data"). The person has decided tau adopts
  CRDTs; this records how. Changes no wire or code yet.
- Date: 2026-11-04

## Context

Since 0045 the interface's state is data: the runs, the catalog and
every plugin's UI state cross to phones whole, and a phone's edit goes
up as an event. A phone keeps what the person did while away in an
outbox and sends it again when it reaches the computer (4ba4e7ce). The
computer takes each request once, by its number, and that number
outlives a restart of the computer (`tau-remote`'s `devices`).

`crates/tau-ui/tests/offline_model.rs` drives the real host, feed,
outbox and encoding with the computer and two phones that edit the same
things while one or both are away: drafts, open sections and a slot's
pick (UI state), goals, the model new chats start on, models hidden,
chats closed, written to and stopped, chats queued to land and taken
out of the queue. Phones come back in any order, between one word of a
draft and the next; a request comes twice; the computer closes and
starts again on what it saved.

### What the model found

- **Devices agree.** No case ended with a phone showing something the
  computer did not, over the default run and a nightly run of 200
  cases of 60 steps.
- **No request is lost or taken twice.** Every edit made on any device
  reached the computer once, through phones going away, coming back in
  any order, answers lost and requests repeated.
- **Three ways today's own rule broke, now fixed.** Each was a small
  path the computer took out of order, and each has a regression test
  in the model's file:
  - _An edit lost to a restart._ The host applied each UI event in a
    task of its own and did not count the save as work. Edits made just
    before tau closed came back older after it started again: a draft
    at `w8` showed `w7`. The nightly run found it and stopped shrinking
    at about twenty steps (drafts typed on the computer and on phones,
    then a restart); the regression test types 200 words on the
    computer and restarts, and came back with no draft at all
    (`the_host_keeps_the_last_of_quick_edits`).
  - _An older model pick kept._ The host changed the models' settings
    in a task per event, so two quick changes could land in either
    order: every device then agreed on the older one
    (`the_host_keeps_the_last_of_quick_model_picks`, which failed in
    two runs of three before the fix).
  - _A message that did not open a chat._ A phone that last saw a chat
    finished closes it while the computer has it going again; that
    close does not stop it, and a message to it after steers it
    without opening it (`a_message_opens_a_chat_closed_while_it_went_on`;
    the nightly case had nine arrivals: two closes, five messages, a
    close, a message).

  The host now applies each of these as the window makes it, in order,
  and is at work until it is saved; a steer opens a closed chat as a
  message to a finished one does. tau still does not wait for the
  host's work when it quits, so a save in flight then is lost either
  way.

- **What is left is the rule itself,** below.

### The rule today's code follows

**The last edit to reach the computer wins.** Every value the person
sets is a whole value, applied in the order the computer takes it, and
the computer sends each to every phone in that order. A phone shows its
own edit at once and is corrected by what comes back. This converges:
once everything has arrived, every device shows the same. It also keeps
every edit's request, while the phone's app stays open and its clock
only moves forward: each reaches the computer once and is applied.

What it does not keep is intent:

- **An older edit can win.** A phone away with a draft typed at 9:00
  comes back at 9:30 and replaces what the person typed on the computer
  at 9:15 (`the_later_edit_wins_over_one_made_away`).
- **Words are lost.** A draft is one string. Typed on two devices at
  once, the one that arrives last replaces the other whole
  (`words_typed_on_two_devices_at_once_are_all_kept`).
- **The outbox lives in memory.** Android closes apps in the background
  freely. What a phone did away is gone if the app closes before it
  reaches the computer (`what_a_phone_did_away_outlives_its_restart`).
- **Request numbers are the phone's clock.** `remote::clock` numbers a
  phone's requests from its wall clock so they grow across its
  restarts. A clock set back (by hand, by the network, by a new phone
  restored from a backup) gives numbers at or below the last one taken;
  the computer answers each as taken and drops it. The phone has
  already shown the edit, so it stays out of step with the computer
  until something replaces that value: a divergence, not only a lost
  edit (`a_phone_whose_clock_went_back_is_still_heard`). The computer's
  own feed numbers (`server::first_seq`) come from its clock too, which
  is the same hazard on one machine.

Each is an ignored test in the model's file that passes once this
decision is built.

## Decision

Each device holds a replica of the shared state. Each shared field
gets the merge it needs, in small CRDT types of our own inside the JSON
state, ordered by a hybrid logical clock. What has a side effect is not
state but an intent, run once. No CRDT library.

### Requirements

Two models found what the merge must also fix: this one, and the
network model in `crates/tau-remote/tests/network_model.rs` (workspace
`tau-mbt-net`, commits fb7ce93f and 653a5644). Each is an ignored test
today; the migration is done when they pass.

1. **A phone's clock moved back.** Request ids come from the phone's
   wall clock (`clock`, `count_from`), so after a clock move and an app
   restart new requests fall at or below the computer's stored
   `last_up`, and are answered and dropped
   (`a_request_after_the_phones_clock_went_back_is_taken`; here,
   `a_phone_whose_clock_went_back_is_still_heard`).
2. **The computer's clock moved back across a restart.** The feed's
   first number is the clock (`first_seq`); a phone that comes back with
   an old `last_seq` gets an empty replay and misses messages
   (`a_phone_from_before_the_computers_clock_went_back_gets_a_snapshot`).
3. **Revoked, then paired again.** The phone sends its unsent requests
   again as the new device, so one whose answer was lost is taken twice
   (`a_request_is_not_taken_again_after_pairing_again`).
4. **The outbox is not saved.** What the person did away is lost when
   the app restarts (`a_request_made_while_away_survives_the_apps_restart`;
   here, `what_a_phone_did_away_outlives_its_restart`).
5. **An older edit wins** and **concurrent typing keeps one device's
   text** (this model's `the_later_edit_wins_over_one_made_away` and
   `words_typed_on_two_devices_at_once_are_all_kept`).

How the design meets them:

- **Operation ids are (replica id, counter).** The counter is saved
  with the replica and only grows; no id comes from a wall clock. A
  clock moved back on either side changes no id (1, 2).
- **The replica id is the phone's own,** made once on the phone and
  kept apart from the device token. Pairing again gives a new token
  and keeps the replica id, so the computer knows the ops it already
  has (3).
- **Sync is by version vector.** Each side says, per replica, the last
  counter it has; the other sends what is after it. No clock-derived
  sequence decides what a phone missed, so nothing is skipped after a
  restart on either side (2). A computer that cannot send the ops a
  phone lacks sends its state, as a snapshot does today.
- **The phone's replica is saved,** unsent ops included, so what was
  done away outlives the app (4).
- **Commands are intents, run once.** Starting a run, sending or
  steering, cancelling, closing and landing have effects a merge cannot
  undo. Each is an entry in an add-only log of intents, with its op id.
  The computer keeps the set of ids it has carried out, saved before
  it answers, and carries out an intent only if its id is not in the
  set: once, however often and through whatever pairing it comes
  (3). Its result reaches the devices as state.
- **Values merge by stamp,** last writer wins by HLC, so the person's
  later edit wins whatever arrives last (5); drafts as text if wanted
  (below).
- **Transient streams stay outside.** Tokens as a turn streams, view
  trees built for a phone (ADR 0046), pictures and plugin replies are
  best effort, as today: resent or rebuilt, never merged.

### Who writes what

Most of the shared state has one writer, the computer, and needs no
merge: what a device sends for it is an intent, which the computer
carries out once or refuses.

| State                                                               | Written by                                  | Concurrent hazard                                      | Semantics it needs                                                                        |
| ------------------------------------------------------------------- | ------------------------------------------- | ------------------------------------------------------ | ----------------------------------------------------------------------------------------- |
| Runs, transcripts, plugin records folded into them                  | the computer (host folds what it stores)    | none; a message is an intent                           | none: the computer's state, sent as it is                                                 |
| Messages to a chat (`Say`), new chats, forks                        | any device                                  | the same intent delivered twice, or after a re-pairing | intent with an op id, run once                                                            |
| Landing queue (`Land`, `Unqueue`)                                   | the computer, on intents                    | an intent on a chat that ended is refused, as today    | intent with an op id, run once                                                            |
| Cancel                                                              | any device                                  | a stop for a turn that ended does nothing              | intent with an op id, run once                                                            |
| Catalog, pushes, pull requests, code diffs, query results, resuming | the computer                                | none                                                   | none                                                                                      |
| A chat closed or open                                               | any device (`CloseRun`; a message opens it) | close on one, write on another                         | last-writer-wins register per chat, by HLC; the stop a close makes is an intent           |
| A chat's goal                                                       | any device (tau-goal record)                | set on two                                             | last-writer-wins register per chat, by HLC                                                |
| The model an agent starts on                                        | any device                                  | picked on two                                          | last-writer-wins register per agent, by HLC                                               |
| Models hidden                                                       | any device                                  | hide on one, show on another                           | last-writer-wins register per model (a set of them)                                       |
| Repositories hidden                                                 | any device; shown again by adding           | hide on two, the last listed one                       | add-wins set, with the host's "one stays listed" rule on top                              |
| UI state: sections, flags, a slot's pick, a page's tab              | any device                                  | set on two                                             | last-writer-wins register per key, by HLC                                                 |
| UI state: drafts (tree inputs)                                      | any device                                  | typed on two at once                                   | text sequence CRDT per draft, or a register if the person accepts one device's text whole |
| Route, open repositories, scroll, caret, hover                      | each device its own                         | none                                                   | none (never shared)                                                                       |

### The clock

Which edit is later comes from a hybrid logical clock; which op is
which comes from the op id, never from a clock:

- **A stamp is (wall millis, logical, replica id).** A replica takes
  the larger of its wall clock and the last stamp it saw, and bumps the
  logical part on a tie. Every stamp it receives moves it forward. So a
  phone whose clock is set back still orders after what it has seen,
  and two stamps never tie. The wall part only says which edit the
  person made later; it never names, numbers or skips an op.
- **The replica is saved on each device:** its id, its op counter, its
  last stamp, its state and its unsent ops.

### Small types in the JSON state

In `tau-ui-plugin` (the kernel's shared types), beside `UiState`:

```rust
pub struct ReplicaId(Uuid);
pub struct OpId { replica: ReplicaId, counter: u64 }
pub struct Hlc { millis: u64, logical: u32, replica: ReplicaId }
pub struct VersionVector(BTreeMap<ReplicaId, u64>);

/// The value with the larger stamp wins.
pub struct Lww<T> { value: Option<T>, stamp: Hlc }

/// Elements added, and removed with the adds a device had seen.
pub struct AddWins<T> { adds: BTreeMap<T, BTreeSet<Hlc>>, removed: BTreeSet<Hlc> }
```

- Each serializes as plain JSON, so `ui.json`, the store's records and
  the wire stay JSON, and a snapshot is the state itself.
- `merge` is a pure function of two states. `UiState` becomes
  `BTreeMap<plugin, BTreeMap<key, Lww<Value>>>`; a `UiEvent` carries its
  stamp, and applying it is merging a one-key state. The host, the
  computer's window and phones apply the same `merge`.
- A forgotten key is a register with no value and a stamp (a
  tombstone), so a late older edit cannot bring it back. Tombstones of
  UI state are few and small; the host may drop one older than every
  connected phone's last seen stamp.
- Drafts stay `Lww<String>` first. If the person wants words typed on
  two devices kept, a draft becomes a sequence (below).

### Drafts as text, if wanted

Merging text is the one place a hand-rolled type is the wrong call. If
the answer to `words_typed_on_two_devices_at_once_are_all_kept` is yes,
drafts alone use a library's text type, kept as bytes inside the
draft's register. The options below are judged for that.

## Options considered

The version, license and release figures are from crates.io's API on
2026-11-04. None of the docs pages say anything about Android, and none
of the crates was built for Android here: the Android notes are
unverified.

### Small hand-rolled types in the existing JSON state

- **Fit with "UI state is data".** Exact: the state stays the serde
  JSON it is, deterministic, saved in `ui.json` and the records, sent
  whole in a snapshot and as events. `view` reads the value, not the
  CRDT.
- **Android.** No new crate. A few hundred lines in a crate the phone
  links already.
- **Wire and size.** A stamp per value: about 30 bytes of JSON per key,
  per event. No history.
- **Cost.** We own the merge laws, so they need property tests (below).
  No text merge.

### automerge

- 0.12.0, 2026-09-16; MIT; 8 releases in 12 months.
- **Fit.** A document with its own history and binary format. The UI
  state would live in an automerge document and be read out as JSON for
  `view`. Deterministic, persisted as its own bytes, synced by its own
  protocol, which wants a reliable in-order transport (the feed is
  one). Two sources of truth unless the whole `Synced` moves into it.
- **Android.** No `-sys` crates listed; pure Rust not verified. About
  43k lines of Rust: a larger `libtau_phone.so`.
- **Wire and size.** Compressed binary with full history; history grows
  with every keystroke unless compacted.

### yrs (Yjs)

- 0.28.1, 2026-11-04; MIT; 9 releases in 12 months; the most used of
  the three.
- **Fit.** Shared types (map, array, text) in a `Doc`; updates are
  binary (lib0, v1 or v2 encoding). Its `Text` is the best-known text
  CRDT. The same two-sources-of-truth concern as automerge if used for
  everything; a good fit for drafts alone.
- **Android.** No `-sys` crates listed; pure Rust not verified.
- **Wire and size.** Compact updates; v2 helps for many updates and may
  not for small ones (its docs say so). Tombstones stay unless garbage
  collected.

### loro

- 1.17.1, 2026-10-22; MIT; 15 releases in 12 months: the most active.
- **Fit.** Map, list, movable tree and rich text, with snapshots,
  update exports and shallow snapshots that drop old history. Same
  model as yrs: a document beside our JSON.
- **Android.** No `-sys` crates listed; most logic in `loro-internal`,
  not checked. Pure Rust not verified.
- **Wire and size.** Snapshot with compressed history, or a shallow one
  without; no size figures in its docs.

### A clock crate

`uhlc` (0.9.0, 2026-01-12; EPL-2.0 OR Apache-2.0) is a hybrid logical
clock, about 1k lines, one release in 12 months. Ours is about 40
lines with the counter persisted; not worth a dependency.

## Recommendation

1. Hand-rolled `OpId`, `Hlc`, `VersionVector`, `Lww` and `AddWins` in
   `tau-ui-plugin`, as plain serde JSON, with the merge laws
   property-tested.
2. Every multi-writer field in the table above moves to them; the
   computer-only state stays as it is.
3. Commands become intents with op ids; the computer saves the ids it
   carried out and runs each once.
4. Each phone keeps a saved replica, with an id of its own apart from
   its device token, and syncs by version vector, not the feed's
   clock-derived numbers.
5. Drafts stay registers. If the person wants concurrent typing merged,
   drafts alone take yrs's `Text` (most used, compact updates), stored as
   bytes in the draft's register; loro is the alternative if its
   shallow snapshots matter more.
6. No CRDT library for the rest: it would put a second copy of the state
   beside the JSON that `view`, `ui.json` and the snapshot read.

### Migration

The project has no users: the schema changes in place, with no
migration and no compatibility reading.

1. **Replicas and intents.** `ReplicaId` made once on each device and
   saved apart from the device token; `OpId` from its saved counter;
   the phone's replica and unsent ops saved; commands sent as intents,
   and the computer's set of carried-out ids saved; sync by version
   vector, with the state as a snapshot when the ops are gone.
   Requirements 1 to 4 pass.
2. **UI state.** `UiState` holds `Lww<Value>`; `UiEvent` carries a stamp;
   `apply` is `merge`. `ui.json` is written in the new shape; an old
   file that does not read is dropped, as `load_ui_state` does today.
   The later-edit question passes for UI state.
3. **Settings.** Closed chats, goals (the tau-goal record carries its
   stamp), the default model, hidden models and hidden repositories
   take their types. `repos.json` and `models.json` change in place.
4. **Drafts,** if decided: the text type, behind the draft's register.

## Testing

- **The models are the acceptance tests.** `offline_model.rs`'s
  convergence check stays as it is: once everything has arrived, every
  device shows the same. Its rule check changes from "the last to
  arrive wins" to "the largest stamp wins". The migration is done when
  these ignored tests pass and are un-ignored:
  - in `crates/tau-remote/tests/network_model.rs`:
    `a_request_after_the_phones_clock_went_back_is_taken`,
    `a_phone_from_before_the_computers_clock_went_back_gets_a_snapshot`,
    `a_request_is_not_taken_again_after_pairing_again`,
    `a_request_made_while_away_survives_the_apps_restart`;
  - in `crates/tau-ui/tests/offline_model.rs`:
    `the_later_edit_wins_over_one_made_away`,
    `what_a_phone_did_away_outlives_its_restart`,
    `a_phone_whose_clock_went_back_is_still_heard`, and
    `words_typed_on_two_devices_at_once_are_all_kept` if drafts merge
    as text.
- **Intents run once.** A property over the intent log: delivered in
  any order, any number of times, through any number of pairings and
  restarts of either side, each intent is carried out exactly once.
- **The CRDT laws, at the type level.** For `Lww`, `AddWins` and the
  merged `UiState`, hegel properties over generated states and events:
  `merge` is commutative (`a ⊔ b = b ⊔ a`), associative
  (`(a ⊔ b) ⊔ c = a ⊔ (b ⊔ c)`) and idempotent (`a ⊔ a = a`), and
  applying events in any order, with duplicates, gives the same state.
  For `Hlc`: a stamp taken after receiving another is larger than it,
  whatever the wall clock does.
- **Fault probes,** as for the model: a merge that keeps the left side,
  a stamp from the wall clock alone, an outbox not saved; each must
  fail a property or the model.

## Consequences

- Every device ends with the same state, and the value it shows is the
  person's latest edit, not the last one to reach the computer.
- A phone that was away keeps what it did there, across its own
  restarts and clock changes.
- Each shared value carries a stamp, on disk and on the wire: a little
  more JSON.
- Tombstones for forgotten UI keys need a rule for when they go.
- Drafts merged as text, if chosen, add a library to the phone and
  computer builds, for one field.
