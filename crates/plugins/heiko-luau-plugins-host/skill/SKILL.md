---
name: heiko-plugins
description: Writing, changing and testing heiko's Luau plugins, in the heiko-plugins repository. Load it before you write or change a plugin, or when someone asks heiko to do something on its own whenever a run calls a tool, stops, or ends a turn.
---

# heiko's Luau plugins

A plugin changes how heiko's runs go: it adds tools the model can call,
looks at each tool call before it runs (to block it or flag it for
review), checks a run before it stops, and shows a line in the run's
status. Plugins are Luau files in the `heiko-plugins` repository, one
folder each:

```
<name>/
  heiko.toml           -- what it is: name, version, description
  src/plugin.luau    -- returns the plugin: heiko.plugin { ... }
  src/<module>.luau  -- modules of its own: require("<module>")
  tests/<file>.luau  -- its tests, run against fake runs
  README.md        -- what it is for, for the person
```

`heiko.toml` has `name`, the folder's; `version`, three numbers such as
`0.1.0`; `description`, one line for the Plugins screen; and `entry`, if
the plugin starts anywhere but `src/plugin.luau`. Nothing else goes in
it. The name and the description live only there: `heiko.plugin { ... }`
refuses them. A plugin is active once its
commit is on `main` and its tests pass. Write it, write its tests, run
`plugin_test` with the plugin's name until they pass, then commit. When
your turn ends, heiko lands your commits on `main` by itself if every
plugin you changed passes and reaches nothing new; this chat goes on.

## What the person sees

When `main` moves, heiko loads every plugin, runs its tests and shows the
outcome on the Plugins screen:

- **Active**: its tests pass, and it reaches nothing it did not before.
- **Waiting**: it reaches further than the version the person allowed:
  a new tool in `uses`, `jev` or `infer`, or a new `before_tool` or
  `before_stop`. The person clicks Allow; until then, the version
  before stays active. Say so when you commit such a change.
- **Failing**: its tests fail; the version before stays active.
- **Broken**: it does not load; the error and its line show.

A change is live at once, with no new chat or message:

- **In this chat**, a plugin as this workspace has it runs from your
  next model request, once `plugin_test` would pass and it reaches
  nothing new. Write it, test it, then use it right here.
- **Everywhere else**, once it lands on `main`: heiko lands this chat's
  commits when a turn ends with every plugin you changed passing and
  reaching nothing new, and this chat goes on from there. Work that
  fails its tests, waits for the person, or would conflict stays here
  until a later turn fixes it; say so to the person.
- A tool the run did not start with is not in your tool list: call it
  from `codemode`, where `search_tools` finds it. From the next message
  it is a tool like the others. A changed tool you already have runs
  its new version.

## The plugin

```luau
local heiko = require("heiko")
local ui = heiko.ui

return heiko.plugin {
  uses = { tools = { "exec_command" }, jev = false, infer = false,
           network = { "ntfy.sh" }, secrets = { "ntfy" },
           transcript = false, archive = false, chats = false,
           files = { "workspace" } },
  settings = { schema = { ... }, default = { ... }, view = function(settings, model, env) end },
  tools = { name = { description, parameters, call, card } },
  start = function(plan, model, env) end,
  before_tool = function(call, model, env) end,
  before_request = function(request, model, env) end,
  after_tool_result = function(result, model, env) end,
  rewrite_context = function(view, model, env) end,
  rewritten = function(info, model, env) end,
  before_stop = function(stop, model, env) end,
  turn_end = function(turn, model, env) end,
  continues = 10,            -- times before_stop may keep a run going
  events = { "tool_end" },   -- what `on_event` hears
  on_event = function(event, model, env) end,
  run_end = function(run, model, env) end,
  view = function(model, env) end,
  actions = { name = function(args, model, env) end },
  nav = { label = "Deploys", icon = "plug", page = "deploys" },  -- its sidebar entry
  pages = { board = function(params, model, env) end },  -- its pages, of a repository or of the app
  repo_nav = { label = "Todo", page = "board", icon = "list" },
  repo_menu = { label = "Todo", page = "board", icon = "list" },  -- in each repository's menu
  keys = { ["ctrl-2"] = "board" },  -- a key heiko leaves to plugins, to a page of the repository
  drafts = { "search" },           -- inputs its pages read as typed: params.drafts
  repo_data = function(_, model, env) end,   -- what its entry under a repository shows
  recover = function(input, model, env) end,  -- as heiko starts: the runs cut off
  commands = { name = { description, args, icon, popover, run = function(text, model, env) end } },
  jobs = { name = { every = 3600, label = "...", run = function(_, model, env) end } },
  fills = { "heiko.title" },  -- the slots it fills
  title = function(run, model, env) end,
  notify = function(notice, model, env) end,
  compaction = { rewriters = { "heiko-fast-compaction" } },
}
```

Every field is optional. `tools` adds tools the model can
call, each with its arguments' JSON Schema, its `call` and, if it draws
one, its `card`. A tool with `when = "on"` is offered only while the
boolean setting `on` is on. One with `model_only = true` is the model's
alone, never a script's: a tool whose call waits for the person
(`heiko.ask`) should be, as a script's call ends with the script. In a
tool's `call`, `env.run.call` is the call's id.

### Hooks

Every hook is a function of `(input, model, env)`:

- `input` is what the hook is given, as the table below says, and `nil`
  for a hook that is given nothing (a job's `run`, `repo_data`, `app`).
- `model` is the plugin's model for this run (a job's is its own): the
  model itself, not a copy. Change it in place. Never put a new table in its place
  (`model = {}` changes nothing); to reset it, clear its keys.
- `env` is what the run gives the hook: `run`, `now`, `settings`,
  `models` and, in a command, `drafts` (below).

A tool's `call` is `(args, model, env)`, and an action's too. Drawing is
different: `view` is `(model, env)`, a tool's `card` is `(input, model,
env)` with `input = { call, result }`, and `settings.view` is
`(settings, model, env)`. Each draws from a frozen copy of the model,
and none of them may await.

| Hook                | Given                                                                     | Returns                                                                       | Time   |
| ------------------- | ------------------------------------------------------------------------- | ----------------------------------------------------------------------------- | ------ |
| tool `call`         | the arguments, as `parameters` describes                                  | a JSON value, or `error(message)`                                             | 60 s   |
| tool `card`         | `{ call, result }`: the call's arguments and the result                   | a view piece, drawn on the call's card                                        | 200 ms |
| `start`             | `{ input, context, instructions, model, efforts, effort, prefix_tokens }` | `nil`, `heiko.start{ context, instructions, effort, input }` or `heiko.fail(why)` | 2 s    |
| `before_tool`       | `{ id, name, args }`                                                      | `nil`, `heiko.block(why)`, `heiko.flag(why)` or `heiko.change(args)`                | 2 s    |
| `before_request`    | `{ model, effort, turn, messages, replies }`                              | `nil`, or `heiko.effort(level)`                                                 | 200 ms |
| `after_tool_result` | `{ id, name, args, ok, text, truncated, images, nested }`                 | `nil`, or `heiko.result(text)`                                                  | 2 s    |
| `rewrite_context`   | `{ trigger, turn, tokens, window, messages }`                             | `nil`, or `heiko.rewrite{ keep, insert, drop_calls, texts, ... }`               | 10 s   |
| `rewritten`         | `{ plugin, before, after, drops_conversation }`                           | nothing                                                                       | 2 s    |
| `before_stop`       | `{ text, turn, reason }`                                                  | `nil`, or `heiko.continue(message)`                                             | 10 s   |
| `turn_end`          | `{ turn, text, calls = { { name, ok } }, cost, commit? }`                 | nothing                                                                       | 2 s    |
| `on_event`          | `{ kind, turn, ... }`, for each of `events`                               | nothing                                                                       | 200 ms |
| `run_end`           | `{ stop, turns, cost, text }`, `text` its last reply                      | nothing                                                                       | 10 s   |
| `title`             | `{ task, model }`, outside any run                                        | a title, or `nil`                                                             | 2 s    |
| `notify`            | `{ title, body }`, outside any run                                        | nothing                                                                       | 10 s   |
| `view`              | nothing: it reads a frozen copy of the model                              | `{ status?, note?, page? }`, or `{}`                                          | 200 ms |
| `actions[x]`        | a button's or an input's arguments                                        | nothing; what it does to `model` stays                                        | 2 s    |

The time is each step's: the hook's own code from its start or an
await to its end or its next await. What it awaits is carried out by
heiko outside that time, with a limit of its own.

- `heiko.block(why)` keeps the call from running; the model reads why.
  `heiko.flag(why)` lets it run and marks it for the person's review.
  `heiko.change(args)` lets it run with `args` in place of its own, marked
  for review too; it needs `uses.change_args = true`, which the person
  allows as they allow new tools. The tool checks the new arguments as
  it would the model's, and the plugins after it see them.
- `heiko.start{ context = { ... }, instructions = "..." }` adds to what
  the run starts with: `context`'s texts open its first message, after
  the ones already there, and `instructions` follow the run's own. The
  plugins after it see what it added. Instructions are sent with every
  request: keep them the same from run to run, or each run pays to
  send them in full. Having `start` at all is new reach. `start` is
  also given the run's `model`, the `efforts` it takes (lowest first),
  its `effort` (`nil` on auto) and
  `prefix_tokens`, what the run inherits and its instructions, which a
  change of effort resends uncached; `heiko.start{ effort = "high" }`
  sets the effort the run starts at, on auto only, the first plugin
  that sets one setting it.
- `heiko.result(text)` has the model read `text` in place of the call's
  result, marked for review; the plugins after it read it as changed.
  `text` is the result's text, cut at 256 KiB (`truncated` says so);
  `images` counts its images, and `nested` is true for a call a tool
  made, such as a script's, whose result goes back to that tool, not
  the model. `heiko.result(text, { flag = false })` does not mark the call
  for review: for a result the plugin cut but kept verbatim, as pruning
  does. With `uses.transcript`, `heiko.await(cmd.transcript())` here is
  what the model had read when it made the call.
  Having `after_tool_result` at all is new reach, which the person
  allows: it changes what the model reads.
- `rewrite_context` is offered the run's context between turns, when a
  request overflowed, and while the run is idle: `messages` holds each
  message's `index` (from 1), `role` (`user`, `assistant`,
  `toolResult`), `text` (whole), `size` (its bytes), `tokens` (the
  loop's estimate of it) and `images`, when it has any, with its
  `parts`, `{ text }` or `{ image = true }` in order; an answer's
  `thinking`, when it has any, and `calls`, each `{ id, name, args,
order, json }`, `json` the arguments as the model wrote them; a
  result's `call`, `name` and `ok`. `window` is the model's, `nil` when
  it is not known.

  `heiko.rewrite{ keep = { 1, 4 }, insert = { { before = 4, text = "..." } }, drops_conversation = true }` keeps those messages and puts the texts
  before them as the person's; `details`, any JSON, is stored with the
  rewrite. `drop_calls = { "call-1" }` drops those calls with their
  results, and an answer left with neither text nor a call goes too.
  `texts = { { index = 3, text = "..." } }` replaces the text of a
  person's message or a result, its images gone: an answer's text
  cannot be replaced. `heiko.await(cmd.measure(rewrite))` gives `{ before, after }`,
  the bytes of JSON the transcript is and would be after that rewrite,
  to decide whether it saves enough. The last message always stays as it was, and so
  does every call with its results. The first plugin that rewrites
  wins; which plugins may rewrite at all is the person's pick on the
  Plugins screen. Having `rewrite_context` at all is new reach. One
  that errors, when none rewrites, tells the run why: on an overflow,
  the run fails with it. It has ten minutes when it reaches the model
  or Jev, as a compaction's summaries take.

- A plugin may fill four of heiko's slots, named in `fills`. Filling one
  is new reach, and heiko-luau-plugins must be the slot's pick on the
  Plugins screen; the first active plugin by name that fills it does.
  - `heiko.title`: `title` writes a run's title from its `task` (at most
    36 characters; `nil` leaves the prompt's first line). It runs
    outside any run: `env.run` and `model` are empty.
  - `heiko.notify`: `notify` gets each notice heiko would show while its
    window is not in front. `heiko.give(cmd.notice{ title, body })` shows
    it on the desktop, and a click opens its chat; or the plugin sends
    it where it reaches over HTTP, such as a push service.
  - `heiko.forge`: where repositories are hosted. Its `forge` table
    names the forge (`name`, `id`, the `host` repositories clone from)
    and the secret that signs in (`token`, which `uses.secrets` must
    name); the person sets it on the plugin's settings. heiko clones and
    pushes over HTTPS with it, and never hands it to the plugin. Its
    hooks reach the forge's API over HTTP: `account(input, model, env)` (who
    is signed in), `repos` (a list of
    `{ name, description, branch, pushed_at }`), `can_push` (`{ repo }`, a boolean), `open_pull`
    (`{ repo, title, body, head, base, draft, reviewers }`, answering
    `{ number, url }`) and `checks` (`{ repo, head }`, answering
    `"running"`, `"passed"`, `"failed"` or `"none"`).
  - `heiko.compaction`: which plugins' rewrites run. The Luau plugins'
    own always do; the `compaction` table's `rewriters` names the
    others, plugins of heiko's own.
- `rewritten` hears that a rewrite replaced the context, whoever made
  it: how many messages there were `before` and are `after`.
- `heiko.effort(level)` sends the turn's request with that reasoning
  effort (`"low"`, `"medium"`, `"high"`…), and the ones after it; the
  first plugin that picks one sets it. `messages` counts the transcript.
- `heiko.continue(message)` keeps the run going with `message` as the
  next thing the model reads. A plugin keeps a run going at most three
  times, or as many as it declares, up to 20 (`continues = 10`).
- `heiko.start{ input = text }` gives the model `text` in place of the
  run's message; the first plugin that gives one sets it.
- `heiko.fail(why)`, from `start`, ends the run at once with `why` as its
  error: for a plugin whose own setup is broken, so the run does not go
  on unchecked.
- `turn_end`'s `cost` is what the turn cost, in US dollars. In a run
  with a workspace, `commit = { change, paths }` is the commit the turn
  left: its change id and the paths it changed, however they changed.
- A hook that errors or runs out of time counts as `nil`, and its error
  shows on the Plugins screen. The step that failed keeps nothing it
  changed in the model and drops the commands it gave; the steps before
  it stand. After three failures in a run the plugin
  is off for the rest of it. Never rely on an error to block.
- A tool's `call` refuses with `error(message, 0)`: the model reads
  the message as the call's failure, and it does not count as a
  failure of the plugin's.
- `on_event` hears the run's events that `events` names, each with its
  `kind` and `turn`: `turn_start`; `tool_start` (`id`, `name`, `args`);
  `tool_end` (`id`, `name`, `ok`); `retry` (`attempt`, `error`);
  `steered` (`text`); `continued` (`plugin`, `message`);
  `context_rewritten` (`plugin`, `tokens_before`, `tokens_after`);
  `plugin_error` (`plugin`, `message`). A name not among them keeps the
  plugin from loading.
- Keep what must last in `model`, never in a module's top-level
  values: the model is what heiko stores, and what a restart starts from.
- Hooks have no clock and no randomness: `os.time`, `os.clock`,
  `os.date` and `math.random` are not there. The time is `env.now`.
- A plugin whose `uses` names `infer` or `jev` may await them from every
  run hook but `on_event` (`start`, `before_tool`, `before_request`,
  `after_tool_result`, `rewrite_context`, `rewritten`, `before_stop`,
  `turn_end`, `run_end`), and those hooks then have 60 s, as a tool's
  `call` does. The run waits for them: a model call in `before_request`
  delays every request it runs before, so keep it to the turns that
  need it. A plugin that names neither keeps the times in the table.

### `model`, `env` and commands

A hook reaches nothing itself. Every effect is a **command**, a value
made by `heiko.cmd`, which heiko carries out:

- A command that answers is **awaited**: `heiko.await(command)` gives its
  answer, and `nil` for a value that is not there. The hook stops there
  until heiko has the answer, then goes on.
- A command that needs no answer is **given**: `heiko.give(command)`. It
  is carried out once the hook's step ends, in the order given, so an
  await later in the hook sees what it did.
- A failure raises the command's own message. Catch it with `pcall`
  where the hook goes on without the answer:
  `local ok, values = pcall(heiko.await, cmd.http.request(request))`.
  `heiko.jev.ask` and `heiko.infer` do it for Jev and the model.

```luau
local cmd = heiko.cmd

local notes = heiko.await(cmd.store.get("notes")) or {}   -- a read: awaited
heiko.give(cmd.store.put("seen", env.now.unix))           -- a write: given
local text = heiko.await(cmd.tool("read", { path = "a" })) -- a host tool
heiko.log("read " .. #text .. " bytes")                    -- a line in its log
```

**The rule: across an await, anything may have changed.** Between two
awaits, nothing else touches the model: that stretch of code is one
step. While a hook waits, other things run on the same model: an
action the person pressed, another hook, a job's tick. Another run in
the repository may write the store. So after each await, read the model
fields and the store keys you use again. Do not keep a value from
before an await and then decide on it, or write it back. `t.hooks`
(below) finds a hook that does.

```luau
before_stop = function(stop, model)
  local asked = model.goal
  if asked == nil or asked.status ~= "active" then
    return nil
  end
  local answers = heiko.await(cmd.jev.ask({ state = { goal = asked.condition, final_answer = stop.text },
                                          questions = { met = QUESTION } }))
  -- The model as it is now: a Pause may have come while Jev answered.
  local g = model.goal
  if g == nil or g.status ~= "active" or g.condition ~= asked.condition then
    return nil
  end
  ...
end,
```

An action is a message to the same model, not a copy: a Pause pressed
while the check waits is in `model.goal` when the check goes on, and
nothing is lost or done twice.

`heiko.all({ command, ... })` awaits several at once and answers a list,
in order, each `{ ok, values, error }`: `values` is the list of what
that command answered (a JSON null in it stays `json.null`), `error`
why it failed. One that fails does not raise.

`env` holds:

| Field          | What it is                                                                                                                                                                                  |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `env.run`      | `{ id, kind = "main" or "chat" or "sub_agent", repo, model, turn }`; in a tool's `call`, `call` too; in an action, `{ id, live, jev }`: whether the run is going on, and a Jev key is saved |
| `env.now`      | `{ unix, iso, weekday, local, offset, ms }`; `weekday` is `"Monday"` and so on, `local` the host's time as `YYYY-MM-DD HH:MM`, `offset` its seconds from UTC, `ms` milliseconds since 1970  |
| `env.settings` | what the person set, over `settings.default`; per repository when one keeps its own copy                                                                                                    |
| `env.models`   | the plan's models, newest of each family: `{ id, family, efforts }`                                                                                                                         |
| `env.drafts`   | in a command's `run`: what its popover's inputs say, by input id                                                                                                                            |

The commands, in `heiko.cmd`:

| Command                                                                    | What it does                                                                                                                                                                                                                        |
| -------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `cmd.tool(name, args)`                                                     | in a tool's `call`: one of the tools `uses.tools` names, as `cmd.tool("read", { path = "a" })`; awaited, it gives the tool's answer                                                                                                 |
| `cmd.infer(request)`                                                       | with `uses.infer`: the person's model (below); awaited                                                                                                                                                                              |
| `cmd.jev.ask(request)`                                                     | with `uses.jev`: Jev, in a tool's `call` and every run hook but `on_event`, booked to the run; awaited, it gives the answer and what it cost: `local answers, cost = heiko.await(cmd.jev.ask(req))`                                   |
| `cmd.http.request(request)`                                                | with `uses.network`: an HTTP request (below); awaited                                                                                                                                                                               |
| `cmd.store.get(key)`, `list(prefix)`; `put(key, value)`, `delete(key)`     | with `uses.store`: the plugin's store in the repository the hook works in (below); reads awaited, writes given                                                                                                                      |
| `cmd.transcript()`                                                         | with `uses.transcript`, in `before_request`, `after_tool_result` and `rewrite_context`: the run's conversation, whole (below); in `rewritten`, what the rewrite replaced; in `run_end`, the run's conversation as it ended; awaited |
| `cmd.measure(rewrite)`                                                     | in `rewrite_context`: the bytes of JSON before and after `rewrite` (above); awaited                                                                                                                                                 |
| `cmd.archive.path(kind)`; `cmd.archive.write(path, text)`                  | with `uses.archive`: a new file's path in heiko's archive of the repository, awaited; and writing it, once, for its owner alone, given                                                                                                |
| `cmd.files.*`                                                              | with `uses.files`: `read`, `list`, `image`, `frontmatter`, `roots`, `remote`, `repositories`, `repository`, awaited; `write`, `remove`, `rename`, `mkdir`, given (below)                                                            |
| `cmd.blobs.put_file(path)`                                                 | with `uses.files`: keeps a file's bytes in the run's repository, for `ui.image` (below); awaited                                                                                                                                    |
| `cmd.time["local"](at)`, `cmd.time.parse(text)`, `cmd.time.duration(text)` | the host's local time: writes `at` (seconds since 1970) as `YYYY-MM-DD HH:MM`, reads one back, and reads `90m`, `2h` or `3d`; `nil` when they are none; awaited                                                                     |
| `cmd.secrets.has(name)`; `cmd.secrets.forget(name)`                        | whether a secret is set, awaited; emptying it, given (below)                                                                                                                                                                        |
| `cmd.notice{ title, body }`                                                | in `notify`: a notice on the desktop, whose click opens its chat; given                                                                                                                                                             |
| `cmd.repos()`, `cmd.stores(repo)`, `cmd.chats.wake(run, text)`             | in a job, with `uses.chats`: the repositories the plugin keeps a store in, each one's store, and waking a chat (below); awaited, but for a store's writes                                                                           |

`heiko.log(text)` writes a line on the plugin's page, never for the model.
It needs no `give`.

`heiko.await` works anywhere in a hook's calls: in a module's function
the hook calls too. It does not work in `view`, a `card` or
`settings.view`, nor in a coroutine the plugin makes itself.

`model` is stored with the run: a resumed or forked run keeps it.
JSON has no empty array: use `array({})` for one that must stay a list.

### The model

A plugin with `uses.infer` asks the person's model one task:
`heiko.await(cmd.infer({ task, context, schema?, effort? }))` gives the
answer: its text, or JSON that holds to `schema`. `effort` is `"low"`,
`"medium"` and so on, else the model's own, and a model that does not
take it is asked with its own. `model`, a model or a family, asks that
one in place of `uses.model`'s: a model the person picks in the
plugin's settings. An answer that stopped with an error, was cut off at
the model's output limit or called a tool is refused, and the await
raises why: wrap it in `pcall` where the hook goes on without it.
`heiko.all({ cmd.infer(request) })` keeps the whole answer instead, as
the first of its `values`: `{ ok, value, error, reason }`, `reason`
being `"error"`, `"length"` or `"tool_call"`.

```luau
local ok, summary = pcall(heiko.await, cmd.infer({ task = "Summarize this.", context = text, effort = "low" }))
if not ok then
  heiko.log("no summary: " .. tostring(summary))
  return nil
end
```

With `tools`, the names of the plugin's own tools (no other tool), the
model may call them: each call runs the tool's `call` with the hook's
`model` and `env`, its answer goes back, and the model is asked again,
until it calls none, or none of `wait_for` (every one named, unless
given), or `rounds` (4) are done. `task` goes as written, then
`context` as JSON unless it is `json.null`, with `instructions` if
given; it takes no `schema`. The await gives two values: the last
answer's text, and `calls`, each `{ name, args, ok }` it made; a tool
that raises an error answers the model with it. Changes a tool makes to
`model` stay, as the hook's do.

```luau
local text, calls = heiko.await(cmd.infer({
  task = "Keep what this conversation learned.", context = said,
  instructions = KEEPER, tools = { "note_search", "note_write" },
  wait_for = { "note_search" },     -- a round of only writes ends it
}))
```

With `instructions`, the request is the plugin's own: its instructions
in place of `infer`'s, `task` as the message, as written, then
`context` as JSON unless it is `json.null`, and the answer its text. It
takes no `schema`, and no bound on the message but the VM's. `messages`,
`{ { role = "user" or "assistant", text } }`, go before the task, as a
conversation that sends a reply back; a `task` or a `text` that is a
list of texts is that many blocks.

In `rewrite_context`, `conversation = true` asks in the run's own
conversation: its messages go first, then `task` as written, then
`context` as JSON unless it is `json.null`, on the run's connection with
its instructions, tools and effort. The provider then reads the part the
conversation starts with from its cache, which a request of its own
would send again in full. `conversation = 3` sends only the first three
messages. It takes no `schema`, `effort` or `model`: the run's settings
are what the cache holds.
`uses.model` names a model, or a family (`"luna"`): the plan's newest
of it, else the model the hook would ask without one. In a run, from
any run hook, it asks that model, else the run's, booked to the run. In
a hook outside any run (`title`, `notify`, the forge's) it asks the
model `uses.model` names, or for `title` the run's own when it names
none, booked to the plugin.

### HTTP

A plugin that names hosts in `uses.network` sends requests to them, from
any hook: `heiko.await(cmd.http.request{ method?, url, headers?, body?, timeout? })`
answers `{ status, headers, body, json, truncated }`. A header or the
body names a secret with `"Bearer " .. heiko.secret("ntfy")`: its value
goes in as the request leaves and is taken out of everything that comes
back, so the plugin never sees it. A request may go only to a host
`uses.network` names, and name only a secret `uses.secrets` names; the
person types each secret's value on the plugin's settings pane, kept for
this plugin alone. Redirects are not followed. A new host or secret is
new reach.

A test fakes it with
`spec.http = function(request) return { status, json, body } end`.
A sign-in's token never reaches the
plugin: `keep = { secret = "github", from = "access_token" }` on a
request saves that field of the JSON that comes back as the secret and
takes it out of the answer (`kept` says it did), and `ui.secret(name)`
is an input whose text goes straight to the store.
`heiko.await(cmd.secrets.has(name))` says whether one is set, never its
value, and `heiko.give(cmd.secrets.forget(name))` empties it: signing
out. A test gives `spec.secrets = { github = true }` and reads
`run.secrets`.

### The archive and the transcript

`cmd.archive` keeps what a plugin cuts from the context, whole, for the
model to read back: tell the model the path. It writes only a path
`cmd.archive.path` gave, and never writes one twice. `uses.archive` is
new reach: the plugin writes files on the computer.

A Jev request's `state_json`, JSON text, may stand for its `state`:
its keys keep the order the text gives them, which a table cannot
keep.

`heiko.await(cmd.transcript())` is the run's conversation as the model will read it,
each message `{ index, role, text }`: `role` is `"user"`, `"assistant"`
or `"toolResult"`, and `text` is whole. An answer has its `calls`, each
`{ id, name, args, order }`, `order` the names of `args` in the order
the model gave them (a table keeps none); a result has its `call`,
`name` and `ok`. `uses.transcript` is new reach: the plugin reads all
the person said. `before_request` gets `replies`, the agent's messages
so far, to count steps without reading it.

### `heiko`

- `heiko.plugin(table)`, `heiko.block(why)`, `heiko.flag(why)`,
  `heiko.start(plan)`, `heiko.change(args)`, `heiko.result(text, options)`,
  `heiko.rewrite(rewrite)`, `heiko.effort(level)`,
  `heiko.continue(message)`.
- `heiko.start_chat`, `heiko.steer_chat` and `heiko.allow`: what an action
  or a tool answers to start or steer a chat (below).
- `heiko.ask` and `heiko.answer`: what a tool answers to ask the person a
  form, and what an action answers to give it the answers (below).
- `heiko.cmd`, the commands; `heiko.await(command)`, `heiko.all(commands)`,
  `heiko.give(command)` and `heiko.log(text)` (above).
- `heiko.secret(name)`: a secret in an HTTP request (above).
- `heiko.text`, `heiko.count`, `heiko.values`, `heiko.store`, `heiko.jev`,
  `heiko.infer` and `heiko.now`: the standard part (below).
- `heiko.machine(spec)`: a lifecycle as data (below).

### The standard part

What most plugins need and should not write again. It is part of
`heiko`, the same for every plugin, and will not change under you.

| Function                                  | What it gives                                                                                              |
| ----------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| `heiko.text.is_space(code)`               | whether a code point is white space, as Unicode has it                                                      |
| `heiko.text.trim(s)`                      | `s` without white space at either end                                                                       |
| `heiko.text.one_line(s)`                  | `s` on one line: each run of white space one space                                                          |
| `heiko.text.prefix(s, chars)`             | the first `chars` characters, and whether more is left                                                      |
| `heiko.text.cut(s, chars)`                | `s` in at most `chars` characters, the last an ellipsis when it was cut                                     |
| `heiko.text.codes(s)`                     | its characters as code points                                                                               |
| `heiko.count.plural(n, one, many)`        | `1 note`, `3 notes`; `many` is `one` with an `s` unless given                                               |
| `heiko.count.tokens(n)`                   | `900`, `184k`, `1.2M`, rounded down                                                                         |
| `heiko.values.count(map)`                 | how many keys                                                                                               |
| `heiko.values.copy(value)`                | a copy, however deep; a list stays a list                                                                   |
| `heiko.values.same(a, b)`                 | whether two values hold the same, however deep                                                              |
| `heiko.values.is_array(value)`            | whether a table is a list                                                                                   |
| `heiko.values.contains(list, value)`      | whether `list` holds `value`                                                                                |
| `heiko.store.get(key)`, `list(prefix)`    | with `uses.store`: awaited reads of the plugin's store                                                      |
| `heiko.store.put(key, value)`, `delete`   | writes, given: they land when the step ends                                                                 |
| `heiko.store.bump()`                      | adds one to `rev`, which a repository's pages draw again on                                                 |
| `heiko.store.of(repo)`                    | the same five on another repository's store, in a job                                                       |
| `heiko.jev.ask(request)`                  | with `uses.jev`: the answers and what they cost, or `nil` and why; it never raises                          |
| `heiko.jev.scores(request, ids)`          | each of `ids`' probability, in order, and the cost; or `nil` and why when one is missing                    |
| `heiko.infer(request)`                    | with `uses.infer`: `{ ok, value, error, reason }`; with `tools`, `value` is `{ text, calls }`               |
| `heiko.now(env)`                          | the step's time, in Unix seconds                                                                            |

The text functions count characters, not bytes. `heiko.store`,
`heiko.jev` and `heiko.infer` await, so only a hook may call them, and
what they answer may be stale by the next await.

### `heiko.ui`: what views are made of

Plain tables, drawn by heiko on the computer and on the phone:

- `ui.text(text)`, `ui.rich(text)` (with `` `code` `` and `**bold**`),
  `ui.mono(text)`
- `ui.badge(text, tone)`; a tone is `"neutral"`, `"good"`, `"warn"`,
  `"bad"` or `"info"`
- `ui.rows({ { "key", "value" }, ... })`, or `ui.rows({ key = value })`
- `ui.list({ ... })`, `ui.stack({ ... })` (one under another),
  `ui.row({ ... })` (side by side); strings in them are text
- `ui.progress(share, label)` with `share` from 0 to 1
- `ui.code(lang, text, { first_line = 1 })`: highlighted code; with
  `first_line`, its lines are numbered
- `ui.toned(text, tone, mono)`: a line in a tone; `"neutral"` dims it
- `ui.heading(text, ink)`: a small label over a group, in an ink when
  given; `ui.markdown(text)`, `ui.pill(text, tone)`
- an ink is the color a piece is printed in: a tone, or `"teal"`,
  `"cyan"`, `"purple"`, `"yellow"` or `"bright"`. A warm ink is for
  outlines, marks and text, never a fill
- `ui.header(title, { meta = "3 files", actions = { ... } })`: a
  section's head, which does not fold: its title over a rule, `meta` in
  small print at its right, then the `actions`, such as buttons
- `ui.ticks(lit, total, ink, { 6, 16 }, { { at = 16, text = "compacts" } })`:
  a quantity as `total` square ticks, the first `lit` in the ink; the
  fourth argument lists the places, from 0, of ticks drawn white, as
  thresholds. The fifth is labels, small print under the strip: each
  `text` is centred under the tick at the place `at`, from 0
- `ui.mark(state, count)`: the small mark of a state: `"working"`,
  `"asks"`, `"failing"`, `"met"`, `"limit"`, `"cap"` or `"idle"`; a
  `count` goes before it
- `ui.stamp(label, ink)`: a word or two outlined in the ink, such as
  `exit 0` or `queued`
- `ui.square(label, style, ink, action, args)`: a small square holding
  a number or a letter: a step, a check, a place in a queue; `style` is
  `"quiet"` (not reached yet), `"outlined"` or `"filled"`. With
  `action`, a click carries out `actions[action]` with `args`, so a row
  of squares can be a choice
- `ui.slip(text, ink, dashed, { page = "note", plugin = "...", params = { ... } })`:
  a note or an id on a label outlined in the ink; `dashed` for one
  proposed, not kept yet. With `page`, a click opens that page
- `ui.figure(text)`: a big number or time
- `ui.kanji(text, ink)`: one large glyph, the mark beside a note. A
  row that starts with one does not wrap under it: the glyph keeps its
  width, and the rest of the row takes what is left
- `ui.icon(name, tone)`: one of heiko's icons by name, such as
  `"memory"`, `"check"` or `"sub-agent"`
- `ui.table({ "head", ... }, { { piece, ... }, ... }, { widths = { 1, 4 } })`:
  a header over rows of pieces or strings; `widths` are the columns'
  shares of the width. With no head (`{}`), rows only
- `ui.scroll({ ... }, height)`: pieces that scroll past `height` rems
- `ui.rule()`, `ui.empty(text)`, `ui.spacer()` (room that pushes what
  follows in a row to its end), `ui.card({ ... })` (pieces in a group
  of their own, under a thin line, with no frame)
- `ui.gap(size)`: room between two pieces one under another; `size` is
  `"small"`, `"medium"` (when not given) or `"large"`. A `ui.header`
  after other pieces has room over it already, so sections need none
- `ui.band({ ... })`: pieces one under another on a darker plane: a
  result
- `ui.rail({ ... })`: pieces one under another, a line down the left
  joining the squares they start with: the steps of a change, a queue.
  Start each with a `ui.square`, as in
  `ui.row({ ui.square("1", "filled", "info"), "Fetched" })`
- `ui.keys(id, { "up", "enter", "1" }, action, { ... }, { through = { "escape" }, focus = n })`:
  an area that takes the keys. While it has them, each of its keys
  carries out `actions[action]` with `{ key, field = false }`; in a
  field inside it, the `through` keys still come, with `field = true`.
  A click outside its fields gives it the keys, and so does a new
  `focus` number: keep the number in the model and raise it to move
  the keys there once. `ui.input`'s `focus` does the same for a field
- `ui.diff(text, path)`: a unified diff, its lines numbered, colored as
  `path`'s language
- `ui.marked(text, { { from, to }, ... })`: one line of code with parts
  of it marked: a line a search found, and where. `from` and `to` are
  the first and last byte of a part, from 1, as `string.find` gives them
- `ui.terminal(id, text, { cols = 80, rows = 24, live = false, fold = true })`:
  what a program wrote, escapes and all, on a terminal heiko keeps under
  `id`; with `fold`, it shows a few rows until the person opens it
- `ui.ticker(id, since, stopped)`: the time since `since` (milliseconds
  since 1970), counting until `stopped` is given; keep `since` in the
  model
- `ui.due(id, at)`: the time left until `at` (milliseconds since 1970),
  `in 5m`, then `now`
- `ui.image(blob, mime, repo, height)`: a blob `cmd.blobs.put_file` kept
  in the repository `repo` (`env.run.repo`), `height` rems tall; the
  interface asks for its bytes once and keeps them
- `ui.graph(nodes, edges, height)`: a small map of nodes
  (`{ id, label, x, y, tone, focus, page }`, `x` and `y` from 0 to 1)
  joined by `{ from, to }` edges
- `ui.board({ { id, title, cards = { { id = "td-1", children = { ... } } } } }, action)`:
  columns of cards; dragging a card to another column carries out
  `actions[action]` with `{ card, from, to }`, the card's id and the
  two columns'. Too narrow for its columns, as on a phone, it shows
  them as tabs, so give each card a button for what a drag would do
- `ui.section(id, title, { ... }, { open = true })`: a heading that
  opens and closes what is under it
- `ui.tabs(id, { { "label", { ... } }, ... })`: one tab shown at a
  time
- `ui.button(label, action, args, kind, icon, size)`: carries out
  `actions[action](args, model, env)` on the run's model, then draws `view`
  again; `kind` is `"primary"` (the one main action, once or twice a
  view), `"secondary"`, `"danger"` or `"hot"` (one that needs the
  person), and `icon` one of heiko's icons, alone when `label` is empty.
  `size` is a primary button's: `"small"`, `"medium"` or `"large"`
- `ui.button(label, action, args, kind, icon, size, wide, detail)`:
  with `wide` true, the button takes the whole width, its label at the
  left. A wide primary button may have `detail`, a second line in
  smaller text inside the block
- `ui.segments(choices, picked, action, args)`: a labelled choice, its
  `choices` side by side, the one whose value is `picked` outlined. A
  choice is a string, or `{ label = "refuse", value = "block" }`; a
  string's value is the string. A click carries out `actions[action]`
  with `args` and the choice's `value`
- `ui.qr(text, { size = 200, offset = true })`: `text` as a QR code for
  a phone's camera, `size` pixels wide; with `offset`, printed over the
  offset
- `ui.input(id, { placeholder = "...", multiline = true, submit = "action" })`:
  a text input; Enter carries out `submit` with `{ text = ... }` and
  empties it, or keeps what it holds with `keep = true`
- `ui.modal(title, { ... }, { dismiss = "action" })`: a panel over the
  page; return it from `view` only while it should show
- heiko keeps where the person left a section or tabs, and what they
  typed in an input, under its `id`, on every device and across
  restarts
- `ui.link(label, page, { plugin = "...", params = { ... } })`: opens
  one of its pages, or another plugin's
- `ui.switch(label, detail, on, action, args)`: a switch, `on` or off,
  that carries out `actions[action]` with `args` when flipped
- `ui.status(text, detail, tone)`: the run's status line, from `view`

`view` is drawn after each step that changed the model, from a frozen
copy of it: `view(model, env)` reads, and cannot change the model nor
await. What it returns goes in these places:

- `status`: a `ui.status(...)`, the run's status line.
- `note`: a piece shown in the run's conversation, where the run is
  when the note first appears; a different note later shows where the
  run is then, and the earlier one stays.
- `notes`: `{ [key] = piece }`, notes that stay put: each shows where
  the run is when its key first comes, and is drawn again there as it
  changes, as a note answered or opened.
- `page`: pieces shown on the plugin's own page, from its newest run;
  buttons there act on that run.
- `banner`: pieces across the top of the run.
- `inspector`: pieces in the run's inspector, beside the transcript.
- `pane`: `{ title, fresh, body }`, the run's pane beside the transcript:
  its bar says `title`, and it opens by itself when `fresh`, a number,
  grew since the run was last drawn. Only one plugin's draws, the first
  by name.
- `band`: pieces just above the composer.
- `composer`: pieces in the composer's place, while the run goes on: a
  form the run waits on. Only one plugin's draws, the first by name;
  the composer comes back once none does.
- `row`: `{ line, count?, tone?, icon?, phone_line? }`, the run's line
  under its title in the sidebar; `tone` is `"good"`, `"warn"` or
  `"bad"`, `icon` one of the interface's icons, and `phone_line` the
  line on a phone, when it differs.
- `calls`: `{ [call_id] = { head?, label?, badge?, body?, mark?, reason?, dropped?, cut? } }`,
  pieces on a tool's card, its own or another's: `head` in place of its
  arguments, `label` at the header's end, `badge` beside its title,
  `body` under it, such as why a check flagged or blocked the call. `mark =
"blocked"` (with the `reason` the model got) or `"flagged"` marks the
  card so, as the phone and the run's review list show it. `dropped =
"result"` or `"call"` shows that a rewrite cut its result or dropped
  it. `cut = { kept, lines, archive, tokens_before, tokens_after, cost }`
  says an output reached the model cut: the lines it kept of how many,
  where the whole output is, and the tokens before and after; its card
  shows both.
- `messages`: `{ [text] = pieces }`, how a person's message whose text
  is exactly `text` draws, such as a command the plugin read, or a
  message it sent with `heiko.continue`; `ui.stack({})` draws nothing.
- `asks`: what the run asks the person, in words: its row says so, and
  a notice shows it while heiko's window is not in front.
- `plan`: `{ name, value }`, a field of the run's plan, as
  `{ name = "reasoning", value = "high" }`, shown as the plugin's.
- `plan_steps`: pieces shown as the plugin's step on the Plan screen.

A plugin that picks the effort says so under the effort picker's Auto
(`picker_auto`) with `picker_auto = { jev = "...", no_jev = "..." }` in its
declaration: the first with a Jev key saved, the second without.

A button whose action starts with `heiko.` asks heiko, not the plugin, on
the run the tree is drawn in (or its page shows):
`ui.button("Ship it", "heiko.send", { text = "ship it" })` sends the next
message; `heiko.steer` cuts into a turn going on; `heiko.composer` puts
`text` in the composer; `heiko.start` starts a run in `repo` with `text`;
`heiko.alert` shows `title` and `message`; `heiko.details` opens the run's
details; `heiko.pane` opens the run's pane; `heiko.open_run` opens the run
`run`; `heiko.cancel` stops the run, as its Stop button does. An action of the
plugin's own may end in one of them as well: it answers `heiko.interface("composer", { text = "..." })`, and once what
it did to the model is kept, heiko does that on the action's run.

A plugin's `commands` are slash commands in the composer's menu, each
with its `description`, what follows it (`args`, such as
`"<version>"`) and an `icon`. Typed in a chat, `/name text` runs
`run(text, model, env)` on that chat's model, as an action does. Typed
outside a chat, it runs with `env.run.id` empty and an empty model: what it
keeps is dropped, and `heiko.interface("send", { text })` starts a chat
with `text`. In tests,
`run:action("/name", { text = "..." })`. A chat started with `/name
text` takes its title from `text`, its leading `--flag value` pairs
dropped.

While a command is written, its `popover` shows above the composer:
`popover = { jev = pieces, no_jev = pieces }`, the second without a
Jev key (it defaults to the first). What its inputs say reaches `run`
as `env.drafts`, by input id; an input whose `submit` is `"heiko.submit"`
sends the composer on Enter and keeps what it says. In tests,
`run:action("/name", { text = "...", drafts = { id = "..." } })`.

An action of a plugin with `uses.jev`, a button's or a page's, may
await `cmd.jev` too, the host's, and has a tool's time for it.

A plugin's `jobs` are work it does by itself while heiko runs: each
`run(_, model, env)` once `every` seconds passed since it last ran (at least 1),
heiko's restarts or not, shown with the person's other jobs. One that
runs more often than every minute is a tick: it is not shown, and when
it last ran is kept only while heiko runs. Its `model` is its own, one
per repository, kept from run to run; `env.run` is empty. A job reaches what the
plugin's other hooks outside a run do: HTTP to its hosts, and `infer`
with `uses.model`. A new job is new reach. In tests, `run:job(name)`.

With `uses = { chats = true }`, a job reaches every chat: `heiko.await(cmd.repos())`
lists the repositories the plugin keeps a store in, `cmd.stores(repo)`
is one's store, as `cmd.store` is in a hook (`heiko.await(cmd.stores(repo).get(key))`,
`heiko.give(cmd.stores(repo).put(key, value))`), and
`heiko.await(cmd.chats.wake(run, text))` starts a turn of the chat `run`
on heiko's message `text`. It answers `"started"`, `"busy"` when a turn of the chat is going, or
`"gone"` when the chat takes no more messages, and fails when the turn
could not start; what the job wrote to its stores is kept just before,
as the turn may read them. `uses.chats` is new reach: the plugin starts
turns, which cost.

A plugin with `nav`, `{ label, icon? }`, has an entry in the sidebar that
opens its page; `icon` names one of the interface's icons.

### A repository's store, pages and chats

With `uses = { store = true }`, a plugin keeps a store in each
repository, apart from heiko's and from every other plugin's: keys and
JSON values. Reads are awaited, `heiko.await(cmd.store.get(key))` (`nil`
when the key is not there) and `heiko.await(cmd.store.list(prefix))`
(each `{ key, value }`, in key order); writes are given,
`heiko.give(cmd.store.put(key, value))` and
`heiko.give(cmd.store.delete(key))`. A read after a write sees it. The
store is shared by every run in the repository, and taken a step at a
time: a step's writes are kept when it ends, all of them or, when it
fails, none. Across an await, another run may write. So a hook that
reads, then writes, as claiming something does, reads again after any
await between the two. A store is new reach.

```luau
local function claim(id, env)
  local item = heiko.await(cmd.store.get("item/" .. id))
  if item == nil or item.claimed then
    return false
  end
  item.claimed = env.run.id           -- no await since the read
  heiko.give(cmd.store.put("item/" .. id, item))
  return true
end
```

`pages` are the plugin's pages of a repository: each `page(params,
model, env)` returns a view tree, `params.repo` the repository, its
store in `cmd.store`, and `env.run.jev` whether a Jev key is saved. A
page is a hook: it may await its store and files. A run's view
links to one of them with `params.repo`:
`ui.link("Review", "rules", { params = { repo = env.run.repo } })`. `repo_nav`, `{ label, page, icon? }`, is its entry under
each repository in the sidebar, opening that page; `repo_data(_, model, env)`
returns what the entry shows, `{ nav = { detail?, badge?, tone? } }`
among it. A page's buttons and inputs carry out the plugin's `actions`
in that repository, on its store there, with the page's own params in
`args.params`. Its links to its own pages (`ui.link(label, page, {
params })`) open them of that repository.

`recover(input, model, env)` is called once as heiko starts, in each
repository, with `input.runs`, the runs heiko found cut off when it last
closed: what the plugin keeps for them it lets go of there.

An action never waits on a chat: it answers what it asks, and heiko
carries it out once the action ended and its store is kept.
`heiko.start_chat{ repo?, text, ["then"] = "claimed", args = { ... } }`
starts a chat in `repo` (the action's own when it names none) on the
person's `text`; `heiko.steer_chat{ run, text, ["then"], args }` says
`text` to the chat `run`, cutting into a turn going on. Then
`actions[then]` is called with `args` and the chat's `run` (and
`repo`), or with `error` when it could not be done.

A tool asks the person first: it answers `heiko.allow{ title, text?,
action, args }`, and the run shows a card, on phones too, saying what it
would do, with Allow and Deny. The first answer takes it. Allowed,
`actions[action]` is carried out with `args` as the person's own, and
the model reads what it answered; denied, the model reads why; a run
that ends first withdraws it. A person's own click never asks.

A tool asks the person a form and waits for the answers: it answers
`heiko.ask{ questions, action }`, one to four questions, each
`{ question, header, options = { { label, description, preview } },
multi_select, other }`, at least one choice each, its labels written
and each once, `other = true` taking the person's own answer. Keep in
`model` what the form needs, with `env.run.call`: the plugin's view
draws it, in `composer`, on the computer and on phones, and its buttons
and keys are the plugin's actions on the run's model. One of them
answers `heiko.answer{ call, answers }`, each answer `{ picked = { labels
}, other?, note? }` in the questions' order, or `heiko.answer{ call,
declined = true }`. The first answers that fit, from whichever device,
take the form; answers that do not fit, or that come once it is taken,
are that action's error, and the form waits on. An action that answers
should leave the model as it was: the run takes the answers from there.
Then `actions[action]` is called in the run with `{ call, answers }` or
`{ call, declined = true }`, and the model reads what it answers. A
run cancelled first ends the call with an error.

### Files and blobs

With `uses = { files = { "workspace", "~/Pictures" } }`, a plugin's tools
read the folders it names, read-only: `"workspace"` the run's own, `"~"`
or `"~/x"` under the home folder, or an absolute path. In a tool's
`call`, a path is relative to the run's workspace, `~/` from home, or
absolute, and is read where it really is: through `..` or a link it
must still land in one of the folders. Each read is awaited:
`heiko.await(cmd.files.read(path))` gives the
text, `{ bytes = true }` the bytes in base64 and `limit` the most bytes
it reads (8 MiB without one); `cmd.files.list(path)` what a folder
holds, `{ name, dir, size }` each; `cmd.files.image(path)` its `size`
and, when heiko draws it, its `mime`, `width` and `height` (none for an
SVG). `cmd.blobs.put_file(path)` keeps a file of at most 8 MiB in the
run's repository and gives `{ id, digest, size_bytes }`, for
`ui.image`: its bytes never pass through the plugin. Each folder is new
reach.

`cmd.files` reaches the same folders in `start` and `app` (below).
`"skills"` names the person's skills folder as the host has it
(`~/.agents/skills`), `"package"` the plugin's own folder when it is one
of heiko's own, and `"data"` heiko's data folder, each with folders in it
as `"package/x"`; `cmd.files.roots()` says where each one named is.
`cmd.files.frontmatter(path)` reads a Markdown file's YAML frontmatter,
between `---` lines at its start: `{ keys, body }`, each key
`{ key, kind, text }` in order (`kind` is `string`, `integer`, `float`,
`boolean`, `null` or `other`, `text` the value as written), or
`{ error }` when it has none or is no mapping.

A folder named `{ "data/x", write = true }` is written too, each write
given: `heiko.give(cmd.files.write(path, text))` writes a file whole or not at all, making
the folders it is in; `cmd.files.remove(path)` removes a file or an
empty folder; `cmd.files.rename(from, to)` moves one, in place of a file
there; `cmd.files.mkdir(path)` makes a folder. A path is written where
it really is, and nothing past the written folders, nor the folder
itself, is. `"repository"` names heiko's own folder for the run's
repository, beside its rules; a plugin's own folders, `"data/<name>"`
and `"repository/<name>"`, are written with no Allow, and any other it
writes waits for an Allow of its own. Outside a run (a job, an action
of the app) `cmd.files.repository(name)` is that folder for each
repository heiko has, by name, or `nil`; a `"repository/..."` folder is
that folder of each of them there, and `cmd.files.repositories()` lists
them, `{ id, name, folder, remote, normalized }` each, for a plugin
whose `uses.files` names `repository`. `cmd.files.remote(path)` is the
Git remote of the repository a folder it reads is in (a checkout, a
worktree or a jj repository), `origin` first: `{ url, normalized }`,
`normalized` its host lowercased and its path, with no scheme, user,
port or `.git`, or `nil`. `cmd.files` reaches `after_tool_result`,
`rewritten`, `turn_end`, `run_end`, jobs and actions too.

### The app: `app`, `page`, `setup`, `models` and listed commands

What a plugin keeps of the whole app, outside any run:

```luau
app = function(_, model, env)        -- no input: cmd.files as uses.files names
  local roots = heiko.await(cmd.files.roots())
  local found = heiko.await(cmd.files.list(roots.skills))
  return { count = #found, nav = { detail = #found .. " found", badge = nil, tone = nil } }
end,
page = function(app, model, env)     -- the page `nav` opens
  return ui.stack({ ui.heading("Found"), ui.button("Open folder", "heiko.reveal", { path = "..." }) })
end,
commands = {                         -- in place of a table of commands
  list = function(app) return { name = { description = "...", args = "what to do", icon = "skill" } } end,
  run = function(name, text, model, env) return heiko.interface("send", { text = "/" .. name .. " " .. text }) end,
},
```

The host runs `app` as it draws the catalog, at most once a second; its
`nav` gives the sidebar entry's `detail` and `badge`. `page` draws in
place of the plugin's card on the plugins page, and its buttons are its
actions, run with no run and no repository; `heiko.reveal` shows a folder
`uses.files` names in the file manager. `commands.list` gives the
composer's commands by name, and `run` carries one out. A test reaches
them with `run:app()`, `run:app_page(app)` and `run:commands(app)`, its
`spec.roots` saying where each folder is.

A page of `pages` is drawn outside any repository too, as a page of
the app: a link that names no `repo` opens it there, as `nav.page`
does, with `cmd.files` and the plugin's store of the app, and it is
drawn again as what `app` keeps changes. A page reads what the inputs
`drafts` names hold as `params.drafts`, drawn again as they change. A
button may name `heiko.open` (`{ page, params, repo }`), and an action
answer `heiko.interface("open", { ... })`: one of the plugin's own pages,
of `repo` or of the app. What `app` keeps as `menu`, a piece, is drawn
in each repository's menu, after its `repo_menu` entry.
`ui.switch(label, detail, on, action, args)` is a switch whose flip
carries out `actions[action]`. A test draws a page of the app with
`run:app_page_of(name, params)`.

`setup(app, model, env)` gives one of heiko's own plugins' step of first-run setup,
`{ step, at, title, heading, detail, body }` (`at` is `"model"`,
`"before_repositories"` or `"after_repositories"`), drawn again of the
repositories picked so far, `env.run.picked` (`{ name, id, folder,
remote }` each); `run:setup(app, picked)` in a test. `models(app)` its
section of the Models screen. Their buttons may name `heiko.copy` (`text`,
and a `url` to open after), `heiko.open_url` (an `https` one) and
`heiko.setup_done`. Outside a repository, `cmd.store` in `app`, a page of
the app, its actions and a job is the plugin's store of the app. A test
reaches them with `run:setup(app)`, `run:models(app)`, and a forge's
hooks with `run:forge(name, input)`.

### Settings

What the person can set lives in `settings`:

- `schema`: a JSON schema of an object, its properties each a boolean,
  a string (with an `enum`, a choice), a number, an integer, or an
  array of strings with an `enum` (a choice of several). Nothing is
  saved that it does not accept, and it takes no keys beyond its
  properties.
- `default`: the values before anything is set.
- `view(settings, model, env)`, optional: the plugin's own settings
  page, drawn from `heiko.ui` of a frozen copy of the model; it cannot
  await. Without it, heiko draws a form from `schema`.

The page is drawn in the plugin's pane on the Plugins screen, and in a
chat's side panel. Three pieces are bound to a key of the settings
(dotted for a table inside: `limits.max`), and changing one saves it:

- `ui.toggle(key, label)`: a switch, for a boolean.
- `ui.choice(key, options, { multi = true })`: one of `options`, or
  several.
- `ui.field(key, { kind = "number", placeholder = "..." })`: a line of
  text, or a number; Enter saves it.

The person sets them everywhere or per repository. A value the schema
no longer accepts gives way to the defaults, and the pane says so.

### Modules

`src/<module>.luau` is `require("<module>")` in `src/plugin.luau`, the
two side by side: `src/` has no folders of its own. A module may require `heiko` and the plugin's other modules, as long as no two
require each other, however far around. `plugin`, `heiko` and any name
starting with `heiko_` are taken. A plugin cannot require another
plugin's modules: what plugins share is the standard part of `heiko`.

### A lifecycle: `heiko.machine`

Anything that goes through steps (a sign-in, an import, an approval)
is one machine, not flags kept beside each other. The bugs it rules
out: a step nobody wrote (a failure that never lets a new try start),
a field left from a step before (an error shown after it was fixed),
and an old answer written over a newer one.

```luau
local gate = heiko.machine({
  states = { idle = {}, waiting = { "change" }, refused = { "change", "why?" } },
  initial = { state = "idle" },
  on = {
    ask = { idle = function(_, e) return { state = "waiting", change = e.change } end },
    refuse = { waiting = function(s, e) return { state = "refused", change = s.change, why = e.why } end },
  },
})
model.gate = gate:step(model.gate, { event = "ask", change = "v1.2" })
```

- A state is a plain table, `{ state = "<name>", ... }`, with exactly
  the fields its entry in `states` lists (`name?` may be absent): keep it
  in `model` or the store, as any value. `step` refuses one out of
  shape, so a field left over is an error the tests find.
- `machine:step(state, event)` gives the next state and its effects.
  `event` is a table whose `event` names it; `state` nil is `initial`.
  An event the state has no arm for changes nothing (a stale button, a
  late answer); `strict = true` makes that an error.
- An arm is a function of the state and the event, and touches nothing
  else: it returns the next state, and the effects to carry out, a list
  of names or `{ name, ... }` tables. The hook that stepped carries them
  out as commands: the machine decides, the hook does the asking and the
  writing.
- When an effect waits (on the model, on Jev, on HTTP), step its answer
  on the state as it is after the await, read again, never on the one
  from before it. A Stop pressed meanwhile has moved the state, and the
  answer is an event that state has no arm for: it changes nothing, and
  the Stop is not undone.

```luau
local function step(model, event)
  local next, effects = sign_in:step(model.sign_in, event)
  model.sign_in = next
  for _, effect in ipairs(effects) do
    if effect == "poll" then
      local ok, polled = pcall(heiko.await, cmd.http.request({ url = POLL_URL }))
      -- `model.sign_in` as it is now, read again by the step below.
      step(model, ok and { event = "polled", answer = polled.json } or { event = "refused", why = tostring(polled) })
    end
  end
end
```

- `machine:is(state, name)`, `machine:handles(state, event)`,
  `machine:shape(state)`, `machine:tags()` and `machine:events()` read it.

## Tests

Each file under `tests/` is a list of cases; `plugin_test` and every
reload run them all. `t.run(spec)` is a fake run of the plugin, and its
hooks are methods on it. Its fakes answer the commands a hook awaits,
and carry out the ones it gives, so a case calls a hook and reads what
it left:

```luau
local heiko = require("heiko")
local t = heiko.test

t.case("what it checks", function()
  local run = t.run {
    now = "2026-10-09T10:00:00Z",  -- an ISO date: a Friday
    id = "r1", repo = "heiko",       -- the run's id and repository, else "test"
    settings = { ... },            -- else the defaults
    state = { ... },               -- the model, else {}
    tools = { read = "text" },     -- what cmd.tool answers, by name: a value, or a function of the args;
                                   -- `infer` here answers cmd.infer, as `{ ok, value }`
    store = { ... },               -- the store's keys to start from
    transcript = { ... },          -- what cmd.transcript() answers, with uses.transcript
    models = { ... },              -- env.models, else none
    jev = { ask = function(req) return answers, cost end },  -- what cmd.jev answers, else it fails
    http = function(request) return { status = 200, json = { ... } } end,  -- cmd.http.request
    secrets = { github = true },   -- which secrets are set
    offset = 3 * 3600,             -- the host's seconds from UTC, else 0
    stores = { other = { ... } },  -- a job's cmd.stores, by repository; the run's own is "test"
    wake = "started",              -- what cmd.chats.wake answers: a value, or a function of run and text
    job_state = { ... },           -- a job's own model, else {}
    files = { ["plot.png"] = { size = 120, image = { mime = "image/png", width = 4, height = 3 } },
              ["notes.md"] = "text" },  -- what cmd.files reads, by path; a blob is its path
    roots = { data = "d" },        -- what cmd.files.roots() answers
    remotes = { ["."] = "git@host:o/n.git" },  -- cmd.files.remote, by path
    repositories = { heiko = "r" },  -- cmd.files.repository, by name
    repos = { { name = "heiko", folder = "/f", remote = "..." } },  -- cmd.files.repositories()
    infer = { { calls = { { name = "note_search", args = {} } } }, { text = "done" } },
                                   -- infer{ tools }' rounds, in order, or a function of the request
  }
  t.equal(run:before_tool { name = "exec_command", args = { cmd = "ls" } }, nil)
end)
```

- `run:tool(name, args, call)` (`call` is `env.run.call`, `"call_1"`
  when not given), `run:card(name, args, result)`,
  `run:settings_view(settings)`, `run:action(name, args)`,
  `run:start(plan)`, `run:before_tool(call)`,
  `run:before_request(request)`, `run:after_tool_result(result)`,
  `run:rewrite_context(view)`, `run:rewritten(info)`,
  `run:on_event(event)`, `run:before_stop(stop)`, `run:turn_end(turn)`,
  `run:run_end(run)`, `run:title(run)`, `run:notify(notice)`,
  `run:job(name)`,
  `run:view()`; each takes what the hook is given, and runs it to its
  end, answering each await from the fakes. `run:call(f)` runs
  `f(model, env)` as a hook, for a module's function that awaits.
- `run:at(iso)` moves the run's clock, `env.now`.
- `run.notices` is each notice `cmd.notice` showed, in order.
- `run.files` is `files` as the plugin left them, what it wrote too;
  `run.folders` the folders it made; `run.infers` each round
  `infer{ tools }` asked, with the `turns` before it.
- `run.state` is the model as the last hook left it; `run.logs` its log
  lines; `run.job_state` a job's model; `run.woken` each `{ run, text }`
  a job woke; `run.secrets` which secrets are set.
- `t.equal(got, want, what)` compares tables by what they hold.

Test what the plugin decides, on the days, commands and states where
its answer changes, and its card and status line.

### Properties

A property says what holds for every input, and heiko looks for one where
it does not: hegel's engine draws the inputs, and a failure shrinks to
its smallest case before it is reported. Use one where examples would
miss cases: anything over lists, graphs, text or dates.

```luau
local t = require("heiko").test
local g = t.gen

t.property("a checked list has one step in progress at most", function(draw)
  local list = draw(g.lists(g.records {
    text = g.text { min = 1, max = 20 },
    status = g.sampled_from { "pending", "in_progress", "done" },
  }, { max = 6 }))
  local checked = steps.check(list)
  t.assume(checked ~= nil)          -- drop the case: not a failure
  t.note(#checked .. " steps")      -- a line in the report
  local going = 0
  for _, step in ipairs(checked) do
    if step.status == "in_progress" then going += 1 end
  end
  t.equal(going <= 1, true, "in progress")
end, { cases = 200 })
```

- Generators, in `t.gen`: `integers { min, max }`, `floats { min, max }`,
  `booleans { p }`, `text { min, max, codec }` (`"ascii"`, `"latin-1"` or
  Unicode), `from_regex(pattern)`, `just(v)`, `sampled_from(list)`,
  `one_of(gens)`, `optional(gen)`, `lists(gen, { min, max, unique })`,
  `dicts(keys, values, { min, max })`, `records { field = gen }`,
  `composite(function(draw) ... end)` for one built from others, and
  `gen:map(f)`, `gen:filter(keep)` on any of them.
- A failure says what each `draw` gave, the notes, and a blob:
  `{ reproduce = "..." }` as the property's options replays it.
- `t.machine(name, { init, rules, invariant, steps })` tests something
  with a lifecycle: `init(draw)` makes the model, each rule (a function
  of the model and `draw`, or `{ weight, run }`) changes it, and
  `invariant(model)` holds after every step. A failure shrinks to its
  fewest steps. `t.assume` in a rule skips it.
- `t.machine_of(name, machine, spec)` tests a `heiko.machine`: each of its
  events fires from every state, in any order, with the data
  `spec.events[name]` draws. Every step is checked for no error, a state
  in shape, an event with no arm changing nothing, the same answer when
  taken again, and a state that reads back the same from JSON. Add your
  rule: `spec.init(draw)` makes a model, `spec.event(model, event, state,
effects)` tells it each event, and `spec.invariant(state, model,
effects)` holds after each. A failure notes its events as a `t.case`
  to paste into the tests. Once every case held, each arm (an event from
  a state) must have been taken: the test fails naming those no case
  took. Reach them with `spec.states`, a generator of each state's
  fields by state, so cases start from those states too (as data an
  older version kept may), or list the ones that may stay untaken in
  `spec.untaken`, as `"event from state"`.
- `t.hooks(name, spec)` drives the plugin through its ways in, in any
  order: each of `spec.hooks` (a function of the run, `draw` and the
  world) starts one, on a fake run made from `spec.run`. An await on
  what `spec.run`'s `jev`, `tools`, `http` or `infer` answer pauses the
  hook, until the rule `answer` (which calls the fake with `draw` after
  its arguments) or `fail` lets it go on. Meanwhile any other way in
  takes its steps on the same model and store, as on heiko: a job's tick
  and a button can run while a check waits. This is how the tests find
  a hook that broke the rule above: one that kept a value from before
  an await and acted on it breaks the invariant in some order. A hook
  that raises fails the test unless `spec.may_fail(hook, why)`;
  `spec.invariant(world)` holds after every step.

  ```luau
  t.hooks("a Pause pressed during a check is kept", {
    run = { jev = { ask = function(_, draw)
      return { met = { probability = draw(g.floats({ min = 0, max = 0.8 })) } }, 0.001
    end } },
    hooks = {
      stop = function(r) r:before_stop({ text = "done?", turn = 1, reason = "stop" }) end,
      pause = function(r) r:action("act", { what = "pause" }) end,
    },
    invariant = function(world)
      local g = world.run.state.goal
      ...
    end,
  })
  ```

- A property runs 100 cases unless it says (1000 at most), for 30 s at
  most, and gives the same result each time for the same files. Its
  cases share one VM: keep what a case changes inside it, not in the
  file's top-level values.

### Fixtures

A fixture is a value the plugin makes, kept in `fixtures/<name>.json`:
what a view draws at a step, what a run leaves. `plugin_test` makes each
one and holds it to its file, as JSON, so a change to what the plugin
draws shows as the first place it differs. The package's declaration is
always `fixtures/declaration.json`, with no fixture of its own; a file
there that nothing makes fails.

```luau
t.fixture("counted", function()
  local run = t.run { id = "demo", tools = { read = "one two three" } }
  run:tool("word_count", { path = "notes.md" }, "c1")
  return { state = run.state, view = run:view() }
end)
```

The files are written from what the tests make, never by hand: blessing
writes each one anew and removes those nothing makes.

### Loops

A loop runs the plugin in a real agent loop, not a fake run: a scripted
model calls tools, the plugin's hooks run as heiko runs them, with a store,
its files and real time. Give the run as data; the check gets what it
came to.

```luau
t.loop("an image is kept as a blob", {
  files = { ["plot.png"] = t.png(12, 8) },     -- the run's workspace
  model = {                                    -- the model's turns, in order
    { tool = "canvas_show", args = { path = "plot.png", title = "Plot" } },
    { text = "Shown." },
  },
}, function(run)
  t.equal(run.results[1].text, 'Image 1, "Plot", is on the canvas (12×8 PNG).')
end)
```

- The spec: `prompt`, `steer` (a message steered in at the start),
  `model` (each turn a part or a list: `{ text }`, `{ thinking }`,
  `{ tool, args }`, `{ usage = { input, output } }`,
  `{ error = { code, message } }`, `{ stop = "length" }`), `name` (the
  model's id), `settings`, `kind`, `repo`, `files` (text,
  `t.bytes(base64)` or `t.png(w, h)`), `tools` (fake tools: a name to a
  text, a list taken in order, or `{ error }`; `{path}` in a text is the
  call's argument, `{x*size}` is `x` that many times), `jev` (rules,
  first match answers: `{ id = "call_", noul = 0.9 }`,
  `{ has = "error[", noul = 0.95 }`, or `{ error = 503 }`), and `then`
  (forks, each from where the run before it ended, with its own
  `prompt`, `steer`, `settings` and `jev`), `data` and `repository`
  (what heiko's data folder and its folder for the repository hold, as
  `files`), and `commits` (`{ turn, change, paths }`, the commit a turn
  leaves, for `turn_end`).
- What the check gets: `results` (each call's `text` and `error`),
  `outcome` (`text`, `stop`, `usage`), `error`, `events`, `state`,
  `rewrites` (the context's, by plugin), `logs`, `charged`, `archive`
  (each archived file's `text` and `mode`), and for the first run
  `forks`, `requests` (each request the model got: its `messages`,
  `instructions`, `tools`), `jev` (what Jev was asked), `workspace`,
  `blobs`, and `data` and `repository` (each file there as the loop left
  it, by path).
- A loop runs for 60 s at most; its check, like a case, 10 s.

## Seven plugins

### A rule: `before_tool`

{{no-friday-deploys}}

### A tool with a card, a module and a status line

{{word-count}}

### A stop check: `turn_end` and `before_stop`

{{say-the-tests}}

### A context rewrite: `rewrite_context`

{{forget-old-calls}}

### A slot filler: `heiko.title`

{{short-titles}}

### A page with actions: `view.page` and `nav`

{{todo-page}}

### A lifecycle: `heiko.machine`

{{ship-gate}}
