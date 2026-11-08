---
name: tau-plugins
description: Writing, changing and testing tau's Luau plugins, in the tau-plugins repository. Load it before you write or change a plugin, or when someone asks tau to do something on its own whenever a run calls a tool, stops, or ends a turn.
---

# tau's Luau plugins

A plugin changes how tau's runs go: it adds tools the model can call,
looks at each tool call before it runs (to block it or flag it for
review), checks a run before it stops, and shows a line in the run's
status. Plugins are Luau files in the `tau-plugins` repository, one
folder each:

```
<name>/
  plugin.luau      -- returns the plugin: tau.plugin { ... }
  lib/<module>.luau  -- modules of its own: require("<module>")
  tests/<file>.luau  -- its tests, run against fake runs
  README.md        -- what it is for, for the person
```

The folder's name is the plugin's `name`. A plugin is active once its
commit is on `main` and its tests pass. Write it, write its tests, run
`plugin_test` with the plugin's name until they pass, then commit. When
your turn ends, tau lands your commits on `main` by itself if every
plugin you changed passes and reaches nothing new; this chat goes on.

## What the person sees

When `main` moves, tau loads every plugin, runs its tests and shows the
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
- **Everywhere else**, once it lands on `main`: tau lands this chat's
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
local tau = require("tau")
local ui = tau.ui

return tau.plugin {
  name = "...",              -- the folder's name
  description = "...",       -- one line, for the Plugins screen
  uses = { tools = { "exec_command" }, jev = false, infer = false,
           network = { "ntfy.sh" }, secrets = { "ntfy" } },
  settings = { schema = { ... }, default = { ... }, view = function(settings, ctx) end },
  tools = { name = { description, parameters, call, card } },
  start = function(plan, ctx) end,
  before_tool = function(call, ctx) end,
  before_request = function(request, ctx) end,
  after_tool_result = function(result, ctx) end,
  rewrite_context = function(view, ctx) end,
  rewritten = function(info, ctx) end,
  before_stop = function(stop, ctx) end,
  turn_end = function(turn, ctx) end,
  events = { "tool_end" },   -- what `on_event` hears
  on_event = function(event, ctx) end,
  run_end = function(run, ctx) end,
  view = function(state, ctx) end,
  nav = { label = "Deploys", icon = "plug" },  -- its sidebar entry
  pages = { board = function(params, ctx) end },  -- its pages of a repository
  repo_nav = { label = "Todo", page = "board", icon = "list" },
  repo_data = function(ctx) end,   -- what its entry under a repository shows
  recover = function(input, ctx) end,  -- as tau starts: the runs cut off
  commands = { name = { description, args, run = function(text, ctx) end } },
  jobs = { name = { every = 3600, label = "...", run = function(ctx) end } },
  fills = { "tau.title" },  -- the slots it fills
  title = function(run, ctx) end,
  notify = function(notice, ctx) end,
  compaction = { rewriters = { "tau-fast-compaction" } },
}
```

Every field but `name` is optional. `tools` adds tools the model can
call, each with its arguments' JSON Schema, its `call` and, if it draws
one, its `card`.

### Hooks

| Hook                | Given                                                | Returns                                                        | Time   |
| ------------------- | ---------------------------------------------------- | -------------------------------------------------------------- | ------ |
| tool `call`         | the arguments, as `parameters` describes             | a JSON value, or `error(message)`                              | 60 s   |
| tool `card`         | the call's arguments and the result                  | a view piece, drawn on the call's card                         | 200 ms |
| `start`             | `{ input, context, instructions }`                   | `nil`, or `tau.start{ context, instructions }`                 | 2 s    |
| `before_tool`       | `{ id, name, args }`                                 | `nil`, `tau.block(why)`, `tau.flag(why)` or `tau.change(args)` | 2 s    |
| `before_request`    | `{ model, effort, turn, messages }`                  | `nil`, or `tau.effort(level)`                                  | 200 ms |
| `after_tool_result` | `{ id, name, args, ok, text, truncated }`            | `nil`, or `tau.result(text)`                                   | 2 s    |
| `rewrite_context`   | `{ trigger, turn, tokens, window, messages }`        | `nil`, or `tau.rewrite{ keep, insert, drops_conversation }`    | 10 s   |
| `rewritten`         | `{ plugin, before, after, drops_conversation }`      | nothing                                                        | 2 s    |
| `before_stop`       | `{ text, turn, reason }`                             | `nil`, or `tau.continue(message)`                              | 10 s   |
| `turn_end`          | `{ turn, text, calls = { { name, ok } } }`           | nothing                                                        | 2 s    |
| `on_event`          | `{ kind, turn, ... }`, for each of `events`          | nothing                                                        | 200 ms |
| `run_end`           | `{ stop, turns, cost, text }`, `text` its last reply | nothing                                                        | 10 s   |
| `title`             | `{ task, model }`, outside any run                   | a title, or `nil`                                              | 2 s    |
| `notify`            | `{ title, body }`, outside any run                   | nothing                                                        | 10 s   |
| `view`              | the run's state                                      | `{ status?, note?, page? }`, or `{}`                           | 200 ms |
| `actions[x]`        | a button's or an input's arguments                   | nothing; what it does to `ctx.state` stays                     | 2 s    |

- `tau.block(why)` keeps the call from running; the model reads why.
  `tau.flag(why)` lets it run and marks it for the person's review.
  `tau.change(args)` lets it run with `args` in place of its own, marked
  for review too; it needs `uses.change_args = true`, which the person
  allows as they allow new tools. The tool checks the new arguments as
  it would the model's, and the plugins after it see them.
- `tau.start{ context = { ... }, instructions = "..." }` adds to what
  the run starts with: `context`'s texts open its first message, after
  the ones already there, and `instructions` follow the run's own. The
  plugins after it see what it added. Instructions are sent with every
  request: keep them the same from run to run, or each run pays to
  send them in full. Having `start` at all is new reach.
- `tau.result(text)` has the model read `text` in place of the call's
  result, marked for review; the plugins after it read it as changed.
  `text` is the result's text, cut at 256 KiB (`truncated` says so).
  Having `after_tool_result` at all is new reach, which the person
  allows: it changes what the model reads.
- `rewrite_context` is offered the run's context between turns, when a
  request overflowed, and while the run is idle: `messages` holds each
  message's `index` (from 1), `role` (`user`, `assistant`,
  `toolResult`), `text` (its first 2,000 characters) and `size`; an
  answer's `calls` (`id`, `name`); a result's `call`, `name` and `ok`.

  `tau.rewrite{ keep = { 1, 4 }, insert = { { before = 4, text = "..." } }, drops_conversation = true }` keeps those messages and puts the texts
  before them as the person's. The last message always stays, and so
  does every call with its results. The first plugin that rewrites
  wins; which plugins may rewrite at all is the person's pick on the
  Plugins screen. Having `rewrite_context` at all is new reach.

- A plugin may fill four of tau's slots, named in `fills`. Filling one
  is new reach, and tau-luau-plugins must be the slot's pick on the
  Plugins screen; the first active plugin by name that fills it does.
  - `tau.title`: `title` writes a run's title from its `task` (at most
    36 characters; `nil` leaves the prompt's first line). It runs
    outside any run: `ctx.run` and `ctx.state` are empty.
  - `tau.notify`: `notify` gets each notice tau would show while its
    window is not in front, and sends it where the plugin reaches over
    HTTP, such as a push service.
  - `tau.forge`: where repositories are hosted. Its `forge` table
    names the forge (`name`, `id`, the `host` repositories clone from)
    and the secret that signs in (`token`, which `uses.secrets` must
    name); the person sets it on the plugin's settings. tau clones and
    pushes over HTTPS with it, and never hands it to the plugin. Its
    hooks reach the forge's API over HTTP: `account(input, ctx)` (who
    is signed in), `repos` (a list of
    `{ name, description, branch, pushed_at }`), `can_push` (`{ repo }`, a boolean), `open_pull`
    (`{ repo, title, body, head, base, draft, reviewers }`, answering
    `{ number, url }`) and `checks` (`{ repo, head }`, answering
    `"running"`, `"passed"`, `"failed"` or `"none"`).
  - `tau.compaction`: which plugins' rewrites run. The Luau plugins'
    own always do; the `compaction` table's `rewriters` names the
    others, such as `tau-fast-compaction`.
- `rewritten` hears that a rewrite replaced the context, whoever made
  it: how many messages there were `before` and are `after`.
- `tau.effort(level)` sends the turn's request with that reasoning
  effort (`"low"`, `"medium"`, `"high"`…), and the ones after it; the
  first plugin that picks one sets it. `messages` counts the transcript.
- `tau.continue(message)` keeps the run going with `message` as the
  next thing the model reads; a run continues at most three times.
- A hook that errors or runs out of time counts as `nil`, and its error
  shows on the Plugins screen. After three failures in a run the plugin
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
- Hooks run each in a fresh VM: keep what must last in `ctx.state`, not
  in local variables.

### The model

A plugin with `uses.infer` asks the person's model one task:
`tools.infer({ task, context, schema? })` answers `{ ok, value, error? }`,
`value` the answer's text, or JSON that holds to `schema`. In a run it
asks the run's model, booked to the run. In a hook outside any run
(`title`, `notify`, the forge's) it asks the model `uses.model` names,
or for `title` the run's own when it names none, booked to the
Plugins screen's tau-luau-plugins.

### HTTP

A plugin that names hosts in `uses.network` sends requests to them, from
any hook: `http.request{ method?, url, headers?, body?, timeout? }`
answers `{ status, headers, body, json, truncated }`. A header or the
body names a secret with `"Bearer " .. http.secret("ntfy")`: its value
goes in as the request leaves and is taken out of everything that comes
back, so the plugin never sees it. A request may go only to a host
`uses.network` names, and name only a secret `uses.secrets` names; the
person types each secret's value on the plugin's settings pane, kept for
this plugin alone. Redirects are not followed. A new host or secret is
new reach.

### `ctx`

| Field          | What it is                                                                                        |
| -------------- | ------------------------------------------------------------------------------------------------- |
| `ctx.run`      | `{ id, kind = "main" or "chat" or "sub_agent", repo, model, turn }`                               |
| `ctx.now`      | `{ unix, iso, weekday }`; `weekday` is `"Monday"` and so on                                       |
| `ctx.settings` | what the person set, over `settings.default`; per repository when one keeps its own copy          |
| `ctx.state`    | the plugin's table for this run; what a hook leaves in it is kept                                 |
| `ctx.tools`    | in a tool's `call` only: the tools `uses.tools` names, called as `ctx.tools.read({ path = "a" })` |
| `ctx.jev`      | Jev, when `uses.jev`                                                                              |
| `ctx.log`      | `ctx.log(text)`: a line on the plugin's page, never for the model                                 |
| `ctx.store`    | with `uses.store`: the plugin's store in the repository the hook works in (below)                 |

`ctx.state` is stored with the run: a resumed or forked run keeps it.
JSON has no empty array: use `array({})` for one that must stay a list.

### `tau`

- `tau.plugin(table)`, `tau.block(why)`, `tau.flag(why)`,
  `tau.start(plan)`, `tau.change(args)`, `tau.result(text)`,
  `tau.rewrite(rewrite)`, `tau.effort(level)`,
  `tau.continue(message)`.
- `tau.start_chat`, `tau.steer_chat` and `tau.allow`: what an action
  or a tool answers to start or steer a chat (below).
- `tau.contains(list, value)`: whether `list` holds `value`.

### `tau.ui`: what views are made of

Plain tables, drawn by tau on the computer and on the phone:

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
- `ui.heading(text)`, `ui.markdown(text)`, `ui.pill(text, tone)`
- `ui.icon(name, tone)`: one of tau's icons by name, such as
  `"memory"`, `"check"` or `"sub-agent"`
- `ui.table({ "head", ... }, { { piece, ... }, ... })`: a header over
  rows of pieces or strings
- `ui.scroll({ ... }, height)`: pieces that scroll past `height` rems
- `ui.rule()`, `ui.empty(text)`
- `ui.diff(text, path)`: a unified diff, colored as `path`'s language
- `ui.terminal(id, text, { cols = 80, rows = 24, live = false, fold = true })`:
  what a program wrote, escapes and all, on a terminal tau keeps under
  `id`; with `fold`, it shows a few rows until the person opens it
- `ui.ticker(id, since, stopped)`: the time since `since` (milliseconds
  since 1970), counting until `stopped` is given; keep `since` in the
  state
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
- `ui.button(label, action, args, kind)`: carries out
  `actions[action](args, ctx)` on the run's state, then draws `view`
  again; `kind` is `"primary"`, `"secondary"` or `"danger"`
- `ui.input(id, { placeholder = "...", multiline = true, submit = "action" })`:
  a text input; Enter carries out `submit` with `{ text = ... }` and
  empties it, or keeps what it holds with `keep = true`
- `ui.modal(title, { ... }, { dismiss = "action" })`: a panel over the
  page; return it from `view` only while it should show
- tau keeps where the person left a section or tabs, and what they
  typed in an input, under its `id`, on every device and across
  restarts
- `ui.link(label, page, { plugin = "...", params = { ... } })`: opens
  another plugin's page
- `ui.status(text, detail, tone)`: the run's status line, from `view`

`view` is called after each hook that changed the state. It must only
read the state: it has no tools. What it returns goes in three places:

- `status`: a `ui.status(...)`, the run's status line.
- `note`: a piece shown in the run's conversation, where the run is
  when the note first appears; a different note later shows where the
  run is then, and the earlier one stays.
- `page`: pieces shown on the plugin's own page, from its newest run;
  buttons there act on that run.
- `banner`: pieces across the top of the run.
- `inspector`: pieces in the run's inspector, beside the transcript.
- `band`: pieces just above the composer.
- `row`: `{ line, count?, tone? }`, the run's line under its title in
  the sidebar; `tone` is `"good"`, `"warn"` or `"bad"`.
- `asks`: what the run asks the person, in words: its row says so, and
  a notice shows it while tau's window is not in front.

A button whose action starts with `tau.` asks tau, not the plugin, on
the run the tree is drawn in (or its page shows):
`ui.button("Ship it", "tau.send", { text = "ship it" })` sends the next
message; `tau.steer` cuts into a turn going on; `tau.composer` puts
`text` in the composer; `tau.start` starts a run in `repo` with `text`;
`tau.alert` shows `title` and `message`; `tau.details` opens the run's
details.

A plugin's `commands` are slash commands in the composer's menu, each
with its `description` and what follows it (`args`, such as
`"<version>"`). Typed in a chat, `/name text` runs `run(text, ctx)` on
that chat's state, as an action does; in tests, `run:action("/name",
{ text = "..." })`.

A plugin's `jobs` are work it does by itself while tau runs: each
`run(ctx)` once `every` seconds passed since it last ran (at least 60),
tau's restarts or not, shown with the person's other jobs. Its
`ctx.state` is its own, kept from run to run; `ctx.run` is empty. A job
reaches what the plugin's other hooks outside a run do: HTTP to its
hosts, and `infer` with `uses.model`. A new job is new reach. In tests,
`run:job(name)`.

A plugin with `nav`, `{ label, icon? }`, has an entry in the sidebar that
opens its page; `icon` names one of the interface's icons.

### A repository's store, pages and chats

With `uses = { store = true }`, a plugin keeps a store in each
repository, apart from tau's and from every other plugin's: keys and
JSON values, through `ctx.store.get(key)`, `ctx.store.put(key,
value)`, `ctx.store.delete(key)` and `ctx.store.list(prefix)` (each
`{ key, value }`, in key order). A hook reads back what it wrote at
once, and hooks on one store take turns: what a hook wrote is kept
when it ends, all of it or, when it fails, none. So a hook that reads,
then writes, as claiming something does, is one step. A store is new
reach.

`pages` are the plugin's pages of a repository: each `page(params,
ctx)` returns a view tree, `params.repo` the repository, its store in
`ctx.store`. `repo_nav`, `{ label, page, icon? }`, is its entry under
each repository in the sidebar, opening that page; `repo_data(ctx)`
returns what the entry shows, `{ nav = { detail?, badge?, tone? } }`
among it. A page's buttons and inputs carry out the plugin's `actions`
in that repository, on its store there, with the page's own params in
`args.params`. Its links to its own pages (`ui.link(label, page, {
params })`) open them of that repository.

`recover(input, ctx)` is called once as tau starts, in each
repository, with `input.runs`, the runs tau found cut off when it last
closed: what the plugin keeps for them it lets go of there.

An action never waits on a chat: it answers what it asks, and tau
carries it out once the action ended and its store is kept.
`tau.start_chat{ repo?, text, ["then"] = "claimed", args = { ... } }`
starts a chat in `repo` (the action's own when it names none) on the
person's `text`; `tau.steer_chat{ run, text, ["then"], args }` says
`text` to the chat `run`, cutting into a turn going on. Then
`actions[then]` is called with `args` and the chat's `run` (and
`repo`), or with `error` when it could not be done.

A tool asks the person first: it answers `tau.allow{ title, text?,
action, args }`, and the run shows a card, on phones too, saying what it
would do, with Allow and Deny. The first answer takes it. Allowed,
`actions[action]` is carried out with `args` as the person's own, and
the model reads what it answered; denied, the model reads why; a run
that ends first withdraws it. A person's own click never asks.

### Settings

What the person can set lives in `settings`:

- `schema`: a JSON schema of an object, its properties each a boolean,
  a string (with an `enum`, a choice), a number, an integer, or an
  array of strings with an `enum` (a choice of several). Nothing is
  saved that it does not accept, and it takes no keys beyond its
  properties.
- `default`: the values before anything is set.
- `view(settings, ctx)`, optional: the plugin's own settings page,
  drawn from `tau.ui`. Without it, tau draws a form from `schema`.

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

`lib/<module>.luau` is `require("<module>")` in `plugin.luau`. A module
may require `tau` and the plugin's other modules, as long as no two
require each other, however far around. `tau` and `plugin` are taken.

## Tests

Each file under `tests/` is a list of cases; `plugin_test` and every
reload run them all. `t.run(spec)` is a fake run of the plugin, and its
hooks are methods on it:

```luau
local tau = require("tau")
local t = tau.test

t.case("what it checks", function()
  local run = t.run {
    now = "2026-10-09T10:00:00Z",  -- an ISO date: a Friday
    settings = { ... },            -- else the defaults
    state = { ... },               -- else {}
    tools = { read = "text" },     -- what ctx.tools give: a value, or a function of the args
  }
  t.equal(run:before_tool { name = "exec_command", args = { cmd = "ls" } }, nil)
end)
```

- `run:tool(name, args)`, `run:card(name, args, result)`,
  `run:settings_view(settings)`, `run:action(name, args)`,
  `run:start(plan)`, `run:before_tool(call)`,
  `run:before_request(request)`, `run:after_tool_result(result)`,
  `run:rewrite_context(view)`, `run:rewritten(info)`,
  `run:on_event(event)`, `run:before_stop(stop)`, `run:turn_end(turn)`,
  `run:run_end(run)`, `run:title(run)`, `run:notify(notice)`,
  `run:job(name)`,
  `run:view()`; each takes what the hook is given.
- `run.state` is the state the last hook left; `run.logs` its log lines.
- `t.equal(got, want, what)` compares tables by what they hold.

Test what the plugin decides, on the days, commands and states where
its answer changes, and its card and status line.

### Properties

A property says what holds for every input, and tau looks for one where
it does not: hegel's engine draws the inputs, and a failure shrinks to
its smallest case before it is reported. Use one where examples would
miss cases: anything over lists, graphs, text or dates.

```luau
local t = require("tau").test
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
- A property runs 100 cases unless it says (1000 at most), for 30 s at
  most, and gives the same result each time for the same files. Its
  cases share one VM: keep what a case changes inside it, not in the
  file's top-level values.

## Six plugins

### A rule: `before_tool`

{{no-friday-deploys}}

### A tool with a card, a module and a status line

{{word-count}}

### A stop check: `turn_end` and `before_stop`

{{say-the-tests}}

### A context rewrite: `rewrite_context`

{{forget-old-calls}}

### A slot filler: `tau.title`

{{short-titles}}

### A page with actions: `view.page` and `nav`

{{todo-page}}
