# 0056: Luau plugins are Elm, written as coroutines

- Status: accepted.
- Date: 2026-11-14

## Context

A Luau plugin today is a table of hooks. Each hook gets a `ctx`, changes
`ctx.state` in place, and reaches the world through it: `ctx.jev.ask`,
`ctx.http.request`, `ctx.tools.infer`, `ctx.store.get`, `ctx.files.read`
and the rest. A reach is a host function that blocks the hook until it
answers. The host reads the state back when the hook ends and stores it.

That shape caused the bugs the last sweeps found (ADR 0054):

- **A hook that waits holds a copy.** heiko-goal's check asked Jev, and a
  Pause pressed meanwhile was written over when the check stored its
  state. The host now replays such an action on what the hook left
  (`Record::Acted` with its action and args). A replayed action that
  reaches Jev, its store or the network does that twice, and a slow action
  still loses its change if a hook moves the state under it (ADR 0050).
- **State is shared by compare and swap.** An action's result is taken
  only `still_on` the state it began on, so a second key pressed before
  the first came back is dropped. Every reader folds the same rule.
- **Staleness is each plugin's job.** heiko-memory's import carries a
  generation so a tick that read before a stop does not write after it.
  Each plugin that waits wrote its own guard, or did not.
- **Tests imitate the host's timing.** `t.hooks` runs hooks as
  coroutines and pauses them on their waits so that it can try orders.
  It works because a Luau hook is already a coroutine under the host. It
  is a model of the host, not the host.

An inventory of the 24 plugins (17 of heiko's, 7 examples) found 12 that
wait on the outside world. Fifteen hooks give the loop an answer that
depends on what they waited for: before_stop in heiko-goal and
heiko-constitution, before_tool in heiko-constitution, start and
before_request in heiko-reasoning, rewrite_context in the three
compactions, after_tool_result in heiko-fast-compaction, title in
heiko-titles, four forge hooks in heiko-github, and word-count's tool. Eight
more wait and then change only their state. The other 12 plugins only
read and write their store and files.

On 2026-11-09 the person chose a full Elm model for plugins: messages,
one update, commands for every effect, tools and hooks pure too. On
2026-11-14 they confirmed it after `heiko.machine` and `t.hooks` were
built, with two refinements: every effect is a command, a local read
too, and plugins write it in direct style on Luau's coroutines.

## Decision

### Elm underneath

Each plugin has one **model** per run (and one per app, for its jobs and
pages), and one **mailbox** in front of it. Everything that reaches a
plugin is a **message** in that mailbox:

- a hook firing (`start`, `before_tool`, `before_stop`, ...), with its
  input;
- an action, a command (`/name`), a tool call, a job tick, a page drawn;
- the answer to a command the plugin gave.

The host takes messages one at a time. Handling one runs the plugin's
code until it ends or gives a command; that run is a **step**, and a
step is atomic: no other message is handled for that model while it
runs. A step reads and changes the model, gives at most one command to
wait on, and gives commands that need no answer (a log line, a store
write, a notice). Its changes are committed when it ends: the model, the
store writes, the files written.

A command is data, carried out by the host outside the step. Its answer
comes back as a message, and the step that waited on it goes on.

### Coroutines on top

A plugin does not write messages and an update function by hand. It
writes its hooks as today, in direct style, and waits with `heiko.await`:

```lua
before_stop = function(stop, model)
  local g = model.goal
  if g == nil or g.status ~= "active" then
    return nil
  end
  local answers, cost = heiko.await(cmd.jev.ask({
    state = { goal = g.condition, final_answer = stop.text },
    questions = { met = QUESTION },
  }))
  -- The model as it is now: a Pause may have come while Jev answered.
  g = model.goal
  if g.status ~= "active" then
    return nil
  end
  goal.check(model, verdict(answers, cost))
  return heiko.continue(goal.continuation(g))
end,
```

- A hook is a coroutine. `heiko.await(command)` yields the command; the
  host carries it out, and resumes the coroutine with the answer, in a
  later step. What the hook returns is its answer to the loop, as now:
  the loop waits for it, within the hook's limit.
- `model` is the model itself, not a copy. Between two awaits nothing
  else changes it. Across an await anything may have: an action, another
  hook, a job. A hook reads what it needs again after each await. That is
  the whole concurrency rule, and the tests check it (below).
- `ctx` goes. What it carried as data comes in the message: `run`,
  `settings`, `drafts`, `models`, the time the message was made. What it
  reached becomes `cmd`.
- `view`, `card` and `settings.view` stay plain functions of a frozen
  copy of the model. Awaiting in them is an error.
- `heiko.all({ ... })` awaits several commands at once and answers them
  together (today's `parallel`).

### Every effect is a command

| Today                                                | Command                                            | Waits on        |
| ---------------------------------------------------- | -------------------------------------------------- | --------------- |
| `ctx.store.get` / `list`, `ctx.stores(repo)`         | `cmd.store.get` / `list`, `cmd.stores(repo)`       | the store       |
| `ctx.store.put` / `delete`                           | `cmd.store.put` / `delete`                         | nothing         |
| `ctx.files.read` / `list` / `image` / `frontmatter`  | `cmd.files.*`                                      | the disk        |
| `ctx.files.write` / `remove` / `rename` / `mkdir`    | `cmd.files.*`                                      | nothing         |
| `ctx.transcript()`, `ctx.measure`, `ctx.secrets.has` | `cmd.transcript`, `cmd.measure`, `cmd.secrets.has` | the host        |
| `ctx.time.local` / `parse`, the clock                | `cmd.time.*`, or the message's time                | the host        |
| `ctx.jev.*`, `ctx.http.request`                      | `cmd.jev.*`, `cmd.http.request`                    | the network     |
| `ctx.tools.infer`, `ctx.tools.<tool>`                | `cmd.infer`, `cmd.tool(name, args)`                | a model, a tool |
| `ctx.chats.wake`, `ctx.desktop.notice`               | `cmd.chats.wake`, `cmd.notice`                     | another run     |
| `ctx.blobs.put_file`, `ctx.archive.*`                | `cmd.blobs.put_file`, `cmd.archive.*`              | the disk        |
| `ctx.log`                                            | `heiko.log` (given, not awaited)                     | nothing         |

A command that needs no answer is given with `heiko.give(command)` and does
not end the step. A read is awaited like any other: it costs a word,
`heiko.await`, and keeps replay exact.

`infer` with the plugin's own tools (heiko-memory's distill) asks the
plugin for each tool call as a tool message, handled in its own steps;
the answers go back to the model, and the infer's answer ends the await.

`heiko.start_chat`, `heiko.steer_chat`, `heiko.allow` and `heiko.ask` return to
their first form: commands awaited where they are asked, not a `then`
action named for the answer. `heiko.ask` in a tool awaits the person's
answers in the tool's own coroutine.

### The log, and durable hooks

The host keeps, per model, the **log**: each message handled, and each
command's answer, in order. Steps are deterministic: the same model, the
same message and the same answers give the same step. So the log is the
whole truth, and a model with hooks under way can be rebuilt from it:

- After a restart, the host starts from the last **checkpoint**, a model
  snapshot taken when no hook was under way, and handles the log's
  messages again. A command already answered is not carried out again;
  its logged answer is given back. The coroutines come back suspended
  where they were, and the commands still unanswered are carried out.
- A checkpoint is taken whenever the model has no hook under way, which
  is most of the time; the log before it is dropped.
- The log records the plugin's digest. A new version does not replay an
  old log: it starts from the last checkpoint, and the hooks that were
  under way end as failed, which the loop already handles.

Determinism is enforced, not asked for: a plugin's VM has no clock, no
randomness and no reach outside commands. Time comes in the message or
from `cmd.time`. Iteration of a table is deterministic for the same
operations in the same VM build, which the digest pins.

### What a run records

Readers (the computer, phones, history) still fold **model snapshots**,
not the log: a `Record::State` after each step that changed the model,
and the view drawn from it, as today. A phone needs no Luau.

Actions go to the model's mailbox, wherever they are pressed: the run's
host while the run goes on, the registry's own while it does not. No
interface carries out an action itself any more, so `Record::Acted`,
`still_on`, `catch_up` and the action replay go. Two keys pressed at
once are two messages, both handled, in order.

### `heiko.machine` and effects

A machine's `step` stays pure and gives effects as data (ADR 0054). The
hook that steps it carries the effects out as commands. That answers ADR
0054's "effects across hooks": effects stay a plugin's own names, and a
hook turns them into commands.

### Stores take turns per step

A plugin's store is shared by every run in its repository. Today a hook
holds the store for its whole run. Under steps, the store is taken for a
step: its reads see what is committed, and its writes commit with it.
Across an await, another run's step may write. A hook that reads, waits,
and writes reads again after the wait, as with the model.

## Testing

- **A case drives the coroutine.** `t.run` gives a hook's pending
  command and takes its answer:

  ```lua
  local h = run:hook("before_stop", { text = "done" })
  t.equal(h.waits.kind, "jev.ask")
  run:action("act", { what = "pause" })
  h:answer({ met = { probability = 0.9 } }, 0)
  t.equal(h.returned, nil)
  t.equal(run.model.goal.status, "paused")
  ```

  A fake answers commands by default as today's fakes answer reaches, so
  cases that do not care about timing stay short.

- **`t.hooks` tries every order at the yield points.** A way in runs to
  its next await; a rule answers a waiting command or starts another way
  in. The orders are the host's own, not a model of them, because the
  fake and the host run the same steps.
- **Stale reads are found.** A rule may change the model between any two
  steps; a hook that kept a field from before an await and acted on it
  breaks an invariant in some order.
- **Replay is a property.** For any case, rebuilding from the log gives
  the same model and the same commands still waiting. A step that reads
  something outside its message and answers fails it.
- **Fixtures are logs.** A fixture is a recorded log, replayed; the view
  it draws is compared as now.
- `t.machine_of` is unchanged.

## Order of work

In one feature workspace, landed when every plugin passes, so main never
holds half of it:

1. The runtime: a VM per model, kept while the model lives; the mailbox;
   steps; commands and their executor; `heiko.await`, `heiko.give`,
   `heiko.all`; the log and checkpoints; determinism in the VM.
2. The harness: `t.run` with pending commands, `t.hooks` on steps, replay
   as a property, log fixtures.
3. The port, one commit per plugin: the 12 that only read and write
   first, then the 12 that wait.
4. Removal: `ctx`, `Record::Acted`, `still_on`, `catch_up`, the action
   replay, the `then` actions, the generation in heiko-memory's import.
5. Docs: the plugins skill, the reference, `heiko-plugin new`'s template.

## Alternatives considered

- **Hand-written messages and update.** The plain Elm form: a plugin
  writes `update(msg, model)` and threads each wait through a message of
  its own. It is what the runtime does underneath, but heiko-constitution's
  before_tool or heiko-tree-compaction's levels become chains of messages
  and a state for each link. Coroutines give the same steps in the code
  people write.
- **Local reads stay synchronous.** Reading the store or a file inside a
  step, logged with the step. Shorter, but a read outside the command set
  makes the step's input larger than its message, and replay has to trust
  the log of reads. The person chose every effect a command.
- **No durable hooks.** Dropping a suspended hook on restart, with no
  log. Simpler, but determinism becomes a habit, the `heiko.ask` form open
  for hours is lost on a restart, and fixtures stay hand-written.
- **A message log for readers.** Records as messages, every reader
  running update. Smaller records, but every phone needs the Luau VM and
  the plugin's version. Snapshots stay.

## Consequences

- The bugs of ADR 0054's sweep that came from copies (a lost Pause, a
  stale import tick, a dropped second key) cannot happen: there is one
  model, and steps do not overlap.
- Every hook that waits gains a rule: read the model again after an
  await. The tests find a hook that does not.
- Store-heavy plugins (heiko-todo, heiko-memory) gain an `await` per read.
- A plugin's VM lives as long as its model, not one hook: memory per run
  and plugin, released when the run ends and its checkpoint is taken.
- The host gains the log's storage and its replay, and loses the CAS,
  `catch_up` and the action replay.
- No plugin keeps the old API; there are no users to migrate.

## As built, 2026-11-14

- **The actor is Luau** (`heiko_actor.luau`, installed by `heiko`): `heiko.cmd`,
  `heiko.await`, `heiko.give`, `heiko.all`, `heiko.log`, and `heiko._actor`, whose
  `deliver` takes one message a step. The host and the test harness run
  the same steps.
- **Hooks are `(input, model, env)`**, `view` is `(model, env)` and a
  card's input is `{ call, result }`; drawings read a frozen copy of
  plain tables. A host tool's command has its name as its op, Jev is an
  op like any other (`PluginHost::command`), and only `all` carries
  several. An awaited null reads as nil, an infer's value comes out of
  its envelope, and a failure raises the command's own message.
- **The host keeps a VM per model** (`actor.rs`), its `require` over the
  plugin's modules, no clock nor randomness, and a limit per step. A
  run's hooks go through a mailbox per plugin (`mailbox.rs`), whose steps
  keep their model, log and view with the run's records; `Loaded::call`
  runs a hook alone on an actor made from its context's state. An
  interface's action on a run going on is a message to that run's
  mailbox (`Registry::live`). `ctx`, `catch_up` and the action replay
  are gone.
- **Replay works and is tested**: a mailbox comes back from the model it
  kept when quiet and the messages taken since, its hooks under way again.
- All 24 plugins are ported, with the tests of the host, heiko-ui and the
  built-ins; the plugins skill and the reference teach the new API.

Not yet built, each to come on this:

- `heiko.start_chat`, `heiko.steer_chat`, `heiko.allow` and `heiko.ask` are still
  answers with a "then" action, not commands awaited where they are asked.
- The harness has no `run:hook` with pending commands, no replay as a
  property and no log fixtures; `t.run` runs a hook to its end against
  the fakes, and `t.hooks` pauses on them.

## As built: the journal kept with a run, 2026-11-14

A run keeps a mailbox's journal in its records only while a hook of it
awaits: `Record::Checkpoint` (the model when none awaited, and the
version's digest) as a step leaves one under way, `Record::Taken` for
each message taken from the last quiet model on, and `Record::Quiet` once
none awaits. A model that never awaits keeps no more than its states.
A run that inherits a journal still open, as when it stopped with a hook
awaiting, takes it again by the same version: the hooks come back under
way, each carried on from what it awaited, beside the run. Another
version starts from the run's last state, and closes the journal.

## As built: actions on a run not going on, 2026-11-14

An action on a run not going on is a message to the host's mailbox of
it: made from the model an interface folded as the first comes, then the
model every later action goes on, whatever an interface shows meanwhile.
The 64 latest are kept, and one is let go as its run goes on again, whose
mailbox starts from the run's records. The interface records the model
whole (`Record::State`), and `Record::Acted` and `still_on` are gone:
actions at once, or one after another from devices that had not seen
each other's, are all kept, where the compare and swap kept one.

## As built: a store's changes go in whole, 2026-11-14

A hook holds its store a step at a time, so two runs of one repository
can each read, await and write, and one write the other over: two
issues of heiko-todo made at once took one id, and two claims of one
issue both won. The person chose a commit with expectations:
`cmd.store.commit({ expect, put, delete })` writes all of it in one
turn with the store, only while each key of `expect` still holds its
value (`json.null` for a key the store does not have), and answers true;
else it writes nothing and answers false, and the hook reads again and
tries again. heiko-todo keeps a hook's writes until it ends and commits
them while its `rev` is what it read first; every change bumps `rev`.

## Settled at review, 2026-11-14

- **Limits.** A hook's limit counts only its own steps, so its code stays
  fast. Each waiting command has its own limit (Jev, http and infer as
  today's), and the loop caps how long it waits for a hook's answer.
- **An idle run's model is made on demand.** When an action, page or
  command comes for a run that is not going on, the registry rebuilds its
  model from the last checkpoint and the log, handles the message, takes
  a checkpoint, and lets the model go once no hook is under way.
- **Jobs run against one model per plugin per repository,** as
  `job_state` is now.
