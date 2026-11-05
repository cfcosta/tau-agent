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
  fills = { "tau.title" },  -- the slots it fills
  title = function(run, ctx) end,
  compaction = { rewriters = { "tau-fast-compaction" } },
}
```

Every field but `name` is optional. `tools` adds tools the model can
call, each with its arguments' JSON Schema, its `call` and, if it draws
one, its `card`.

### Hooks

| Hook          | Given                                      | Returns                                    | Time   |
| ------------- | ------------------------------------------ | ------------------------------------------ | ------ |
| tool `call`   | the arguments, as `parameters` describes   | a JSON value, or `error(message)`          | 60 s   |
| tool `card`   | the call's arguments and the result        | a view piece, drawn on the call's card     | 200 ms |
| `start`       | `{ input, context, instructions }`         | `nil`, or `tau.start{ context, instructions }` | 2 s |
| `before_tool` | `{ id, name, args }`                       | `nil`, `tau.block(why)`, `tau.flag(why)` or `tau.change(args)` | 2 s    |
| `before_request` | `{ model, effort, turn, messages }`    | `nil`, or `tau.effort(level)`              | 200 ms |
| `after_tool_result` | `{ id, name, args, ok, text, truncated }` | `nil`, or `tau.result(text)`  | 2 s    |
| `rewrite_context` | `{ trigger, turn, tokens, window, messages }` | `nil`, or `tau.rewrite{ keep, insert, drops_conversation }` | 10 s |
| `rewritten`   | `{ plugin, before, after, drops_conversation }` | nothing                               | 2 s    |
| `before_stop` | `{ text, turn, reason }`                   | `nil`, or `tau.continue(message)`          | 10 s   |
| `turn_end`    | `{ turn, text, calls = { { name, ok } } }` | nothing                                    | 2 s    |
| `on_event`    | `{ kind, turn, ... }`, for each of `events` | nothing                                    | 200 ms |
| `run_end`     | `{ stop, turns, cost }`                    | nothing                                    | 10 s   |
| `title`       | `{ task, model }`, outside any run         | a title, or `nil`                          | 2 s    |
| `view`        | the run's state                            | `{ status?, note?, page? }`, or `{}`       | 200 ms |
| `actions[x]`  | a button's or an input's arguments         | nothing; what it does to `ctx.state` stays | 2 s    |

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
  `tau.rewrite{ keep = { 1, 4 }, insert = { { before = 4, text = "..." } },
  drops_conversation = true }` keeps those messages and puts the texts
  before them as the person's. The last message always stays, and so
  does every call with its results. The first plugin that rewrites
  wins; which plugins may rewrite at all is the person's pick on the
  Plugins screen. Having `rewrite_context` at all is new reach.
- A plugin may fill two of tau's slots, named in `fills`. Filling one
  is new reach, and tau-luau-plugins must be the slot's pick on the
  Plugins screen; the first active plugin by name that fills it does.
  - `tau.title`: `title` writes a run's title from its `task` (at most
    36 characters; `nil` leaves the prompt's first line). It runs
    outside any run: `ctx.run` and `ctx.state` are empty.
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
- `on_event` hears the run's events that `events` names, each with its
  `kind` and `turn`: `turn_start`; `tool_start` (`id`, `name`, `args`);
  `tool_end` (`id`, `name`, `ok`); `retry` (`attempt`, `error`);
  `steered` (`text`); `continued` (`plugin`, `message`);
  `context_rewritten` (`plugin`, `tokens_before`, `tokens_after`);
  `plugin_error` (`plugin`, `message`). A name not among them keeps the
  plugin from loading.
- Hooks run each in a fresh VM: keep what must last in `ctx.state`, not
  in local variables.

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

`ctx.state` is stored with the run: a resumed or forked run keeps it.
JSON has no empty array: use `array({})` for one that must stay a list.

### `tau`

- `tau.plugin(table)`, `tau.block(why)`, `tau.flag(why)`,
  `tau.start(plan)`, `tau.change(args)`, `tau.result(text)`,
  `tau.rewrite(rewrite)`, `tau.effort(level)`,
  `tau.continue(message)`.
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
may require `tau`, but not another module of the plugin. `tau` and
`plugin` are taken.

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
  `run:settings_view(settings)`,
  `run:before_tool(call)`, `run:before_stop(stop)`,
  `run:turn_end(turn)`, `run:run_end(run)`, `run:view()`.
- `run.state` is the state the last hook left; `run.logs` its log lines.
- `t.equal(got, want, what)` compares tables by what they hold.

Test what the plugin decides, on the days, commands and states where
its answer changes, and its card and status line.

## Three plugins

### A rule: `before_tool`

{{no-friday-deploys}}

### A tool with a card, a module and a status line

{{word-count}}

### A stop check: `turn_end` and `before_stop`

{{say-the-tests}}
